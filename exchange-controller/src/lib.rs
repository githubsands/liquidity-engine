use crossbeam_channel::Sender;

use market_objects::DepthUpdate;
use quoter_errors::{ErrorHotPath, ErrorInitialState};

use config::ExchangeConfig;
use exchange_stream::ExchangeStream;

use futures::stream::SplitSink;
use futures_util::{try_join, SinkExt};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::{tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream};

use futures::future::join_all;
use tokio_context::context::Context;

use tokio::sync::watch::{
    channel as watchChannel, Receiver as watchReceiver, Sender as watchSender,
};

use futures::pin_mut;

use futures::stream::FuturesOrdered;
use futures::StreamExt;
use std::pin::Pin;

use std::rc::Rc;

use seq_macro::seq;
use std::cell::RefCell;

pub struct ExchangeController {
    exchanges: Vec<Rc<RefCell<Exchange>>>,
    orderbook_snapshot_trigger: watchReceiver<()>,
    exchange_snapshot_trigger: watchSender<()>,
}

struct Exchange {
    pub inner: ExchangeStream,
    pub ws_sink: Option<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>,
    pub websocket_uri: String,
    pub watched_pair: String,
}

impl Exchange {
    fn new(
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
    async fn start(&mut self) -> Result<(), ErrorInitialState> {
        let ws_sink = self.inner.start().await?;
        self.ws_sink = Some(ws_sink);
        Ok(())
    }
    async fn subscribe_orderbooks(&mut self) -> Result<(), ErrorInitialState> {
        // TODO: Add many different subscription messages here and a configurable trigger
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

    async fn run_snapshot(&mut self) -> Result<(), ErrorInitialState> {
        self.inner.run_snapshot().await?;
        Ok(())
    }

    async fn push_buffered_ws_depths(&mut self) {
        self.inner.push_buffered_ws_depths().await;
    }

    async fn stream_depths(&mut self) -> Result<(), ErrorHotPath> {
        self.inner.run().await;
        Ok(())
    }

    async fn reconnect(&mut self) -> Result<(), ErrorHotPath> {
        let ws_sink = self.inner.reconnect().await?;
        self.ws_sink = Some(ws_sink);
        Ok(())
    }

    async fn close(&mut self) {
        let _ = self.ws_sink.as_mut().unwrap().send(Message::Close(None));
    }
}

// TODO: Need to propagate around the orderbook_snapshot_trigger
impl ExchangeController {
    pub fn new(
        exchange_configs: &Vec<ExchangeConfig>,
        depths_producer: Sender<DepthUpdate>,
        orderbook_snapshot_trigger: watchReceiver<()>,
    ) -> Result<ExchangeController, ErrorInitialState> {
        let mut exchanges: Vec<Rc<RefCell<Exchange>>> = Vec::new();
        let (snapshot_trigger, inner_snapshot_consumer) = watchChannel(());
        for exchange_config in exchange_configs {
            let exchange = Exchange::new(
                exchange_config,
                depths_producer.clone(),
                inner_snapshot_consumer.clone(),
            );
            exchanges.push(Rc::new(RefCell::new(exchange)));
        }
        Ok(ExchangeController {
            exchanges,
            orderbook_snapshot_trigger,
            exchange_snapshot_trigger: snapshot_trigger,
        })
    }

    pub async fn websocket_connect(&mut self) -> Result<(), ErrorInitialState> {
        for exchange in self.exchanges.iter_mut() {
            exchange.as_ref().borrow_mut().start().await?;
        }
        Ok(())
    }

    pub async fn subscribe_depths(&mut self) -> Result<(), ErrorInitialState> {
        for exchange in self.exchanges.iter_mut() {
            exchange
                .as_ref()
                .borrow_mut()
                .subscribe_orderbooks()
                .await?;
        }
        Ok(())
    }

    pub async fn build_orderbook(&mut self) -> Result<(), ErrorInitialState> {
        for exchange in self.exchanges.iter_mut() {
            exchange.as_ref().borrow_mut().run_snapshot().await?;
        }
        for exchange in self.exchanges.iter_mut() {
            exchange
                .as_ref()
                .borrow_mut()
                .push_buffered_ws_depths()
                .await;
        }
        Ok(())
    }

    pub async fn run_streams(&mut self, ctx: &mut Context) -> Result<(), ErrorHotPath> {
        let inner_trigger = &mut self.orderbook_snapshot_trigger;
        let mut exchanges: Vec<*mut Exchange> = vec![];
        let mut streams = FuturesOrdered::new();
        for exchange in &mut self.exchanges {
            exchanges.push(exchange.as_ref().as_ptr())
        }

        unsafe {
            for stream in exchanges {
                streams.push_back(stream.as_mut().unwrap().stream_depths())
            }
        }

        pin_mut!(streams);

        loop {
            tokio::select! {
                    // TODO: This may not rebuild orderbook correctly but the trigger currently is not
                    // implemented within the orderbook. This future handles orderbook rebuilds.
                        _ = inner_trigger.changed()=> {
                            // send snapshot trigger to our N exchange streams
                            // this sets a streams run function to grabs a http depth load from multiplie exchanges, and buffers
                            // the websocket depths.
                            let _ = self.exchange_snapshot_trigger.send(());
                        }
                        _ = streams.next() => {}
                        _ = ctx.done() => {
                            return Ok(());
                        }
            }
        }
    }

    pub async fn close_exchanges(&mut self) {
        for exchange in &mut self.exchanges {
            let mut exchange = exchange.borrow_mut();
            exchange.close().await
        }
    }
}
