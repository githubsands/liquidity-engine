use market_objects::DepthUpdate;
use rand::thread_rng;
use rand_distr::{Distribution, Normal};

extern crate rand;

#[derive(Clone)]
pub struct DepthMessageGenerator {
    volume: f64,
    price: f64,
    vol_std: f64,
    price_std: f64,
}

impl DepthMessageGenerator {
    pub fn new(
        initial_volume: f64,
        initial_price: f64,
        vol_std: f64,
        price_std: f64,
    ) -> DepthMessageGenerator {
        DepthMessageGenerator {
            volume: initial_volume,
            price: initial_price,
            vol_std,
            price_std,
        }
    }
    pub fn depth_message_random(&mut self, location: u8) -> DepthUpdate {
        let kind: u8;
        let _ = match rand::random() {
            true => {
                kind = 0;
            }
            false => {
                kind = 1;
            }
        };
        let normal = Normal::new(0.0, 1.0).unwrap();
        let volume_diff = normal.sample(&mut thread_rng()) * self.vol_std;
        let price_diff = normal.sample(&mut thread_rng()) * self.price_std;
        let volume = (self.volume + volume_diff).max(0.0);
        let price: f64;
        let _ = match rand::random() {
            true => {
                price = self.price - price_diff;
            }
            false => {
                price = self.price + price_diff;
            }
        };
        DepthUpdate {
            k: kind,
            p: DepthMessageGenerator::round_to_hundreth(price as f64),
            q: volume as f64,
            l: location,
        }
    }
    pub fn depth_message(&mut self, location: u8, is_ask: bool) -> DepthUpdate {
        let kind: u8 = if is_ask { 0 } else { 1 };
        let normal = Normal::new(0.0, 1.0).unwrap();
        let volume_diff = normal.sample(&mut rand::thread_rng()) * self.vol_std;
        let price_diff = normal.sample(&mut rand::thread_rng()) * self.price_std;
        let volume = (self.volume + volume_diff).max(0.0);
        let price = if is_ask {
            self.price - price_diff
        } else {
            self.price + price_diff
        };
        DepthUpdate {
            k: kind,
            p: DepthMessageGenerator::round_to_hundreth(price as f64),
            q: volume as f64,
            l: location,
        }
    }
    pub fn depth_bulk(&mut self, location: u8, size: u8) -> (Vec<DepthUpdate>, Vec<DepthUpdate>) {
        let asks = vec![0; size as usize];
        let bids = vec![0; size as usize];
        let asks: Vec<DepthUpdate> = asks
            .iter()
            .map(|_| self.depth_message(location, true))
            .collect();
        let bids: Vec<DepthUpdate> = bids
            .iter()
            .map(|_| self.depth_message(location, false))
            .collect();
        (asks, bids)
    }
    fn round_to_hundreth(n: f64) -> f64 {
        (n * 100.0).round() / 100.0
    }
    pub fn depth_balanced_orderbook(
        depth: usize,
        exchange_num: usize,
        mid_price: usize,
    ) -> (Vec<DepthUpdate>, Vec<DepthUpdate>) {
        let mut asks: Vec<DepthUpdate> = vec![];
        let mut bids: Vec<DepthUpdate> = vec![];
        for i in 0..exchange_num {
            for j in mid_price..mid_price + depth {
                let mut depth_update = DepthUpdate::default();
                depth_update.k = 0;
                depth_update.p = j as f64;
                depth_update.q = 1 as f64;
                depth_update.l = i as u8;
                asks.push(depth_update);
            }
        }
        for i in 0..exchange_num {
            for j in mid_price..mid_price - depth {
                let mut depth_update = DepthUpdate::default();
                depth_update.k = 1;
                depth_update.p = j as f64;
                depth_update.q = 1 as f64;
                depth_update.l = i as u8;
                bids.push(depth_update);
            }
        }
        (asks, bids)
    }
    pub fn depth_imbalanced_orderbook_ask(
        depth: usize,
        exchange_num: usize,
        mid_price: usize,
    ) -> (Vec<DepthUpdate>, Vec<DepthUpdate>) {
        let mut asks: Vec<DepthUpdate> = vec![];
        let mut bids: Vec<DepthUpdate> = vec![];
        for i in 1..exchange_num {
            for j in mid_price..mid_price + depth {
                let mut depth_update = DepthUpdate::default();
                depth_update.k = 0;
                depth_update.p = j as f64;
                depth_update.q = 1 as f64;
                depth_update.l = i as u8;
                asks.push(depth_update);
            }
        }
        for i in 1..exchange_num {
            for j in mid_price..mid_price - 2 * depth {
                let mut depth_update = DepthUpdate::default();
                depth_update.k = 1;
                depth_update.p = j as f64;
                depth_update.q = 1 as f64;
                depth_update.l = i as u8;
                bids.push(depth_update);
            }
        }
        (asks, bids)
    }
    pub fn depth_imbalanced_orderbook_bid(
        depth: usize,
        exchange_num: usize,
        mid_price: usize,
    ) -> (Vec<DepthUpdate>, Vec<DepthUpdate>) {
        let mut asks: Vec<DepthUpdate> = vec![];
        let mut bids: Vec<DepthUpdate> = vec![];
        for i in 0..exchange_num {
            for j in mid_price..mid_price + depth * 2 {
                let mut depth_update = DepthUpdate::default();
                depth_update.k = 0;
                depth_update.p = j as f64;
                depth_update.q = 1 as f64;
                depth_update.l = i as u8;
                asks.push(depth_update);
            }
        }
        for i in 0..exchange_num {
            for j in mid_price..mid_price - 2 * depth {
                let mut depth_update = DepthUpdate::default();
                depth_update.k = 1;
                depth_update.p = j as f64;
                depth_update.q = 1 as f64;
                depth_update.l = i as u8;
                bids.push(depth_update);
            }
        }
        (asks, bids)
    }
}

impl Default for DepthMessageGenerator {
    fn default() -> Self {
        Self {
            volume: 400.0,
            price: 27000.0,
            vol_std: 200.0,
            price_std: 0.1,
        }
    }
}
