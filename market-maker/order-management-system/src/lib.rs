pub struct OrderEntry {
    asset: String,
    direction: u8,
    order_size: f64,
    price_limits: f64
    partial_fills: f64,
}

pub enum OrderCondition {
    Day,
    GoodTillCancel,
    Immediate,
    FillOrKill,
    GoodTillCrossing,
    GoodTillDate,
    AtTheClose,
    GoodThroughCrossing,
    AtCrossing,
}

pub enum OrderType {
    Market,
    Limit,
    StopLoss,
    StopLimit,
    MarketIfTouched,
    Pegged
}
