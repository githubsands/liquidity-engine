use std::pin::Pin;
use std::time::Duration;

use futures::stream::SplitSink;
use futures_util::{try_join, SinkExt};

use futures::future::join_all;
use tokio::net::TcpStream;
use tokio::sync::watch::{
    channel as watchChannel, Receiver as watchReceiver, Sender as watchSender,
};
use tokio_tungstenite::{tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream};
use tracing::info;

use crossbeam_channel::Sender;

use config::ExchangeConfig;
use exchange_stream::ExchangeStream;
use market_objects::DepthUpdate;
use quoter_errors::{ErrorHotPath, ErrorInitialState};

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
    ) -> Self {
        let inner = ExchangeStream::new(
            exchange_config,
            depth_producer.clone(),
            watch_trigger,
            exchange_config.http_client,
        )
        .unwrap();
        Exchange {
            inner: inner,
            ws_sink: None,
            websocket_uri: exchange_config.ws_uri.clone(),
            watched_pair: exchange_config.watched_pair.clone(),
        }
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
            "method": "SUBSCRIBE",
            "params": [
                "btcusdt@depth",
            ],
            "id": 1
        });
        let exchange_response = self
            .ws_sink
            .as_mut()
            .unwrap()
            .send(Message::Text(json_obj_binance.to_string()))
            .await;
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

    pub async fn close(&mut self) {
        let _ = self.ws_sink.as_mut().unwrap().send(Message::Close(None));
    }
}
