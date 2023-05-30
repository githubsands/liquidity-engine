use futures::stream::SplitSink;
use futures_util::{future, SinkExt, StreamExt, TryStreamExt};
use market_object::DepthUpdate;
use rand::distributions::{Distribution, Uniform};
use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;
use std::{net::SocketAddr, time::Duration};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::WebSocketStream;
use tracing::{info, warn};
use tungstenite::{accept, Message, WebSocket};

pub struct ExchangeServer {
    name: &str,
    address: SocketAddr,
    websocket: Option<WebSocketStream<TcpStream>>,
    writer: Option<SplitSink<TcpStream, Message>>,
}

impl ExchangeServer {
    pub fn new(name: &str) -> ExchangeServer {
        let ip_address = Ipv4Addr::new(127, 0, 0, 1); // Replace with your desired IP address
        let port = 8080; // Replace with your desired port number
        let socket_addr = SocketAddrV4::new(ip_address, port);
        let socket_addr = SocketAddr::V4(socket_addr);
        ExchangeServer {
            name: name,
            writer: None::<SplitSink<TcpStream, Message>>,
            websocket: None,
            address: socket_addr,
        }
    }
    pub async fn run(&mut self) {
        let tcp_listener = TcpListener::bind(&self.address).await;
        let listener = tcp_listener.unwrap();
        while let Ok((stream, _)) = listener.accept().await {
            // tokio::spawn(ExchangeServer::accept_connection(stream));
            tokio::spawn(ExchangeServer::accept_connection(self.name, stream));
        }
    }
    async fn accept_connection(name: &str, stream: TcpStream) {
        let addr = stream
            .peer_addr()
            .expect("connected streams should have a peer address");
        info!("Peer address: {}", addr);
        let mut interval = tokio::time::interval(Duration::from_millis(1000));
        if let Ok(ws_stream) = tokio_tungstenite::accept_async(stream).await {
            let (mut write, mut read) = ws_stream.split();
            loop {
                tokio::select! {
                        msg = read.next() => {
                            match name {
                                "1" => {
                                    write.send(Message::Text("".to_string())).await;
                                }
                                "2" => {
                                    write.send(Message::Text("".to_string())).await;
                                }
                                "3" => {
                                    write.send(Message::Text("".to_string())).await;
                                    }
                            }
                    }
                }
            }
        }
    }
    async fn make_market(
        name: &str,
        depth: f64,
        orderbook_diff: f64,,
    ) -> impl Iterator<Item = DepthUpdate> {
        let range = Uniform::new(-1.0f64, 1.0);
        let mut rng = rand::thread_rng();

        let total = 1_000_000;
        let mut in_circle = 0;

        for _ in 0..total {
            let bid = range.sample(&mut rng);
            let ask = range.sample(&mut rng);
            if a * a + b * b <= 1.0 {
                in_circle += 1;
            }
        }

        let mut depths: Vec<DepthUpdate>;
        return depths.into_iter();
    }
}
