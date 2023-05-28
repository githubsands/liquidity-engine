use crossbeam_channel::Sender;
use rustc_hash::FxHashMap;
use tokio::sync::mpsc::Sender as TokioSender;

use ordered_float::OrderedFloat;

use thin_vec::ThinVec;

use config::OrderbookConfig;
use market_object::DepthUpdate;
use quoter_errors::ErrorHotPath;
use ring_buffer::RingBuffer;
use std::thread;
use tracing::{error, info, warn};

struct Level<DepthUpdate> {
    deque: ThinVec<DepthUpdate>,
}

impl Level<DepthUpdate> {
    fn new(exchanges: u8) -> Self {
        Level {
            deque: ThinVec::with_capacity(exchanges as usize),
        }
    }
}

struct DepthEntry {
    pub volume: f64,
    pub location: u8,
}

struct OrderBook {
    exchange_count: f64,
    ring_buffer: RingBuffer,
    best_deal_bids_level: f64,
    best_deal_asks_level: f64,
    asks: FxHashMap<OrderedFloat<f64>, Level<DepthUpdate>>,
    bids: FxHashMap<OrderedFloat<f64>, Level<DepthUpdate>>,
    quote_producer: TokioSender<Quote>,
    best_ten_asks_cache: ThinVec<DepthUpdate>,
    best_ten_bids_cache: ThinVec<DepthUpdate>,
    level_diff: f64,
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
            let mut level = Level::new(config.exchange_count as u8);
            asks.insert(OrderedFloat(level_price as f64), level);
        }
        let bid_level_range: i64 = config.mid_price - config.depth;
        let mut bids = FxHashMap::<OrderedFloat<f64>, Level<DepthUpdate>>::default();
        for j in bid_level_range..config.mid_price {
            let level_price: f64 = j as f64;
            let mut level = Level::new(config.exchange_count as u8);
            asks.insert(OrderedFloat(level_price as f64), level);
        }
        let (ring_buffer, depth_producers) = RingBuffer::new(config.ring_buffer);
        let orderbook: OrderBook = OrderBook {
            ring_buffer: ring_buffer,
            exchange_count: config.exchange_count as f64,
            best_deal_bids_level: 0.0,
            best_deal_asks_level: 0.0,
            asks: asks,
            bids: bids,
            best_ten_asks_cache: ThinVec::with_capacity(10),
            best_ten_bids_cache: ThinVec::with_capacity(10),
            quote_producer: quote_producer,
            level_diff: config.level_diff,
        };
        (orderbook, depth_producers)
    }
    fn consume_depths(&mut self) {
        self.ring_buffer.consume();
    }
    fn receive_depths(&mut self) {
        while let Some(depth_update) = self.ring_buffer.pop_depth() {
            if let Err(err) = self.update_book(depth_update) {
                warn!("failed to update the book")
            }
        }
    }
    // check if we have a better deal.  if we have a better deal provide a quote and the insert
    // the deal
    // if we don't have a better deal insert the quote
    fn update_book(&mut self, depth_update: DepthUpdate) -> Result<(), ErrorHotPath> {
        match depth_update.k {
            0 => {
                if let Some(asks) = self.asks.get_mut(&OrderedFloat(depth_update.p)) {
                    if let Some(entry) = asks.deque.iter_mut().find(|e| e.l == depth_update.l) {
                        entry.q = depth_update.q;
                        asks.deque.sort_by(|a, b| a.q.partial_cmp(&b.q).unwrap());
                    }
                }
                Ok(())
            }
            1 => {
                if let Some(bids) = self.bids.get_mut(&OrderedFloat(depth_update.p)) {
                    if let Some(entry) = bids.deque.iter_mut().find(|e| e.l == depth_update.l) {
                        entry.q = depth_update.q;
                        bids.deque.sort_by(|a, b| a.q.partial_cmp(&b.q).unwrap());
                    }
                }
                Ok(())
            }
            _ => Err(ErrorHotPath::OrderBook),
        }
    }
    fn traverse_asks(&mut self) {
        let count: u8 = 0;
        let mut entry_count: u8 = 0;
        let mut level: f64 = self.best_deal_asks_level;
        while entry_count < 10 {
            let deals = &mut self
                .asks
                .get_mut(&OrderedFloat(self.best_deal_asks_level))
                .unwrap()
                .deque;
            self.best_ten_asks_cache
                .iter_mut()
                .zip(deals)
                .for_each(|(existing, new)| {
                    if entry_count == 10 {
                        return;
                    }
                    entry_count += 1;
                    *existing = *new;
                });
            level = level + self.level_diff;
            if entry_count >= count {
                break;
            }
        }
    }
    fn traverse_bids(&mut self) {
        let count: u8 = 0;
        let mut entry_count: u8 = 0;
        let mut level: f64 = self.best_deal_asks_level;
        while entry_count < 10 {
            let deals = &mut self
                .asks
                .get_mut(&OrderedFloat(self.best_deal_asks_level))
                .unwrap()
                .deque;
            self.best_ten_asks_cache
                .iter_mut()
                .zip(deals)
                .for_each(|(existing, new)| {
                    if entry_count == 10 {
                        return;
                    }
                    entry_count += 1;
                    *existing = *new;
                });
            level = level - self.level_diff;
            if entry_count >= count {
                break;
            }
        }
    }
    fn quote(&mut self) {
        let spread = self.best_ten_asks_cache.first().unwrap().p
            - self.best_ten_bids_cache.first().unwrap().p;
        let mut asks: ThinVec<DepthUpdate> = ThinVec::new();
        asks.extend_from_slice(&self.best_ten_asks_cache[..10]);
        let mut bids: ThinVec<DepthUpdate> = ThinVec::new();
        bids.extend_from_slice(&self.best_ten_bids_cache[..10]);
        if let Err(send_error) = self.quote_producer.try_send(Quote {
            spread: spread,
            asks: asks,
            bids: bids,
        }) {
            warn!("failed to send quote: {}", send_error)
        }
    }
}

pub struct Quote {
    spread: f64,
    asks: ThinVec<DepthUpdate>,
    bids: ThinVec<DepthUpdate>,
}
