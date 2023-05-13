use serde::{Deserialize, Serialize};
use serde_yaml;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub exchanges: Vec<ExchangeConfig>,
    pub order_book: OrderbookConfig,
    pub grpc_server: GRPCServerConfig,
}

#[derive(Debug, Deserialize)]
pub struct OrderbookConfig {
    pub ring_buffer: RingBufferConfig,
}

#[derive(Debug, Deserialize)]
pub struct RingBufferConfig {
    pub ring_buffer_size: usize,
    pub channel_buffer_size: usize,
}

#[derive(Debug, Deserialize)]
pub struct ExchangeConfig {
    pub name: String,
    pub uri: String,
    pub orderbook_subscription_message: SubscriptionMessage,
    pub buffer_size: usize,
}

#[derive(Debug, Deserialize)]
pub struct GRPCServerConfig {
    pub host_uri: String,
}

#[derive(Debug, Deserialize)]
pub struct SubscriptionMessage {
    pub method: String,
    pub params: Vec<String>,
    pub id: u32,
}

pub fn read_yaml_config<P: AsRef<Path>>(path: P) -> Result<Config, Box<dyn std::error::Error>> {
    let mut file = File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let config: Config = serde_yaml::from_str(&contents)?;
    Ok(config)
}
