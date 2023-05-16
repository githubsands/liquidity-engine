use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub struct OrderBookUpdateBinance {
    pub b: f64, // best bid price
    pub B: f64, // best bid qty
    pub a: f64, // best ask price
    pub A: f64, // best ask qty
}

/// OrderbookUpdateBinance returns a bid and a ask order
impl OrderBookUpdateBinance {
    pub fn split_update(self) -> (Order, Order) {
        (
            Order {
                k: 0,
                p: self.b,
                q: self.B,
                l: 1,
            },
            Order {
                k: 1,
                p: self.a,
                q: self.A,
                l: 1,
            },
        )
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ByBitData {
    pub s: String,
    pub t: u64,
    pub a: Vec<ByBitOrder>,
    pub b: Vec<ByBitOrder>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SnapShotDepthResponseByBit {
    pub retCode: i32,
    pub retMsg: String,
    pub topic: String,
    pub ts: u64,
    pub r#type: String,
    pub data: ByBitData,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct SnapShotDepthResponseBinance {
    pub lastUpdateId: u64,
    pub bids: Vec<BinanceOrder>,
    pub asks: Vec<BinanceOrder>,
}

impl SnapShotDepthResponseBinance {
    pub fn orders(self) -> (Vec<Order>, Vec<Order>) {
        let asks: Vec<Order> = self
            .asks
            .iter()
            .map(|item| Order {
                l: 1,
                p: item.price,
                q: item.quantity,
                k: 0,
            })
            .collect();
        let bids: Vec<Order> = self
            .bids
            .iter()
            .map(|item| Order {
                l: 1,
                p: item.price,
                q: item.quantity,
                k: 1,
            })
            .collect();
        (asks, bids)
    }
}

impl SnapShotDepthResponseByBit {
    pub fn orders(&self) -> (Vec<Order>, Vec<Order>) {
        let asks: Vec<Order> = self
            .data
            .a
            .iter()
            .map(|item| Order {
                l: 0,
                p: item.price,
                q: item.quantity,
                k: 0,
            })
            .collect();
        let bids: Vec<Order> = self
            .data
            .a
            .iter()
            .map(|item| Order {
                l: 0,
                p: item.price,
                q: item.quantity,
                k: 1,
            })
            .collect();
        (asks, bids)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ByBitOrder {
    pub price: f64,
    pub quantity: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BinanceOrder {
    pub price: f64,
    pub quantity: f64,
}

// TODO: Rename this OrderBookUpdate
#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
// Order defines an order object:
// k: 0 ask, 1 bid
// p: price or level
// q: quantity
// l: location: 0 bybit, 1 binance
pub struct Order {
    pub k: u8,
    pub p: f64,
    pub q: f64,
    pub l: u8,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MarketUpdate {
    lastUpdateId: u64,
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}
