use serde::Deserialize;
use serde_yaml;
use std::fs::File;
use std::io::Read;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub io_thread_percentage: f64,
    pub exchanges: Vec<ExchangeConfig>,
    pub orderbook: OrderbookConfig,
    pub grpc_server: GRPCServerConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OrderbookConfig {
    pub ring_buffer: RingBufferConfig,
    pub exchange_count: i64,
    pub depth: i64,
    pub mid_price: i64,
    pub tick_size: f64,
    pub ticker: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RingBufferConfig {
    pub ring_buffer_size: usize,
    pub channel_buffer_size: usize,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExchangeConfig {
    pub client_name: String,
    pub exchange_name: u8,
    pub snapshot_enabled: bool,
    pub snapshot_uri: String,
    pub ws_uri: String,
    pub ws_poll_rate_milliseconds: u8,
    pub http_client: bool,
    pub depth: u64,
    pub buffer_size: usize,
    pub watched_pair: String,
    pub ignore_snapshot_websocket: Option<bool>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GRPCServerConfig {
    pub host_uri: String,
}

pub fn read_yaml_config<P: AsRef<Path>>(path: P) -> Result<Config, Box<dyn std::error::Error>> {
    let mut file = File::open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;

    let config: Config = serde_yaml::from_str(&contents)?;
    Ok(config)
}
