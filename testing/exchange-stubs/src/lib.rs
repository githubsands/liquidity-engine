use depth_generator::DepthMessageGenerator;
use futures::stream::SplitSink;
use futures::SinkExt;
use futures_util::StreamExt;
use market_object::{BinanceDepthUpdate, DepthUpdate, WSDepthUpdateBinance};
use serde::{Deserialize, Serialize};
use serde_json::to_string;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Duration;
use tokio_tungstenite::WebSocketStream;
use tracing::{info, warn};
use tungstenite::Message;

pub struct ExchangeServer {
    name: String,
    port: String,
    address: SocketAddr,
}

impl ExchangeServer {
    pub fn new(name: String) -> ExchangeServer {
        let ip_address = Ipv4Addr::new(127, 0, 0, 1); // Replace with your desired IP address
        let port = 8080; // Replace with your desired port number
        let socket_addr = SocketAddrV4::new(ip_address, port);
        let socket_addr = SocketAddr::V4(socket_addr);
        ExchangeServer {
            name: name,
            port: port.to_string(),
            address: socket_addr,
        }
    }
    pub fn ip_address(&self) -> String {
        return self.address.to_string();
    }
    pub fn port(&self) -> String {
        return self.port.to_string();
    }
    pub async fn run(&mut self) {
        let tcp_listener = TcpListener::bind(&self.address).await;
        let listener = tcp_listener.unwrap();
        while let Ok((client_stream, _)) = listener.accept().await {
            let name = self.name.clone();
            tokio::spawn(ExchangeServer::accept_connection(name, client_stream));
        }
    }
    async fn accept_connection(_: String, stream: TcpStream) {
        let mut depth_generator = DepthMessageGenerator::default();
        let _ = stream
            .peer_addr()
            .expect("connected streams should have a peer address");
        if let Ok(ws_io) = tokio_tungstenite::accept_async(stream).await {
            let (mut ws_sink, _) = ws_io.split();
            loop {
                let (asks, bids) = depth_generator.generate_depth_bulk(1, 8);
                let mut obj = WSDepthUpdateBinance::default();
                obj.a = ExchangeServer::convert_to_binance(asks);
                obj.b = ExchangeServer::convert_to_binance(bids);
                let text = to_string(&obj);
                if text.is_ok() {
                    info!("sending depth message");
                    let send_result = ws_sink.send(Message::Text(text.unwrap())).await;
                    match send_result {
                        Ok(_) => continue,
                        Err(e) => {
                            warn!("failed to send object {}", e)
                        }
                    }
                } else {
                    warn!("failed to serialize json object")
                }
            }
        }
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
