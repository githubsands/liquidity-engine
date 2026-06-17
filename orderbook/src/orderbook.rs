
use core::cmp::Ordering;
use core::mem::uninitialized;

use rust_decimal::prelude::Signed;
use rust_decimal::{prelude::FromPrimitive, Decimal};
use rustc_hash::FxHashMap;
use arrayvec::ArrayVec;

mod level;
mod pointer;
mod errors;

use crate::errors::OrderBookError;
use crate::level::{Level, LiquidityNode};
use crate::pointer::Pointer;

use config::OrderbookConfig;
use market_objects::DepthUpdate;

const EXCHANGES: usize = 5;

pub struct OrderBook {
    asks: FxHashMap<Decimal, Level<LiquidityNode>>,
    bids: FxHashMap<Decimal, Level<LiquidityNode>>,
    tick_size: f64,
}

impl OrderBook {
    pub fn new(config: &OrderbookConfig) -> Self {
        let (asks, bids) = OrderBook::build_orderbook(
            Decimal::from_f64(config.tick_size).unwrap(),
            Decimal::from_f64(config.mid_price).unwrap(),
            config.depth as u64,
        )
            OrderBook {
                asks: asks,
                bids: bids,

                tick_size: config.tick_size,
            }
    }
    fn build_orderbook(
        tick_size: Decimal,
        mid_level: Decimal,
        depth: u64,
    ) -> (
        FxHashMap<Decimal, Level<LiquidityNode>>,
        FxHashMap<Decimal, Level<LiquidityNode>>,
    ) {
        let mut asks: FxHashMap<Decimal, Level<LiquidityNode>> = FxHashMap::default();
        let mut current_level = mid_level;
        let mut max_ask_level = Decimal::ZERO;
        for i in 0..=depth as u64 {
            if i == 0 {
                let level = Level::new(current_level);
                asks.insert(current_level, level);
            } else {
                current_level = current_level + tick_size;
                let level = Level::new(current_level);
                asks.insert(current_level, level);
                if i == depth as u64 {
                    max_ask_level = current_level
                }
            }
        }

        // build the opposite side of the mid level for asks
        current_level = mid_level;
        let mut min_ask_level = Decimal::ZERO;
        for i in 0..=depth as u64 {
            if i == 0 {
                continue;
            } else {
                current_level = current_level - tick_size;
                let level = Level::new(current_level);
                asks.insert(current_level, level);
                if i == depth as u64 {
                    min_ask_level = current_level;
                }
            }
        }
        // building bid side
        let mut bids: FxHashMap<Decimal, Level<LiquidityNode>> = FxHashMap::default();
        current_level = mid_level;
        let mut min_bid_level = Decimal::ZERO;
        for i in 0..=depth as u64 {
            if i == 0 {
                let level = Level::new(current_level);
                bids.insert(current_level, level);
            } else {
                current_level = current_level - tick_size;
                let level = Level::new(current_level);
                bids.insert(current_level, level);
                if i == depth as u64 {
                    min_bid_level = current_level;
                }
            }
        }

        // build the opposite side of the mid level for bids
        current_level = mid_level;
        let mut max_bid_level = Decimal::ZERO;
        for i in 0..=depth as u64 {
            if i == 0 {
                continue;
            } else {
                current_level = current_level + tick_size;
                let level = Level::new(current_level);
                bids.insert(current_level, level);
                if i == depth as u64 {
                    max_bid_level = current_level
                }
            }
        }

        return (
            asks,
            bids,
        );
    }

    /*
    #[inline]
    // TODO: This wasn't made to be real time or performant - its just for TDD for now
    pub fn local_snapshot(
        &mut self,
        mid_point: f64,
        depth: f64,
    ) -> Result<(Vec<DepthUpdate>, Vec<DepthUpdate>), OrderBookError> {
        let mut asks: Vec<DepthUpdate> = vec![];
        let mut bids: Vec<DepthUpdate> = vec![];
        let mut depth_counter: f64 = depth;
        let mut current_level = mid_point;
        while depth_counter != 0.0 {
            if let Some(ask_level) = self.asks.get_mut(&OrderedFloat(current_level)) {
                let mut vec: Vec<DepthUpdate> = ask_level
                    .deque
                    .as_mut()
                    .iter_mut()
                    .filter(|liquidity_node| liquidity_node.q != 0.0)
                    .map(|liquidity_node| DepthUpdate {
                        k: 0,
                        p: current_level,
                        q: liquidity_node.q,
                        l: liquidity_node.l,
                        s: true,
                    })
                    .collect();
                asks.append(&mut vec);
            }
            current_level = current_level + self.tick_size;
            depth_counter -= 1.0;
        }
        current_level = mid_point;
        depth_counter = depth;
        while depth_counter != 0.0 {
            if let Some(bid_level) = self.bids.get_mut(&OrderedFloat(current_level)) {
                let mut vec: Vec<DepthUpdate> = bid_level
                    .deque
                    .as_mut()
                    .iter_mut()
                    .filter(|liquidity_node| liquidity_node.q != 0.0)
                    .map(|liquidity_node| DepthUpdate {
                        k: 1,
                        p: current_level,
                        q: liquidity_node.q,
                        l: liquidity_node.l,
                        s: true,
                    })
                    .collect();
                bids.append(&mut vec);
            }
            current_level = current_level - self.tick_size;
            depth_counter -= 1.0;
        }
        Ok((asks, bids))
    }
    */

    #[inline]
    fn ask_update(&mut self, depth_update: DepthUpdate, ptr: &mut Pointer) -> Result<(), OrderBookError> {
        if let Some(asks) = self
            .asks
            .get_mut(&depth_update.p)
        {
            asks.nodes
                .iter_mut()
                .find(|liquidity_node| liquidity_node.l == depth_update.l)
                .map(|liquidity_node| {
                    let liquidity = liquidity_node.q + depth_update.p;
                    match liquidity.cmp(ptr.level()) {
                        Ordering::Less => {
                            return Err(OrderBookError::NegativeLiquidity);
                        },
                        Ordering::Equal if zero_mid => {
                            liquidity_node.q = liquidity;
                        },
                        Ordering::Greater | Ordering::Equal => {
                            liquidity_node.q = liquidity;
                        },
                    }
                    Ok(())
                });
        } else {
            return Err(OrderBookError::NoLevel);
        }
        Ok(())
    }

    #[inline]
    fn bid_update(&mut self, depth_update: DepthUpdate, ptr: &mut Pointer) -> Result<(), OrderBookError> {
        if let Some(bids) = self
            .bids
            .get_mut(&depth_update.p)
        {
        bids.nodes
        .iter_mut()
        .find(|liquidity_node| liquidity_node.l == depth_update.l)
        .map(|liquidity_node| {
            let liquidity = liquidity_node.q + depth_update.p;
            match liquidity.cmp(ptr.level()) {
                Ordering::Less => {
                    return Err(OrderBookError::NegativeLiquidity)
                },
                Ordering::Equal if zero_mid => {
                    liquidity_node.q = liquidity;
                    // self.bids_mid.take();
                },
                Ordering::Greater | Ordering::Equal => {
                    liquidity_node.q = liquidity;
                },
            }
            Ok(())
        });
        } else {
            return Err(OrderBookError::NoLevel);
        }
        Ok(())
    }
}
