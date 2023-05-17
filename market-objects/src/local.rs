use serde::{Deserialize, Serialize};

// TODO: Rename this OrderBookUpdate
#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
// Order defines an order object:
// k: 0 ask, 1 bid
// p: price or level
// q: quantity
// l: location: 0 bybit, 1 binance
pub struct DepthUpdate {
    pub k: u8,
    pub p: f64,
    pub q: f64,
    pub l: u8,
}
