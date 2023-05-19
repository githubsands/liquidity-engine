use std::convert::Infallible;
use std::error::Error;
use std::{pin::Pin, task::Poll};

use serde_json::{from_str, to_string as jsonify};

use pin_project_lite::pin_project;

use futures::stream::{SplitSink, SplitStream, Stream, StreamExt};
use futures_util::sink::SinkExt;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async_with_config, tungstenite::protocol::Message,
    tungstenite::protocol::WebSocketConfig, MaybeTlsStream, WebSocketStream,
};

use bounded_vec_deque::BoundedVecDeque;
use crossbeam_channel::{Sender, TrySendError};
use reqwest::{Client, Error as HTTPError};
use tracing::{debug, error, info, warn};

use config::ExchangeConfig;
use market_object::{DepthUpdate, HTTPSnapShotDepthResponseBinance, WSDepthUpdateBinance};
use quoter_errors::{ErrorHotPath, ErrorInitialState};

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
        buffer: BoundedVecDeque<DepthUpdate>,
        #[pin]
        ws_connection_orderbook: Option<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        #[pin]
        ws_connection_orderbook_reader: Option<SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>>,

        depths_producer: Sender<DepthUpdate>,

        http_client: Option<Client>,
    }
}

impl ExchangeWS {
    pub fn new(
        exchange_config: &ExchangeConfig,
        orders_producer: Sender<DepthUpdate>,
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
            buffer: BoundedVecDeque::new(exchange_config.buffer_size),
            ws_connection_orderbook_reader: None,
            depths_producer: orders_producer,
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
    pub async fn run_snapshot(&mut self) -> Result<(), Box<dyn Error>> {
        let snapshot_orders = self.orderbook_snapshot().await?;
        info!("received snapshot orders - sending orders to the orderbook");
        let mut asks = futures::stream::iter(snapshot_orders.0);
        let mut bids = futures::stream::iter(snapshot_orders.1);

        // TODO: Handle this logic correctly.  i.e retries, more defined errors, ideally with
        // the use of no heap, understanding the unknown state, or possibly use write_all over
        // this select way of doing things
        loop {
            info!("sending snapshot depths to the orderbook");
            tokio::select! {
                Some(ask_depth) = asks.next() => {
                    if let Err(channel_error) = self.depths_producer.try_send(ask_depth) {
                        warn!("failed to send snapshot depths to the orderbook");
                        return Err(Box::new(ErrorInitialState::Snapshot(channel_error.to_string())));
                    }
                }
                Some(bid_depth) = bids.next() => {
                    if let Err(channel_error) = self.depths_producer.try_send(bid_depth) {
                        warn!("failed to send snapshot depths to the orderbook");
                        return Err(Box::new(ErrorInitialState::Snapshot(channel_error.to_string())));
                    }
                }
                else => {
                    warn!("unknown state - continue streaming snapshot depths");
                    continue
                }
            }
        }
        Ok(())
    }

    // TODO: Don't return dynamic errors here - this hack was used to deal with Reqwest errors
    // specifically. This isn't necessarily in the hotpath as is but in future developments the
    // orderbook may need to be rebuilt when we are in a steady/chaotic state
    async fn orderbook_snapshot(
        &mut self,
    ) -> Result<
        (
            impl Iterator<Item = DepthUpdate>,
            impl Iterator<Item = DepthUpdate>,
        ),
        Box<dyn std::error::Error>,
    > {
        info!("curling snap shot from {}", self.snapshot_uri);
        let req_builder = self
            .http_client
            .as_ref()
            .unwrap()
            .get(self.snapshot_uri.clone());
        let snapshot_response_result = req_builder.send().await;
        match snapshot_response_result {
            Ok(snapshot_response) => match self.exchange_name {
                0 => {
                    let body = snapshot_response.text().await?;
                    let snapshot: HTTPSnapShotDepthResponseBinance = from_str(&body)?;
                    info!("finished receiving snaps for {}", self.exchange_name);
                    let snapshot_depths = snapshot.depths(0);
                    Ok(snapshot_depths)
                }
                1 => {
                    let snapshot: HTTPSnapShotDepthResponseBinance =
                        snapshot_response.json().await?;
                    info!("finished receiving snaps for {}", self.exchange_name);
                    let snapshot_depths = snapshot.depths(1);
                    Ok(snapshot_depths)
                }
                _ => {
                    error!(
                        "failed to create snapshot due to exchange_name {} ",
                        self.exchange_name
                    );
                    return {
                        Err(Box::new(ErrorInitialState::Snapshot(
                            "Failed to create snapshot".to_string(),
                        )))
                    };
                }
            },
            Err(err) => {
                error!("failed to reconcile snapshot result: {}", err);
                return Err(Box::new(ErrorInitialState::Snapshot(err.to_string())));
            }
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
        let json_obj = serde_json::json!({
            "method": "SUBSCRIBE",
            "params": [
                "btcusdt@depth",
            ],
            "id": 1
        });
        // jsonify(&self.orderbook_subscription_message).unwrap(),

        let exchange_response = sink.send(Message::Text(json_obj.to_string())).await;
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
        debug!("waiting for items for exchange: {}\n", this.exchange_name);
        while let Some(depth) = this.buffer.pop_front() {
            if let Err(channel_error) = this.depths_producer.try_send(depth) {
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
                        let depth_result: Result<WSDepthUpdateBinance, Infallible> =
                            WSDepthUpdateBinance::try_from(ws_message);
                        match depth_result {
                            Ok(depth_update) => {
                                let depths = depth_update.depths(*this.exchange_name);
                                for (bids, asks) in depths.0.into_iter().zip(depths.1) {
                                    this.buffer.push_back(bids);
                                    this.buffer.push_back(asks);
                                }
                            }
                            Err(error) => {
                                warn!("failed to deserialize the object. received {}", error)
                            }
                        }
                    }
                    Err(ws_error) => return Poll::Ready(Some(WSStreamState::WSError(ws_error))),
                },
                None => break,
            }
        }
        Poll::Pending
    }
}
