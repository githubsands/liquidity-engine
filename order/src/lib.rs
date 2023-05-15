use serde::Deserialize;

/*
#[derive(Debug, Deserialize, Copy, Clone)]
pub enum Type {
    A,
    B,
}
*/

#[derive(Debug, Deserialize, Copy, Clone)]
pub struct Order {
    pub size: f64,
    pub price: f64,
    pub kind: u8,
}
