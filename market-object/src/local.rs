use serde::{Deserialize, Serialize};
use serde_json::from_str;
use std::convert::TryFrom;
use tungstenite::Message;

// TODO: Rename this OrderBookUpdate
#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
// Order defines an order object:
// k: 0 ask, 1 bid
// p: price or level
// q: quantity
// l: location: 0 bybit, 1 binance
pub struct DepthUpdate {
    pub k: u8,
    pub p: f64,
    pub q: f64,
    pub l: u8,
}

impl TryFrom<Message> for DepthUpdate {
    type Error = std::io::Error;

    fn try_from(msg: Message) -> Result<Self, Self::Error> {
        match msg {
            Message::Text(text) => {
                let depth_update: DepthUpdate = from_str(&text)?;
                Ok(depth_update)
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "not a text message given when appyling DepthUpdate try_from",
            )),
        }
    }
}
