use market_objects::DepthUpdate;
use rand::{thread_rng, Rng};
use rand_distr::{Distribution, Normal};

extern crate rand;

#[derive(Clone)]
pub struct DepthMessageGenerator {
    pub volume: f64,
    pub price: f64,
    pub vol_std: f64,
    pub price_std: f64,
    pub level_diff: f64,
    pub tick_size: f64,
}

impl DepthMessageGenerator {
    pub fn new(
        initial_volume: f64,
        initial_price: f64,
        vol_std: f64,
        price_std: f64,
        level_diff: f64,
        tick_size: f64,
    ) -> DepthMessageGenerator {
        DepthMessageGenerator {
            volume: initial_volume,
            price: initial_price,
            vol_std,
            price_std,
            level_diff,
            tick_size,
        }
    }
    pub fn depth_message_random(&mut self) -> DepthUpdate {
        let kind: u8;
        let location: u8;
        let _ = match rand::random() {
            true => {
                kind = 0;
            }
            false => {
                kind = 1;
            }
        };
        let _ = match rand::random() {
            true => {
                location = 1;
            }
            false => {
                location = 2;
            }
        };

        let normal = Normal::new(0.0, 1.0).unwrap();
        let volume_diff = normal.sample(&mut thread_rng()) * self.vol_std;
        let price_diff = normal.sample(&mut thread_rng()) * self.price_std;
        let volume = (self.volume + volume_diff).max(0.0);
        let price: f64;
        match kind {
            0 => {
                price = self.price - price_diff;
            }
            1 => {
                price = self.price + price_diff;
            }
            _ => {
                panic!()
            }
        };
        DepthUpdate {
            k: kind,
            p: DepthMessageGenerator::round_to_hundreth(price as f64),
            q: volume as f64,
            l: location,
            s: false,
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
            p: round_to_hundreth(price as f64),
            q: volume as f64,
            l: location,
            s: false,
        }
    }
    pub fn depth_bulk(
        &mut self,
        exchange_locations: u8,
        depth_size_one_sided: u8,
    ) -> (Vec<DepthUpdate>, Vec<DepthUpdate>) {
        let mut rng = rand::thread_rng();

        let asks = vec![0; depth_size_one_sided as usize];
        let bids = vec![0; depth_size_one_sided as usize];
        let asks: Vec<DepthUpdate> = asks
            .iter()
            .map(|_| self.depth_message(rng.gen_range(1..=exchange_locations), true))
            .collect();
        let bids: Vec<DepthUpdate> = bids
            .iter()
            .map(|_| self.depth_message(rng.gen_range(1..=exchange_locations), false))
            .collect();
        (asks, bids)
    }
    fn round_to_hundreth(n: f64) -> f64 {
        (n * 100.0).round() / 100.0
    }
    pub fn depth_balanced_orderbook(
        &mut self,
        depth: usize,
        exchange_locations: usize,
        mid_point: usize,
    ) -> (Vec<DepthUpdate>, Vec<DepthUpdate>) {
        let mut asks: Vec<DepthUpdate> = vec![];
        let mut bids: Vec<DepthUpdate> = vec![];
        for i in 1..=exchange_locations {
            let mut current_level: f64 = mid_point as f64; // reset the mid point for each exchange
            for _ in 0..depth {
                let normal = Normal::new(0.0, self.vol_std).unwrap();
                let volume_diff = normal.sample(&mut thread_rng()).abs();
                let mut depth_update = DepthUpdate::default();
                depth_update.k = 0;
                depth_update.p = current_level;
                depth_update.q = volume_diff;
                depth_update.l = i as u8;
                depth_update.s = true;
                asks.push(depth_update);
                current_level = round(current_level + self.tick_size, 100.0);
            }
        }
        for i2 in 1..=exchange_locations {
            let mut current_level: f64 = mid_point as f64;
            for _ in 0..depth {
                let normal = Normal::new(0.0, self.vol_std).unwrap();
                let volume_diff = normal.sample(&mut thread_rng()).abs();
                let mut depth_update = DepthUpdate::default();
                depth_update.k = 1;
                depth_update.p = current_level;
                depth_update.q = volume_diff;
                depth_update.l = i2 as u8;
                depth_update.s = true;
                bids.push(depth_update);
                current_level = round(current_level - self.tick_size, 100.0);
            }
        }
        (asks, bids)
    }
    pub fn depth_market_continuity_from_existing_book(
        &self,
        snapshot: (Vec<DepthUpdate>, Vec<DepthUpdate>),
    ) -> (Vec<DepthUpdate>, Vec<DepthUpdate>) {
        let mut asks: Vec<DepthUpdate> = vec![];
        let mut bids: Vec<DepthUpdate> = vec![];

        // build corresponding bids from the asks that are on the book
        for depth in snapshot.0 {
            asks.push(DepthUpdate {
                k: 0,
                p: depth.p,
                q: -1.0 * (depth.q / 2.0), // stimulate asks orders being filled on an exchange. a
                // negative update that does not exceed or already
                // existing liquidity
                l: depth.l,
                s: false, // not a snapshot but a depth update (false)
            })
        }
        // build corresponding asks from the bids that are on the book
        for depth in snapshot.1 {
            bids.push(DepthUpdate {
                k: 1,
                p: depth.p,
                q: -1.0 * (depth.q / 2.0), // stimulate bid orders being filled on an exchange...
                l: depth.l,
                s: false, // not a snapshot but a depth update (false)
            })
        }
        return (asks, bids);
    }
    pub fn depth_generate_geometric_brownian_motion_upward_trend(
        &self,
        exchanges: u8,
        tick: f64,
        s_0: f64,
        dt: f64,
        spread: f64,
        length: usize,
        drift: f64,
        diffusion: f64,
    ) -> (Vec<DepthUpdate>, Vec<DepthUpdate>) {
        let mut rng = rand::thread_rng();
        let dist = Normal::new(0.0, 1.0).unwrap();
        let depth_update_asks_0 = DepthUpdate {
            k: 0,
            p: s_0,
            q: 30.0,
            l: 1,
            s: false,
        };
        let depth_update_bids_0 = DepthUpdate {
            k: 1,
            p: s_0,
            q: 30.0,
            l: 1,
            s: false,
        };
        let mut asks = Vec::<DepthUpdate>::with_capacity(length);
        let mut bids = Vec::<DepthUpdate>::with_capacity(length);
        let mut price_diffs = Vec::<f64>::with_capacity(length);
        let mut price_levels = Vec::<f64>::with_capacity(length);
        price_diffs.push(tick);
        price_levels.push(s_0);
        asks.push(depth_update_asks_0);
        bids.push(depth_update_bids_0);
        let drift_factor = 0.1 + drift * dt;
        let diffusion_factor = diffusion * dt.sqrt();
        for idx in 1..length {
            let price_diff = drift_factor + diffusion_factor * dist.sample(&mut rng);
            let price_current = price_levels[idx - 1] + price_diffs[idx - 1];
            price_levels.push(price_current);
            price_diffs.push(price_diff);
            for i in 1..=exchanges {
                asks.push(DepthUpdate {
                    k: 0,
                    p: DepthMessageGenerator::round_to_hundreth(price_current),
                    q: 30.0,
                    l: i,
                    s: false,
                });
                bids.push(DepthUpdate {
                    k: 1,
                    p: DepthMessageGenerator::round_to_hundreth(price_current),
                    q: 30.0,
                    l: i,
                    s: false,
                })
            }
            for i in 1..=exchanges {
                asks.push(DepthUpdate {
                    k: 0,
                    p: DepthMessageGenerator::round_to_hundreth(price_current - spread),
                    q: -30.0,
                    l: i,
                    s: false,
                });
                bids.push(DepthUpdate {
                    k: 1,
                    p: DepthMessageGenerator::round_to_hundreth(price_current - spread),
                    q: -30.0,
                    l: i,
                    s: false,
                })
            }
        }
        (asks, bids)
    }
    pub fn depth_generate_geometric_brownian_motion_downward_trend(
        &self,
        exchanges: u8,
        tick: f64,
        s_0: f64,
        dt: f64,
        spread: f64,
        length: usize,
        drift: f64,
        diffusion: f64,
    ) -> (Vec<DepthUpdate>, Vec<DepthUpdate>) {
        let mut rng = rand::thread_rng();
        let dist = Normal::new(0.0, 1.0).unwrap();
        let depth_update_asks_0 = DepthUpdate {
            k: 0,
            p: s_0,
            q: 30.0,
            l: 1,
            s: false,
        };
        let depth_update_bids_0 = DepthUpdate {
            k: 1,
            p: s_0,
            q: 30.0,
            l: 1,
            s: false,
        };
        let mut asks = Vec::<DepthUpdate>::with_capacity(length);
        let mut bids = Vec::<DepthUpdate>::with_capacity(length);
        let mut price_diffs = Vec::<f64>::with_capacity(length);
        let mut price_levels = Vec::<f64>::with_capacity(length);
        price_diffs.push(tick);
        price_levels.push(s_0);
        asks.push(depth_update_asks_0);
        bids.push(depth_update_bids_0);
        let drift_factor = 0.1 + drift * dt;
        let diffusion_factor = diffusion * dt.sqrt();
        for idx in 1..length {
            let price_diff = drift_factor + diffusion_factor * dist.sample(&mut rng);
            let price_current = price_levels[idx - 1] - price_diffs[idx - 1];
            price_levels.push(price_current);
            price_diffs.push(price_diff);
            for i in 1..=exchanges {
                asks.push(DepthUpdate {
                    k: 0,
                    p: round_to_hundreth(price_current),
                    q: 30.0,
                    l: i,
                    s: false,
                });
                bids.push(DepthUpdate {
                    k: 1,
                    p: round_to_hundreth(price_current),
                    q: 30.0,
                    l: i,
                    s: false,
                })
            }
            for i in 1..=exchanges {
                asks.push(DepthUpdate {
                    k: 0,
                    p: round_to_hundreth(price_current - spread),
                    q: -30.0,
                    l: i,
                    s: false,
                });
                bids.push(DepthUpdate {
                    k: 1,
                    p: round_to_hundreth(price_current - spread),
                    q: -30.0,
                    l: i,
                    s: false,
                })
            }
        }
        (asks, bids)
    }
}

fn round(num: f64, place: f64) -> f64 {
    (num * place).round() / (place)
}

fn round_to_hundreth(num: f64) -> f64 {
    (num * 100.0).round() / 1000.0
}

impl Default for DepthMessageGenerator {
    fn default() -> Self {
        Self {
            volume: 400.0,
            price: 2700.0,
            vol_std: 200.0,
            price_std: 0.1,
            level_diff: 0.01,
            tick_size: 0.01,
        }
    }
}
