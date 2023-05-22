use crossbeam_channel::Sender;
use rustc_hash::FxHashMap;
use tracing::info;

use config::OrderbookConfig;
use market_object::DepthUpdate;
use ring_buffer::RingBuffer;

use ordered_float::OrderedFloat;

use bounded_vec_deque::BoundedVecDeque;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::{hint, thread};

use std::marker::PhantomData;
use typed_arena::Arena;

pub struct Stack<T> {
    head: Link<T>,
}

type Link<T> = Option<Box<Node<T>>>;

struct Node<T> {
    elem: T,
    next: Link<T>,
}

struct VolumeNode<'a> {
    volume: f64,
    location: f64,
    _phantom: PhantomData<&'a ()>,
}

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

pub struct OrderBook<'a> {
    asks: FxHashMap<OrderedFloat<f64>, Stack<VolumeNode<'a>>>,
    bids: FxHashMap<OrderedFloat<f64>, Stack<VolumeNode<'a>>>,
}

impl<'a> OrderBook<'a> {
    pub fn new(exchange_count: usize, depth: usize, current_price: usize) -> OrderBook<'a> {
        let ask_level_range: usize = current_price + depth;
        let mut asks = FxHashMap::<OrderedFloat<f64>, Stack<VolumeNode<'a>>>::default();
        for i in current_price..ask_level_range {
            let level: f64 = i as f64;
            let mut volume_nodes: Stack<VolumeNode<'a>> = Stack::new();
            for _ in 0..exchange_count {
                volume_nodes.push(VolumeNode {
                    volume: 0.0,
                    location: 0.0,
                    _phantom: PhantomData,
                });
            }
            asks.insert(OrderedFloat(level), volume_nodes);
        }
        let bid_level_range: usize = current_price - depth;
        let mut bids = FxHashMap::<OrderedFloat<f64>, Stack<VolumeNode<'a>>>::default();
        for j in bid_level_range..current_price {
            let level: f64 = j as f64;
            let mut volume_nodes: Stack<VolumeNode<'a>> = Stack::new();
            for _ in 0..exchange_count {
                volume_nodes.push(VolumeNode {
                    volume: 0.0,
                    location: 0.0,
                    _phantom: PhantomData,
                });
            }
            bids.insert(OrderedFloat(level), volume_nodes);
        }
        let orderbook_allocation: Arena<OrderBook> = Arena::<OrderBook>::new();
        let orderbook: OrderBook<'a> = Box::new(OrderBook {
            asks: asks,
            bids: bids,
        });
        return orderbook;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_default_order_book() {
        let order_book: OrderBook = Default::default();
    }
}
