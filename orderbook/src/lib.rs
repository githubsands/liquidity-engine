use crossbeam_channel::{bounded, Sender};
use rustc_hash::FxHashMap;
use std::mem;
use tokio::sync::mpsc::Sender as TokioSender;

use ordered_float::OrderedFloat;

use std::time::Duration;
use thin_vec::ThinVec;

use config::OrderbookConfig;
use internal_objects::{Deal, Quote};
use market_objects::DepthUpdate;

use quoter_errors::ErrorHotPath;
use ring_buffer::RingBuffer;
use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::thread;
use tokio::runtime::Handle;
use tracing::{debug, info, warn};

// TODO: This should be a pragma so we can initialize these fixed sized
// arrays at precompile time
struct Level<LiquidityNode> {
    price: f64,
    deque: [LiquidityNode; 2],
}

#[derive(Copy, Clone, Debug)]
pub struct LiquidityNode {
    q: f64,
    l: u8,
}

impl Level<LiquidityNode> {
    fn new(price_level: f64) -> Self {
        Level {
            price: price_level,
            deque: [
                LiquidityNode { q: 0.0, l: 1 },
                LiquidityNode { q: 0.0, l: 2 },
            ],
        }
    }
}

struct OrderBook {
    ring_buffer: RingBuffer,
    best_deal_bids_level: f64,
    best_deal_asks_level: f64,
    asks: FxHashMap<OrderedFloat<f64>, Level<LiquidityNode>>,
    bids: FxHashMap<OrderedFloat<f64>, Level<LiquidityNode>>,
    level_diff: f64,
    quote_producer: TokioSender<Quote>,
}

fn round_to_hundreth(num: f64) -> f64 {
    (num * 100.0).round() / 1000.0
}

fn round(x: f64, decimals: u32) -> f64 {
    let y = 10i64.pow(decimals) as f64;
    (x * y).round() / y
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
            ring_buffer: ring_buffer,
            best_deal_bids_level: 0.0,
            best_deal_asks_level: 0.0,
            asks: asks,
            bids: bids,
            level_diff: config.level_diff,
            quote_producer: quote_producer,
        };
        (orderbook, depth_producers)
    }
    fn build_orderbook(
        level_diff: f64,
        mid_level: f64,
        depth: f64,
    ) -> (
        FxHashMap<OrderedFloat<f64>, Level<LiquidityNode>>,
        FxHashMap<OrderedFloat<f64>, Level<LiquidityNode>>,
    ) {
        let mut asks = FxHashMap::<OrderedFloat<f64>, Level<LiquidityNode>>::default();
        let mut current_level = mid_level;
        for _ in 0..=depth as u64 * 2 {
            current_level = current_level + level_diff;
            info!("asks: {}", round_to_hundreth(current_level));
            let level = Level::new(current_level);
            asks.insert(OrderedFloat(round_to_hundreth(current_level)), level);
        }
        current_level = mid_level;
        for i in 0..=depth as u64 * 2 {
            if i > 0 {
                current_level = current_level - level_diff;
            }
            info!("asks: {}", round_to_hundreth(current_level));
            let level = Level::new(current_level);
            asks.insert(OrderedFloat(round_to_hundreth(current_level)), level);
        }
        current_level = mid_level;
        let mut bids = FxHashMap::<OrderedFloat<f64>, Level<LiquidityNode>>::default();
        for _ in 0..=depth as u64 * 2 {
            current_level = current_level + level_diff;
            info!("bids: {}", round_to_hundreth(current_level));
            let level = Level::new(current_level);
            bids.insert(OrderedFloat(round_to_hundreth(current_level)), level);
        }
        current_level = mid_level;
        for i in 0..=depth as u64 * 2 {
            if i > 0 {
                current_level = current_level - level_diff;
            }
            info!("bids: {}", round_to_hundreth(current_level));
            let level = Level::new(current_level);
            bids.insert(OrderedFloat(round_to_hundreth(current_level)), level);
        }
        return (asks, bids);
    }
    pub fn consume_depths(&mut self) -> Result<(), ErrorHotPath> {
        let buffer_consume_result = self.ring_buffer.consume();
        match buffer_consume_result {
            Ok(()) => {
                return Ok(());
            }
            // TODO: Work out this buffer error
            Err(_) => return Err(ErrorHotPath::OrderBook("buffer error".to_string())),
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
    fn update_book(&mut self, depth_update: DepthUpdate) -> Result<(), ErrorHotPath> {
        match depth_update.k {
            0 => {
                let update_result = self.ask_update(depth_update);
                match update_result {
                    Ok(_) => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
            1 => {
                let update_result = self.bid_update(depth_update);
                match update_result {
                    Ok(_) => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
            _ => {
                warn!("poor depth update direction received: {}", depth_update.k);
                return Err(ErrorHotPath::OrderBook(
                    "0 1 ASKS or 1 BIDS given by depth update".to_string(),
                ));
            }
        }
    }
    #[inline]
    fn bid_update(&mut self, depth_update: DepthUpdate) -> Result<(), ErrorHotPath> {
        if let Some(bids) = self.bids.get_mut(&OrderedFloat(depth_update.p)) {
            bids.deque
                .iter_mut()
                .find(|liquidity_node| liquidity_node.l == depth_update.l)
                .map(|liquidity_node| liquidity_node.q = depth_update.q);
            bids.deque
                .sort_by(|prev, next| next.q.partial_cmp(&prev.q).unwrap());
        } else {
            warn!(
                "no location {} found at price level {} in BIDS",
                depth_update.l, depth_update.p
            );
            return Err(ErrorHotPath::OrderBook("no level found".to_string()));
        }
        Ok(())
    }
    #[inline]
    fn ask_update(&mut self, depth_update: DepthUpdate) -> Result<(), ErrorHotPath> {
        if let Some(asks) = self.asks.get_mut(&OrderedFloat(depth_update.p)) {
            asks.deque
                .iter_mut()
                .find(|liquidity_node| liquidity_node.l == depth_update.l)
                .map(|liquidity_node| liquidity_node.q = depth_update.q);
            asks.deque
                .sort_by(|prev, next| next.q.partial_cmp(&prev.q).unwrap());
        } else {
            warn!(
                "no location {} found at price level {} in BIDS",
                depth_update.l, depth_update.p
            );
            return Err(ErrorHotPath::OrderBook("no level found".to_string()));
        }
        Ok(())
    }

    #[inline]
    fn run_quote(&mut self) -> Result<(), ErrorHotPath> {
        let ask_deals = self.traverse_asks()?;
        let bid_deals = self.traverse_bids()?;
        let quote = Quote {
            spread: ask_deals[0].p - bid_deals[0].p,
            ask_deals: ask_deals,
            bid_deals: bid_deals,
        };
        self.send_quote(quote)?;
        Ok(())
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
        while current_ask_deal_counter < 10 {
            let current_level_asks = self.asks.get_mut(&OrderedFloat(current_level));
            if current_level_asks.is_none() {
                warn!("level {} does not exist in the ASKS book", current_level);
                return Err(ErrorHotPath::OrderBook(
                    "level does not exist in orderbook".to_string(), //TODO: get rid of this dst
                ));
            }
            let mut liquid_asks_levels = current_level_asks
                .unwrap()
                .deque
                .into_iter()
                .filter(|&liquidity_node| liquidity_node.q != 0.0)
                .peekable();
            if liquid_asks_levels.peek().is_none() {
                current_level = round(current_level + 0.01, 2);
                continue;
            }
            while let Some(bid) = liquid_asks_levels.next() {
                deals[current_ask_deal_counter].p = current_level; // price level
                deals[current_ask_deal_counter].l = bid.l; // the exchange id
                deals[current_ask_deal_counter].q = bid.q; // quantity/volume/liquidity
                current_ask_deal_counter += 1;
            }
            // this is the ask side so traverse up the orderbook
            current_level = round(current_level + 0.01, 2);
            continue;
        }
        return Ok(deals);
    }
    #[inline]
    fn traverse_bids(&mut self) -> Result<[Deal; 10], ErrorHotPath> {
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
        let mut current_bid_deal_counter: usize = 0;
        let mut current_level: f64 = self.best_deal_bids_level;
        'next_level: loop {
            while current_bid_deal_counter < 10 {
                let current_level_bids = self.bids.get_mut(&OrderedFloat(current_level));
                if current_level_bids.is_none() {
                    warn!("level {} does not exist in the BIDS book", current_level);
                    return Err(ErrorHotPath::OrderBook(
                        "level does not exist in orderbook".to_string(), // TODO: get rid of this
                                                                         // dst
                    ));
                }
                let mut liquid_bids_levels = current_level_bids
                    .unwrap()
                    .deque
                    .into_iter()
                    .filter(|&liquidity_node| liquidity_node.q != 0.0)
                    .peekable();
                if liquid_bids_levels.peek().is_none() {
                    debug!("is none");
                    current_level = round(current_level - 0.01, 2);
                    continue;
                }
                while let Some(bid) = liquid_bids_levels.next() {
                    deals[current_bid_deal_counter].p = current_level; // price level
                    deals[current_bid_deal_counter].l = bid.l; // the exchange id
                    deals[current_bid_deal_counter].q = bid.q; // quantity/volume/liquidity
                    current_bid_deal_counter += 1;
                }
                // this is the bid side so traverse down the orderbook
                current_level = round(current_level - 0.01, 2);
                continue 'next_level;
            }
            return Ok(deals);
        }
    }
    #[inline]
    fn send_quote(&self, quote: Quote) -> Result<(), ErrorHotPath> {
        while let Err(e) = self.quote_producer.try_send(quote) {
            warn!("failed to send quote to grpc server {:?}", quote);
            continue;
        }
        Ok(())
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Deal {
    pub p: f64,
    pub q: f64,
    pub l: u8,
}

#[derive(Copy, Clone, Debug)]
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
    use std::thread;
    use std::time::Duration;
    use test_log::test;
    use testing_traits::ConsumerDefault;
    use tokio::sync::mpsc::{channel, Sender as TokioSender};
    use tracing::info;
    use tracing_test::traced_test;

    impl<'a> ConsumerDefault<'a, OrderBook, DepthUpdate> for OrderBook {
        fn consumer_default() -> (Box<Self>, Sender<DepthUpdate>) {
            let level_diff: f64 = 0.10;
            let mid_level: f64 = 27000.0; // TODO: this needs to be thought about
            let depth: f64 = 5000.0;
            let (asks, bids) = OrderBook::build_orderbook(level_diff, mid_level, depth);
            let (ring_buffer, depth_producer) = RingBuffer::new(RingBufferConfig {
                ring_buffer_size: 300,
                channel_buffer_size: 300,
            });
            let (quote_producer, _) = channel(10);
            let orderbook = OrderBook {
                ring_buffer: ring_buffer,
                best_deal_bids_level: 0.0,
                best_deal_asks_level: 0.0,
                asks: asks,
                bids: bids,
                level_diff: 0.010,
                quote_producer: quote_producer,
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
    #[traced_test]
    #[test]
    fn test_sorted_by_greatest_volume_bids_and_asks() {
        debug!("running this test");
        let (mut orderbook, _) = OrderBook::consumer_default();
        let price_level = 2700.63;
        let result = orderbook.bid_update(DepthUpdate {
            k: 0,
            p: price_level,
            q: 30.0,
            l: 1,
        });
        let result = orderbook.bid_update(DepthUpdate {
            k: 0,
            p: price_level,
            q: 40.0,
            l: 2,
        });
        let level = orderbook.bids.get_mut(&OrderedFloat(price_level)).unwrap();
        let decaying_volumes_check = level.deque.iter().scan(100.0, |past_value, deal| {
            let result = *past_value < deal.q;
            if !result {
                *past_value = deal.q;
            }
            info!(past_value);
            Some(result)
        });
        debug!("{:?}", level.deque);
        for boolean in decaying_volumes_check {
            assert!(boolean == false)
        }
        let (mut orderbook, _) = OrderBook::consumer_default();
        let result = orderbook.bid_update(DepthUpdate {
            k: 0,
            p: price_level,
            q: 77.0,
            l: 1,
        });
        let result = orderbook.bid_update(DepthUpdate {
            k: 0,
            p: price_level,
            q: 134.0,
            l: 2,
        });
        assert!(result.is_err() != true);
        let decaying_volumes_check = level.deque.iter().scan(100.0, |past_value, deal| {
            let result = *past_value < deal.q;
            if !result {
                *past_value = deal.q;
            }
            info!(past_value);
            Some(result)
        });
        debug!("{:?}", level.deque);
        for boolean in decaying_volumes_check {
            debug!(boolean);
            assert!(boolean == false)
        }
    }
    #[test]
    #[traced_test]
    fn test_traverse_asks() {
        info!("testing traverse asks");
        let (mut orderbook, _) = OrderBook::consumer_default();
        let best_deal = 2700.00;
        let result = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: best_deal,
            q: 40.0,
            l: 1,
        });
        let result = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2700.63, // skip some levels
            q: 1.0,
            l: 2,
        });
        let result = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2700.63,
            q: 77.0,
            l: 1, // add liquidty at another location (exchange)
        });
        let result = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2700.64,
            q: 13.0,
            l: 1,
        });
        let result = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2705.65,
            q: 25.0,
            l: 2,
        });
        let result = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2705.66,
            q: 34.0,
            l: 1,
        });
        let result = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2705.67,
            q: 23.0,
            l: 1,
        });
        let result = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2705.68,
            q: 99.0,
            l: 1,
        });
        let result = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2705.69,
            q: 70.0,
            l: 1,
        });
        let result = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2709.70,
            q: 47.0,
            l: 1,
        });
        let result = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2709.71, // TODO: BUG this value returns a 2709.7
            q: 23.0,
            l: 1,
        });

        // (1) test that we have inserted depths with no issues
        assert!(result.is_ok() == true);

        // (2) test that our traverse asks return deals with no errors
        orderbook.best_deal_asks_level = best_deal;
        orderbook.level_diff = 0.10;
        let result = orderbook.traverse_asks();
        assert!(result.is_ok() == true);
        let deals: [Deal; 10] = result.unwrap();

        // (3) test that none of our deals have a quantity of 0
        assert!(deals.iter().any(|depth| depth.q == 0.0) != true);
    }
    #[test]
    #[traced_test]
    fn test_traverse_bids() {
        info!("testing traverse bids");
        let (mut orderbook, _) = OrderBook::consumer_default();
        let best_deal = 2700.00;
        let result = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: best_deal,
            q: 30.0,
            l: 1,
        });
        let result = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2699.63, // skip some levels
            q: 1.0,
            l: 2,
        });
        let result = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2698.64,
            q: 77.0,
            l: 1, // add liquidty at another location (exchange)
        });
        let result = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2650.33,
            q: 13.09,
            l: 1,
        });
        let result = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2649.39,
            q: 25.22,
            l: 2,
        });
        let result = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2649.20,
            q: 34.88,
            l: 1,
        });
        let result = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2649.20,
            q: 23.0,
            l: 1,
        });
        let result = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2648.33,
            q: 99.1,
            l: 1,
        });
        let result = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2647.99,
            q: 70.22,
            l: 1,
        });
        let result = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2645.33,
            q: 47.20,
            l: 1,
        });
        let result = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2666.22,
            q: 23.33,
            l: 1,
        });

        // (1) test that we have inserted depths with no issues
        assert!(result.is_ok() == true);

        // (2) test that our traverse asks return deals with no errors
        orderbook.best_deal_bids_level = best_deal;
        orderbook.level_diff = 0.10;
        let result = orderbook.traverse_bids();
        assert!(result.is_ok() == true);
        let deals: [Deal; 10] = result.unwrap();
        info!("deals are: {:?}", deals);

        // (3) test that none of our deals have a quantity of 0
        assert!(deals.iter().any(|depth| depth.q == 0.0) != true);

        /*
        let decaying_price_level_check = deals.iter().scan(0.0, |past_value, deal| {
            let result = *past_value < deal.q;
            if !result {
                *past_value = deal.q;
            }
            info!(past_value);
            Some(result)
        });
        for boolean in decaying_price_level_check {
            assert!(boolean != true)
        }
        */
    }
}
