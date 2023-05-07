use std::fs::File;
use std::io::Read;
use std::path::Path;
use serde::{Serialize, Deserialize};
use serde_yaml;


#[derive(Debug, Deserialize)]
pub struct Config {
    exchanges: Vec<ExchangeConfig>,
    io_thread_percent: f64,
    ring_buffer_size: usize,
    channel_buffer_size: usize,
}


#[derive(Debug, Deserialize)]
struct ExchangeConfig {
    name: String,
    uri: String,
    orderbook_subscription_message: SubscriptionMessage,
}

#[derive(Debug, Deserialize)]
struct SubscriptionMessage {
    method: String,
    params: Vec<String>,
    id: u32,
}

pub fn read_yaml_config<P: AsRef<Path>>(path: P) -> Result<Config, Box<dyn std::error::Error>> {
    let mut file = File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let config: Config = serde_yaml::from_str(&contents)?;
    Ok(config)
}
