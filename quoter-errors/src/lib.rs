use std::fmt;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ErrorHotPath {
    ExchangeWSError(String),
    ExchangeWSReconnectError(String),
    Serialization(String),
    QuoterGRPCError(String),
    QuoteServerSinkError(String),
    ExchangeStreamSnapshot(String),
    ReceivedNonTextMessageFromExchange,
    OrderBook(String),
    OrderBookNegativeLiquidity,
    OrderBookMaxTraversedReached,
    OrderBookDealSendFail,
}

impl fmt::Display for ErrorHotPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorHotPath::ExchangeWSError(s) => write!(f, "Exchange WS Error: {}", s),
            ErrorHotPath::ExchangeWSReconnectError(s) => {
                write!(f, "Exchange WS Reconnect Error: {}", s)
            }
            ErrorHotPath::QuoterGRPCError(s) => write!(f, "Quoter GRPC Error: {}", s),
            ErrorHotPath::QuoteServerSinkError(s) => write!(f, "Quote Server Sink Error: {}", s),
            ErrorHotPath::Serialization(s) => write!(f, "Serialization error: {}", s),
            ErrorHotPath::ReceivedNonTextMessageFromExchange => {
                write!(f, "ReceivedNonTextMessage")
            }
            ErrorHotPath::OrderBook(s) => {
                write!(f, "OrderBookError: {}", s)
            }
            ErrorHotPath::ExchangeStreamSnapshot(s) => {
                write!(f, "HTTPSnapshot error: {}", s)
            }
            ErrorHotPath::OrderBookNegativeLiquidity => {
                write!(f, "NegativeQuantity")
            }
            ErrorHotPath::OrderBookMaxTraversedReached => {
                write!(f, "MaxTraversedReached")
            }
            ErrorHotPath::OrderBookDealSendFail => {
                write!(f, "DealSendFail")
            }
        }
    }
}

// TODO: Update these errors to take in the upstream error
#[derive(Error, Debug)]
pub enum ErrorInitialState {
    ExchangeWSError(tokio_tungstenite::tungstenite::Error),
    MessageSerialization(String),
    WSConnection(String),
    Snapshot(String),
    ExchangeController,
}

impl fmt::Display for ErrorInitialState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorInitialState::ExchangeWSError(e) => {
                write!(f, "Exchange WS Error: {}", e)
            }
            ErrorInitialState::MessageSerialization(s) => {
                write!(f, "Message Serialization Error: {}", s)
            }
            ErrorInitialState::WSConnection(s) => write!(f, "WS Connection Error: {}", s),
            ErrorInitialState::ExchangeController => write!(f, "Exchange Controller Error"),
            ErrorInitialState::Snapshot(s) => write!(f, "Exchange Snapshot error: {}", s),
        }
    }
}
