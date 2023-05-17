use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ByBitOrder {
    pub price: f64,
    pub quantity: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ByBitData {
    pub s: String,
    pub t: u64,
    pub a: Vec<ByBitOrder>,
    pub b: Vec<ByBitOrder>,
}

#[allow(non_snake_case)]
#[derive(Debug, Serialize, Deserialize)]
pub struct HTTPSnapShotDepthResponseByBit {
    pub retMsg: Option<String>,
    pub topic: String,
    pub ts: u64,
    pub r#type: String,
    pub data: ByBitData,
}
