use market_object::DepthUpdate;
use rand::thread_rng;
use rand_distr::{Distribution, Normal};
use std::iter;

extern crate rand;

#[derive(Clone)]
pub struct DepthMessageGenerator {
    volume: f64,
    price: f64,
    vol_std: f64,
    price_std: f64,
}

enum MessageType {
    Bid,
    Ask,
}

impl DepthMessageGenerator {
    fn new(
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
        let mut kind: u8;
        let msg_type = match rand::random() {
            true => {
                kind = 0;
            }
            false => {
                kind = 1;
            }
        };
        let normal = Normal::new(0.0, 1.0).unwrap();
        let volume_diff = normal.sample(&mut rand::thread_rng()) * self.vol_std;
        let price_diff = normal.sample(&mut rand::thread_rng()) * self.price_std;
        let volume = (self.volume + volume_diff).max(0.0);
        let mut price: f64;
        let msg_type = match rand::random() {
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
    pub fn generate_depth_bulk(
        &mut self,
        location: u8,
        size: u8,
    ) -> (Vec<DepthUpdate>, Vec<DepthUpdate>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    fn test_depth_message_generator_spread() {
        let mut depth_message_generator = DepthMessageGenerator::new(400.0, 27000.0, 200.0, 0.1);
        println!(
            "depth message is {:?}",
            depth_message_generator.depth_message(0)
        );
    }
}
