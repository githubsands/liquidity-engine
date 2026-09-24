use futures::stream::SplitSink;
use futures_util::SinkExt;

use compio::net::TcpStream;
use compio::ws::{tungstenite::protocol::Message, WebSocketStream};
use tokio::sync::watch::Receiver as watchReceiver;

use kanal::*;

use tracing::{error, info};

use crate::stream::{ExchangeStream, DEPTH_BUFFER_SIZE};

// most depth slots one exchange can hold at once: its three depth buffers plus a
// snapshot being ingested. its DepthPool is sized from this plus the depth channel.
pub const MAX_BUFFERED_DEPTHS: usize = 4 * DEPTH_BUFFER_SIZE;
use config::ExchangeConfig;
use depth_pool::{DepthPool, DepthSlot};
use std::cell::RefCell;
use std::rc::Rc;
use crate::errors::ExchangeStreamError;

const SUBSCRIBE: &'static str = "SUBSCRIBE";

pub struct Exchange {
    pub inner: ExchangeStream,
    pub ws_sink: Option<SplitSink<WebSocketStream<TcpStream>, Message>>,
    pub websocket_uri: String,
    pub watched_pair: String,
    // this exchange's own depth pool: every DepthSlot it sends points in here. shared
    // with `inner` (the writer) and with the reader, which releases the slots; all of
    // them run on the same compio thread, so Rc<RefCell<_>> is enough
    pub depth_pool: Rc<RefCell<DepthPool>>,
}

impl Exchange {
    // new builds this exchange's depth pool, stamped with `pool_id`. it holds enough slots
    // for a full depth channel plus every buffer of this exchange, so it only runs dry if
    // the reader stops releasing slots. `depth_producer` must be a bounded channel.
    pub fn new(
        exchange_config: &ExchangeConfig,
        pool_id: u16,
        depth_producer: AsyncSender<DepthSlot>,
        watch_trigger: watchReceiver<()>,
    ) -> Result<Exchange, ExchangeStreamError> {
        let pool_capacity = depth_producer
            .capacity()
            .checked_add(MAX_BUFFERED_DEPTHS)
            .filter(|&capacity| capacity <= u32::MAX as usize)
            .ok_or_else(|| {
                ExchangeStreamError::DepthPool(
                    "depth channel must be bounded to size the depth pool".to_string(),
                )
            })?;
        let depth_pool = Rc::new(RefCell::new(DepthPool::with_capacity(
            pool_id,
            pool_capacity,
        )));
        let inner = ExchangeStream::new(
            exchange_config,
            depth_producer.clone(),
            depth_pool.clone(),
            watch_trigger,
            exchange_config.http_client,
        )?;
        Ok(Exchange {
            inner,
            ws_sink: None,
            websocket_uri: exchange_config.ws_uri.clone(),
            watched_pair: exchange_config.watched_pair.clone(),
            depth_pool,
        })
    }
    pub async fn start(&mut self) -> Result<(), ExchangeStreamError> {
        let ws_sink = self.inner.start().await?;
        self.ws_sink = Some(ws_sink);
        Ok(())
    }
    pub async fn subscribe_orderbooks(&mut self) -> Result<(), ExchangeStreamError> {
        // TODO: Add many different subscription messages here and a configurable trigger
        info!(
            "exchange {} subscribing to the orderbooks",
            self.websocket_uri
        );
        let json_obj_binance = serde_json::json!({
            "method": SUBSCRIBE,
            "params": [
                "btcusdt@depth5",
            ],
            "id": 1
        });
        let exchange_response = self
            .ws_sink
            .as_mut()
            .ok_or(ExchangeStreamError::ExchangeController)?
            .send(Message::Text(json_obj_binance.to_string().into()))
            .await;
        // TODO: handle this differently;
        match exchange_response {
            Ok(response) => {
                info!("subscription success: {:?}", response);
            }
            Err(error) => {
                error!("error {}", error)
            }
        }
        Ok(())
    }

    pub async fn run_snapshot(&mut self) -> Result<(), ExchangeStreamError> {
        self.inner.run_snapshot().await?;
        Ok(())
    }

    pub async fn push_buffered_ws_depths(&mut self) {
        self.inner.push_buffered_ws_depths().await;
    }

    pub async fn stream_depths(&mut self) -> Result<(), ExchangeStreamError> {
        info!("streaming depths");
        self.inner.run().await?;
        Ok(())
    }

    async fn reconnect(&mut self) -> Result<(), ExchangeStreamError> {
        let ws_sink = self.inner.reconnect().await?;
        self.ws_sink = Some(ws_sink);
        Ok(())
    }

    pub async fn close(&mut self) -> Result<(), ExchangeStreamError> {
        self.ws_sink
            .as_mut()
            .ok_or(ExchangeStreamError::ExchangeController)?
            .close()
            .await
            .map_err(|e| ExchangeStreamError::ExchangeWSError(e.to_string()))?;
        Ok(())
    }
}
