use crossbeam_channel::{bounded, Receiver, Sender};
use std::collections::VecDeque;

use config::RingBufferConfig;
use order::Order;

pub struct RingBuffer {
    size: usize,
    buffer: VecDeque<Order>,
    order_consumer: Receiver<Order>,
}

impl RingBuffer {
    pub fn new(ring_buffer_config: RingBufferConfig) -> (Self, Sender<Order>) {
        let (producer, consumer) = bounded::<Order>(ring_buffer_config.channel_buffer_size);
        let rb = RingBuffer {
            size: ring_buffer_config.ring_buffer_size,
            buffer: VecDeque::with_capacity(ring_buffer_config.ring_buffer_size),
            order_consumer: consumer,
        };
        (rb, producer)
    }
    pub fn consume(&mut self) {
        while let Ok(order) = self.order_consumer.recv() {
            self.buffer.push_back(order)
        }
    }
    pub fn pop_order(&mut self) -> Option<Order> {
        if let Some(order) = self.buffer.pop_front() {
            return Some(order);
        }
        None
    }
    pub fn push_back(&mut self, item: Order) {
        if self.buffer.len() == self.size {
            // get rid of the oldest item in the ring buffer before
            // pushing an item in the back
            self.buffer.pop_front();
        }
        self.buffer.push_back(item);
    }
    // possibly remove this function
    pub fn push_front(&mut self, item: Order) {
        if self.buffer.len() == self.size {
            self.buffer.pop_back();
        }
        self.buffer.push_front(item);
    }
    fn pop_front(&mut self) -> Option<Order> {
        self.buffer.pop_front()
    }
    // possibly remove this function
    pub fn pop_back(&mut self) -> Option<Order> {
        self.buffer.pop_back()
    }
}
