use std::error::Error;
use std::fmt;
use std::thread;
use std::{pin::Pin, task::Poll};

use serde_json::{from_str, to_string as jsonify};

use pin_project_lite::pin_project;

use futures::stream::{SplitSink, SplitStream, Stream, StreamExt};
use futures_util::sink::SinkExt;
use itertools::interleave;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async_with_config, tungstenite::protocol::Message,
    tungstenite::protocol::WebSocketConfig, MaybeTlsStream, WebSocketStream,
};

use crossbeam_channel::{Sender, TrySendError};
use futures::stream::iter;
use reqwest::Client;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use config::ExchangeConfig;
use market_object::{
    DepthUpdate, HTTPSnapShotDepthResponseBinance, HTTPSnapShotDepthResponseByBit,
    WSDepthUpdateBinance, WSDepthUpdateByBit,
};
use quoter_errors::{ErrorHotPath, ErrorInitialState};

pin_project! {
    #[must_use = "streams do nothing unless polled"]
    pub struct ExchangeStream {
        pub client_name: String,
        pub exchange_name: u8,
        pub snapshot_enabled: bool,
        pub snapshot_buffer: Vec<DepthUpdate>,
        pub snapshot_uri: String,
        pub ws_subscribe: bool,
        pub websocket_uri: String,
        pub watched_pair: String,
        pub trigger_snapshot: bool,

        orderbook_subscription_message: String,

        #[pin]
        buffer: Vec<DepthUpdate>,
        #[pin]
        ws_connection_orderbook: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        #[pin]
        ws_connection_orderbook_reader: Option<SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>>,

        depths_producer: Sender<DepthUpdate>,

        http_client: Option<Client>,
    }
}

impl ExchangeStream {
    pub fn new(
        exchange_config: &ExchangeConfig,
        orders_producer: Sender<DepthUpdate>,
        _http_client_option: bool,
    ) -> Result<Box<Self>, ErrorInitialState> {
        let mut http_client: Option<Client> = None;
        if exchange_config.snapshot_enabled {
            info!("snapshot is enabled building http client");
            http_client = Some(Client::new());
        }
        let exchange = ExchangeStream {
            client_name: exchange_config.client_name.clone(),
            exchange_name: exchange_config.exchange_name,
            trigger_snapshot: false,
            snapshot_buffer: Vec::with_capacity(15000),
            snapshot_enabled: exchange_config.snapshot_enabled,
            snapshot_uri: exchange_config.snapshot_uri.clone(),
            ws_subscribe: false,
            websocket_uri: exchange_config.ws_uri.clone(),
            watched_pair: exchange_config.watched_pair.clone(),
            orderbook_subscription_message: jsonify(
                &exchange_config.orderbook_subscription_message,
            )
            .unwrap(),
            ws_connection_orderbook: None::<WebSocketStream<MaybeTlsStream<TcpStream>>>,
            buffer: Vec::with_capacity(exchange_config.buffer_size),
            ws_connection_orderbook_reader: None,
            depths_producer: orders_producer,
            http_client: http_client,
        };
        Ok(Box::new(exchange))
    }
    pub async fn start(&mut self) -> Result<(), ErrorInitialState> {
        let config = WebSocketConfig {
            max_send_queue: None,
            max_message_size: None,
            max_frame_size: None,
            accept_unmasked_frames: true,
        };
        let ws_conn_result =
            connect_async_with_config(self.websocket_uri.clone(), Some(config), false).await;
        let _ = match ws_conn_result {
            Ok((ws_conn, _)) => {
                let (sink, stream) = ws_conn.split();
                if self.ws_subscribe {
                    self.subscribe_orderbooks(sink).await?;
                }
                self.ws_connection_orderbook_reader = Some(stream);
                info!(
                    "connected to exchange {} at {}",
                    self.exchange_name, self.websocket_uri
                )
            }
            Err(ws_error) => {
                error!("failed to connect to exchange {}", self.websocket_uri);
                return Err(ErrorInitialState::WSConnection(ws_error.to_string()));
            }
        };
        Ok(())
    }
    pub async fn buffer_snapshots(&mut self) -> Result<(), Box<dyn Error>> {
        let mut buffer = self.snapshot_buffer.clone();
        let snapshot_orders = self.orderbook_snapshot().await?;
        buffer.extend(interleave(snapshot_orders.0, snapshot_orders.1));
        Ok(())
    }
    // sequence_depths pushes the snapshot depths into WSStreaming's main buffer
    pub async fn sequence_depths(&mut self) {
        let mut prepared_snapshot_stream = iter(&*self.snapshot_buffer);
        while let Some(depth_update) = prepared_snapshot_stream.next().await {
            self.buffer.push(*depth_update)
        }
    }
    async fn orderbook_snapshot(
        &mut self,
    ) -> Result<
        (
            impl Iterator<Item = DepthUpdate>,
            impl Iterator<Item = DepthUpdate>,
        ),
        Box<dyn std::error::Error>,
    > {
        info!("curling snap shot from {}", self.snapshot_uri);
        let req_builder = self
            .http_client
            .as_ref()
            .unwrap()
            .get(self.snapshot_uri.clone());
        let snapshot_response_result = req_builder.send().await;
        match snapshot_response_result {
            Ok(snapshot_response) => match (snapshot_response, self.exchange_name) {
                (snapshot_response, 1) => {
                    let body = snapshot_response.text().await?;
                    let snapshot: HTTPSnapShotDepthResponseBinance = from_str(&body)?;
                    info!("finished receiving snaps for {}", self.exchange_name);
                    let snapshot_depths = snapshot.depths(1);
                    Ok(snapshot_depths)
                }
                (snapshot_response, 2) => {
                    let snapshot: HTTPSnapShotDepthResponseBinance =
                        snapshot_response.json().await?;
                    info!("finished receiving snaps for {}", self.exchange_name);
                    let snapshot_depths = snapshot.depths(2);
                    Ok(snapshot_depths)
                } /*
                (snapshot_response, 3) => {
                let snapshot: HTTPSnapShotDepthResponseByBit = snapshot_response.json().await?;
                info!("finished receiving snaps for {}", self.exchange_name);
                let snapshot_depths = snapshot.depths(3);
                Ok(snapshot_depths)
                }
                 */
                _ => {
                    error!(
                        "failed to create snapshot due to exchange_name {}",
                        self.exchange_name
                    );
                    return Err(Box::new(ErrorInitialState::Snapshot(
                        "Failed to create snapshot".to_string(),
                    )));
                }
            },
            Err(err) => {
                error!("failed to reconcile snapshot result: {}", err);
                return Err(Box::new(ErrorInitialState::Snapshot(err.to_string())));
            }
        }
    }
    async fn subscribe_orderbooks(
        &mut self,
        mut sink: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    ) -> Result<(), ErrorInitialState> {
        info!(
            "sending orderbook subscription message: {}\nto exchange {}",
            self.orderbook_subscription_message, self.websocket_uri
        );
        let json_obj_binance = serde_json::json!({
            "method": "SUBSCRIBE",
            "params": [
                "btcusdt@depth",
            ],
            "id": 1
        });
        /*
        let json_obj_bybit = serde_json::json!({
              "op": "subscribe",
              "args": ["orderbook.1.USDTBTC"]
        });
        */

        let exchange_response = sink.send(Message::Text(json_obj_binance.to_string())).await;
        // TODO: handle this differently;
        match exchange_response {
            Ok(response) => {
                print!("subscription success: {:?}", response);
            }
            Err(error) => {
                print!("error {}", error)
            }
        }
        Ok(())
    }
    #[allow(dead_code)]
    async fn reconnect(&mut self) -> Result<(), ErrorHotPath> {
        let config = WebSocketConfig {
            max_send_queue: None,
            max_message_size: None,
            max_frame_size: None,
            accept_unmasked_frames: true,
        };
        let ws_conn_result =
            connect_async_with_config(self.websocket_uri.clone(), Some(config), false).await;
        let _ = match ws_conn_result {
            Ok((ws_conn, _)) => {
                self.ws_connection_orderbook = Some(ws_conn);
                info!("reconnected to exchange: {}", self.exchange_name)
            }
            Err(_) => {
                return Err(ErrorHotPath::ExchangeWSReconnectError(
                    "Failed to reconnect to Exchange WS Server".to_string(),
                ));
            }
        };
        Ok(())
    }
}

pub enum WSStreamState {
    WSError(tokio_tungstenite::tungstenite::Error),
    SenderError,
    FailedStream,
    FailedDeserialize, // TODO: Pass down the deserialize error like we do with the WSError
    Success,
    WaitingForDepth,
}

impl fmt::Debug for WSStreamState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WSStreamState::WSError(err) => write!(f, "WebSocket Error: {:?}", err),
            WSStreamState::SenderError => write!(f, "Sender Error"),
            WSStreamState::FailedStream => write!(f, "Failed Stream"),
            WSStreamState::FailedDeserialize => write!(f, "Failed Deserialize"),
            WSStreamState::Success => write!(f, "Success"),
            WSStreamState::WaitingForDepth => write!(f, "Waiting for Depth"),
        }
    }
}

impl Stream for ExchangeStream {
    type Item = WSStreamState;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let mut this = self.project();
        debug!(
            "waiting for items to buffer in the exchange: {}\n",
            this.exchange_name
        );
        if let Some(depth) = this.buffer.pop() {
            debug!("trying to send buffered item",);
            if let Err(channel_error) = this.depths_producer.try_send(depth) {
                match channel_error {
                    // NOTE: If we see warnings here we may need to increase the size of our main
                    // buffer, or the size of the channel. This should not happen.
                    TrySendError::Full(_) => {
                        warn!("failed to try_send to order bid producer, trying again");
                        return Poll::Ready(Some(WSStreamState::SenderError));
                    }
                    TrySendError::Disconnected(_) => {
                        error!("depth producer within ExchangeStream disconnected while streaming");
                        return Poll::Ready(Some(WSStreamState::SenderError));
                    }
                }
            }
        }
        // check for websocket errors before proceeding
        let Some(mut orderbooks) = this.ws_connection_orderbook_reader.as_mut().as_pin_mut() else {
            error!("failed to copy the orderbooks stream");
            return Poll::Ready(Some(WSStreamState::FailedStream))
        };
        while let Poll::Ready(stream_option) = orderbooks.poll_next_unpin(cx) {
            debug!("AHHHHHHHHHHHHHHHHHHHHHHHHHHH");
            match stream_option {
                Some(Ok(ws_message)) => match (&*this.exchange_name, ws_message) {
                    (1, ws_message) => {
                        if let Ok(depth_update) = WSDepthUpdateBinance::try_from(ws_message) {
                            let depths = depth_update.depths(1);
                            let woven_depths = interleave(depths.0, depths.1);
                            this.buffer.extend(woven_depths);
                        } else {
                            warn!("failed to deserialize the object.");
                        }
                    }
                    (2, ws_message) => {
                        if let Ok(depth_update) = WSDepthUpdateBinance::try_from(ws_message) {
                            let depths = depth_update.depths(2);
                            let woven_depths = interleave(depths.0, depths.1);
                            this.buffer.extend(woven_depths);
                        } else {
                            warn!("failed to deserialize the object.");
                        }
                    }
                    (3, ws_message) => {
                        if let Ok(depth_update) = WSDepthUpdateByBit::try_from(ws_message) {
                            let depths = depth_update.depths(3);
                            let woven_depths = interleave(depths.0, depths.1);
                            this.buffer.extend(woven_depths);
                        } else {
                            warn!("failed to deserialize the object.");
                        }
                    }
                    _ => break,
                },
                Some(Err(ws_error)) => {
                    warn!("poor depth received: {}", ws_error);
                    return Poll::Ready(Some(WSStreamState::WSError(ws_error)));
                }
                _ => {
                    return Poll::Ready(Some(WSStreamState::WaitingForDepth));
                }
            }
        }
        thread::sleep(Duration::from_secs(2));
        Poll::Ready(Some(WSStreamState::WaitingForDepth))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::{bounded, Receiver};
    use exchange_stubs::ExchangeServer;
    use std::sync::Arc;
    use std::sync::Mutex as syncMutex;
    use std::thread;
    use std::thread::sleep as threadSleep;
    use testing_traits::ProducerDefault;
    use tokio::sync::oneshot::channel;
    use tokio::sync::Mutex;
    use tokio::time::{sleep, Duration};
    use tracing::info;
    use tracing_test::traced_test;

    impl<'a> ProducerDefault<'a, ExchangeStream, DepthUpdate> for ExchangeStream {
        fn producer_default() -> (Box<Self>, Receiver<DepthUpdate>) {
            let (depths_producer, depths_consumer) = bounded::<DepthUpdate>(100);
            let exchange_stream = Self {
                depths_producer: depths_producer,
                client_name: String::from(""),
                exchange_name: 1,
                snapshot_enabled: false,
                snapshot_buffer: Vec::new(),
                snapshot_uri: String::from(""),
                ws_subscribe: false,
                websocket_uri: String::from(""),
                watched_pair: String::from(""),
                trigger_snapshot: false,
                orderbook_subscription_message: String::from(""),
                buffer: Vec::new(),
                ws_connection_orderbook: None,
                ws_connection_orderbook_reader: None,
                http_client: None,
            };
            (Box::new(exchange_stream), depths_consumer)
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn test_exchange_stream_receive_depths_from_ws_server() {
        let depth_count_client_received: Arc<syncMutex<i32>> = Arc::new(syncMutex::new(0));
        let desired_depths: i32 = 1;
        let (mut exchange_stream, depth_consumer) = ExchangeStream::producer_default();
        let exchange_server = Arc::new(Mutex::new(
            ExchangeServer::new("1".to_string(), 8080).unwrap(),
        ));
        exchange_stream.websocket_uri =
            "ws://".to_owned() + exchange_server.lock().await.ip_address().as_str();
        info!("starting test");
        let depth_count_clone = depth_count_client_received.clone();
        sleep(Duration::from_secs(2)).await;
        thread::spawn(move || loop {
            threadSleep(Duration::from_secs(2));
            println!("4 waiting from orderbook thread");
            let result = depth_consumer.try_recv();
            if result.is_err() {
                info!("result : {:?}", result);
                continue;
            }
        });
        let (tx1, rx1) = oneshot::channel();
        let t1 = tokio::spawn(async move {
            info!("spawning runtime");
            let server_clone = Arc::clone(&exchange_server);
            info!("starting server");
            tokio::spawn(async move {
                server_clone.lock().await.run().await;
            });
            sleep(Duration::from_secs(2)).await;
            exchange_stream.start().await?;
            let server_clone = Arc::clone(&exchange_server);
            tokio::spawn(async move {
                'send_loop: loop {
                    println!("sending depth");
                    let result = server_clone.lock().await.supply_depths().await;
                    match result {
                        Ok(_) => {
                            info!("send success");
                        }
                        Err(_) => {
                            info!("send sucess");
                            continue 'send_loop;
                        }
                    }
                    exchange_stream.next().await;
                    // Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
                }
            });
            /*
            tokio::select! {
                 val = rx1 => {
                    println!("rx1 completed first with {:?}", val);
                    Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
                }
            }
            */
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });
        sleep(Duration::from_secs(4000)).await;
        println!("ending corotuine");
        tx1.send(true);
    }
}
