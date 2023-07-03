pub mod pb {
    tonic::include_proto!("lib");
}

use futures::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use std::env;
use std::process;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message, MaybeTlsStream};
use tonic::transport::Channel;
use url::Url;

use pb::{quoter_client::QuoterClient, QuoterRequest, QuoterResponse};

const AGENT: &str = "market-maker-strategy-tbd";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut orderbook_quoter_server_uri: String = "".to_string();
    if let Ok(orderbook_quoter_server_uri) = env::var("ORDERBOOK_QUOTER_SERVER_URI") {
        println!(
            "orderbook-quoter-server uri is: {}",
            orderbook_quoter_server_uri
        );
    } else {
        println!("ORDERBOOK_QUOTER_SERVER_URI is not set");
        process::exit(0);
    }
    let mut exchange_server_one_uri: String = "".to_string();
    if let Ok(exchange_server_one_uri) = env::var("EXCHANGE_SERVER_1_URI") {
        println!("exchange server 1 uri is: {}", exchange_server_one_uri);
    } else {
        println!("exchange server 1 uri is not set");
        process::exit(0);
    }
    let mut exchange_server_two_uri: String = "".to_string();
    if let Ok(exchange_server_two_uri) = env::var("EXCHANGE_SERVER_2_URI") {
        println!("exchange server 2 uri is: {}", exchange_server_two_uri);
    } else {
        println!("exchange server 2 uri is not set");
        process::exit(0);
    }
    let mut client = QuoterClient::connect(orderbook_quoter_server_uri)
        .await
        .unwrap();
    /*
    let mut market_maker = MarketMaker::new().stream_quotes(&mut client, 50000).await;
    market_maker.submit_trades();
    */
    Ok(())
}

struct Exchange {
    uri: String,
    order_sink: Option<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>>,
    order_stream: Option<SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>>,
}

impl Exchange {
    fn new(uri: String) -> Exchange {
        Exchange {
            uri: uri,
            order_sink: None,
            order_stream: None,
        }
    }
    async fn boot(&mut self) {
        let (websocket_stream, _) = connect_async("ws://".to_owned() + self.uri.as_str())
            .await
            .expect("Failed to connect");
        let (sink, stream) = websocket_stream.split();
        self.order_sink = Some(sink);
        self.order_stream = Some(stream);
    }
    async fn receive_order_stream(&mut self) {
        while let Some(order_feedback) = self.order_stream.as_mut().unwrap().next().await {}
    }
    async fn send_order_transaction(
        &mut self,
    ) -> Result<(), tokio_tungstenite::tungstenite::Error> {
        self.order_sink
            .as_mut()
            .unwrap()
            .send(Message::Text("wip".to_string()));
        Ok(())
    }
}

// TODO: We need to get feedback from the exchange i.e status of our orders so add in a ws reader
// here
pub struct MarketMaker {
    agent: String,
    exchanges: Vec<Exchange>,
    quote_producer: Sender<QuoterResponse>,
    quote_consumer: Receiver<QuoterResponse>,
}

impl MarketMaker {
    fn new(exchange_uris: Vec<String>) -> Self {
        let (producer, consumer) = channel(1000);
        let mut exchanges: Vec<Exchange> = vec![];
        for uri in exchange_uris {
            let exchange = Exchange::new(uri);
            exchanges.push(exchange);
        }
        MarketMaker {
            agent: "market-maker-strategy-tbd".to_string(),
            exchanges,
            quote_producer: producer,
            quote_consumer: consumer,
        }
    }
    async fn stream_quotes(self, client: &mut QuoterClient<Channel>, num: usize) {
        let stream = client
            .server_streaming_quoter(QuoterRequest {})
            .await
            .unwrap()
            .into_inner();
        let mut stream = stream.take(num);
        while let Some(quote) = stream.next().await {
            println!("\treceived quote: {}", quote.unwrap().spread);
        }
    }
}
