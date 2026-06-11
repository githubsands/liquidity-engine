use thiserror::Error;

#[derive(Error, Debug)]
pub enum ErrorHotPath {
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Quoter GRPC Error: {0}")]
    QuoterGRPCError(String),
    #[error("Quote Server Sink Error: {0}")]
    QuoteServerSinkError(String),
    #[error("Exchange Stream Snapshot Error: {0}")]
    ExchangeStreamSnapshot(String),
    #[error("Deal send failed")]
    OrderBookDealSendFail,
}

#[derive(Error, Debug)]
pub enum ErrorInitialState {
    #[error("Exchange WS Error: {0}")]
    ExchangeWSError(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("Message Serialization Error: {0}")]
    MessageSerialization(String),
    #[error("WS Connection Error: {0}")]
    WSConnection(String),
    #[error("Exchange Snapshot error: {0}")]
    Snapshot(String),
    #[error("Exchange Controller Error")]
    ExchangeController,
}

#[derive(Error, Debug)]
pub enum ErrorWithSource {
    #[error("Network error")]
    Network(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("Serialization failed")]
    Serialization(#[from] serde_json::Error),
    #[error("IO error: {message}")]
    Io {
        #[source]
        source: std::io::Error,
        message: String,
    },
    #[error("Custom error with context: {context}")]
    Custom {
        context: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}
