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

// TODO: This should be a pragma so we can initialize these fixed sized
// arrays at precompile time
struct Level<DepthUpdate> {
    deque: [DepthUpdate; 2],
}

impl Level<DepthUpdate> {
    fn new(direction: u8, price_level: f64) -> Self {
        Level {
            deque: [
                DepthUpdate {
                    k: direction,
                    p: price_level,
                    q: 0.0,
                    l: 1,
                },
                DepthUpdate {
                    k: direction,
                    p: price_level,
                    q: 0.0,
                    l: 2,
                },
            ],
        }
    }
}

struct OrderBook {
    lock: AtomicBool,
    exchange_count: f64,
    ring_buffer: RingBuffer,
    best_deal_bids_level: f64,
    best_deal_asks_level: f64,
    building_book: bool,
    asks: FxHashMap<OrderedFloat<f64>, Level<DepthUpdate>>,
    bids: FxHashMap<OrderedFloat<f64>, Level<DepthUpdate>>,
    quote_producer: Option<TokioSender<Quote>>,
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
        );
        let (ring_buffer, depth_producers) = RingBuffer::new(config.ring_buffer);
        let orderbook: OrderBook = OrderBook {
            lock: AtomicBool::new(false),
            ring_buffer: ring_buffer,
            exchange_count: config.exchange_count as f64,
            best_deal_bids_level: config.mid_price as f64,
            best_deal_asks_level: config.mid_price as f64,
            building_book: true,
            asks: asks,
            bids: bids,
            quote_producer: Some(quote_producer),
            level_diff: config.level_diff,
        };
        (orderbook, depth_producers)
    }
    fn build_orderbook(
        level_diff: f64,
        mid_level: f64,
        depth: f64,
    ) -> (
        FxHashMap<OrderedFloat<f64>, Level<DepthUpdate>>,
        FxHashMap<OrderedFloat<f64>, Level<DepthUpdate>>,
    ) {
        let mut asks = FxHashMap::<OrderedFloat<f64>, Level<DepthUpdate>>::default();
        let mut current_level = mid_level;
        for _ in 0..=depth as u64 * 2 {
            current_level = current_level + level_diff;
            info!("asks: {}", round_to_hundreth(current_level));
            let level = Level::new(0, current_level);
            asks.insert(OrderedFloat(round_to_hundreth(current_level)), level);
        }
        current_level = mid_level;
        for i in 0..=depth as u64 * 2 {
            if i > 0 {
                current_level = current_level - level_diff;
            }
            info!("asks: {}", round_to_hundreth(current_level));
            let level = Level::new(0, current_level);
            asks.insert(OrderedFloat(round_to_hundreth(current_level)), level);
        }
        current_level = mid_level;
        let mut bids = FxHashMap::<OrderedFloat<f64>, Level<DepthUpdate>>::default();
        for _ in 0..=depth as u64 * 2 {
            current_level = current_level + level_diff;
            info!("bids: {}", round_to_hundreth(current_level));
            let level = Level::new(1, current_level);
            bids.insert(OrderedFloat(round_to_hundreth(current_level)), level);
        }
        current_level = mid_level;
        for i in 0..=depth as u64 * 2 {
            if i > 0 {
                current_level = current_level - level_diff;
            }
            info!("bids: {}", round_to_hundreth(current_level));
            let level = Level::new(1, current_level);
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
                        let updated_quantity = entry.q + depth_update.q;
                        if updated_quantity < 0.0 {
                            return Err(ErrorHotPath::OrderBook("negative quantity".to_string()));
                        }
                        entry.q = updated_quantity;
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
                        let updated_quantity = entry.q + depth_update.q;
                        if updated_quantity < 0.0 {
                            return Err(ErrorHotPath::OrderBook(
                                "negative quantity - ignoring this depth update".to_string(),
                            ));
                        }
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
                return Err(ErrorHotPath::OrderBook(
                    "0 1 ASKS or 1 BIDS given by depth update".to_string(),
                ));
            }
        }
    }
    #[inline(always)]
    fn run_quote(&mut self) -> Result<Quote, ErrorHotPath> {
        debug!("traversing asks for the best deals");
        let ask_deals = self.traverse_asks()?;
        debug!("traversing bids for the best deals");
        let bid_deals = self.traverse_bids()?;
        Ok(Quote {
            spread: ask_deals[0].p - bid_deals[0].p,
            ask_deals: ask_deals,
            bid_deals: bid_deals,
        })
    }
    #[inline(always)]
    fn traverse_bids(&mut self) -> Result<[Deal; 10], ErrorHotPath> {
        let mut current_bid_deal_counter: usize = 0;
        let mut deals: [Deal; 10] = [
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
        ];
        let mut current_level: f64 = self.best_deal_asks_level;
        'next_level: loop {
            while current_bid_deal_counter < 10 {
                let current_level_bids =
                    self.asks.get_mut(&OrderedFloat(self.best_deal_asks_level));
                if current_level_bids.is_none() {
                    warn!(
                        "level {} does not exist in the ASKS book",
                        self.best_deal_asks_level
                    );
                    return Err(ErrorHotPath::OrderBook(
                        "level does not exist in orderbook".to_string(),
                    ));
                }
                // get the asks where liquidity exist we do this by filtering out any value that
                // has a quantity of q = 0
                let mut liquid_bid_levels = current_level_bids
                    .unwrap()
                    .deque
                    .into_iter()
                    .filter(|&depth_entry| depth_entry.q != 0.0);
                while let Some(bid) = liquid_bid_levels.next() {
                    current_bid_deal_counter = current_bid_deal_counter + 1;
                    deals[current_bid_deal_counter].p = bid.p; // price level
                    deals[current_bid_deal_counter].l = bid.l; // the exchange id
                    deals[current_bid_deal_counter].q = bid.q; // quantity/volume/liquidity
                }
                // this is the bid side so traverse down the orderbook
                current_level = current_level - self.level_diff;
                continue 'next_level;
            }
            return Ok(deals);
        }
    }
    fn traverse_asks(&mut self) -> Result<[Deal; 10], ErrorHotPath> {
        let mut deals: [Deal; 10] = [
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
            Deal {
                p: 0.0,
                q: 0.0,
                l: 0,
            },
        ];
        let mut current_ask_deal_counter: usize = 0;
        let mut current_level: f64 = self.best_deal_asks_level;
        'next_level: loop {
            while current_ask_deal_counter < 10 {
                let current_level_asks =
                    self.asks.get_mut(&OrderedFloat(self.best_deal_asks_level));
                if current_level_asks.is_none() {
                    warn!(
                        "level {} does not exist in the ASKS book",
                        self.best_deal_asks_level
                    );
                    return Err(ErrorHotPath::OrderBook(
                        "level does not exist in orderbook".to_string(),
                    ));
                }
                // get the asks where liquidity exist we do this by filtering out any value that
                // has a quantity of q = 0
                let mut liquid_asks_levels = current_level_asks
                    .unwrap()
                    .deque
                    .into_iter()
                    .filter(|&depth_entry| depth_entry.q != 0.0);
                while let Some(bid) = liquid_asks_levels.next() {
                    current_ask_deal_counter = current_ask_deal_counter + 1;
                    deals[current_ask_deal_counter].p = bid.p; // price level
                    deals[current_ask_deal_counter].l = bid.l; // the exchange id
                    deals[current_ask_deal_counter].q = bid.q; // quantity/volume/liquidity
                }
                // this is the ask side so traverse up the orderbook
                current_level = current_level + self.level_diff;
                continue 'next_level;
            }
            return Ok(deals);
        }
    }
}

#[derive(Debug)]
pub struct Deal {
    p: f64,
    q: f64,
    l: u8,
}

#[derive(Debug)]
pub struct Quote {
    pub spread: f64,
    pub ask_deals: [Deal; 10],
    pub bid_deals: [Deal; 10],
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::RingBufferConfig;
    use crossbeam_channel::Sender;
    use depth_generator::DepthMessageGenerator;
    use std::thread::sleep;
    use std::time::Duration;
    use test_log::test;
    use testing_traits::ConsumerDefault;
    use tracing::info;
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
            let (asks, bids) = OrderBook::build_orderbook(level_diff, mid_level, depth);
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
                building_book: true,
                asks: asks,
                bids: bids,
                quote_producer: None,
                level_diff: 0.010,
            };
            (Box::new(orderbook), depth_producer)
        }
    }
    #[traced_test]
    #[test]
    fn test_build_orderbook() {
        let (mut orderbook, producer) = OrderBook::consumer_default();
        let mut dg = DepthMessageGenerator::default();
        dg.volume = 400.0;
        dg.price = 2700.0;
        dg.vol_std = 200.0;
        dg.price_std = 15.0; // NOTE: our queue gets filled the larger the price sigma (std) is
        thread::spawn(move || loop {
            let depth_update = dg.depth_message_random();
            loop {
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
            thread::sleep(Duration::from_nanos(1));
            orderbook.consume_depths();
            info!("updating the book");
            orderbook.process_depths();
            /*
            if !orderbook.building_book {
                let quote = orderbook.run_quote();
                match quote {
                    Ok(_) => {}
                    Err(e) => {
                        debug!("received quote error {}", e)
                    }
                }
            }
            */
        });
        thread::sleep(Duration::from_secs(5));
    }
}
