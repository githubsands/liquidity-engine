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

pub struct ExchangeController {
    exchanges: Vec<Box<ExchangeWS>>,
}

impl ExchangeController {
    pub fn new(
        exchange_configs: &Vec<ExchangeConfig>,
        orders_producer: Sender<Order>,
    ) -> Result<Box<ExchangeController>, ErrorInitialState> {
        let mut exchanges: Vec<Box<ExchangeWS>> = Vec::new();
        for exchange_config in exchange_configs {
            let exchange_creation_result = ExchangeWS::new(
                exchange_config,
                orders_producer.clone(),
                exchange_config.http_client,
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
        loop {
            info!("handling orders");
            self.handle_orders().await;
        }
    }
    pub async fn handle_orders(&mut self) {
        let exchange_1 = async {
            while let Some(exchange_streaming_state) = self.exchanges[0].next().await {
                match exchange_streaming_state {
                    WSStreamState::Success => {
                        println!("received stream state succcess\n");
                        continue;
                    }
                    WSStreamState::FailedStream => {
                        println!("received stream state fail \n");
                        continue;
                    }
                    WSStreamState::SenderError => {
                        println!("received stream state fail \n");
                        continue;
                    }
                    WSStreamState::FailedDeserialize => {
                        println!("received stream state fail \n");
                        continue;
                    }
                    WSStreamState::WSError(_) => continue,
                }
            }
        };
        exchange_1.await
    }
}
