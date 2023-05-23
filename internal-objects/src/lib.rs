use thin_vec::ThinVec;

pub struct Deal {
    pub volume: f64,
    pub price: f64,
    pub location: u8,
}

pub struct Quotes {
    pub spread: f64,
    pub best_bids: ThinVec<Deal>,
    pub best_asks: ThinVec<Deal>,
}
