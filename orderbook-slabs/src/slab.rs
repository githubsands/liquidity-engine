use crate::liquidity::{Level, LiquidityNode};
use fixed::{FixedU32, types::extra::U0};
use rustc_hash::FxHashMap;

struct Side {
    internal: FxHashMap<FixedU32<U0>, Level<LiquidityNode>>,
}

impl Side {
    fn new(price_depth: FixedU32<U0>) -> Side {
        let internal: FxHashMap<FixedU32<U0>, Level<LiquidityNode>> = FxHashMap::default();
        Side {
            internal: FxHashMap::default(),
        }
    }
}
