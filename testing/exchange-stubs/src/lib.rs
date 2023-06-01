use depth_generator::DepthMessageGenerator;
use futures::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use market_object::{BinanceDepthUpdate, DepthUpdate, WSDepthUpdateBinance};
use port_killer::kill;
use serde_json::to_string;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio_tungstenite::WebSocketStream;
use tracing::{error, info, warn};
use tungstenite::Message;

pub struct ExchangeServer {
    name: String,
    address: SocketAddr,
    depth_producer: Sender<DepthUpdate>,
    depth_consumer: Receiver<DepthUpdate>,
    depth_sink: Option<SplitSink<WebSocketStream<TcpStream>, Message>>,
}

impl ExchangeServer {
    pub fn new(name: String, port_num: u16) -> ExchangeServer {
        let result = kill(port_num);
        if result.is_err() {
            panic!("failed to kill port for the server")
        }
        let ip_address = Ipv4Addr::new(127, 0, 0, 1);
        let socket_addr = SocketAddrV4::new(ip_address, port_num);
        let socket_addr = SocketAddr::V4(socket_addr);
        let (depth_producer, depth_consumer) = channel(10);
        ExchangeServer {
            name: name,
            address: socket_addr,
            depth_producer: depth_producer,
            depth_consumer: depth_consumer,
            depth_sink: None,
        }
    }
    pub fn ip_address(&self) -> String {
        return self.address.to_string();
    }
    pub async fn run(&mut self) {
        let tcp_listener = TcpListener::bind(&self.address).await;
        info!("running exchange--");
        let listener = tcp_listener.unwrap();
        'client_wait: loop {
            if let Ok((client_stream, _)) = listener.accept().await {
                if let Ok(ws_io) = tokio_tungstenite::accept_async(client_stream).await {
                    let (ws_sink, _) = ws_io.split();
                    self.depth_sink = Some(ws_sink);
                    info!("client received");
                    break 'client_wait;
                }
                _ = self.depth_sink.as_mut().unwrap().send(Message::Text(obj_text.unwrap())).await
            }
            }
        }
    }
    async fn fanout_depth(&mut self) {
        while let Some(depth_message) = self.depth_consumer.recv().await {
            let obj = ExchangeServer::convert_to_binance(vec![depth_message]);
            let obj_text = to_string(&obj[0]);
            if obj_text.is_err() {
                warn!("failed to create binance object")
            }
            let send_result = self
                .depth_sink
                .as_mut()
                .unwrap()
                .send(Message::Text(obj_text.unwrap()))
                .await;
            if send_result.is_err() {
                error!("failed to send result to ws client")
            }
        }
    }
    async fn supply_depth(&self) -> Result<(), tokio::sync::mpsc::error::SendError<DepthUpdate>> {
        let mut depth_generator = DepthMessageGenerator::default();
        let mut sent = false;
        while sent == false {
            let result = self
                .depth_producer
                .try_send(depth_generator.depth_message_random(1));
            match result {
                Ok(_) => sent = true,
                Err(e) => {
                    error!("failed to send: {}", e);
                    continue;
                }
            }
        }
        Ok(())
    }
    fn convert_to_binance(depth_update: Vec<DepthUpdate>) -> Vec<BinanceDepthUpdate> {
        depth_update
            .into_iter()
            .map(|du| BinanceDepthUpdate {
                price: du.p,
                quantity: du.q,
            })
            .collect()
    }
}

#[cfg(test)]
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
            let (mut ws_stream, _) = connect_async("ws://".to_owned() + &connect_addr)
                .await
                .expect("Failed to connect");
            while let Some(_) = ws_stream.next().await {
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
