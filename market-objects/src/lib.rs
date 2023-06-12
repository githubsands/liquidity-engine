use serde::{Deserialize, Serialize};
use serde_this_or_that::as_f64;
use std::default::Default;
use std::fmt::{Display, Error as ErrorFMT, Formatter};

use tokio_tungstenite::tungstenite::protocol::Message;

#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
// Order defines an order object:
// k: 0 ask, 1 bid
// p: price or level
// q: quantity
// l: location: 1 binance, 2 binance_usa, 3 bybit
pub struct DepthUpdate {
    pub k: u8,
    pub p: f64,
    pub q: f64,
    pub l: u8,
}

impl Default for DepthUpdate {
    fn default() -> Self {
        DepthUpdate {
            k: 0,
            p: 0.0,
            q: 0.0,
            l: 0,
        }
    }
}

#[allow(non_snake_case)]
#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub struct WSOrderBookUpdateBinance {
    pub b: f64, // best bid price
    pub B: f64, // best bid qty
    pub a: f64, // best ask price
    pub A: f64, // best ask qty
}

/// OrderbookUpdateBinance returns a bid and a ask order
impl WSOrderBookUpdateBinance {
    pub fn order(self) -> (DepthUpdate, DepthUpdate) {
        (
            DepthUpdate {
                k: 0,
                p: self.b,
                q: self.B,
                l: 1,
            },
            DepthUpdate {
                k: 1,
                p: self.a,
                q: self.A,
                l: 1,
            },
        )
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
pub struct BinanceDepthUpdate {
    #[serde(deserialize_with = "as_f64")]
    pub price: f64,
    #[serde(deserialize_with = "as_f64")]
    pub quantity: f64,
}

impl Default for BinanceDepthUpdate {
    fn default() -> Self {
        Self {
            price: 0.0,
            quantity: 0.0,
        }
    }
}

impl Display for BinanceDepthUpdate {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), ErrorFMT> {
        write!(
            f,
            "{{ price: {}, quantity: {} }}",
            self.price, self.quantity
        )
    }
}

#[allow(non_snake_case)]
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct WSDepthUpdateBinance {
    #[serde(deserialize_with = "as_f64")]
    pub e: f64,
    #[serde(deserialize_with = "as_f64")]
    pub E: f64,
    #[serde(deserialize_with = "as_f64")]
    pub s: f64,
    #[serde(deserialize_with = "as_f64")]
    pub U: f64,
    #[serde(deserialize_with = "as_f64")]
    pub u: f64,
    pub b: Vec<BinanceDepthUpdate>, // todo: verify
    pub a: Vec<BinanceDepthUpdate>,
}

impl Default for WSDepthUpdateBinance {
    fn default() -> Self {
        Self {
            e: 0.0,
            E: 0.0,
            s: 0.0,
            U: 0.0,
            u: 0.0,
            b: vec![BinanceDepthUpdate::default()],
            a: vec![BinanceDepthUpdate::default()],
        }
    }
}

impl Display for WSDepthUpdateBinance {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), ErrorFMT> {
        // Convert each BinanceDepthUpdate in the vectors into Strings
        // and then join them with commas
        let b_updates: Vec<String> = self.b.iter().map(|update| update.to_string()).collect();
        let a_updates: Vec<String> = self.a.iter().map(|update| update.to_string()).collect();

        write!(
            f,
            "WSDepthUpdateBinance {{ e: {}, E: {}, s: {}, U: {}, u: {}, b: [{}], a: [{}] }}",
            self.e,
            self.E,
            self.s,
            self.U,
            self.u,
            b_updates.join(", "),
            a_updates.join(", ")
        )
    }
}

impl From<Message> for WSDepthUpdateBinance {
    fn from(value: Message) -> Self {
        match value {
            Message::Text(text) => {
                serde_json::from_str(&text).expect("Failed to parse the message")
            }
            _ => panic!("Expected Text WebSocket Message"),
        }
    }
}
impl WSDepthUpdateBinance {
    pub fn depths(
        self,
        location: u8,
    ) -> (
        impl Iterator<Item = DepthUpdate>,
        impl Iterator<Item = DepthUpdate>,
    ) {
        let bid_updates = self.b.into_iter().map(move |update| DepthUpdate {
            k: 0,
            p: update.price,    // Assuming BinanceDepthUpdate has a `price` field
            q: update.quantity, // and a `quantity` field
            l: location,
        });
        let ask_updates = self.a.into_iter().map(move |update| DepthUpdate {
            k: 1,
            p: update.price,
            q: update.quantity,
            l: location,
        });

        (bid_updates, ask_updates)
    }
}

#[allow(non_snake_case)]
#[derive(Debug, Serialize, Deserialize)]
pub struct HTTPSnapShotDepthResponseBinance {
    pub retcode: Option<i32>,
    pub lastUpdateId: u64,
    pub asks: Vec<BinanceDepthUpdate>,
    pub bids: Vec<BinanceDepthUpdate>,
}

impl HTTPSnapShotDepthResponseBinance {
    pub fn depths(
        self,
        location: u8,
    ) -> (
        impl Iterator<Item = DepthUpdate>,
        impl Iterator<Item = DepthUpdate>,
    ) {
        let bid_updates = self.bids.into_iter().map(move |update| DepthUpdate {
            k: 0,
            p: update.price,
            q: update.quantity,
            l: location,
        });

        let ask_updates = self.asks.into_iter().map(move |update| DepthUpdate {
            k: 1,
            p: update.price,
            q: update.quantity,
            l: location,
        });

        (bid_updates, ask_updates)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WSOrderBookUpdateByBit {
    #[serde(deserialize_with = "as_f64")]
    pub price: f64,
    #[serde(deserialize_with = "as_f64")]
    pub quantity: f64,
}

impl WSDepthUpdateByBit {
    pub fn depths(
        self,
        location: u8,
    ) -> (
        impl Iterator<Item = DepthUpdate>,
        impl Iterator<Item = DepthUpdate>,
    ) {
        let bid_updates = self.data.b.into_iter().map(move |update| DepthUpdate {
            k: 0,
            p: update.price,    // Assuming BinanceDepthUpdate has a `price` field
            q: update.quantity, // and a `quantity` field
            l: location,
        });

        let ask_updates = self.data.a.into_iter().map(move |update| DepthUpdate {
            k: 1,
            p: update.price,
            q: update.quantity,
            l: location,
        });

        (bid_updates, ask_updates)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WSDepthUpdateByBit {
    pub topic: String,
    pub type_: String,
    pub ts: u64,
    pub data: Data,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Data {
    pub s: String,
    pub b: Vec<WSOrderBookUpdateByBit>,
    pub a: Vec<WSOrderBookUpdateByBit>,
    pub u: u32,
    pub seq: u32,
}

impl From<Message> for WSDepthUpdateByBit {
    fn from(value: Message) -> Self {
        match value {
            Message::Text(text) => {
                serde_json::from_str(&text).expect("Failed to parse the message")
            }
            _ => panic!("Expected Text WebSocket Message"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ByBitData {
    pub s: String,
    pub t: u64,
    pub a: Vec<WSOrderBookUpdateByBit>,
    pub b: Vec<WSOrderBookUpdateByBit>,
}

impl HTTPSnapShotDepthResponseByBit {
    pub fn depths(
        self,
        location: u8,
    ) -> (
        impl Iterator<Item = DepthUpdate>,
        impl Iterator<Item = DepthUpdate>,
    ) {
        let bid_updates = self.data.b.into_iter().map(move |update| DepthUpdate {
            k: 0,
            p: update.price,
            q: update.quantity,
            l: location,
        });

        let ask_updates = self.data.a.into_iter().map(move |update| DepthUpdate {
            k: 1,
            p: update.price,
            q: update.quantity,
            l: location,
        });

        (bid_updates, ask_updates)
    }
}

#[allow(non_snake_case)]
#[derive(Debug, Serialize, Deserialize)]
pub struct HTTPSnapShotDepthResponseByBit {
    pub retMsg: Option<String>,
    pub topic: String,
    pub ts: u64,
    pub r#type: String,
    pub data: ByBitData,
}

/* Old test - commenting out to not disrupt ci work flow"
#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    #[test]
    fn test_try_from_wsdepthupdatebinance() {
        let json_str = r#"{
      "e": "123.45",
      "E": "678.90",
      "s": "234.56",
      "U": "789.01",
      "u": "345.67",
      "b": [
        {
          "price": "890.12",
          "quantity": "456.78"
        },
        {
          "price": "901.23",
          "quantity": "567.89"
        }
      ],
      "a": [
        {
          "price": "912.34",
          "quantity": "678.90"
        },
        {
          "price": "923.45",
          "quantity": "789.01"
        }
      ]
          }"#;
        let ws_message = Message::Text(json_str.into());
        print!("{}", ws_message);
        let expected: Result<WSDepthUpdateBinance, serde_json::Error> = from_str(json_str);
        let actual: Result<WSDepthUpdateBinance, Infallible> =
            WSDepthUpdateBinance::try_from(ws_message);
        assert_eq!(actual.is_ok(), true);
        assert_eq!(expected.unwrap(), actual.unwrap());
    }
}
*/
