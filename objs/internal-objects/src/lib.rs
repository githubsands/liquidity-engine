#[derive(PartialEq, Copy, Clone, Debug)]
pub struct Deal {
    pub p: f64,
    pub q: f64,
    pub l: u8,
}

#[derive(Copy, Clone, Debug)]
pub struct Deals {
    pub asks: [Deal; 10],
    pub bids: [Deal; 10],
}

#[derive(Copy, Clone, Debug)]
pub struct Quote {
    pub spread: f64,
    pub ask_deals: [Deal; 10],
    pub bid_deals: [Deal; 10],
}
