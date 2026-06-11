use quoter_errors::ErrorInitialState;
use thiserror::Error;

impl From<ExchangeStreamError> for ErrorInitialState {
    fn from(err: ExchangeStreamError) -> Self {
        match err {
            ExchangeStreamError::ExchangeController => ErrorInitialState::ExchangeController,
            ExchangeStreamError::WSConnection(msg) => ErrorInitialState::WSConnection(msg),
            ExchangeStreamError::Snapshot(msg)
            | ExchangeStreamError::ExchangeNoSnapshot(msg)
            | ExchangeStreamError::ExchangeStreamSnapshot(msg) => ErrorInitialState::Snapshot(msg),
            ExchangeStreamError::Deserialization(err) => {
                ErrorInitialState::MessageSerialization(err.to_string())
            }
            other => ErrorInitialState::WSConnection(other.to_string()),
        }
    }
}

#[derive(Error, Debug)]
pub enum ExchangeStreamError {
    #[error("Exchange No Snapshot: {0}")]
    ExchangeNoSnapshot(String),
    #[error("Exchange WS Error: {0}")]
    ExchangeWSError(String),
    #[error("Exchange WS Reconnect Error: {0}")]
    ExchangeWSReconnectError(String),
    #[error("HTTP Snapshot error: {0}")]
    ExchangeStreamSnapshot(String),
    #[error("Received non-text message from exchange")]
    ReceivedNonTextMessageFromExchange,
    #[error("WS Connection Error: {0}")]
    WSConnection(String),
    #[error("Exchange Snapshot error: {0}")]
    Snapshot(String),
    #[error("Exchange Controller Error")]
    ExchangeController,
    #[error("HTTP request error: {0}")]
    HttpRequest(#[from] reqwest::Error),
    #[error("Snapshot deserialization error: {0}")]
    Deserialization(#[from] serde_json::Error),
}
