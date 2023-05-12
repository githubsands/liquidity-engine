use futures::stream::StreamExt;
use serde_json::to_string;

use std::collections::VecDeque;
use std::marker::PhantomPinned;
use std::pin::Pin;

use crossbeam_channel::{bounded, Sender};

use futures::Stream;
use tokio::io::ReadHalf;

use serde_json::Value;

use futures::task::Context;
use std::task::Poll;

use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

use tokio::net::TcpStream;

use config::ExchangeConfig;
use order::Order;

pub struct ExchangeWS {
    uri: String,

    watched_pairs: String,
    orderbook_subscription_message: String,

    buffer: Box<VecDeque<Message>>,

    ws_connection_orderbook: Option<Pin<Box<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
    ws_read_orderbook: Option<ReadHalf<TcpStream>>,
    orderbook_reader_running: bool,
    order_producer: Sender<Order>,

    _pin: PhantomPinned,
}

pub enum ErrorInitialState {
    MessageSerialization(String),
    WSConnection(String),
}

// Wse pin_project_lite::pin_project;
impl ExchangeWS {
    pub fn new(
        exchange_config: &ExchangeConfig,
        message: &str,
        order_producer: Sender<Order>,
    ) -> Result<Pin<Box<Self>>, ErrorInitialState> {
        let subscription_message = to_string("TBD");
        let msg = match subscription_message {
            Ok(message) => message,
            Err(_) => {
                return Err(ErrorInitialState::MessageSerialization(
                    "Failed to serialize the subscription message from config file".to_string(),
                ))
            }
        };
        let exchange = ExchangeWS {
            uri: exchange_config.uri.to_string(),
            watched_pairs: exchange_config.watched_pairs.to_string(),
            orderbook_subscription_message: message.to_string(),
            ws_connection_orderbook: None::<Pin<Box<WebSocketStream<MaybeTlsStream<TcpStream>>>>>,
            buffer: Box::new(VecDeque::new()),
            ws_read_orderbook: None,
            orderbook_reader_running: false,
            order_producer: order_producer,
            _pin: PhantomPinned,
        };
        Ok(Box::pin(exchange))
    }
    pub async fn start(&mut self) -> Result<(), ErrorInitialState> {
        let ws_conn_result = connect_async("test").await;
        let tuple = match ws_conn_result {
            Ok((ws_conn, exchange_response)) => {
                // let (write, read) = ws_conn.split();
                self.ws_connection_orderbook = Some(Box::pin(ws_conn));
            }
            Err(_) => {
                return Err(ErrorInitialState::WSConnection(
                    "Failed to connect to Exchange WS Server".to_string(),
                ))
            }
        };
        Ok(())
    }
    /*
    pub async stream(&mut self) {
        self.poll_next()
    }
    */
}

impl Stream for ExchangeWS {
    type Item = ();

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        // check if we have any orders in the buffer if we do send them forward
        let this = self.as_mut();
        while let Some(ws_message) = this.poll_next_unpin().pop_front() {
            let order = deserialize_message_to_order(ws_message);
            self.order_producer.send(order.unwrap());
            Poll::Ready(Some(()));
        }
        // let sloop through our stream to see whats ready and check the ready for an option that
        // has some value. if the option has a value check if its a ws message or ws error. if
        // it is a ws error return Poll::Pending if its not an error return poll ready
        while let Poll::Ready(stream_option) = self
            .ws_connection_orderbook
            .unwrap()
            .as_mut()
            .poll_next_unpin(cx)
        {
            match stream_option {
                Some(ws_result) => match ws_result {
                    Ok(ws_message) => {
                        self.buffer.push_back(ws_message);
                        break;
                    }
                    Err(_) => break,
                },
                None => {
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
