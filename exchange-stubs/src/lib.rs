use futures_util::{future, StreamExt, TryStreamExt};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tracing::{info, warn};
use tungstenite::{accept, WebSocket};

pub struct ExchangeServer {
    address: SocketAddr,
    tcp_listener: Option<Rc<TcpListener>>,
    websocket: Option<WebSocketStream<TcpStream>>,
}

impl ExchangeServer {
    pub fn new() -> ExchangeServer {
        let ip_address = Ipv4Addr::new(127, 0, 0, 1); // Replace with your desired IP address
        let port = 8080; // Replace with your desired port number
        let socket_addr = SocketAddrV4::new(ip_address, port);
        let socket_addr = SocketAddr::V4(socket_addr);
        ExchangeServer {
            tcp_listener: None,
            websocket: None,
            address: socket_addr,
        }
    }
    pub async fn run(&mut self) {
        let tcp_listener = TcpListener::bind(&self.address).await;
        self.tcp_listener = Some(Rc::new(tcp_listener.unwrap()));
        while let Ok((stream, _)) = self.tcp_listener.clone().unwrap().accept().await {
            tokio::spawn(ExchangeServer::accept_connection(stream));
        }
    }
    async fn accept_connection(stream: TcpStream) {
        let addr = stream
            .peer_addr()
            .expect("connected streams should have a peer address");
        info!("Peer address: {}", addr);

        let ws_stream = tokio_tungstenite::accept_async(stream)
            .await
            .expect("Error during the websocket handshake occurred");

        info!("New WebSocket connection: {}", addr);

        let (write, read) = ws_stream.split();
        // We should not forward messages other than text or binary.
        read.try_filter(|msg| future::ready(msg.is_text() || msg.is_binary()))
            .forward(write)
            .await
            .expect("Failed to forward messages")
    }
}
