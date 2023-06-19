use crossbeam_channel::Sender;

use market_objects::DepthUpdate;
use quoter_errors::ErrorInitialState;

use config::ExchangeConfig;
use exchange_stream::ExchangeStream;

use futures::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream};

use tokio::sync::watch::Receiver as watchReceiver;

use std::rc::Rc;

use std::cell::RefCell;

pub struct ExchangeController {
    exchanges: Vec<Rc<RefCell<Exchange>>>,
}

struct Exchange {
    pub exchange_stream: Rc<RefCell<ExchangeStream>>,
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
        let exchange_stream = ExchangeStream::new(
            exchange_config,
            depth_producer.clone(),
            watch_trigger,
            exchange_config.http_client,
        )
        .unwrap();
        Exchange {
            exchange_stream: exchange_stream,
            ws_sink: None,
            websocket_uri: exchange_config.ws_uri.clone(),
            watched_pair: exchange_config.watched_pair.clone(),
        }
    }
    async fn start(&mut self) -> Result<(), ErrorInitialState> {
        let stream = self.exchange_stream.as_ref();
        let ws_sink = stream.borrow_mut().start().await?;
        self.ws_sink = Some(ws_sink);
        Ok(())
    }
    async fn subscribe_orderbooks(&mut self) -> Result<(), ErrorInitialState> {
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
    async fn stream_depths(&mut self) {
        self.exchange_stream.as_ref().borrow_mut().next().await;
    }
}

// TODO: Need to propagate around the orderbook_snapshot_trigger
impl ExchangeController {
    pub fn new(
        exchange_configs: &Vec<ExchangeConfig>,
        depths_producer: Sender<DepthUpdate>,
        watch_trigger: watchReceiver<()>,
    ) -> Result<ExchangeController, ErrorInitialState> {
        let mut exchanges: Vec<Rc<RefCell<Exchange>>> = Vec::new();
        for exchange_config in exchange_configs {
            let exchange = Exchange::new(
                exchange_config,
                depths_producer.clone(),
                watch_trigger.clone(),
            );
            exchanges.push(Rc::new(RefCell::new(exchange)));
        }
        Ok(ExchangeController {
            exchanges: exchanges,
        })
    }

    pub async fn websocket_connect(&mut self) {
        let mut exchange_0 = self.exchanges[0].as_ref().borrow_mut();
        let mut exchange_1 = self.exchanges[1].as_ref().borrow_mut();
        let connect_task_e0 = exchange_0.start();
        let connect_task_e1 = exchange_1.start();
        tokio::join!(connect_task_e0, connect_task_e1);
    }

    pub async fn subscribe_depths(&mut self) {
        let mut exchange_0 = self.exchanges[0].as_ref().borrow_mut();
        let mut exchange_1 = self.exchanges[1].as_ref().borrow_mut();
        let subscribe_task_e1 = exchange_0.subscribe_orderbooks();
        let subscribe_task_e2 = exchange_1.subscribe_orderbooks();
        tokio::join!(subscribe_task_e1, subscribe_task_e2);
    }

    pub async fn stream_depths(&mut self) {
        let mut exchange_0 = self.exchanges[0].as_ref().borrow_mut();
        let mut exchange_1 = self.exchanges[1].as_ref().borrow_mut();
        tokio::select! {
                _ = exchange_0.stream_depths( )=> {}
                _ = exchange_1.stream_depths( )=> {}
        }
    }
}
