use crossbeam_channel::{bounded, Sender};
use rustc_hash::FxHashMap;
use std::mem;
use tokio::sync::mpsc::Sender as TokioSender;

use ordered_float::OrderedFloat;

use thin_vec::ThinVec;

use config::OrderbookConfig;
use market_object::DepthUpdate;
use quoter_errors::ErrorHotPath;
use ring_buffer::RingBuffer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{EnvFilter, FmtSubscriber};

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
    lock: AtomicBool,
    exchange_count: f64,
    ring_buffer: RingBuffer,
    best_deal_bids_level: f64,
    best_deal_asks_level: f64,
    asks: FxHashMap<OrderedFloat<f64>, Level<DepthUpdate>>,
    bids: FxHashMap<OrderedFloat<f64>, Level<DepthUpdate>>,
    quote_producer: Option<TokioSender<Quote>>,
    best_ten_asks_cache: ThinVec<DepthUpdate>,
    best_ten_bids_cache: ThinVec<DepthUpdate>,
    level_diff: f64,
}

fn round_to_hundreth(num: f64) -> f64 {
    (num * 100.0).round() / 1000.0
}

impl OrderBook {
    pub fn new(
        quote_producer: TokioSender<Quote>,
        config: OrderbookConfig,
    ) -> (OrderBook, Sender<DepthUpdate>) {
        let (asks, bids) = OrderBook::build_orderbook(
            config.level_diff,
            config.mid_price as f64,
            config.depth as f64,
            config.exchange_count as u8,
        );
        let (ring_buffer, depth_producers) = RingBuffer::new(config.ring_buffer);
        let orderbook: OrderBook = OrderBook {
            lock: AtomicBool::new(false),
            ring_buffer: ring_buffer,
            exchange_count: config.exchange_count as f64,
            best_deal_bids_level: config.mid_price as f64,
            best_deal_asks_level: config.mid_price as f64,
            asks: asks,
            bids: bids,
            best_ten_asks_cache: ThinVec::with_capacity(10),
            best_ten_bids_cache: ThinVec::with_capacity(10),
            quote_producer: Some(quote_producer),
            level_diff: config.level_diff,
        };
        (orderbook, depth_producers)
    }
    fn build_orderbook(
        level_diff: f64,
        mid_level: f64,
        depth: f64,
        exchange_count: u8,
    ) -> (
        FxHashMap<OrderedFloat<f64>, Level<DepthUpdate>>,
        FxHashMap<OrderedFloat<f64>, Level<DepthUpdate>>,
    ) {
        let mut asks = FxHashMap::<OrderedFloat<f64>, Level<DepthUpdate>>::default();
        let mut current_level = mid_level;
        for i in (0..=depth as u64 * 2) {
            current_level = current_level + level_diff;
            info!("asks: {}", round_to_hundreth(current_level));
            let mut level = Level::new(exchange_count);
            for i2 in 1..=exchange_count {
                level.deque.push(DepthUpdate {
                    k: 1,
                    p: current_level,
                    q: 0.0,
                    l: i2,
                })
            }
            asks.insert(OrderedFloat(round_to_hundreth(current_level)), level);
        }
        current_level = mid_level;
        for i in (0..=depth as u64 * 2) {
            if i > 0 {
                current_level = current_level - level_diff;
            }
            info!("asks: {}", round_to_hundreth(current_level));
            let mut level = Level::new(exchange_count);
            for i2 in 1..=exchange_count {
                level.deque.push(DepthUpdate {
                    k: 1,
                    p: current_level,
                    q: 0.0,
                    l: i2,
                })
            }
            asks.insert(OrderedFloat(round_to_hundreth(current_level)), level);
        }
        current_level = mid_level;
        let mut bids = FxHashMap::<OrderedFloat<f64>, Level<DepthUpdate>>::default();
        for i in (0..=depth as u64 * 2) {
            current_level = current_level + level_diff;
            info!("bids: {}", round_to_hundreth(current_level));
            let mut level = Level::new(exchange_count);
            for i2 in 1..=exchange_count {
                level.deque.push(DepthUpdate {
                    k: 1,
                    p: current_level,
                    q: 0.0,
                    l: i2,
                })
            }
            bids.insert(OrderedFloat(round_to_hundreth(current_level)), level);
        }
        current_level = mid_level;
        for i in (0..=depth as u64 * 2) {
            if i > 0 {
                current_level = current_level - level_diff;
            }
            info!("bids: {}", round_to_hundreth(current_level));
            let mut level = Level::new(exchange_count);
            for i2 in 1..=exchange_count {
                level.deque.push(DepthUpdate {
                    k: 1,
                    p: current_level,
                    q: 0.0,
                    l: i2,
                })
            }
            bids.insert(OrderedFloat(round_to_hundreth(current_level)), level);
        }

        (asks, bids)
    }
    pub fn consume_depths(&mut self) {
        let result = self.ring_buffer.consume();
        if result.is_err() {
            info!("failed {:#?}", result);
            panic!("happend {:?}", result)
        }
    }
    // ran in its own thread
    pub fn process_depths(&mut self) {
        if let Some(depth_update) = self.ring_buffer.pop_depth() {
            //while self.lock.compare_and_swap(false, false, Ordering::Acquire) != false {
            if let Err(update_book_err) = self.update_book(depth_update) {
                warn!("failed to update the book: {}", update_book_err)
            }
        } else {
            warn!("no depth in buffer or buffer not working")
        }
    }
    #[inline(always)]
    fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }
    // check if we have a better deal.  if we have a better deal provide a quote and the insert
    // the deal
    // if we don't have a better deal insert the quote
    fn update_book(&mut self, depth_update: DepthUpdate) -> Result<(), ErrorHotPath> {
        match depth_update.k {
            0 => {
                if let Some(asks) = self.asks.get_mut(&OrderedFloat(depth_update.p)) {
                    if depth_update.p < self.best_deal_asks_level {
                        self.best_deal_asks_level = depth_update.p
                    }
                    if let Some(entry) = asks.deque.iter_mut().find(|e| e.l == depth_update.l) {
                        entry.q = entry.q + depth_update.q;
                        asks.deque.sort_by(|a, b| a.q.partial_cmp(&b.q).unwrap());
                    } else {
                        warn!(
                            "no location {} found at price level {} in ASKS",
                            depth_update.l, depth_update.p
                        );
                    }
                } else {
                    warn!("no price level {} found", depth_update.p)
                }

                Ok(())
            }
            1 => {
                if let Some(bids) = self.bids.get_mut(&OrderedFloat(depth_update.p)) {
                    if depth_update.p > self.best_deal_bids_level {
                        self.best_deal_asks_level = depth_update.p
                    }
                    if let Some(entry) = bids.deque.iter_mut().find(|e| e.l == depth_update.l) {
                        entry.q = entry.q + depth_update.q;
                        bids.deque.sort_by(|a, b| a.q.partial_cmp(&b.q).unwrap());
                    } else {
                        warn!(
                            "no location {} found at price level {} in BIDS",
                            depth_update.l, depth_update.p
                        );
                    }
                } else {
                    warn!("no price level {} found", depth_update.p)
                }
                Ok(())
            }
            _ => {
                warn!("poor depth update direction received: {}", depth_update.k);
                Err(ErrorHotPath::OrderBook)
            }
        }
    }
    #[inline(always)]
    fn run_quote(&mut self) {
        // self.traverse_asks();
        self.traverse_bids();
        // self.quote();
    }
    #[inline(always)]
    fn traverse_asks(&mut self) {
        let count: u8 = 0;
        let mut entry_count: u8 = 0;
        let mut level: f64 = self.best_deal_asks_level;
        while entry_count < 10 {
            debug!("traversing bids at entry_count {}", entry_count);
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
        // info!("cache is {:#?}", self.best_deal_asks_level);
    }
    #[inline(always)]
    fn traverse_bids(&mut self) {
        let deals: [DepthUpdate; 10];
        let count: u8 = 0;
        let mut entry_count: u8 = 0;
        let mut level: f64 = self.best_deal_asks_level;
        debug!("beginning traverse {}", entry_count);
        while entry_count < 10 {
            // while entry count is not yet 10 get each level and build the best bid deals
            //

            if let Some(deals) = self.asks.get_mut(&OrderedFloat(level)) {
                debug!("deals {:?}", deals.deque);
                while let Some(depth_update) = &mut self
                    .asks
                    .get_mut(&OrderedFloat(level))
                    .unwrap()
                    .deque
                    .into_iter()
                    .filter(|&depth_entry| depth_entry.q != 0.0)
                    .next()
                {}

                /*
                self.best_ten_bids_cache
                    .iter_mut()
                    .zip(deals.deque.iter())
                    .for_each(|(existing, new)| {
                        if entry_count == 10 {
                            debug!("entry_count is {}", entry_count);
                            return;
                        }
                        entry_count += 1;
                        *existing = *new;
                        info!("traversing bids at deal {}", entry_count)
                    });
                */
            } else {
                warn!("level {} does not exist", level);
            }
            level = level - self.level_diff;
            if entry_count >= count {
                info!("breaking count");
                break;
            }
        }
        info!("cache is {:#?}", self.best_ten_bids_cache)
    }
    #[inline(always)]
    fn quote(&mut self) {
        loop {
            if let Err(send_error) = self.quote_producer.as_mut().unwrap().try_send(Quote {
                spread: self.best_ten_asks_cache[0].p - self.best_ten_bids_cache[0].p,
                ask_1: self.best_ten_asks_cache[0],
                ask_2: self.best_ten_asks_cache[1],
                ask_3: self.best_ten_asks_cache[2],
                ask_4: self.best_ten_asks_cache[3],
                ask_5: self.best_ten_asks_cache[4],
                ask_6: self.best_ten_asks_cache[5],
                ask_7: self.best_ten_asks_cache[6],
                ask_8: self.best_ten_asks_cache[7],
                ask_9: self.best_ten_asks_cache[8],
                ask_10: self.best_ten_asks_cache[9],
                bid_1: self.best_ten_bids_cache[0],
                bid_2: self.best_ten_bids_cache[1],
                bid_3: self.best_ten_bids_cache[2],
                bid_4: self.best_ten_bids_cache[3],
                bid_5: self.best_ten_bids_cache[4],
                bid_6: self.best_ten_bids_cache[5],
                bid_7: self.best_ten_bids_cache[6],
                bid_8: self.best_ten_bids_cache[7],
                bid_9: self.best_ten_bids_cache[8],
                bid_10: self.best_ten_bids_cache[9],
            }) {
                warn!("failed to send quote: {}", send_error);
                continue;
            } else {
                return;
            }
        }
    }
}

pub struct Quote {
    pub spread: f64,
    pub ask_1: DepthUpdate,
    pub ask_2: DepthUpdate,
    pub ask_3: DepthUpdate,
    pub ask_4: DepthUpdate,
    pub ask_5: DepthUpdate,
    pub ask_6: DepthUpdate,
    pub ask_7: DepthUpdate,
    pub ask_8: DepthUpdate,
    pub ask_9: DepthUpdate,
    pub ask_10: DepthUpdate,
    pub bid_1: DepthUpdate,
    pub bid_2: DepthUpdate,
    pub bid_3: DepthUpdate,
    pub bid_4: DepthUpdate,
    pub bid_5: DepthUpdate,
    pub bid_6: DepthUpdate,
    pub bid_7: DepthUpdate,
    pub bid_8: DepthUpdate,
    pub bid_9: DepthUpdate,
    pub bid_10: DepthUpdate,
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{ExchangeConfig, RingBufferConfig};
    use crossbeam_channel::{bounded, Sender};
    use depth_generator::DepthMessageGenerator;
    use env_logger::{Builder, Target};
    use std::thread::sleep as threadSleep;
    use std::time::Duration;
    use test_log::test;
    use testing_traits::ConsumerDefault;
    use tracing::{info, Level as tracingLevel};
    use tracing_test::traced_test;

    impl<'a> ConsumerDefault<'a, OrderBook, DepthUpdate> for OrderBook {
        fn consumer_default() -> (Box<Self>, Sender<DepthUpdate>) {
            let level_diff: f64 = 0.10;
            let mid_level: f64 = 27000.0;
            let depth: f64 = 2500.0;
            let exchange_count: u8 = 2;
            let ring_buffer_config = RingBufferConfig {
                ring_buffer_size: 1000,
                channel_buffer_size: 30,
            };
            let (asks, bids) =
                OrderBook::build_orderbook(level_diff, mid_level, depth, exchange_count);
            let (ring_buffer, depth_producer) = RingBuffer::new(RingBufferConfig {
                ring_buffer_size: 300,
                channel_buffer_size: 300,
            });
            let orderbook = OrderBook {
                lock: AtomicBool::new(false),
                exchange_count: 2.0,
                ring_buffer: ring_buffer,
                best_deal_bids_level: 0.0,
                best_deal_asks_level: 0.0,
                asks: asks,
                bids: bids,
                quote_producer: None,
                best_ten_asks_cache: ThinVec::with_capacity(10),
                best_ten_bids_cache: ThinVec::with_capacity(10),
                level_diff: 0.010,
            };
            (Box::new(orderbook), depth_producer)
        }
    }
    #[traced_test]
    #[test]
    fn test_produce_quote_simple() {
        info!("test");
        let (mut orderbook, producer) = OrderBook::consumer_default();
        let mut dg = DepthMessageGenerator::default();
        dg.volume = 400.0;
        dg.price = 2700.0;
        dg.vol_std = 200.0;
        dg.price_std = 15.0; // NOTE: our queue gets filled the larger the price sigma (std) is
        thread::spawn(move || loop {
            let depth_update = dg.depth_message_random();
            loop {
                // NOTE: sleep 1 second so our buffer does not get full and has time to process depths
                thread::sleep(Duration::from_nanos(3));
                let send_result = producer.try_send(depth_update);
                match send_result {
                    Ok(_) => {
                        break;
                    }
                    Err(e) => {
                        info!("send failure retrying {:?}", e);
                        continue;
                    }
                }
            }
        });
        thread::spawn(move || loop {
            orderbook.consume_depths();
            orderbook.process_depths();
            orderbook.run_quote();
        });
        thread::sleep(Duration::from_secs(1000));
    }
}
