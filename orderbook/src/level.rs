use rust_decimal::{prelude::FromPrimitive, Decimal};

const DEFAULT_DECIMAL_PRECISION: u32 = 100;
const DEFAULT_LIQUIDITY_LEVEL_EXCHANGES: usize = 10;

#[derive(Copy, Clone)]
pub struct LiquidityNode{
    pub q: Decimal,
    pub l: u8,
}

macro_rules! new_liquidity_level {
    ($exchanges:expr) => {
        #[derive(Clone)]
        pub struct Level<LiquidityNode> {
            pub price: Decimal,
            pub deque: [LiquidityNode; $exchanges],
        }

        impl Level<LiquidityNode> {
            pub fn new(price_level: Decimal) -> Self {
                let mut level = Level {
                        price: price_level,
                        deque: [LiquidityNode {
                        q: Decimal::new(0, DEFAULT_DECIMAL_PRECISION),
                        l: 0,
                    }; $exchanges],
                };

                for i in 0..$exchanges {
                    level.deque[i] = LiquidityNode {
                        q: Decimal::new(0, DEFAULT_DECIMAL_PRECISION),
                        l: i as u8,
                    }
                }

                level
            }
        }
    };
}

new_liquidity_level!(DEFAULT_LIQUIDITY_LEVEL_EXCHANGES);
