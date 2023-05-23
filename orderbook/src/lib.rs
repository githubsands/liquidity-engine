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

pub struct Stack<T> {
    head: Link<T>,
}

type Link<T> = Option<Box<Node<T>>>;

struct Node<T> {
    elem: T,
    next: Link<T>,
}

// TODO: Possibly add current pointer
struct OrderBookEntry<'a> {
    pub volume: f64,
    pub location: u8,
    _phantom: PhantomData<&'a ()>,
}

// TODO: Possibly get rid of options sense unwrapping can cause branching to occur.
impl<T> Stack<T> {
    pub fn new() -> Self {
        Stack { head: None }
    }

    pub fn push(&mut self, elem: T) {
        let new_node = Box::new(Node {
            elem: elem,
            next: self.head.take(),
        });

        self.head = Some(new_node);
    }
    pub fn pop(&mut self) -> Option<T> {
        self.head.take().map(|node| {
            let node = *node;
            self.head = node.next;
            node.elem
        })
    }
    pub fn peek(&self) -> Option<&T> {
        self.head.as_ref().map(|node| &node.elem)
    }
    pub fn peek_mut(&mut self) -> Option<&mut T> {
        self.head.as_mut().map(|node| &mut node.elem)
    }
}

impl<T> Drop for Stack<T> {
    fn drop(&mut self) {
        let mut cur_link = self.head.take();
        while let Some(mut boxed_node) = cur_link {
            cur_link = boxed_node.next.take();
        }
    }
}

struct OrderBook<'a> {
    exchange_count: f64,
    level_increment: f64, // level increment is the basis points for each of each depth difference
    // for this asset
    ring_buffer: RingBuffer,
    best_deal_bids_level: f64,
    best_deal_asks_level: f64,
    asks: FxHashMap<OrderedFloat<f64>, Stack<OrderBookEntry<'a>>>,
    bids: FxHashMap<OrderedFloat<f64>, Stack<OrderBookEntry<'a>>>,
    quote_producer: TokioSender<Quotes>,
}

impl<'a> OrderBook<'a> {
    pub fn new(
        quote_producer: TokioSender<Quotes>,
        config: OrderbookConfig,
    ) -> (OrderBook<'a>, Sender<DepthUpdate>) {
        let ask_level_range: i64 = (config.mid_price + config.depth) as i64;
        let mut asks = FxHashMap::<OrderedFloat<f64>, Stack<OrderBookEntry<'a>>>::default();
        for i in config.mid_price..ask_level_range {
            let level: i64 = i as i64;
            let mut volume_nodes: Stack<OrderBookEntry<'a>> = Stack::new();
            for _ in 0..config.exchange_count {
                volume_nodes.push(OrderBookEntry {
                    volume: 0.0,
                    location: 0,
                    _phantom: PhantomData,
                });
            }
            asks.insert(OrderedFloat(level as f64), volume_nodes);
        }
        let bid_level_range: i64 = config.mid_price - config.depth;
        let mut bids = FxHashMap::<OrderedFloat<f64>, Stack<OrderBookEntry<'a>>>::default();
        for j in bid_level_range..config.mid_price {
            let level: f64 = j as f64;
            let mut volume_nodes: Stack<OrderBookEntry<'a>> = Stack::new();
            for _ in 0..config.exchange_count {
                volume_nodes.push(OrderBookEntry {
                    volume: 0.0,
                    location: 0,
                    _phantom: PhantomData,
                });
            }
            bids.insert(OrderedFloat(level), volume_nodes);
        }
        let (ring_buffer, depth_producers) = RingBuffer::new(config.ring_buffer);
        let orderbook: OrderBook<'a> = OrderBook {
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
            if let Ok(_) = self.update_book(depth_update) {
                self.quote();
            }
        }
    }
    fn update_book(&mut self, depth_update: DepthUpdate) -> Result<(), ErrorHotPath> {
        match depth_update.k {
            0 => {
                // we want the highest value on the bid side -- this value is closer to the spread
                if depth_update.p > self.best_deal_asks_level {
                    self.best_deal_asks_level = depth_update.p
                }
                let mut current_seek: usize = 0;
                let max_seek = self.exchange_count as usize;
                let level_list = self.asks.get_mut(&OrderedFloat(depth_update.p)).unwrap();
                let mut current_volume = 0.0;
                let mut current_location = 0;
                let mut to_insert_location = 0;
                let mut to_insert_volume = depth_update.q;
                while current_seek < max_seek + 1 {
                    match level_list.peek_mut().unwrap().volume > to_insert_volume {
                        // our incoming depth update is greater then the first volume
                        // node update it then update the rest of the list
                        true => {
                            current_volume = level_list.peek().unwrap().volume;
                            current_location = level_list.peek_mut().unwrap().location;
                            if to_insert_volume > current_volume {
                                // to insert is greater then current depth. insert
                                // then record then update the to insert depth
                                level_list.peek_mut().unwrap().volume = to_insert_volume;
                                level_list.peek_mut().unwrap().location = to_insert_location;
                                to_insert_volume = current_volume;
                                to_insert_location = current_location;
                                current_seek = current_seek + 1;
                            } else {
                                // current insert is not greater then the current
                                // depth keep pop to the next node in the linked
                                // list
                                _ = level_list.pop().unwrap();
                                current_seek = current_seek + 1;
                            }
                        }
                        // our incoming depth update is greater then the first. update
                        // the node. record the the previous volume then go to the next node for
                        // sorting
                        false => {
                            to_insert_volume = level_list.peek_mut().unwrap().volume;
                            to_insert_location = level_list.peek_mut().unwrap().location;
                            level_list.peek_mut().unwrap().volume = depth_update.q;
                            _ = level_list.pop().unwrap();
                            current_seek = current_seek + 1;
                        }
                    }
                }
                Ok(())
            }
            1 => {
                // we want the lowest value on the asks side -- this value is closer to the spread
                if depth_update.p < self.best_deal_bids_level {
                    self.best_deal_asks_level = depth_update.p
                }
                let mut current_seek: usize = 0;
                let max_seek = self.exchange_count as usize;
                let level_list = self.bids.get_mut(&OrderedFloat(depth_update.p)).unwrap();
                let mut current_volume = 0.0;
                let mut current_location = 0;
                let mut to_insert_location = 0;
                let mut to_insert_volume = depth_update.q;
                while current_seek < max_seek + 1 {
                    match level_list.peek_mut().unwrap().volume > to_insert_volume {
                        // our incoming depth update is greater then the first volume
                        // node update it then update the rest of the list
                        true => {
                            current_volume = level_list.peek().unwrap().volume;
                            current_location = level_list.peek_mut().unwrap().location;
                            if to_insert_volume > current_volume {
                                // to insert is greater then current depth. insert
                                // then record then update the to insert depth
                                level_list.peek_mut().unwrap().volume = to_insert_volume;
                                level_list.peek_mut().unwrap().location = to_insert_location;
                                to_insert_volume = current_volume;
                                to_insert_location = current_location;
                                current_seek = current_seek + 1;
                            } else {
                                // current insert is not greater then the current
                                // depth keep pop to the next node in the linked
                                // list
                                _ = level_list.pop().unwrap();
                                current_seek = current_seek + 1;
                            }
                        }
                        // our incoming depth update is greater then the first. update
                        // the node. record the the previous volume then go to the next node for
                        // sorting
                        false => {
                            to_insert_volume = level_list.peek_mut().unwrap().volume;
                            to_insert_location = level_list.peek_mut().unwrap().location;
                            level_list.peek_mut().unwrap().volume = depth_update.q;
                            _ = level_list.pop().unwrap();
                            current_seek = current_seek + 1;
                        }
                    }
                }
                Ok(())
            }

            _ => Err(ErrorHotPath::OrderBook),
        }
    }
    pub fn get_quotes_bids(&mut self) -> ThinVec<Deal> {
        let mut deals = ThinVec::new();
        let mut current_seek: usize = 0;
        let mut current_level = self.best_deal_asks_level;
        let mut current_deal_index = 0;
        while current_seek < self.exchange_count as usize {
            let node = self
                .bids
                .get(&OrderedFloat(current_level))
                .unwrap()
                .peek()
                .unwrap();
            // check if volume node in list is not dedicated to an exchange if so bump our
            // current level
            if node.location == 0 {
                let _ = self
                    .bids
                    .get_mut(&OrderedFloat(self.best_deal_asks_level))
                    .unwrap()
                    .pop();
                // update to the next level to check for deals. for the bid side we seek down the
                // orderbook
                current_level = current_level - self.level_increment;
                continue;
            }
            deals.insert(
                current_deal_index,
                Deal {
                    volume: node.volume,
                    location: node.location,
                    price: current_level,
                },
            );
            current_deal_index = current_deal_index + 1;
            current_seek = current_seek + 1;
            _ = self
                .bids
                .get_mut(&OrderedFloat(self.best_deal_asks_level))
                .unwrap()
                .pop()
        }
        return deals;
    }
    pub fn get_quotes_asks(&mut self) -> ThinVec<Deal> {
        let mut deals = ThinVec::new();
        let mut current_seek: usize = 0;
        let mut current_level = self.best_deal_asks_level;
        let mut current_deal_index = 0;
        while current_seek < self.exchange_count as usize {
            let node = self
                .bids
                .get(&OrderedFloat(current_level))
                .unwrap()
                .peek()
                .unwrap();
            // check if volume node in list is not dedicated to an exchange if so bump our
            // current level
            if node.location == 0 {
                let _ = self
                    .bids
                    .get_mut(&OrderedFloat(self.best_deal_asks_level))
                    .unwrap()
                    .pop();
                // update to the next level to check for deals. for asks side we seek up the
                // orderbook
                current_level = current_level + self.level_increment;
                continue;
            }
            deals.insert(
                current_deal_index,
                Deal {
                    volume: node.volume,
                    location: node.location,
                    price: current_level,
                },
            );
            current_deal_index = current_deal_index + 1;
            current_seek = current_seek + 1;
            _ = self
                .bids
                .get_mut(&OrderedFloat(self.best_deal_asks_level))
                .unwrap()
                .pop()
        }
        return deals;
    }
    // TODO: get_quotes_$side should be ran in their own thread.
    // TODO: handle channel errors
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
}

pub struct Deal {
    volume: f64,
    price: f64,
    location: u8,
}

pub struct Quotes {
    pub spread: f64,
    pub best_bids: ThinVec<Deal>,
    pub best_asks: ThinVec<Deal>,
}
