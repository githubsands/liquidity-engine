use std::error::Error;
use std::fmt;
use std::{pin::Pin, task::Poll};

use serde_json::from_str;

use pin_project_lite::pin_project;

use futures::stream::{SplitSink, SplitStream, Stream, StreamExt};
use itertools::interleave;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async_with_config, tungstenite::protocol::Message,
    tungstenite::protocol::WebSocketConfig, MaybeTlsStream, WebSocketStream,
};

use tokio::sync::watch::Receiver as watchReceiver;
use tokio::time::{sleep, Duration};

use crossbeam_channel::{Sender, TrySendError};
use futures::stream::iter;
use reqwest::Client;
use tracing::{debug, error, info, warn};

use config::ExchangeConfig;
use market_objects::{
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
        pub websocket_depth_buffer: Vec<DepthUpdate>,
        pub pull_retry_count: u8,
        pub http_snapshot_uri: String,
        pub buffer_websocket_depths: bool,
        pub ws_subscribe: bool,
        pub ws_poll_rate: u64,
        pub websocket_uri: String,
        pub watched_pair: String,

        pub snapshot_trigger: Option<watchReceiver<()>>,

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
        snapshot_trigger: watchReceiver<()>,
        _http_client_option: bool,
    ) -> Result<Self, ErrorInitialState> {
        let mut http_client: Option<Client> = None;
        if exchange_config.snapshot_enabled {
            info!("snapshot is enabled building http client");
            http_client = Some(Client::new());
        }
        let exchange = ExchangeStream {
            client_name: exchange_config.client_name.clone(),
            exchange_name: exchange_config.exchange_name,
            snapshot_trigger: Some(snapshot_trigger),
            websocket_depth_buffer: Vec::with_capacity(15000),
            buffer_websocket_depths: false,
            snapshot_enabled: exchange_config.snapshot_enabled,
            pull_retry_count: 5,
            http_snapshot_uri: exchange_config.snapshot_uri.clone() + "/depths",
            ws_subscribe: false,
            ws_poll_rate: exchange_config.ws_poll_rate_milliseconds.into(),
            websocket_uri: exchange_config.ws_uri.clone(),
            watched_pair: exchange_config.watched_pair.clone(),
            ws_connection_orderbook: None::<WebSocketStream<MaybeTlsStream<TcpStream>>>,
            buffer: Vec::with_capacity(exchange_config.buffer_size),
            ws_connection_orderbook_reader: None,
            depths_producer: orders_producer,
            http_client,
        };
        Ok(exchange)
    }
    pub async fn start(
        &mut self,
    ) -> Result<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>, ErrorInitialState>
    {
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
                self.ws_connection_orderbook_reader = Some(stream);
                info!(
                    "connected to exchange {} at {}",
                    self.exchange_name, self.websocket_uri
                );
                return Ok(sink);
            }
            Err(ws_error) => {
                error!("failed to connect to exchange {}", self.websocket_uri);
                return Err(ErrorInitialState::WSConnection(ws_error.to_string()));
            }
        };
    }

    pub async fn run_snapshot(&mut self) -> Result<(), ErrorInitialState> {
        self.buffer_websocket_depths = true;
        let mut success = false;
        while !success {
            sleep(Duration::from_secs(2)).await;
            let pull_result = self.pull_depths().await;
            match pull_result {
                Ok(mut depths) => {
                    while let Some(depth) = depths.next() {
                        self.buffer.push(depth);
                        self.next().await; // we must keep processing  snapshot depths and depths from the websocket
                                           // but this time the websocket depths are stored in their own buffer
                                           // to be sequenced
                    }
                    success = true;
                }
                Err(pull_error) => {
                    error!(
                        "failed to get websocket depths from exchange {}",
                        pull_error
                    );
                    return Err(ErrorInitialState::Snapshot(pull_error.to_string()));
                }
            }
        }
        Ok(())
    }

    pub async fn push_buffered_ws_depths(&mut self) {
        while let Some(websocket_depth) = self.websocket_depth_buffer.pop() {
            self.buffer.push(websocket_depth);
        }
        self.buffer_websocket_depths = false;
    }

    pub async fn run(&mut self) -> Result<(), ErrorHotPath> {
        if let Some(trigger) = &mut self.snapshot_trigger {
            tokio::select! {
                    _ = trigger.changed()=> {
                        self.buffer_websocket_depths = true;
                        let mut pull_retry_count = 0;
                        while pull_retry_count < self.pull_retry_count {
                            let pull_result = self.pull_depths().await;
                            match pull_result {
                                Ok(mut depths) => {
                                while let Some(depth) = depths.next() {
                                    self.buffer.push(depth);
                                    self.next().await; // we must keep processing  snapshot depths and depths from the websocket
                                                       // but this time the websocket depths are stored in their own buffer
                                                       // to be sequenced
                                }
                                    break
                                }
                                Err(pull_error) => {
                                    if pull_retry_count == self.pull_retry_count {
                                        error!("reached maxed snapshot pull count retry for exchange {} received error: {}", self.exchange_name, pull_error);
                                        return Err(ErrorHotPath::ExchangeStreamSnapshot(pull_error.to_string()))
                                    }
                                    pull_retry_count = pull_retry_count + 1;
                                    warn!("failed to get websocket depths from exchange {}", pull_error);
                                    continue
                                    }
                                }
                            }
                        // we are done push snapshot depths to the orderbook - turn this buffer off
                        // and push the buffer websocket depths to the orderbook
                        self.buffer_websocket_depths = false;

                        while let Some(websocket_depth) = self.websocket_depth_buffer.pop() {
                            self.buffer.push(websocket_depth);
                            self.next().await;
                        }
                        Ok(())
                    }
                // Most exchanges ws updates updates every 100 milliseconds so no need to poll more then
                // this.
                // TODO: Define these errors explicitly - they are not all ExchangeWSErrors
                _ = tokio::time::sleep(Duration::from_millis(self.ws_poll_rate)) => {
                    if let Some(stream_poll_state) = self.next().await {
                match stream_poll_state {
                    WSStreamState::Success => return Ok(()),
                    WSStreamState::WaitingForDepth => return Ok(()),
                    WSStreamState::FailedStream => {
                        return Err(ErrorHotPath::ExchangeWSError("tbd".to_string()))
                    }
                    WSStreamState::SenderError => {
                        return Err(ErrorHotPath::ExchangeWSError("tbd".to_string()))
                    }
                    WSStreamState::WSError(ws_error) => {
                        return Err(ErrorHotPath::ExchangeWSError(ws_error.to_string()))
                    }
                    WSStreamState::FailedDeserialize => {
                        return Err(ErrorHotPath::ExchangeWSError("tbd".to_string()))
                    }
                }
            } else {
                return Err(ErrorHotPath::ExchangeWSError("tbd".to_string()));
            }
            }
            }
        } else {
            tokio::time::sleep(Duration::from_millis(self.ws_poll_rate)).await;
            if let Some(stream_poll_state) = self.next().await {
                match stream_poll_state {
                    WSStreamState::Success => return Ok(()),
                    WSStreamState::WaitingForDepth => return Ok(()),
                    WSStreamState::FailedStream => {
                        return Err(ErrorHotPath::ExchangeWSError("tbd".to_string()))
                    }
                    WSStreamState::SenderError => {
                        return Err(ErrorHotPath::ExchangeWSError("tbd".to_string()))
                    }
                    WSStreamState::WSError(ws_error) => {
                        return Err(ErrorHotPath::ExchangeWSError(ws_error.to_string()))
                    }
                    WSStreamState::FailedDeserialize => {
                        return Err(ErrorHotPath::ExchangeWSError("tbd".to_string()))
                    }
                }
            } else {
                return Err(ErrorHotPath::ExchangeWSError("tbd".to_string()));
            }
        }
    }
    // TODO: Do not dynamically allocate errors in pull_depth
    async fn pull_depths(
        &mut self,
    ) -> Result<impl Iterator<Item = DepthUpdate>, Box<dyn Error + Sync + Send + 'static>> {
        let snapshot_depths = self.orderbook_snapshot().await?;
        let interleaved_depths = interleave(snapshot_depths.0, snapshot_depths.1);
        Ok(interleaved_depths)
    }
    pub async fn sequence_depths(&mut self) {
        let mut prepared_snapshot_stream = iter(&*self.websocket_depth_buffer);
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
        Box<dyn Error + Sync + Send + 'static>,
    > {
        let req_builder = self
            .http_client
            .as_ref()
            .unwrap()
            .get(self.http_snapshot_uri.clone());
        let snapshot_response_result = req_builder.send().await;
        match snapshot_response_result {
            Ok(snapshot_response) => match (snapshot_response, self.exchange_name) {
                (snapshot_response, 1) => {
                    if snapshot_response.status() != 200 {
                        return Err(Box::new(ErrorHotPath::ExchangeStreamSnapshot(
                            "failed to get snapshot through http. received error code: {}"
                                .to_string(),
                        )));
                    }
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
    pub async fn reconnect(
        &mut self,
    ) -> Result<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>, ErrorHotPath> {
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
                self.ws_connection_orderbook_reader = Some(stream);
                info!(
                    "connected to exchange {} at {}",
                    self.exchange_name, self.websocket_uri
                );
                return Ok(sink);
            }
            Err(ws_error) => {
                error!("failed to connect to exchange {}", self.websocket_uri);
                return Err(ErrorHotPath::ExchangeWSReconnectError(ws_error.to_string()));
            }
        };
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
        info!("streaming -- here");
        let mut this = self.project();
        if let Some(depth) = this.buffer.pop() {
            if let Err(channel_error) = this.depths_producer.try_send(depth) {
                match channel_error {
                    TrySendError::Full(error) => {
                        warn!(
                            "failed to try_send to order bid producer, trying again {:?}",
                            error
                        );
                        return Poll::Ready(Some(WSStreamState::SenderError));
                    }
                    TrySendError::Disconnected(error) => {
                        error!("depth producer within ExchangeStream disconnected while streaming {:?}", error);
                        return Poll::Ready(Some(WSStreamState::SenderError));
                    }
                }
            } else {
                info!("orderbook buffer success")
            }
        }
        let Some(mut orderbooks) = this.ws_connection_orderbook_reader.as_mut().as_pin_mut() else {
            error!("failed to copy the orderbooks stream");
            return Poll::Ready(Some(WSStreamState::FailedStream))
        };
        info!("streaming -- here 2");
        while let Poll::Ready(stream_option) = orderbooks.poll_next_unpin(cx) {
            match stream_option {
                Some(Ok(ws_message)) => match (&*this.exchange_name, ws_message) {
                    (1, ws_message) => {
                        if let Ok(depth_update) = WSDepthUpdateBinance::try_from(ws_message) {
                            let depths = depth_update.depths(1);
                            let woven_depths = interleave(depths.0, depths.1);
                            if *this.buffer_websocket_depths {
                                this.websocket_depth_buffer.extend(woven_depths);
                                continue;
                            }
                            debug!("receiving stream 2");
                            this.buffer.extend(woven_depths);
                        } else {
                            warn!("failed to deserialize the object.");
                        }
                    }
                    (2, ws_message) => {
                        if let Ok(depth_update) = WSDepthUpdateBinance::try_from(ws_message) {
                            let depths = depth_update.depths(2);
                            let woven_depths = interleave(depths.0, depths.1);
                            if *this.buffer_websocket_depths {
                                this.websocket_depth_buffer.extend(woven_depths);
                                continue;
                            }
                            this.buffer.extend(woven_depths);
                        } else {
                            warn!("failed to deserialize the object.");
                        }
                    }
                    (3, ws_message) => {
                        if let Ok(depth_update) = WSDepthUpdateByBit::try_from(ws_message) {
                            let depths = depth_update.depths(3);
                            let woven_depths = interleave(depths.0, depths.1);
                            if *this.buffer_websocket_depths {
                                this.websocket_depth_buffer.extend(woven_depths);
                                continue;
                            }
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
        info!("streaming -- here 1");
        return Poll::Ready(Some(WSStreamState::WaitingForDepth));
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
    use testing_traits::ProducerDefault;
    use tokio::sync::watch::channel as watchChannel;
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
                websocket_depth_buffer: Vec::new(),
                pull_retry_count: 5,
                http_snapshot_uri: String::from(""),
                ws_subscribe: false,
                ws_poll_rate: 90,
                websocket_uri: String::from(""),
                watched_pair: String::from(""),
                buffer_websocket_depths: true,
                snapshot_trigger: None,
                buffer: Vec::new(),
                ws_connection_orderbook: None,
                ws_connection_orderbook_reader: None,
                http_client: Some(Client::new()),
            };
            (Box::new(exchange_stream), depths_consumer)
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn test_receive_depths_from_ws_server() {
        let test_length_seconds = 10;
        let depth_count_client_received: Arc<syncMutex<i32>> = Arc::new(syncMutex::new(0));
        let desired_depths: i32 = 6000;
        let (mut exchange_stream, depth_consumer) = ExchangeStream::producer_default();
        let (exchange_server, _) = ExchangeServer::new("1".to_string(), 8080, 9500).unwrap();
        let exchange_server = Arc::new(Mutex::new(exchange_server));
        exchange_stream.websocket_uri =
            "ws://".to_owned() + exchange_server.lock().await.ws_ip_address().as_str();
        exchange_stream.buffer_websocket_depths = false;
        let depth_count = depth_count_client_received.clone();
        sleep(Duration::from_secs(2)).await;
        thread::spawn(move || loop {
            if let Ok(_) = depth_consumer.try_recv() {
                let mut count = depth_count.lock().unwrap();
                *count = *count + 1;
                info!("depth count is: {}", *count);
            }
        });
        let _ = tokio::spawn(async move {
            let server_clone = exchange_server.clone();
            tokio::spawn(async move {
                let _ = server_clone.lock().await.run_websocket().await;
            });
            sleep(Duration::from_secs(2)).await;
            let server_clone = exchange_server.clone();
            tokio::spawn(async move {
                'send_loop: loop {
                    sleep(Duration::from_nanos(1)).await;
                    let result = server_clone.lock().await.supply_depths().await;
                    match result {
                        Ok(result) => {
                            debug!("send success {:?}", result);
                        }
                        Err(e) => {
                            debug!("send error {:?}", e);
                            continue 'send_loop;
                        }
                    }
                }
            });
        });
        tokio::spawn(async move {
            sleep(Duration::from_secs(2)).await;
            _ = exchange_stream.start().await;
            tokio::spawn(async move {
                loop {
                    sleep(Duration::from_nanos(1)).await;
                    exchange_stream.next().await;
                }
            });
        });
        sleep(Duration::from_secs(test_length_seconds)).await;
        let count = depth_count_client_received.lock().unwrap();
        assert!(*count >= desired_depths)
    }

    #[tokio::test]
    #[traced_test]
    async fn test_trigger_snapshots() {
        let test_length_seconds = 10;
        let depth_count_client_received: Arc<syncMutex<i32>> = Arc::new(syncMutex::new(0));
        let desired_depths: i32 = 15;
        let (trigger_producer, trigger_consumer) = watchChannel(());
        let (mut exchange_stream, depth_consumer) = ExchangeStream::producer_default();
        exchange_stream.snapshot_enabled = true;
        exchange_stream.snapshot_trigger = Some(trigger_consumer);
        let (exchange_server, http_shutdown) =
            ExchangeServer::new("1".to_string(), 8081, 9501).unwrap();
        let exchange_server = Arc::new(Mutex::new(exchange_server));
        exchange_stream.websocket_uri =
            "ws://".to_owned() + exchange_server.lock().await.ws_ip_address().as_str();
        exchange_stream.http_snapshot_uri = "http://".to_owned()
            + exchange_server.lock().await.http_ip_address().as_str()
            + "/depths";
        debug!("http: {:?}", exchange_stream.http_snapshot_uri);
        debug!("ws: {:?}", exchange_stream.websocket_uri);
        let depth_count_clone = depth_count_client_received.clone();
        thread::spawn(move || loop {
            if let Ok(_) = depth_consumer.try_recv() {
                let mut count = depth_count_clone.lock().unwrap();
                *count = *count + 1;
                info!("received")
            }
        });
        sleep(Duration::from_secs(1)).await;
        let _ = tokio::spawn(async move {
            let server_clone = exchange_server.clone();
            tokio::spawn(async move {
                let _ = server_clone.lock().await.run_websocket().await;
            });
            let server_clone = exchange_server.clone();
            tokio::spawn(async move {
                let _ = server_clone.lock().await.run_http_server().await;
            });
        });
        debug!("booting up exchange stream");
        sleep(Duration::from_secs(3)).await;
        tokio::spawn(async move {
            let result = exchange_stream.start().await;
            if result.is_err() {
                debug!("failed to start stream")
            }
            loop {
                sleep(Duration::from_secs(1)).await;
                debug!("RUNNING exchange_stream");
                exchange_stream.run().await;
            }
        });
        sleep(Duration::from_secs(7)).await;
        debug!("triggering snapshot");
        let result = trigger_producer.send(());
        if result.is_err() {
            debug!("received error {:?}", result);
        }
        sleep(Duration::from_secs(test_length_seconds)).await;
        let count = depth_count_client_received.lock().unwrap();
        assert!(*count >= desired_depths);
        let _ = http_shutdown.send(());
    }

    #[tokio::test]
    #[traced_test]
    async fn test_trigger_with_ws_depths() {
        let test_length_seconds = 20;
        let ws_depths: i32 = 10;
        let desired_depths: i32 = 80 + ws_depths; // NOTE currently our depth snapshot has 80 total depth
                                                  // updates we want to account for that plus some ws stream
                                                  // depths
        let depth_count_client_received: Arc<syncMutex<i32>> = Arc::new(syncMutex::new(0));
        let (exchange_server, http_shutdown) =
            ExchangeServer::new("1".to_string(), 8082, 9502).unwrap();
        let exchange_server = Arc::new(Mutex::new(exchange_server));
        let (trigger_producer, trigger_consumer) = watchChannel(());
        let (mut exchange_stream, depth_consumer) = ExchangeStream::producer_default();
        exchange_stream.snapshot_enabled = true;
        exchange_stream.snapshot_trigger = Some(trigger_consumer);
        exchange_stream.websocket_uri =
            "ws://".to_owned() + exchange_server.lock().await.ws_ip_address().as_str();
        exchange_stream.http_snapshot_uri = "http://".to_owned()
            + exchange_server.lock().await.http_ip_address().as_str()
            + "/depths";
        exchange_stream.buffer_websocket_depths = false;
        debug!("http: {:?}", exchange_stream.http_snapshot_uri);
        debug!("ws: {:?}", exchange_stream.websocket_uri);
        let depth_count_clone = depth_count_client_received.clone();
        thread::spawn(move || loop {
            if let Ok(_) = depth_consumer.try_recv() {
                let mut count = depth_count_clone.lock().unwrap();
                *count = *count + 1;
                info!("depth count is: {}", *count);
            }
        });
        let _ = tokio::spawn(async move {
            let server_clone = exchange_server.clone();
            tokio::spawn(async move {
                let _ = server_clone.lock().await.run_websocket().await;
            });
            let server_clone = exchange_server.clone();
            tokio::spawn(async move {
                let mut server = server_clone.lock().await;
                let _ = server.run_http_server().await;
                info!("shutting down the http server");
            });
            let server_clone = exchange_server.clone();
            let mut depths_sent_through_websocket_count: i32 = 0;
            tokio::spawn(async move {
                sleep(Duration::from_secs(15)).await;
                // shutdown the server so we can release the
                // exchange server lock to start sending ws
                // depths
                _ = http_shutdown.send(()).await;
                loop {
                    if ws_depths == depths_sent_through_websocket_count {
                        return;
                    }
                    sleep(Duration::from_nanos(1)).await;
                    let _ = server_clone.lock().await.supply_depths().await;
                    depths_sent_through_websocket_count += 1;
                }
            });
        });
        sleep(Duration::from_secs(5)).await;
        tokio::spawn(async move {
            let _ = exchange_stream.start().await;
            loop {
                info!("streaming");
                sleep(Duration::from_nanos(1)).await;
                exchange_stream.run().await;
            }
        });
        sleep(Duration::from_secs(5)).await;
        let result = trigger_producer.send(());
        if result.is_err() {
            debug!("received error {:?}", result);
        }

        sleep(Duration::from_secs(test_length_seconds)).await;
        let count = depth_count_client_received.lock().unwrap();
        assert!(*count >= desired_depths)
    }
}

// test stream with trigger select
// test stream triggered snapsho
// test stream sequenced depths
