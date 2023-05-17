use std::collections::VecDeque;

use crossbeam_channel::{bounded, Sender, TrySendError};

use tokio::io::ReadHalf;

use serde_json::{to_string as jsonify, Value};

use futures::task::Context;

use tokio_tungstenite::{
    connect_async_with_config, tungstenite::protocol::Message,
    tungstenite::protocol::WebSocketConfig, MaybeTlsStream, WebSocketStream,
};

use tokio::net::TcpStream;

use config::ExchangeConfig;
use order::{
    BinanceOrder, ByBitOrder, Order, SnapShotDepthResponseBinance, SnapShotDepthResponseByBit,
};

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
    pub struct ExchangeWS {
        pub client_name: String,
        pub exchange_name: u8,
        pub snapshot_enabled: bool,
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

impl ExchangeWS {
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
        let exchange = ExchangeWS {
            client_name: exchange_config.client_name.clone(),
            exchange_name: exchange_config.exchange_name,
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
        info!("received snapshot orders - now buffering them");
        self.buffer_snapshot_orders(snapshot_orders).await;
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
            Ok(snapshot_response) => match self.exchange_name {
                0 => {
                    let snapshot: SnapShotDepthResponseBinance = snapshot_response.json().await?;
                    info!("finished receiving snaps for {}", self.exchange_name);
                    Ok(snapshot.orders())
                }
                1 => {
                    let snapshot: SnapShotDepthResponseByBit = snapshot_response.json().await?;
                    info!("finished receiving snaps for {}", self.exchange_name);
                    Ok(snapshot.orders())
                }
                _ => {
                    error!(
                        "failed to create snapshot due to exchange_name {} ",
                        self.exchange_name
                    );
                    return {
                        info!("area two");
                        Err(Box::new(ErrorInitialState::Snapshot(
                            "Failed to create snapshot".to_string(),
                        )))
                    };
                }
            },
            Err(err) => {
                info!("area three {}", err);
                return Err(Box::new(ErrorInitialState::Snapshot(err.to_string())));
            }
        }
    }
    // buffer_snap_shot_orders buffers the snapshot orders
    // TODO: Implement this in one function so we don't have to clone a dynamically sized type
    // around ((Vec<Order>, Vec<Order>) or create another future or the on the stack.  Note that
    // async functions cannot be inlined
    async fn buffer_snapshot_orders(&mut self, orders: (Vec<Order>, Vec<Order>)) {
        for (order_bids, order_asks) in orders.0.into_iter().zip(orders.1) {
            info!("pushing snapshot orders");
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
        /*
        let json_obj = serde_json::json!({
            "method": "SUBSCRIBE",
            "params": [
                "btcusdt@depth",
            ],
            "id": 1
        });
        */
        let exchange_response = sink
            .send(Message::Text(
                jsonify(&self.orderbook_subscription_message).unwrap(),
            ))
            .await;
        // TODO: handle this differently;
        match exchange_response {
            Ok(response) => {
                print!("subscription success")
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

impl Stream for ExchangeWS {
    type Item = WSStreamState;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // Check if we have any orders in the buffer if we do try_send them forward to the
        // orderbook. At t=0 or if we have exchange faults this will always pass.
        let mut this = self.project();
        debug!("waiting for items\n");
        while let Some(order) = this.buffer.pop_front() {
            if let Err(channel_error) = this.orders_producer.try_send(order) {
                match channel_error {
                    TrySendError::Full(_) => {
                        warn!("failed to try_send to order bid producer, trying again");
                        continue;
                    }
                    TrySendError::Disconnected(_) => {
                        error!("order producer is disconnected");
                        return Poll::Ready(Some(WSStreamState::SenderError));
                    }
                }
            }
        }

        let Some(mut orderbooks) = this.ws_connection_orderbook_reader.as_mut().as_pin_mut() else {
            error!("failed to copy the orderbooks stream");
            return Poll::Ready(Some(WSStreamState::FailedStream))
        };

        // lets loop through our stream to see whats ready and check the ready for an option that
        // has some value. if the option has a value check if its a ws message or ws error. if
        // it is a ws error return Poll::Pending if its not an error return poll ready
        while let Poll::Ready(stream_option) = orderbooks.poll_next_unpin(cx) {
            match stream_option {
                Some(ws_result) => match ws_result {
                    Ok(ws_message) => {
                        debug!("received order: {}\n", ws_message);
                        // this.buffer.push_back(ws_message);
                    }
                    Err(ws_error) => return Poll::Ready(Some(WSStreamState::WSError(ws_error))),
                },
                None => {
                    break;
                }
            }
        }
        Poll::Pending
    }
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
