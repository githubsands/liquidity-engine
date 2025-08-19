use crate::local::DepthUpdate;
use serde::{Deserialize, Serialize};
use serde_this_or_that::as_f64;

#[allow(non_snake_case)]
#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub struct OrderBookUpdateBinance {
    pub b: f64, // best bid price
    pub B: f64, // best bid qty
    pub a: f64, // best ask price
    pub A: f64, // best ask qty
}

#[allow(non_snake_case)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OrderBookDepthUpdateBinance {
    #[serde(deserialize_with = "as_f64")]
    pub e: f64,
    #[serde(deserialize_with = "as_f64")]
    pub E: f64,
    #[serde(deserialize_with = "as_f64")]
    pub s: f64,
    #[serde(deserialize_with = "as_f64")]
    pub U: f64,
    #[serde(deserialize_with = "as_f64")]
    pub u: f64,
    pub b: Vec<BinanceDepthUpdate>, // todo: verify
    pub a: Vec<BinanceDepthUpdate>,
}

/// OrderbookUpdateBinance returns a bid and a ask order
impl OrderBookUpdateBinance {
    pub fn split_update(self) -> (DepthUpdate, DepthUpdate) {
        (
            DepthUpdate {
                k: 0,
                p: self.b,
                q: self.B,
                l: 1,
            },
            DepthUpdate {
                k: 1,
                p: self.a,
                q: self.A,
                l: 1,
            },
        )
    }
}

#[allow(non_snake_case)]
pub struct HTTPSnapShotDepthResponseBinance {
    pub retcode: Option<i32>,
    pub lastUpdateId: u64,
    pub asks: Vec<BinanceDepthUpdate>,
    pub bids: Vec<BinanceDepthUpdate>,
}

impl HTTPSnapShotDepthResponseBinance {
    pub fn orders(self) -> (Vec<DepthUpdate>, Vec<DepthUpdate>) {
        let asks: Vec<DepthUpdate> = self
            .asks
            .iter()
            .map(|item| DepthUpdate {
                l: 1,
                p: item.price,
                q: item.quantity,
                k: 0,
            })
            .collect();
        let bids: Vec<DepthUpdate> = self
            .bids
            .iter()
            .map(|item| DepthUpdate {
                l: 1,
                p: item.price,
                q: item.quantity,
                k: 1,
            })
            .collect();
        (asks, bids)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct BinanceDepthUpdate {
    #[serde(deserialize_with = "as_f64")]
    pub price: f64,
    #[serde(deserialize_with = "as_f64")]
    pub quantity: f64,
}

#[feature(zero_copy)] 
{
    struct DepthUpdate{
        inut: u8,
    }
}
