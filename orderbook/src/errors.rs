use thiserror::Error;

#[derive(Error, Debug)]
pub enum OrderBookError {
    #[error("OrderBook no Level found")]
    NoLevel,
    #[error("Negative liquidity detected")]
    NegativeLiquidity,
    #[error("Maximum traversal limit reached")]
    MaxTraversedReached,
}
