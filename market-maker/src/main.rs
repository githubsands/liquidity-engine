pub mod pb {
    tonic::include_proto!("lib");
}

use async_trait::async_trait;
use futures::stream::Stream;
use std::env;
use std::process;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::StreamExt;
use tonic::transport::Channel;

use pb::{quoter_client::QuoterClient, QuoterRequest};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut server_uri: String = "".to_string();
    if let Ok(orderbook_quoter_server_uri) = env::var("ORDERBOOK_QUOTER_SERVER_URI") {
        println!(
            "orderbook-quoter-server uri is: {}",
            orderbook_quoter_server_uri
        );
        server_uri = orderbook_quoter_server_uri;
    } else {
        println!("ORDERBOOK_QUOTER_SERVER_URI is not set");
        process::exit(0);
    }
    let mut client = QuoterClient::connect(server_uri).await.unwrap();
    stream_quotes(&mut client, 50000).await;
    Ok(())
}

async fn stream_quotes(client: &mut QuoterClient<Channel>, num: usize) {
    let stream = client
        .server_streaming_quoter(QuoterRequest {})
        .await
        .unwrap()
        .into_inner();
    let mut stream = stream.take(num);
    while let Some(quote) = stream.next().await {
        println!("\treceived quote: {}", quote.unwrap().spread)
    }
}
