use crossbeam_channel::Sender;
use rustc_hash::FxHashMap;
use tokio::sync::mpsc::Sender as TokioSender;
use tracing::info;

use ordered_float::OrderedFloat;

use bounded_vec_deque::BoundedVecDeque;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::{hint, thread};

use std::marker::PhantomData;
use tracing::error;
use typed_arena::Arena;

use std::f64::NAN;

use thin_str::ThinStr;
use thin_vec::ThinVec;

use config::OrderbookConfig;
use market_object::DepthUpdate;
use quoter_errors::ErrorHotPath;
use ring_buffer::RingBuffer;

use std::collections::VecDeque;

struct Level<DepthUpdate> {
    exchanges: u8,
    deque: BoundedVecDeque<DepthUpdate>,
}

impl Level<DepthUpdate> {
    fn new(exchanges: u8) -> Self {
        Level {
            exchanges: exchanges,
            deque: BoundedVecDeque::new(exchanges as usize),
        }
    }
    fn insert(&mut self, entry: DepthUpdate) -> impl Iterator<Item = &DepthUpdate> {
        if self.deque.is_empty() || entry.q > self.deque.front().unwrap().q {
            self.deque.push_front(entry);
        } else {
            let index = self
                .deque
                .iter()
                .position(|current| entry.q <= current.q)
                .unwrap_or_else(|| self.deque.len());
            self.deque.insert(index, entry);
        }
        return self.deque.iter().filter(|entry| entry.q > 0.0);
    }
    fn get_deals(&self, current_deals: u8) -> impl Iterator<Item = &DepthUpdate> {
        let deal_num = self.exchanges - current_deals;
        // if we have more exchanges in the vector then the remaining 10 deals only
        // return a few deals else return all the deals in the level
        if self.exchanges > deal_num {
            return self.deque.iter().filter(|entry| entry.q > 0.0);
        } else {
            return self.deque.iter().filter(|entry| entry.q > 0.0);
        }
    }
}

struct DepthEntry {
    pub volume: f64,
    pub location: u8,
}

struct OrderBook {
    exchange_count: f64,
    level_increment: f64, // level increment is the basis points for each of each depth difference
    // for this asset
    ring_buffer: RingBuffer,
    best_deal_bids_level: f64,
    best_deal_asks_level: f64,
    asks: FxHashMap<OrderedFloat<f64>, Level<DepthUpdate>>,
    bids: FxHashMap<OrderedFloat<f64>, Level<DepthUpdate>>,
    quote_producer: TokioSender<Quote>,
}

impl OrderBook {
    pub fn new(
        quote_producer: TokioSender<Quote>,
        config: OrderbookConfig,
    ) -> (OrderBook, Sender<DepthUpdate>) {
        let ask_level_range: i64 = config.mid_price + config.depth;
        let mut asks = FxHashMap::<OrderedFloat<f64>, Level<DepthUpdate>>::default();
        for i in config.mid_price..ask_level_range {
            let level_price: i64 = i as i64;
            let mut level = Level::new(config.exchange_count as usize);
            asks.insert(OrderedFloat(level_price as f64), level);
        }
        let bid_level_range: i64 = config.mid_price - config.depth;
        let mut bids = FxHashMap::<OrderedFloat<f64>, Level<DepthUpdate>>::default();
        for j in bid_level_range..config.mid_price {
            let level_price: f64 = j as f64;
            let mut level = Level::new(config.exchange_count as usize);
            asks.insert(OrderedFloat(level_price as f64), level);
        }
        let (ring_buffer, depth_producers) = RingBuffer::new(config.ring_buffer);
        let orderbook: OrderBook = OrderBook {
            ring_buffer: ring_buffer,
            exchange_count: config.exchange_count as f64,
            level_increment: 0.0,
            best_deal_bids_level: 0.0,
            best_deal_asks_level: 0.0,
            asks: asks,
            bids: bids,
            quote_producer: quote_producer,
        };
        (orderbook, depth_producers)
    }
    fn consume_depths(&mut self) {
        self.ring_buffer.consume();
    }
    fn receive_depths(&mut self) {
        while let Some(depth_update) = self.ring_buffer.pop_depth() {
            // TODO: insert simple locking for now.
            if let Ok((spread, bids, asks)) = self.update_book(depth_update) {}
        }
    }
    fn update_book(
        &mut self,
        depth_update: DepthUpdate,
    ) -> Result<
        (
            f64,
            impl Iterator<Item = &DepthUpdate>,
            impl Iterator<Item = &DepthUpdate>,
        ),
        ErrorHotPath,
    > {
        match depth_update.k {
            0 => {
                let best_asks = self
                    .asks
                    .get_mut(&OrderedFloat(depth_update.p))
                    .unwrap()
                    .insert(depth_update);
                let best_bids = self.get_deals_bids();
                let float: f64 = best_asks.pop_front() - best_bids.pop_front();
                Ok((float, best_bids, best_asks))
            }
            1 => {
                let best_asks = self
                    .asks
                    .get_mut(&OrderedFloat(depth_update.p))
                    .unwrap()
                    .insert(depth_update);
                let best_bids = self.get_deals_bids();
                let float: f64 = best_asks.pop_front() - best_bids.pop_front();
                Ok((float, best_bids, best_asks))
            }
            _ => Err(ErrorHotPath::OrderBook),
        }
    }
    /*
    fn get_deals_bids(&mut self) -> impl Iterator<Item = DepthEntry> {
        let mut deal_count: usize = 0;
        let mut ten_deals = Vec::new();
        let mut current_level = self.best_deal_asks_level;
        'deal_counter: while deal_count < 11 {
            for deals in self.best_deal_bids_level as u8..(self.best_deal_asks_level - 10.0) as u8 {
                if let Some(depth_entry) = self.bids.get(&OrderedFloat(current_level)) {
                    // todo: make sure we can only return 10 deals
                    let deals_iterators = depth_entry.get_deals();
                    let depth_entries = deals_iterators.collect::<Vec<&DepthEntry>>();
                    deal_count = deal_count + deals_iterators.count();
                    current_level = current_level - 1.0;
                    ten_deals.extend(depth_entries);
                    continue 'deal_counter;
                } else {
                    break 'deal_counter;
                }
            }
        }
        return ten_deals;
    }
    */
    fn get_deals_asks(&mut self) -> impl Iterator<Item = &DepthEntry> {
        let mut deal_count: usize = 0;
        let mut ten_deals = Vec::new();
        let mut current_level = self.best_deal_asks_level;
        'deal_counter: while deal_count < 11 {
            for deals in self.best_deal_bids_level as u8..(self.best_deal_asks_level - 10.0) as u8 {
                if let Some(depth_entry) = self.bids.get(&OrderedFloat(current_level)) {
                    // todo: make sure we can only return 10 deals
                    let deals_iterators = depth_entry.get_deals();
                    let depth_entries = deals_iterators.collect::<Vec<&DepthEntry>>();
                    deal_count = deal_count + deals_iterators.count();
                    current_level = current_level + 1.0;
                    ten_deals.extend(depth_entries);
                    continue 'deal_counter;
                } else {
                    break 'deal_counter;
                }
            }
        }
        return ten_deals;
    }
    // TODO: get_quotes_$side should be ran in their own thread.
    // TODO: handle channel errors
    /*
    pub async fn quote(&mut self) -> Result<(), ErrorHotPath> {
        let ask_deals = self.get_quotes_asks();
        let bid_deals = self.get_quotes_bids();
        // ask_deals should always be higher then bid deals. if not we have a problem
        self.quote_producer.send(Quotes {
            spread: ask_deals.get(0).unwrap().price - bid_deals.get(0).unwrap().price,
            best_asks: ask_deals,
            best_bids: bid_deals,
        });
        Ok(())
    }
    */
}
pub struct Deal {
    volume: f64,
    price: f64,
    location: u8,
}

pub struct Quote {
    pub spread: f64,
    pub best_bids: Vec<DepthEntry>,
    pub best_asks: Vec<DepthEntry>,
}
