use rust_decimal::Decimal;
use rustc_hash::FxHashMap;
use core::{cmp::Ordering, ops::Sub};

use crate::level::{Level, LiquidityNode};

use market_objects::DepthUpdate;

enum Side {
    Bid,
    Ask,
}

pub struct Pointer {
    tick: Decimal,
    side: Side,
    level: Decimal,
}

pub enum PointerError {
    TraverseFailedInvalidLevel
}

impl Pointer {
    fn new(tick: Decimal, side: Side) -> Self {
        Pointer {
            tick,
            side,
            level: Decimal::ZERO,
        }
    }
    pub fn level(&self) -> &Decimal {
        return &self.level
    }
    pub fn traverse(&mut self, book: &FxHashMap<Decimal, Level<LiquidityNode>>) -> Result<(), PointerError> {
        let mut i = &self.level;
        match self.side {
            Side::Bid => {
                loop {
                    if let Some(level) = book.get(i) {
                        if level.liquid() {
                            self.level = *i;
                            return Ok(())
                        } else {
                            i.saturating_sub(self.tick);
                        }
                    } else {
                        return Err(PointerError::TraverseFailedInvalidLevel)
                    }
                }
            },
            Side::Ask => {
                loop {
                    if let Some(level) = book.get(i) {
                        if level.liquid() {
                            self.level = *i;
                            return Ok(())
                        } else {
                            i.saturating_add(self.tick);
                        }
                    } else {
                        return Err(PointerError::TraverseFailedInvalidLevel)
                    }
                }
            }
        }
    }
}
