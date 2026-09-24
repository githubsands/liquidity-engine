use arrayvec::ArrayVec;
use std::{pin::Pin, task::Poll};

use crate::errors::ExchangeStreamError;

use pin_project_lite::pin_project;

use std::time::Duration;

use futures::future::FutureExt;
use futures::stream::iter;
use futures::stream::{SplitSink, SplitStream, Stream, StreamExt};
use futures::{pin_mut, select_biased};
use tokio::sync::watch::Receiver as stateSync;

use compio::net::TcpStream;
use compio::time::sleep;
use compio::ws::{
    connect_async_with_config,
    tungstenite::protocol::{Message, WebSocketConfig},
    Config as WsConfig, WebSocketStream,
};

use kanal::*;

use cyper::Client;
use tracing::{error, info, warn};

use std::cell::RefCell;
use std::rc::Rc;

use config::ExchangeConfig;
use depth_pool::{DepthPool, DepthPoolError, DepthSlot};
use market_objects::book::parse_binance_book;

// compio-ws wraps TLS internally, so the stream is parameterised on the raw socket
type WsStream = WebSocketStream<TcpStream>;

// capacity of each of ExchangeStream's depth buffers
// todo: make this configurable
pub const DEPTH_BUFFER_SIZE: usize = 1000;

type DepthBuffer = ArrayVec<DepthSlot, DEPTH_BUFFER_SIZE>;

#[derive(thiserror::Error, Debug)]
enum IngestError {
    #[error("depth buffer or depth pool is full")]
    Full,
    #[error("failed to parse order book message: {0}")]
    Parse(#[from] serde_json::Error),
}

// ingest_book parses an order book payload straight into depth pool slots appended to
// `target`: prices and quantities are read from the borrowed bytes and archived once, in
// place. on any failure every slot taken for this payload is released so nothing leaks.
fn ingest_book(
    json: &[u8],
    location: u8,
    snapshot: bool,
    pool: &mut DepthPool,
    target: &mut DepthBuffer,
) -> Result<(), IngestError> {
    let start = target.len();
    let mut full = false;
    let parsed = parse_binance_book(json, location, snapshot, |depth| {
        if target.is_full() {
            full = true;
            return false;
        }
        match pool.insert(&depth) {
            Ok(slot) => {
                target.push(slot);
                true
            }
            Err(DepthPoolError::Exhausted) => {
                full = true;
                false
            }
            Err(e) => {
                warn!("failed to archive depth: {}", e);
                false
            }
        }
    });
    if let Err(e) = parsed {
        for slot in target.drain(start..) {
            pool.release(slot);
        }
        return Err(if full { IngestError::Full } else { e.into() });
    }
    Ok(())
}

fn ws_config() -> WsConfig {
    let ws_config = WebSocketConfig::default()
        .max_message_size(None)
        .max_frame_size(None)
        .accept_unmasked_frames(true);
    // market data is many small frames: don't let nagle hold them back
    WsConfig::from(ws_config).disable_nagle(true)
}

pin_project! {
    #[must_use = "streams do nothing unless polled"]
    pub struct ExchangeStream {
        pub client_name: String,
        pub exchange_name: u8,
        pub snapshot_enabled: bool,
        // todo: update webssocket_depth_buffer take the array vecs size by pragma or something similar -- it should be configurable
        pub websocket_depth_buffer: DepthBuffer,
        pub pull_retry_count: u8,
        pub http_snapshot_uri: String,
        pub buffer_websocket_depths: bool,
        pub ws_subscribe: bool,
        pub ws_poll_rate: u64,
        pub stream_count: u8,
        pub websocket_uri: String,
        pub watched_pair: String,

        pub buffer: DepthBuffer,

        pub snapshot_sync: Option<stateSync<()>>,

        // todo: update webssocket_depth_buffer take the array vecs size by pragma or something similar -- it should be configurable
        #[pin]
        depth_update_buffer: DepthBuffer,
        #[pin]
        ws_connection_orderbook: Option<WsStream>,
        #[pin]
        ws_connection_orderbook_reader: Option<SplitStream<WsStream>>,

        // carries slot handles; the depths themselves stay archived in `depth_pool`
        depths_producer: AsyncSender<DepthSlot>,
        // the owning Exchange's depth pool
        pub depth_pool: Rc<RefCell<DepthPool>>,

        http_client: Option<Client>,
    }
}

impl ExchangeStream {
    pub fn new(
        exchange_config: &ExchangeConfig,
        orders_producer: AsyncSender<DepthSlot>,
        depth_pool: Rc<RefCell<DepthPool>>,
        snapshot_sync: stateSync<()>,
        _http_client_option: bool,
    ) -> Result<Self, ExchangeStreamError> {
        let mut http_client: Option<Client> = None;
        if exchange_config.snapshot_enabled {
            info!("snapshot is enabled building http client");
            http_client = Some(
                Client::new().map_err(|e| ExchangeStreamError::HttpRequest(e.to_string()))?,
            );
        }
        let exchange = ExchangeStream {
            client_name: exchange_config.client_name.clone(),
            exchange_name: exchange_config.exchange_name,
            snapshot_sync: Some(snapshot_sync),
            buffer: ArrayVec::new(),
            websocket_depth_buffer: ArrayVec::new(),
            buffer_websocket_depths: false,
            snapshot_enabled: exchange_config.snapshot_enabled,
            pull_retry_count: 5,
            http_snapshot_uri: exchange_config.snapshot_uri.clone(),
            ws_subscribe: false,
            ws_poll_rate: exchange_config.ws_poll_rate_milliseconds.into(),
            websocket_uri: exchange_config.ws_uri.clone(),
            watched_pair: exchange_config.watched_pair.clone(),
            stream_count: 0,
            ws_connection_orderbook: None::<WsStream>,
            depth_update_buffer: ArrayVec::new(),
            ws_connection_orderbook_reader: None,
            depths_producer: orders_producer,
            depth_pool,
            http_client,
        };
        Ok(exchange)
    }
    pub async fn start(
        &mut self,
    ) -> Result<SplitSink<WsStream, Message>, ExchangeStreamError>
    {
        let ws_conn_result = connect_async_with_config(self.websocket_uri.as_str(), ws_config()).await;
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
                return Err(ExchangeStreamError::WSConnection(ws_error.to_string()));
            }
        };
    }

    pub async fn run_snapshot(&mut self) -> Result<(), ExchangeStreamError> {
        self.buffer_websocket_depths = true;
        let mut success = false;
        while !success {
            sleep(Duration::from_secs(2)).await;
            let pull_result = self.pull_depths().await;
            match pull_result {
                Ok(mut depths) => {
                    while let Some(depth) = depths.next() {
                        self.depth_update_buffer.push(depth);
                        // we must keep processing snapshot depths and depths from the websocket
                        // but this time the websocket depths are stored in their own buffer
                        // to be sequenced aftr snapshot depths are processed
                        self.next().await;
                    }
                    success = true;
                }
                Err(pull_error) => {
                    error!(
                        "failed to get websocket depths from exchange {}",
                        pull_error
                    );
                    return Err(ExchangeStreamError::Snapshot(pull_error.to_string()));
                }
            }
        }
        Ok(())
    }

    pub async fn push_buffered_ws_depths(&mut self) {
        while let Some(websocket_depth) = self.websocket_depth_buffer.pop() {
            self.depth_update_buffer.push(websocket_depth);
        }
        self.buffer_websocket_depths = false;
    }

    pub async fn run_with_snapshot(&mut self) -> Result<(), ExchangeStreamError> {
        enum Wake {
            Snapshot,
            Poll,
        }
        // the futures only borrow `snapshot_sync`; they're dropped at the end of this block
        // so the branches below are free to use the rest of `self`
        let wake = {
            let snapshot_sync = self.snapshot_sync.as_mut().unwrap();
            let snapshot = snapshot_sync.changed().fuse();
            let poll = sleep(Duration::from_millis(self.ws_poll_rate)).fuse();
            pin_mut!(snapshot, poll);
            select_biased! {
                _ = snapshot => Wake::Snapshot,
                _ = poll => Wake::Poll,
            }
        };
        match wake {
            Wake::Snapshot => {
                info!("pulling websocket");
                self.buffer_websocket_depths = true;
                let mut pull_retry_count = 0;
                while pull_retry_count < self.pull_retry_count {
                    let pull_result = self.pull_depths().await;
                    match pull_result {
                        Ok(mut depths) => {
                            while let Some(depth) = depths.next() {
                                self.buffer.push(depth);
                                self.next().await;
                            }
                            break;
                        }
                        Err(pull_error) => {
                            if pull_retry_count > self.pull_retry_count {
                                error!("reached maxed snapshot pull count retry for exchange {} received error: {}", self.exchange_name, pull_error);
                                return Err(ExchangeStreamError::ExchangeStreamSnapshot(
                                    pull_error.to_string(),
                                ));
                            }
                            pull_retry_count += 1;
                            warn!("failed to get websocket depths from exchange {}", pull_error);
                            continue;
                        }
                    }
                }
                // we are done. push snapshot depths to the orderbook - turn this buffer off
                // and push the buffer websocket depths to the orderbook
                self.buffer_websocket_depths = false;
                while let Some(websocket_depth) = self.websocket_depth_buffer.pop() {
                    self.buffer.push(websocket_depth);
                    self.next().await;
                }
                Ok(())
            }
            Wake::Poll => {
                if let Some(stream_poll_state) = self.next().await {
                    match stream_poll_state {
                        WSStreamState::Success | WSStreamState::WaitingForDepth => Ok(()),
                        _ => Err(ExchangeStreamError::ExchangeWSError("tbd".to_string())),
                    }
                } else {
                    Err(ExchangeStreamError::ExchangeWSError("tbd".to_string()))
                }
            }
        }
    }

    pub async fn run(&mut self) -> Result<(), ExchangeStreamError> {
        sleep(Duration::from_millis(self.ws_poll_rate)).await;
        if let Some(stream_poll_state) = self.next().await {
            match stream_poll_state {
                WSStreamState::Success | WSStreamState::WaitingForDepth => return Ok(()),
                _ => 'error: {
                    break 'error;
                }
            }
        }
        return Err(ExchangeStreamError::ExchangeWSError("tbd".to_string()));
    }

    async fn pull_depths(&mut self) -> Result<impl Iterator<Item = DepthSlot>, ExchangeStreamError> {
        let body = self.orderbook_snapshot().await?;
        let mut depths = DepthBuffer::new();
        ingest_book(
            &body,
            self.exchange_name,
            true,
            &mut self.depth_pool.borrow_mut(),
            &mut depths,
        )
        .map_err(|e| ExchangeStreamError::Snapshot(e.to_string()))?;
        info!("finished receiving snaps for {}", self.exchange_name);
        Ok(depths.into_iter())
    }
    pub async fn sequence_depths(&mut self) {
        let mut prepared_snapshot_stream = iter(&*self.websocket_depth_buffer);
        while let Some(depth_update) = prepared_snapshot_stream.next().await {
            self.buffer.push(*depth_update)
        }
    }
    // orderbook_snapshot fetches the raw snapshot body; it is parsed in place by pull_depths
    async fn orderbook_snapshot(
        &mut self,
    ) -> Result<impl std::ops::Deref<Target = [u8]>, ExchangeStreamError> {
        if !matches!(self.exchange_name, 1 | 2) {
            error!(
                "failed to create snapshot due to exchange_name {}",
                self.exchange_name
            );
            return Err(ExchangeStreamError::Snapshot(
                "Failed to create snapshot".to_string(),
            ));
        }
        let req_builder = self
            .http_client
            .as_ref()
            .unwrap()
            .get(self.http_snapshot_uri.as_str())
            .map_err(|e| ExchangeStreamError::HttpRequest(e.to_string()))?;
        let snapshot_response = req_builder.send().await.map_err(|err| {
            error!("failed to reconcile snapshot result: {}", err);
            ExchangeStreamError::Snapshot(err.to_string())
        })?;
        if snapshot_response.status() != 200 {
            return Err(ExchangeStreamError::ExchangeStreamSnapshot(format!(
                "failed to get snapshot through http. received error code: {}",
                snapshot_response.status()
            )));
        }
        snapshot_response
            .bytes()
            .await
            .map_err(|e| ExchangeStreamError::HttpRequest(e.to_string()))
    }
    pub async fn reconnect(
        &mut self,
    ) -> Result<SplitSink<WsStream, Message>, ExchangeStreamError> {
        let ws_conn_result = connect_async_with_config(self.websocket_uri.as_str(), ws_config()).await;
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
                return Err(ExchangeStreamError::ExchangeWSReconnectError(ws_error.to_string()));
            }
        };
    }
}

#[derive(Debug)]
pub enum WSStreamState {
    // todo: collapse these errors into 1 Faulty state or do something differently
    // -- this is our async state not a error
    WSError(compio::ws::tungstenite::Error),
    SenderError,
    FailedStream,
    FailedDeserialize, // TODO: Pass down the deserialize error like we do with the WSError
    // a depth buffer or the depth pool had no room for a message's depths
    BufferFull,
    Success,
    WaitingForDepth,
}

impl Stream for ExchangeStream {
    type Item = WSStreamState;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let mut this = self.project();
        if let Some(slot) = this.buffer.pop() {
            match this.depths_producer.try_send(slot) {
                Ok(true) => {
                    info!("orderbook buffer success")
                }
                Ok(false) => {
                    warn!("failed to try_send to order bid producer, channel full, trying again");
                    // keep the depth for the next poll rather than dropping it
                    this.buffer.push(slot);
                    return Poll::Ready(Some(WSStreamState::SenderError));
                }
                Err(SendError::Closed) => {
                    error!("depth producer within ExchangeStream closed while streaming");
                    this.depth_pool.borrow_mut().release(slot);
                    return Poll::Ready(Some(WSStreamState::SenderError));
                }
                Err(SendError::ReceiveClosed) => {
                    error!("depth producer within ExchangeStream disconnected while streaming");
                    this.depth_pool.borrow_mut().release(slot);
                    return Poll::Ready(Some(WSStreamState::SenderError));
                }
            }
        }
        let Some(mut orderbooks) = this.ws_connection_orderbook_reader.as_mut().as_pin_mut() else {
            error!("failed to copy the orderbooks stream");
            return Poll::Ready(Some(WSStreamState::FailedStream));
        };
        while let Poll::Ready(stream_option) = orderbooks.poll_next_unpin(cx) {
            match stream_option {
                Some(Ok(ws_message)) => match (&*this.exchange_name, ws_message) {
                    (1, ws_message) => {
                        // ignore the NULL message from binance
                        if *this.stream_count != 1 {
                            *this.stream_count = 1;
                            return Poll::Ready(Some(WSStreamState::Success));
                        }
                        if ws_message.is_pong() || ws_message.is_ping() {
                            continue;
                        }
                        let Message::Text(text) = &ws_message else {
                            continue;
                        };
                        let ingested = ingest_book(
                            text.as_bytes(),
                            1,
                            false,
                            &mut this.depth_pool.borrow_mut(),
                            this.buffer,
                        );
                        match ingested {
                            Ok(()) => {}
                            Err(IngestError::Full) => {
                                warn!("no room for websocket depths, dropping message");
                                return Poll::Ready(Some(WSStreamState::BufferFull));
                            }
                            Err(e) => {
                                warn!("failed to deserialize the web socket messsage: {}", e);
                                continue;
                            }
                        }
                    }
                    (2, ws_message) => {
                        if *this.stream_count != 1 {
                            // ignore the NULL message from binance
                            *this.stream_count = 1;
                            return Poll::Ready(Some(WSStreamState::Success));
                        }
                        if ws_message.is_pong() || ws_message.is_ping() {
                            continue;
                        }
                        let Message::Text(text) = &ws_message else {
                            continue;
                        };
                        let ingested = ingest_book(
                            text.as_bytes(),
                            1,
                            false,
                            &mut this.depth_pool.borrow_mut(),
                            &mut *this.depth_update_buffer,
                        );
                        match ingested {
                            Ok(()) => {}
                            Err(IngestError::Full) => {
                                warn!("no room for websocket depths, dropping message");
                                return Poll::Ready(Some(WSStreamState::BufferFull));
                            }
                            Err(e) => {
                                warn!("failed to deserialize the web socket messsage: {}", e);
                                continue;
                            }
                        }
                    }
                    // todo: add different exchanges back in
                    /*
                    (3, ws_message) => {
                        if let Ok(depth_update) = WSDepthUpdateByBit::try_from(ws_message) {
                            let depths = depth_update.depths(3);
                            let woven_depths = interleave(depths.0, depths.1);
                            if *this.buffer_websocket_depths {
                                for depth in woven_depths {
                                    this.depth_update_buffer.push(depth);
                                }
                                continue;
                            }
                            for depth in woven_depths {
                                this.depth_update_buffer.push(depth);
                            }
                        } else {
                            warn!("failed to deserialize the object.");
                        }
                    }
                    */
                    _ => break,
                },
                Some(Err(ws_error)) => {
                    return Poll::Ready(Some(WSStreamState::WSError(ws_error)));
                }
                _ => {
                    return Poll::Ready(Some(WSStreamState::WaitingForDepth));
                }
            }
        }
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
                stream_count: 0,
                depths_producer,
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
                snapshot_sync: None,
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
        let desired_depths: i32 = 5000;
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
