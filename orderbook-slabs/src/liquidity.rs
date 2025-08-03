use fixed::{FixedU32, types::extra::U0};

#[derive(Copy, Clone, Debug)]
pub struct LiquidityNode {
    q: FixedU32<U0>,
    l: u8,
}

// Runs a macro to create our level with N size - N being the
// exchange number this service is mirroring
//
/*
impl Level<LiquidityNode> {
    fn new(price_level: f64) -> Self {
        let mut level = Level {
            price: price_level,
            deque: [LiquidityNode {
                        q: FixedU32::MIN,
                        l: 0,
                }; N]
        };
        for i in 0..N {
            level.deque[i] = LiquidityNode {
                q: FixedU32::from_num(0),
                l: i as u8 + 1,
            }
        }
        level
    }
}
*/

macro_rules! level_exchange_depth {
    ($exchanges:expr) => {
        #[derive(Clone)]
        pub struct Level<LiquidityNode> {
            price: FixedU32<U0>,
            deque: [LiquidityNode; $exchanges],
        }

        impl Level<LiquidityNode> {
            pub fn new(price_level: FixedU32<U0>) -> Self {
                let mut level = Level {
                    price: price_level,
                    deque: [LiquidityNode {
                        q: FixedU32::MIN,
                        l: 0,
                    }; $exchanges],
                };
                for i in 0..$exchanges {
                    level.deque[i] = LiquidityNode {
                        q: FixedU32::MIN,
                        l: i as u8 + 1,
                    }
                }
                level
            }
        }
    };
}

// todo: this parameter will be configurable in the build script
//   at the root of this project - its the total number of exchanges
//   this service is mirroring
level_exchange_depth!(10);
