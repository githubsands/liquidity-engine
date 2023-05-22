use depth_generator::DepthMessageGenerator;
use futures::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use market_objects::{
    BinanceDepthUpdate, DepthUpdate, HTTPSnapShotDepthResponseBinance, WSDepthUpdateBinance,
};
use port_killer::kill;
use serde_json::to_string;
use std::error::Error;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::Mutex;
use tokio_tungstenite::WebSocketStream;
use tracing::info;
use tungstenite::Message;
use warp::Filter;

use std::io::{self, ErrorKind};
use tokio::sync::oneshot::{
    channel as oneshotChannel, Receiver as oneshotReceiver, Sender as oneshotSender,
};

const EXCHANGE_LOCATIONS: u8 = 2;

pub struct ExchangeServer {
    ws_address: SocketAddrV4,
    http_address: SocketAddrV4,
    depth_producer: Sender<DepthUpdate>,
    depth_consumer: Receiver<DepthUpdate>,
    depth_sink: Option<SplitSink<WebSocketStream<TcpStream>, Message>>,
    http_shutdown: Receiver<()>,
}

impl ExchangeServer {
    pub fn new(
        _name: String,
        ws_port_num: u16,
        http_port_num: u16,
    ) -> Result<(ExchangeServer, Sender<()>), Box<dyn Error>> {
        kill(ws_port_num)?;
        kill(http_port_num)?;
        let ip_address = Ipv4Addr::new(127, 0, 0, 1);
        let socket_addr_ws = SocketAddrV4::new(ip_address, ws_port_num);
        let socket_addr_http = SocketAddrV4::new(ip_address, http_port_num);
        let (depth_producer, depth_consumer) = channel(10);
        let (http_shutdown, shut_down_receiver) = channel(1);
        Ok((
            ExchangeServer {
                ws_address: socket_addr_ws,
                http_address: socket_addr_http,
                depth_producer: depth_producer,
                depth_consumer: depth_consumer,
                depth_sink: None,
                http_shutdown: shut_down_receiver,
            },
            http_shutdown,
        ))
    }
    pub fn ws_ip_address(&self) -> String {
        return self.ws_address.to_string();
    }
    pub fn http_ip_address(&self) -> String {
        return self.http_address.to_string();
    }
    pub async fn run_websocket(&mut self) -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
        let tcp_listener = TcpListener::bind(&self.ws_address).await?;
        info!("running exchange--");
        let listener = tcp_listener;
        loop {
            if let Ok((client_stream, _)) = listener.accept().await {
                let connection_result = tokio_tungstenite::accept_async(client_stream).await;
                match connection_result {
                    Ok(ws_io) => {
                        let (ws_sink, _) = ws_io.split();
                        self.depth_sink = Some(ws_sink);
                        info!("client received");
                        return Ok(());
                    }
                    Err(connection_error) => return Err(Box::new(connection_error)),
                }
            }
        }
    }
    pub async fn run_http_server(&mut self) -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
        let http_route = warp::path("depths").map(|| {
            let mut depth_generator = DepthMessageGenerator::default();
            let (asks, bids) =
                depth_generator.depth_balanced_orderbook(EXCHANGE_LOCATIONS as usize, 40, 27000);
            let binance_asks: Vec<BinanceDepthUpdate> = asks
                .into_iter()
                .map(|depth| BinanceDepthUpdate {
                    price: depth.p,
                    quantity: depth.q,
                })
                .collect();
            let binance_bids: Vec<BinanceDepthUpdate> = bids
                .into_iter()
                .map(|depth| BinanceDepthUpdate {
                    price: depth.p,
                    quantity: depth.q,
                })
                .collect();
            let response = HTTPSnapShotDepthResponseBinance {
                retcode: Some(0),
                lastUpdateId: 100,
                asks: binance_asks,
                bids: binance_bids,
            };
            warp::reply::json(&response)
        });
        let server = warp::serve(http_route);
        let shutdown = &mut self.http_shutdown;
        let (_, server_result) = tokio::select! {
            _ = server.run(self.http_address) => {
                ((), Ok(()))
            }
            _ = shutdown.recv()=> {
                ((), Ok(()))
            }
        };

        server_result
    }

    pub async fn supply_depths(&mut self) -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
        self.supply_depth_internal_ws().await?;
        self.fanout_depth().await?;
        Ok(())
    }
    pub async fn supply_depth_internal_ws(
        &self,
    ) -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
        let mut depth_generator = DepthMessageGenerator::default();
        self.depth_producer
            .try_send(depth_generator.depth_message_random())?;
        Ok(())
    }
    async fn fanout_depth(&mut self) -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
        if let Some(depth_message) = self.depth_consumer.recv().await {
            let mut obj = ExchangeServer::convert_to_binance(vec![depth_message]);
            if obj.is_empty() {
                return Err(Box::new(io::Error::new(
                    ErrorKind::Other,
                    "failed to create binance object",
                )));
            }
            obj[0].e = 0.0;
            let obj_text = to_string(&obj[0])?;
            info!("sending object to ws client");
            if let Some(depth_sink) = &mut self.depth_sink {
                depth_sink.send(Message::Text(obj_text)).await?;
            } else {
                return Err(Box::new(io::Error::new(ErrorKind::Other, "failed to fanout depths due to no websocket client being established within the exchange server")));
            }
            Ok(())
        } else {
            Err(Box::new(io::Error::new(
                ErrorKind::Other,
                "No depth message received",
            )))
        }
    }
    pub async fn force_disconnect_ws(
        &mut self,
    ) -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
        self.depth_sink.as_mut().unwrap().close().await?;
        Ok(())
    }
    pub async fn shutdown_http(&mut self) -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
        Ok(())
    }
    fn convert_to_binance(depth_updates: Vec<DepthUpdate>) -> Vec<WSDepthUpdateBinance> {
        let binance_depth_updates: Vec<BinanceDepthUpdate> = depth_updates
            .into_iter()
            .map(|depth_update| BinanceDepthUpdate {
                price: depth_update.p,
                quantity: depth_update.q,
            })
            .collect();
        vec![WSDepthUpdateBinance {
            e: 0.0,
            E: 0.0,
            s: 0.0,
            U: 0.0,
            u: 0.0,
            b: binance_depth_updates,
            a: vec![],
        }]
    }
}

/*
impl Default for ExchangeServer {
    fn default() -> Self {
        let (depth_producer, depth_consumer) = channel::<DepthUpdate>(100);
        let ip_address = Ipv4Addr::new(127, 0, 0, 1);
        let ws_port_num = 8080;
        let http_port_num = 8081;
        let (shut_down, shut_down_receiver) = oneshotChannel();
        ExchangeServer {
            ws_address: SocketAddrV4::new(ip_address, ws_port_num),
            http_address: SocketAddrV4::new(ip_address, http_port_num),
            depth_producer,
            depth_consumer,
            depth_sink: None,
            http_shutdown: shut_down_receiver,
            http_shutdown_trigger: shut_down,
        }
    }
}
*/

// NOTE: This test is invalid due to changes on the exchange stub from commit 9369ceb
// exchange stub works correctly if tests from 9369ceb succeed. github actions ci is currently
// ignoring this "ignore" macro so we have it commented out for now
// #[ignore]
// #[cfg(test)]
/*
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tokio::time::{sleep, Duration};
    use tokio_tungstenite::connect_async;
    use tracing::{error, info};
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn test_exchange_stream_receive_depth() {
        let depth_count_client_received: Arc<Mutex<i32>> = Arc::new(Mutex::new(0));
        let desired_depths = 4;
        let server_port = 8080;
        let exchange_server = Arc::new(Mutex::new(ExchangeServer::new(
            "1".to_string(),
            server_port,
        )));
        let server_clone = Arc::clone(&exchange_server);
        let connect_addr = server_clone.lock().await.ip_address();
        tokio::spawn(async move {
            let mut server_lock = server_clone.lock().await;
            info!("running server");
            server_lock.run().await;
        });
        let depth_count_clone = depth_count_client_received.clone();
        tokio::spawn(async move {
            sleep(Duration::from_secs(1)).await;
            let (mut ws_client_stream, _) = connect_async("ws://".to_owned() + &connect_addr)
                .await
                .expect("Failed to connect");
            while let Some(_) = ws_client_stream.next().await {
                let mut count = depth_count_clone.lock().await;
                *count += 1;
                info!("received {} depth from server", count);
            }
        });
        let server_clone = Arc::clone(&exchange_server);
        let depth_supply_handle = tokio::spawn(async move {
            for i in 0..4 {
                info!("supplying depth {}", i);
                let res = server_clone.lock().await.supply_depth().await;
                if res.is_err() {
                    error!("failed to supply depth");
                }
            }
            info!("supplied all depths");
            return;
        });
        let server_clone = Arc::clone(&exchange_server);
        let _ = tokio::spawn(async move {
            sleep(Duration::from_secs(1)).await;
            loop {
                sleep(Duration::from_millis(2)).await;
                server_clone.lock().await.fanout_depth().await;
            }
        });
        sleep(Duration::from_secs(2)).await;
        tokio::select! {
            _ = depth_supply_handle => println!("depth supply has completed"),
        }
        assert!(*depth_count_client_received.lock().await == desired_depths);
    }
}
*/
