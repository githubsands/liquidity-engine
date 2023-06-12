use depth_generator::DepthMessageGenerator;
use futures::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use market_objects::{
    BinanceDepthUpdate, DepthUpdate, HTTPSnapShotDepthResponseBinance, WSDepthUpdateBinance,
};
use port_killer::kill;
use serde_json::to_string;
use std::error::Error;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio_tungstenite::WebSocketStream;
use tracing::info;
use tungstenite::Message;
use warp::Filter;

const EXCHANGE_LOCATIONS: u8 = 2;

pub struct ExchangeServer {
    address: SocketAddr,
    depth_producer: Sender<DepthUpdate>,
    depth_consumer: Receiver<DepthUpdate>,
    depth_sink: Option<SplitSink<WebSocketStream<TcpStream>, Message>>,
}

impl ExchangeServer {
    pub fn new(_name: String, port_num: u16) -> Result<ExchangeServer, Box<dyn Error>> {
        kill(port_num)?;
        let ip_address = Ipv4Addr::new(127, 0, 0, 1);
        let socket_addr = SocketAddrV4::new(ip_address, port_num);
        let socket_addr = SocketAddr::V4(socket_addr);
        let (depth_producer, depth_consumer) = channel(10);
        Ok(ExchangeServer {
            address: socket_addr,
            depth_producer: depth_producer,
            depth_consumer: depth_consumer,
            depth_sink: None,
        })
    }
    pub fn ip_address(&self) -> String {
        return self.address.to_string();
    }
    pub async fn run_websocket(&mut self) -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
        let tcp_listener = TcpListener::bind(&self.address).await?;
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
    pub async fn run_http_server(self) -> Result<(), Box<dyn Error>> {
        let http_route = warp::path("http").map(|| {
            let mut depth_generator = DepthMessageGenerator::default();
            format!("Hello, World!");
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
                lastUpdateId: 100, // TODO: This value may need to be leveraged to properly test a
                // sequence orderbook with websocket messges and snapshot updates
                asks: binance_asks,
                bids: binance_bids,
            };
            warp::reply::json(&response)
        });
        warp::serve(http_route).run(self.address).await;
        Ok(())
    }
    pub async fn supply_depth_snapshot(
        &self,
        _req: warp::http::Request<warp::hyper::Body>,
    ) -> Result<impl warp::Reply, warp::Rejection> {
        Ok("Hello, World!")
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
            .try_send(depth_generator.depth_message_random(1))?;
        Ok(())
    }
    async fn fanout_depth(&mut self) -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
        if let Some(depth_message) = self.depth_consumer.recv().await {
            let mut obj = ExchangeServer::convert_to_binance(vec![depth_message]);
            obj[0].e = 0.0;
            let obj_text = to_string(&obj[0])?;
            info!("sending object to ws client");
            self.depth_sink
                .as_mut()
                .unwrap()
                .send(Message::Text(obj_text))
                .await?;
        }
        Ok(())
    }
    pub async fn force_disconnect(&mut self) -> Result<(), Box<dyn Error + Sync + Send + 'static>> {
        self.depth_sink.as_mut().unwrap().close().await?;
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
