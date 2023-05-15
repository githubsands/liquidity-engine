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
use order::Order;

use quoter_errors::{ErrorHotPath, ErrorInitialState};

use std::{pin::Pin, task::Poll};

use futures::stream::{SplitSink, SplitStream, Stream, StreamExt};
use pin_project_lite::pin_project;

use futures_util::{future, pin_mut};
use tracing::{error, info};

use bounded_vec_deque::BoundedVecDeque;

use native_tls::TlsConnector;

use futures_core::stream::TryStream;

use futures_util::sink::SinkExt;

pin_project! {
    #[must_use = "streams do nothing unless polled"]
    pub struct ExchangeWS {
        pub uri: String,

        watched_pairs: String,
        orderbook_subscription_message: String,

        #[pin]
        buffer: Box<BoundedVecDeque<Message>>,
        #[pin]
        ws_connection_orderbook: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        #[pin]
        ws_connection_orderbook_reader: Option<SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>>,

        orderbook_reader_running: bool,
        order_bid_producer: Sender<Order>,
        order_ask_producer: Sender<Order>,
    }
}

impl ExchangeWS {
    pub fn new(
        exchange_config: &ExchangeConfig,
        order_bid_producer: Sender<Order>,
        order_ask_producer: Sender<Order>,
    ) -> Result<Box<Self>, ErrorInitialState> {
        let exchange = ExchangeWS {
            uri: exchange_config.uri.to_string(),
            watched_pairs: exchange_config.watched_pairs.to_string(),
            orderbook_subscription_message: jsonify(
                &exchange_config.orderbook_subscription_message,
            )
            .unwrap(),
            ws_connection_orderbook: None::<WebSocketStream<MaybeTlsStream<TcpStream>>>,
            buffer: Box::new(BoundedVecDeque::new(exchange_config.buffer_size)),
            orderbook_reader_running: false,
            ws_connection_orderbook_reader: None,
            order_bid_producer: order_bid_producer,
            order_ask_producer: order_ask_producer,
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
        let ws_conn_result = connect_async_with_config(self.uri.clone(), Some(config)).await;
        let _ = match ws_conn_result {
            Ok((ws_conn, _)) => {
                let (sink, stream) = ws_conn.split();
                self.subscribe_orderbooks(sink).await?;
                self.ws_connection_orderbook_reader = Some(stream);
            }
            Err(ws_error) => {
                info!("failed to connect to exchange {}", self.uri);
                return Err(ErrorInitialState::WSConnection(ws_error.to_string()));
            }
        };
        Ok(())
    }
    async fn subscribe_orderbooks(
        &mut self,
        mut sink: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    ) -> Result<(), ErrorInitialState> {
        info!(
            "sending orderbook subscription message: {}",
            self.orderbook_subscription_message
        );
        let json_obj = serde_json::json!({
            "method": "SUBSCRIBE",
            "params": [
                "bnbbtc@depth",
            ],
            "id": 1
        });

        let exchange_response = sink.send(Message::Text(jsonify(&json_obj).unwrap())).await;
        // TODO: handle this differently;
        match exchange_response {
            Ok(response) => {
                print!("subscription success");
            }
            Err(error) => {
                print!("error {}", error)
            }
        }
        Ok(())
    }
    /*
    async fn subscribe_reconnect(&mut self) -> Result<(), ErrorHotPath> {
        self.ws_connection_orderbook
            .unwrap()
            .start_try_send(Message::Text(self.orderbook_subscription_message.clone()))
    }
    */
    pub async fn reconnect(&mut self) -> Result<(), ErrorHotPath> {
        let config = WebSocketConfig {
            max_send_queue: None,
            max_message_size: None,
            max_frame_size: None,
            accept_unmasked_frames: true,
        };
        let ws_conn_result = connect_async_with_config(self.uri.clone(), Some(config)).await;
        let _ = match ws_conn_result {
            Ok((ws_conn, _)) => {
                self.ws_connection_orderbook = Some(ws_conn);
                info!("reconnected to exchange");
                // self.subscribe_reconnect().await?;
            }
            Err(_) => {
                return Err(ErrorHotPath::ExchangeWSReconnectError(
                    "Failed to reconnect to Exchange WS Server".to_string(),
                ))
            }
        };
        Ok(())
    }
}

pub enum WSStreamState {
    WSError(tokio_tungstenite::tungstenite::Error),
    SenderError,
    FailedStream,
    Success,
}

impl Stream for ExchangeWS {
    type Item = WSStreamState;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // check if we have any orders in the buffer if we do try_send them forward
        let mut this = self.project();
        print!("waiting for items\n");
        while let Some(ws_message) = this.buffer.pop_front() {
            let order = deserialize_message_to_order(ws_message);
            if let Some(order) = order {
                if order.kind != 1 || order.kind != 2 {
                    info!("bad order kind")
                }
                let mut success: bool = false;
                // buffer must be full or something similar - keep retrying till we can push
                // the order through
                'channel_retry: while !success {
                    match order.kind {
                        1 => {
                            if let Err(channel_error) = this.order_bid_producer.try_send(order) {
                                match channel_error {
                                    TrySendError::Full(_) => {
                                        info!("failed to try_send to order bid producer, trying again");
                                        continue;
                                    }
                                    TrySendError::Disconnected(_) => {
                                        error!("order producer is disconnected");
                                        return Poll::Ready(Some(WSStreamState::SenderError));
                                    }
                                }
                            } else {
                                success = true
                            }
                        }
                        2 => {
                            if let Err(channel_error) = this.order_ask_producer.try_send(order) {
                                match channel_error {
                                    TrySendError::Full(_) => {
                                        info!("failed to try_send to order ask producerm, trying again");
                                        continue;
                                    }
                                    TrySendError::Disconnected(_) => {
                                        error!("order producer is disconnected");
                                        return Poll::Ready(Some(WSStreamState::SenderError));
                                    }
                                }
                            } else {
                                success = true
                            }
                        }
                        _ => {
                            info!("poor order given. failed to try_send to order ask producer - continuing the stream with a new error");
                            break 'channel_retry;
                        }
                    }
                }
            } else {
                info!("poor order given. failed to deserialize the order - continuing on to next order")
            }
        }

        // TODO: ask yourself why pin_mut would return something false here???
        let Some(mut orderbooks) = this.ws_connection_orderbook_reader.as_mut().as_pin_mut() else {
            info!("failed to copy the orderbooks stream");
            return Poll::Ready(Some(WSStreamState::FailedStream))
        };

        // lets loop through our stream to see whats ready and check the ready for an option that
        // has some value. if the option has a value check if its a ws message or ws error. if
        // it is a ws error return Poll::Pending if its not an error return poll ready
        while let Poll::Ready(stream_option) = orderbooks.poll_next_unpin(cx) {
            match stream_option {
                Some(ws_result) => match ws_result {
                    Ok(ws_message) => {
                        info!("received order {}\n", ws_message);
                        this.buffer.push_back(ws_message);
                    }
                    // return the websocket error
                    Err(ws_error) => return Poll::Ready(Some(WSStreamState::WSError(ws_error))),
                },
                None => {
                    // nothing is ready from the inner ws stream - lets pend
                    break;
                }
            }
        }
        Poll::Pending
    }
}

fn deserialize_message_to_order(message: Message) -> Option<Order> {
    if let Message::Text(text) = message {
        let json: Value = serde_json::from_str(&text).ok()?;
        let order: Order = serde_json::from_value(json).ok()?;
        return Some(order);
    }
    None
}
