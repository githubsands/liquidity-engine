use serde_json::to_string;

use std::collections::VecDeque;

use crossbeam_channel::{bounded, Sender, TrySendError};

use tokio::io::ReadHalf;

use serde_json::Value;

use futures::task::Context;

use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

use tokio::net::TcpStream;

use config::ExchangeConfig;
use order::Order;

use quoter_errors::{ErrorHotPath, ErrorInitialState};

use std::{pin::Pin, task::Poll};

use futures::stream::{SplitSink, SplitStream, Stream, StreamExt};
use pin_project_lite::pin_project;

use tracing::{error, info};

pin_project! {
    #[must_use = "streams do nothing unless polled"]
    pub struct ExchangeWS {
        uri: String,

        watched_pairs: String,
        orderbook_subscription_message: String,

        #[pin]
        buffer: Box<VecDeque<Message>>,
        #[pin]
        ws_connection_orderbook: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        #[pin]
        ws_connection_orderbook_reader: Option<SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>>,
        // ws_connection_orderbook_writer: SplitSink<WebSocketStream, Message>>,

        orderbook_reader_running: bool,
        order_bid_producer: Sender<Order>,
        order_ask_producer: Sender<Order>,
    }
}

// Wse pin_project_lite::pin_project;
impl ExchangeWS {
    pub fn new(
        exchange_config: &ExchangeConfig,
        order_bid_producer: Sender<Order>,
        order_ask_producer: Sender<Order>,
    ) -> Result<Self, ErrorInitialState> {
        let exchange = ExchangeWS {
            uri: exchange_config.uri.to_string(),
            watched_pairs: exchange_config.watched_pairs.to_string(),
            orderbook_subscription_message: exchange_config
                .orderbook_subscription_message
                .to_string(),

            ws_connection_orderbook: None::<WebSocketStream<MaybeTlsStream<TcpStream>>>,
            buffer: Box::new(VecDeque::new()),
            orderbook_reader_running: false,
            ws_connection_orderbook_reader: None,
            // ws_connection_orderbook_writer: None,
            order_bid_producer: order_bid_producer,
            order_ask_producer: order_ask_producer,
        };
        Ok(exchange)
    }
    pub async fn start(&mut self) -> Result<(), ErrorInitialState> {
        let ws_conn_result = connect_async(self.uri.clone()).await;
        let _ = match ws_conn_result {
            Ok((ws_conn, _)) => {
                let (_, stream) = ws_conn.split();
                self.ws_connection_orderbook_reader = Some(stream);
                // self.subscribe_startup().await?;
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
    async fn subscribe_startup(&mut self) -> Result<(), ErrorInitialState> {
        self.ws_connection_orderbook
            .unwrap()
            .start_try_send(Message::Text(self.orderbook_subscription_message.clone()))
    }
    async fn subscribe_reconnect(&mut self) -> Result<(), ErrorHotPath> {
        self.ws_connection_orderbook
            .unwrap()
            .start_try_send(Message::Text(self.orderbook_subscription_message.clone()))
    }
    */
    pub async fn reconnect(&mut self) -> Result<(), ErrorHotPath> {
        let ws_conn_result = connect_async(self.uri.clone()).await;
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
                                    TrySendError::Disconnected(ws_error) => {
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
                        this.buffer.push_back(ws_message);
                        break;
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

/*
#[cfg(test)]
mod tests {
    use futures::{stream, SinkExt};
    use std::sync::Arc;
    use std::{thread, time};
    use tokio_test::{assert_pending, assert_ready_eq};
    use ws::{
        connect, CloseCode, Error, Factory, Handler, Handshake, Message as WSMessage, Request,
        Response, WebSocket,
    };

    use super::*;

    #[tokio::test]
    async fn test_exchange_stream() {
        let waker = futures::task::noop_waker_ref();
        let mut cx = std::task::Context::from_waker(&waker);
        let mut exchange_stream = Exchange::new(stream::empty::<()>());
        let host = Arc::new(Mutex::new("localhost:3082".to_string()));
        let cloned_host = Arc::clone(&host);
        let exchange_server = thread::spawn(move || {
            println!("Running WS server");
            let host = cloned_host.lock();
            if let Err(error) = ws::listen(host.to_string(), |out| {
                move |msg| {
                    println!("Server got message '{}'", msg);
                    out.try_send(msg)
                }
            }) {
                println!("Failed to create websocket due to {:?}", error);
            }
        });
        l
    }
}
*/
