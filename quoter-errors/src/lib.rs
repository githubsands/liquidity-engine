use std::fmt;

pub enum ErrorHotPath {
    ExchangeWSError(String),
    ExchangeWSReconnectError(String),
    QuoterGRPCError(String),
    QuoteServerSinkError(String),
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
        }
    }
}

#[derive(Debug)]
pub enum ErrorInitialState {
    ExchangeWSError(tokio_tungstenite::tungstenite::Error),
    MessageSerialization(String),
    WSConnection(String),
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
        }
    }
}
