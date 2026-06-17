use rust_decimal::{prelude::FromPrimitive, Decimal};

const DEFAULT_DECIMAL_PRECISION: u32 = 100;
const DEFAULT_EXCHANGE_COUNT: usize = 10;

const LIQUID: u8 = 0b0000_10000;
const UNLIQUID: u8 = 0b1111_0111;

#[derive(Copy, Clone)]
pub struct LiquidityNode{
    q: Decimal,
    l: u8,
}

macro_rules! new_liquidity_level {
    ($exchanges:expr) => {
        #[derive(Clone)]
        pub struct Level<LiquidityNode> {
            liquid: u8,
            price: Decimal,
            nodes: [LiquidityNode; $exchanges],
        }

        impl Level<LiquidityNode> {
            pub fn new(price_level: Decimal) -> Self {
                let mut level = Level {
                        liquid: LIQUID,
                        price: price_level,
                        nodes: [LiquidityNode {
                        q: Decimal::new(0, DEFAULT_DECIMAL_PRECISION),
                        l: 0,
                    }; $exchanges],
                };

                for i in 0..$exchanges {
                    level.nodes[i] = LiquidityNode {
                        q: Decimal::new(0, DEFAULT_DECIMAL_PRECISION),
                        l: i as u8,
                    }
                }

                level
            }
            #[inline]
            pub fn associative(&mut self, num: Decimal, exchange: u8) {
                self.nodes[exchange as usize] += num;
            }
            #[inline]
            pub fn set(&mut self) {
                self.liquid |= LIQUID
            }
            #[inline]
            pub fn unset(&mut self) {
                self.liquid &= UNLIQUID
            }
            #[inline]
            pub fn liquid(&self) -> bool {
                self.liquid == LIQUID
            }
        }
    };
}

new_liquidity_level!(DEFAULT_EXCHANGE_COUNT);
