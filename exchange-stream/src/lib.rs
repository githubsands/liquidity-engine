use std::convert::Infallible;
use std::error::Error;
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

use bounded_vec_deque::BoundedVecDeque;
use crossbeam_channel::{Sender, TrySendError};
use futures::stream::iter;
use reqwest::{Client, Error as HTTPError};
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
        http_client_option: bool,
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
            connect_async_with_config(self.websocket_uri.clone(), Some(config)).await;
        let _ = match ws_conn_result {
            Ok((ws_conn, _)) => {
                let (sink, stream) = ws_conn.split();
                self.subscribe_orderbooks(sink).await?;
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
        let json_obj_bybit = serde_json::json!({
              "op": "subscribe",
              "args": ["orderbook.1.USDTBTC"]
        });

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
    async fn reconnect(&mut self) -> Result<(), ErrorHotPath> {
        let config = WebSocketConfig {
            max_send_queue: None,
            max_message_size: None,
            max_frame_size: None,
            accept_unmasked_frames: true,
        };
        let ws_conn_result =
            connect_async_with_config(self.websocket_uri.clone(), Some(config)).await;
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
}

impl Stream for ExchangeStream {
    type Item = WSStreamState;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let mut this = self.project();
        debug!("waiting for items for exchange: {}\n", this.exchange_name);
        while let Some(depth) = this.buffer.pop() {
            if let Err(channel_error) = this.depths_producer.try_send(depth) {
                match channel_error {
                    // NOTE: If we see warnings here we may need to increase the size of our main
                    // buffer, or the size of the channel. This should not happen.
                    TrySendError::Full(_) => {
                        warn!("failed to try_send to order bid producer, trying again");
                        continue;
                    }
                    TrySendError::Disconnected(_) => {
                        error!("depth producer within ExchangeStream disconnected while streaming");
                        return Poll::Ready(Some(WSStreamState::SenderError));
                    }
                }
            }
        }
        let Some(mut orderbooks) = this.ws_connection_orderbook_reader.as_mut().as_pin_mut() else {
            error!("failed to copy the orderbooks stream");
            return Poll::Ready(Some(WSStreamState::FailedStream))
        };
        while let Poll::Ready(stream_option) = orderbooks.poll_next_unpin(cx) {
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
                    _ => {}
                },
                Some(Err(ws_error)) => return Poll::Ready(Some(WSStreamState::WSError(ws_error))),
                _ => {}
            }
        }
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exchange_stubs::ExchangeServer;
    use tokio_test::{assert_pending, assert_ready_eq};

    impl Default for ExchangeStream {
        fn default() -> Self {
            Self {
                client_name: String::from(""),
                exchange_name: 1,
                snapshot_enabled: false,
                snapshot_buffer: Vec::new(),
                snapshot_uri: String::from(""),
                websocket_uri: String::from("BTCUSDT")
                watched_pair: String::from(""),
                trigger_snapshot: false,
                orderbook_subscription_message: String::from(""),
                buffer: Vec::new(),
                ws_connection_orderbook: None,
                ws_connection_orderbook_reader: None,
                depths_producer: None, // You need to replace `Sender::new()` with the correct way to initialize it
                http_client: None,
            }
        }
    }

    #[test]
    fn test_exchange_stream_receive_depth() {
        let waker = futures::task::noop_waker_ref();
        let mut cx = std::task::Context::from_waker(&waker);
        let mut exchange_server = ExchangeServer::default();
        tokio::sleep(Duration::from_millis(10));
        let (depth_producer, depth_consumer) = channel::bounded::<DepthUpdate>(20);
        let exchange_stream = ExchangeStream::default();
        exchange_stream.depths_producer = depths_producer
        let depth_receive = async {
            while let Some(depth_update) = rx.recv().await {
                println!("received depth")

            }
        }
        tokio::sleep(Duration::from_millis(10));

        tokio::select! {
            _ = depth_receive {



            }
        }

    }
}
