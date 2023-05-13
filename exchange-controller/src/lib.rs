use config::ExchangeConfig;
use exchange_ws::ExchangeWS;

use order::Order;
use quoter_errors::ErrorInitialState;

use crossbeam_channel::Sender;

use tungstenite::Error as WsError;

pub struct ExchangeController {
    exchanges: Vec<ExchangeWS>,
}

impl ExchangeController {
    pub fn new(
        exchange_configs: &Vec<ExchangeConfig>,
        order_ask_producer: Sender<Order>,
        order_bid_producer: Sender<Order>,
    ) -> Result<ExchangeController, ErrorInitialState> {
        let mut exchanges: Vec<ExchangeWS> = Vec::new();
        for exchange_config in exchange_configs {
            let exchange_creation_result = ExchangeWS::new(
                exchange_config,
                order_ask_producer.clone(),
                order_bid_producer.clone(),
            );
            match exchange_creation_result {
                Ok(exchange) => {
                    exchanges.push(exchange);
                }
                Err(_) => return Err(ErrorInitialState::ExchangeController),
            }
        }
        Ok(ExchangeController {
            exchanges: exchanges,
        })
    }
    /*
    pub async fn boot_exchanges(&self) -> Result<(), ErrorInitialState> {
        let futures: Vec<_> = (0..10)
            .map(|id| tokio::spawn(task(id, id % 2 == 0)))
            .collect();

        let errors: Vec<_> = stream::iter(futures)
            .filter_map(|future| async move {
                match future.await {
                    Ok(Ok(_)) => None,
                    Ok(Err(e)) => Some(e),
                    Err(e) => {
                        eprintln!("Task panicked: {:?}", e);
                        None
                    }
                }
            })
            .collect::<Vec<_>>()
            .await;
    }
    */
}

/*
pub async fn exchange_handler(&self, exchange: &Exchange) {
    while let Some(exhange_streaming_state) = exchange.next().await {
        match item {
            WSStreamState::Success => {
                continue
            }
            WSStreamState::WSError => {
                exchange.reconnect().await
            }
            WSStreamState::Success => {
                continue
            }
            StreamData::FailedStream => {
                // TODO:
                println!("Got TypeC");
            }
        }
    }
}
*/
/*
fn handle_ws_errors(&self, ws_error: WSError) {
    match message {
        Ok(msg) => {
            // Handle the message here
            println!("Received: {}", msg);
        },
        Err(e) => {
            match e {
                WsError::Io(err) => {
                    // Handle I/O error
                    eprintln!("I/O error: {}", err);
                },
                WsError::UrlParse(err) => {
                    // Handle URL parsing error
                    eprintln!("URL parse error: {}", err);
                },
                WsError::Capacity(err) => {
                    // Handle capacity error
                    eprintln!("Capacity error: {}", err);
                },
                WsError::Protocol(err) => {
                    // Handle protocol error
                    eprintln!("Protocol error: {}", err);
                },
                WsError::Utf8 => {
                    // Handle UTF-8 conversion error
                    eprintln!("UTF-8 conversion error");
                },
                WsError::Tls(err) => {
                    // Handle TLS error
                    eprintln!("TLS error: {}", err);
                },
                WsError::AlreadyClosed => {
                    // Handle "Already Closed" error
                    eprintln!("WebSocket connection is already closed");
                },
                WsError::ConnectionClosed => {
                    // Handle "Connection Closed" error
                    eprintln!("WebSocket connection is closed");
                },
                WsError::SendQueueFull(msg) => {
                    // Handle "Send Queue Full" error
                    eprintln!("WebSocket send queue is full, message could not be sent: {}", msg);
                },
                WsError::InvalidInput => {
                    // Handle "Invalid Input" error
                    eprintln!("Invalid input");
                }
            }
            break;
        }
    }
}
*/

/*
pub async fn boot_exchanges(&self) {
    let mut exchange_start_futures = Vec::new();
    for exchange_config in exchanges_configs {
        exchange_start_futures.push(exchanges.get[index].start_orderbook_connection());
        index += 1
    }
    join_all(exchange_start_futures).await;
}
pub async fn receive_orderbooks(&self) {
    let mut exchange_orderbook_futures = Vec::new();
    for exchange_config in exchanges_configs {
        exchange_orderbook_futures.push(exchanges.get[index].start_orderbook_connection());
        index += 1
    }
    join_all(exchange_start_futures).await;
}
*/
