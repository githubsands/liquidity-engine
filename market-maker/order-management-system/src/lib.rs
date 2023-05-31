use std::collections::HashMap;

pub struct Key {
    exchange: String,
    public_address: String,
    private_key: String,
}

pub struct OrderManagementSystem {
    capital_pool: CapitalPool,
    orders: HashMap<u8, Order>,
}

pub struct CapitalPool {
    account_capital: f64,
}

impl CapitalPool {
    fn new(capital: f64) -> Self {
        CapitalPool {
            account_capital: capital,
        }
    }
    fn move_capital(l0: f64, l1: f64, size: f64) {}
    fn allocate_initial_capital(l0: f64, size: f64) {}
}

impl OrderManagementSystem {
    fn new() -> Self {
        OrderManagementSystem {
            capital_pool: CapitalPool {
                account_capital: 3000.0,
            },
            orders: HashMap::new(),
        }
    }
    fn place_orders(&self) {}
    fn cancel_orders(&self) {}
}

pub struct Order {
    sell_or_buy: String,
    asset: String,
    size: f64,
    order_type: OrderType,
    order_id: u8,
}

enum OrderState {
    Unfilled,
    Filled,
}

enum OrderType {
    Limit,
    StopLoss,
    Market,
    Trailing,
}
