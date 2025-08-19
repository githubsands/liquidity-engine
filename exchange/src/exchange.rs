use futures::stream::SplitSink;
use futures_util::SinkExt;

use tokio::net::TcpStream;
use tokio::sync::watch::Receiver as watchReceiver;
use tokio_tungstenite::{tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream};

use crossbeam_channel::Sender;

use tracing::{error, info};

use crate::stream::ExchangeStream;
use config::ExchangeConfig;
use market_objects::DepthUpdate;
use quoter_errors::{ErrorHotPath, ErrorInitialState};

const SUBSCRIBE: &'static str = "SUBSCRIBE";

pub struct Exchange {
    pub inner: ExchangeStream,
    pub ws_sink: Option<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>,
    pub websocket_uri: String,
    pub watched_pair: String,
}

impl Exchange {
    pub fn new(
        exchange_config: &ExchangeConfig,
        depth_producer: Sender<DepthUpdate>,
        watch_trigger: watchReceiver<()>,
    ) -> Result<Exchange, ErrorInitialState> {
        let inner = ExchangeStream::new(
            exchange_config,
            depth_producer.clone(),
            watch_trigger,
            exchange_config.http_client,
        )?;
        Ok(Exchange {
            inner,
            ws_sink: None,
            websocket_uri: exchange_config.ws_uri.clone(),
            watched_pair: exchange_config.watched_pair.clone(),
        })
    }
    pub async fn start(&mut self) -> Result<(), ErrorInitialState> {
        let ws_sink = self.inner.start().await?;
        self.ws_sink = Some(ws_sink);
        Ok(())
    }
    pub async fn subscribe_orderbooks(&mut self) -> Result<(), ErrorInitialState> {
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
            .ok_or(ErrorInitialState::ExchangeController)?
            .send(Message::Text(json_obj_binance.to_string()))
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

    pub async fn run_snapshot(&mut self) -> Result<(), ErrorInitialState> {
        self.inner.run_snapshot().await?;
        Ok(())
    }

    pub async fn push_buffered_ws_depths(&mut self) {
        self.inner.push_buffered_ws_depths().await;
    }

    pub async fn stream_depths(&mut self) -> Result<(), ErrorHotPath> {
        info!("streaming depths");
        self.inner.run().await;
        Ok(())
    }

    async fn reconnect(&mut self) -> Result<(), ErrorHotPath> {
        let ws_sink = self.inner.reconnect().await?;
        self.ws_sink = Some(ws_sink);
        Ok(())
    }

    pub async fn close(&mut self) -> Result<(), ErrorInitialState> {
        self.ws_sink
            .as_mut()
            .ok_or(ErrorInitialState::ExchangeController)?
            .send(Message::Close(None));
        Ok(())
    }
}
