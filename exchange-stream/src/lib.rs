use std::collections::VecDeque;

use crossbeam_channel::{bounded, Sender, TrySendError};

use tokio::io::ReadHalf;

use serde_json::{from_str, to_string as jsonify, Value};

use futures::task::Context;

use tokio_tungstenite::{
    connect_async_with_config, tungstenite::protocol::Message,
    tungstenite::protocol::WebSocketConfig, MaybeTlsStream, WebSocketStream,
};

use tokio::net::TcpStream;

use config::ExchangeConfig;
use order::{BinanceOrder, Order, OrderBookUpdateBinance, SnapShotDepthResponseBinance};

use quoter_errors::{ErrorHotPath, ErrorInitialState};

use std::{pin::Pin, task::Poll};

use futures::stream::{SplitSink, SplitStream, Stream, StreamExt};
use pin_project_lite::pin_project;

use futures_util::{future, pin_mut};
use tracing::{debug, error, info, warn};

use bounded_vec_deque::BoundedVecDeque;

use native_tls::TlsConnector;

use futures_core::stream::TryStream;

use reqwest::{Client, Error as HTTPError};

use futures_util::sink::SinkExt;

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

        orderbook_subscription_message: String,

        #[pin]
        buffer: Box<BoundedVecDeque<Order>>,
        snap_shot_buffer: Box<BoundedVecDeque<Order>>,
        #[pin]
        ws_connection_orderbook: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        #[pin]
        ws_connection_orderbook_reader: Option<SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>>,

        orders_producer: Sender<Order>,

        http_client: Option<Client>,
    }
}

impl ExchangeStream {
    pub fn new(
        exchange_config: &ExchangeConfig,
        orders_producer: Sender<Order>,
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
            buffer: Box::new(BoundedVecDeque::new(exchange_config.buffer_size)),
            snap_shot_buffer: Box::new(BoundedVecDeque::new(exchange_config.buffer_size)),
            ws_connection_orderbook_reader: None,
            orders_producer: orders_producer,
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
    pub async fn run_snapshot(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let snapshot_orders = self.orderbook_snapshot().await?;
        info!("received snapshot orders - now buffering the orders");
        self.buffer_snapshot_orders(snapshot_orders).await;
        info!("finished buffering the orders");
        Ok(())
    }
    // TODO: Don't return dynamic errors here - this hack was used to deal with Reqwest errors
    // specifically. This isn't necessarily in the hotpath as is but in future developments the
    // orderbook may need to be rebuilt when we are in a steady/chaotic state
    async fn orderbook_snapshot(
        &mut self,
    ) -> Result<(Vec<Order>, Vec<Order>), Box<dyn std::error::Error>> {
        info!("curling snap shot from {}", self.snapshot_uri);
        let req_builder = self
            .http_client
            .as_ref()
            .unwrap()
            .get(self.snapshot_uri.clone());
        info!("area one");
        // TODO: Implement http retries here
        let snapshot_response_result = req_builder.send().await;
        match snapshot_response_result {
            Ok(snapshot_response) => match (snapshot_response, self.exchange_name) {
                (snapshot_response, 1) => {
                    let body = snapshot_response.text().await?;
                    let snapshot: SnapShotDepthResponseBinance = from_str(&body)?;
                    info!("finished receiving snaps for {}", self.exchange_name);
                    Ok(snapshot.orders())
                }
                1 => {
                    let snapshot: SnapShotDepthResponseBinance = snapshot_response.json().await?;
                    info!("finished receiving snaps for {}", self.exchange_name);
                    Ok(snapshot.orders())
                }
                (snapshot_response, 3) => {
                    let snapshot: HTTPSnapShotDepthResponseByBit = snapshot_response.json().await?;
                    info!("finished receiving snaps for {}", self.exchange_name);
                    let snapshot_depths = snapshot.depths(3);
                    Ok(snapshot_depths)
                }
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
    // buffer_snap_shot_orders buffers the snapshot orders
    // TODO: Implement this in one function so we don't have to clone a dynamically sized type
    // around ((Vec<Order>, Vec<Order>) or create another future or the on the stack.  Note that
    // async functions cannot be inlined
    // TODO: Can these push_backs be chunked?
    async fn buffer_snapshot_orders(&mut self, orders: (Vec<Order>, Vec<Order>)) {
        for (order_bids, order_asks) in orders.0.into_iter().zip(orders.1) {
            self.snap_shot_buffer.push_back(order_bids);
            self.snap_shot_buffer.push_back(order_asks);
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
                    // TODO: Pass down wserrors here.
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
        debug!("waiting for items\n");
        while let Some(order) = this.buffer.pop_front() {
            if let Err(channel_error) = this.orders_producer.try_send(order) {
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
                Some(ws_result) => match ws_result {
                    Ok(ws_message) => {
                        // TODO: Need to take multiplie exchanges here
                        debug!("received order: {}\n", ws_message);
                        let Some(order_update) = deserialize_message_to_orders(ws_message);
                        {
                            this.buffer.push_back(order_update.0);
                            this.buffer.push_back(order_update.1);
                        {
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
                None => {
                    break;
                }
            }
        }
    }
    Poll::Pending

}
}

// TODO: Handle errors rather then going with an option
fn deserialize_message_to_orders(message: Message) -> Option<(Order, Order)> {
    if let Message::Text(text) = message {
        let json: Value = serde_json::from_str(&text).ok()?;
        let update: OrderBookUpdateBinance = serde_json::from_value(json).ok()?;
        return Some(update.split_update());
    }
    None
}

// TODO: Great place to possibly do a macro for deserializing rather then going through a match
// statement.
/*
fn deserialize_message_to_orders(
    exchange: &str,
    message: Message,
) -> Result<(Order, Order), ErrorHotPath> {
    if let Message::Text(text) = message {
        match exchange {
            "bybit" => {
                let json: Option<Value> = serde_json::from_str(&text).ok();
                let order_update: Option<OrderBookUpdateByBit> =
                    serde_json::from_value(json.unwrap()).ok();
                Ok(order_update.unwrap().split_update())
            }
            "binance" => {
                let json: Option<Value> = serde_json::from_str(&text).ok();
                let order_update: Option<OrderBookUpdateBinance> =
                    serde_json::from_value(json.unwrap()).ok();
                Ok(order_update.unwrap().split_update())
            }
            _ => {
                warn!("exchange {} not implemented", exchange);
                Err(ErrorHotPath::Serialization(
                    "failed to serialize".to_string(),
                ))
            }
        }
    } else {
        Err(ErrorHotPath::ReceivedNonTextMessageFromExchange)
    }
}
*/
