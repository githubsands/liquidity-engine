use crossbeam_channel::{bounded, Receiver, RecvError, Sender, TryRecvError};
use std::collections::VecDeque;

use config::RingBufferConfig;
use market_objects::DepthUpdate;
use std::error::Error;
use std::fmt;
use tracing::{debug, error, info, warn};

pub enum BufferError {
    Failed(TryRecvError),
}

impl fmt::Display for BufferError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BufferError::Failed(err) => write!(f, "BufferError::Failed: {}", err),
        }
    }
}

impl fmt::Debug for BufferError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BufferError::Failed(err) => write!(f, "BufferError::Failed({:?})", err),
        }
    }
}

impl Error for BufferError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            BufferError::Failed(err) => Some(err),
        }
    }
}

#[derive(Clone)]
pub struct RingBuffer {
    size: usize,
    buffer: VecDeque<DepthUpdate>,
    depth_consumer: Receiver<DepthUpdate>,
}

impl RingBuffer {
    pub fn new(ring_buffer_config: &RingBufferConfig) -> (Self, Sender<DepthUpdate>) {
        let (producer, consumer) = bounded::<DepthUpdate>(ring_buffer_config.channel_buffer_size);
        let rb = RingBuffer {
            size: ring_buffer_config.ring_buffer_size,
            buffer: VecDeque::with_capacity(ring_buffer_config.ring_buffer_size),
            depth_consumer: consumer,
        };
        (rb, producer)
    }
    pub fn consume(&mut self) -> Result<(), BufferError> {
        let result = self.depth_consumer.try_recv();
        match result {
            Ok(depth_update) => {
                self.push_back(depth_update);
                Ok(())
            }
            Err(e) => match e {
                TryRecvError::Empty => Ok(()),
                TryRecvError::Disconnected => {
                    return Err(BufferError::Failed(TryRecvError::Disconnected));
                }
            },
        }
    }

    pub fn pop_depth(&mut self) -> Option<DepthUpdate> {
        if let Some(order) = self.buffer.pop_front() {
            info!("pop_depth in rb");
            return Some(order);
        } else {
            return None;
        }
    }
    pub fn push_back(&mut self, item: DepthUpdate) {
        if self.buffer.len() == self.size {
            self.buffer.pop_front();
        }
        self.buffer.push_back(item);
    }

    pub fn push_front(&mut self, item: DepthUpdate) {
        if self.buffer.len() == self.size {
            self.buffer.pop_back();
        }
        self.buffer.push_front(item);
    }
}
