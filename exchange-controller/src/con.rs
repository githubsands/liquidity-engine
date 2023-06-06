use config::ExchangeConfig;
use exchange_ws::{ExchangeWS, WSStreamState};

use tracing::{debug, error, info, warn};

use order::Order;
use quoter_errors::ErrorInitialState;

use crossbeam_channel::Sender;

use futures::future::IntoFuture;
use futures::{Future, StreamExt};

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::stream::TryStreamExt;

pub struct ExchangeController {
    exchanges: Vec<Box<ExchangeWS>>,
}

impl ExchangeController {
    pub fn new(
        exchange_configs: &Vec<ExchangeConfig>,
        order_ask_producer: Sender<Order>,
        order_bid_producer: Sender<Order>,
    ) -> Result<Box<ExchangeController>, ErrorInitialState> {
        let mut exchanges: Vec<Box<ExchangeWS>> = Vec::new();
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
        Ok(Box::new(ExchangeController {
            exchanges: exchanges,
        }))
    }
    pub fn exchanges(&self) -> usize {
        return self.exchanges.len();
    }
    pub async fn boot_exchanges(&mut self) -> Result<(), ErrorInitialState> {
        info!("booting exchanges");
        for exchange in &mut self.exchanges {
            info!("booting exchanges {}", exchange.uri);
            match exchange.start().await {
                Ok(_) => continue,
                Err(exchange_start_error) => {
                    error!("could not connect to exchange {}", exchange_start_error);
                    return Err(exchange_start_error);
                }
            }
        }
        Ok(())
    }
    pub fn exchange(&mut self) -> Option<Box<ExchangeWS>> {
        return self.exchanges.pop();
    }
}
