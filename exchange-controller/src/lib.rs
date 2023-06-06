use crossbeam_channel::Sender;

use futures::future::try_join_all;

use market_object::DepthUpdate;
use quoter_errors::ErrorInitialState;

use tracing::{error, info};

use config::ExchangeConfig;
use exchange_stream::ExchangeStream;

pub struct ExchangeController {
    exchanges: Vec<Box<ExchangeWS>>,
}

impl ExchangeController {
    pub fn new(
        exchange_configs: &Vec<ExchangeConfig>,
        orders_producer: Sender<DepthUpdate>,
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

        {
            let mut exchange_websocket_initial_boot_tasks: Vec<_> = Vec::new();

            for exchange in &mut self.exchanges {
                let websocket_future = async {
                    match exchange.start().await {
                        Ok(_) => return Ok(()),
                        Err(exchange_start_error) => {
                            error!("could not connect to exchange {}", exchange_start_error);
                            return Err(exchange_start_error);
                        }
                    }
                };
                exchange_websocket_initial_boot_tasks.push(websocket_future);
            }
            let exchange_websockets_results: Result<Vec<_>, ErrorInitialState> =
                try_join_all(exchange_websocket_initial_boot_tasks).await;
            let _ = exchange_websockets_results.unwrap();
        }
        {
            let mut orderbook_snapshot_tasks: Vec<_> = Vec::new();

            for exchange in &mut self.exchanges {
                let snapshot_future = async {
                    let snapshot_result = exchange.run_snapshot().await;
                    match snapshot_result {
                        Ok(()) => return Ok(()),
                        Err(snapshot_error) => {
                            error!(
                                "could not get snapshot for exchange {}",
                                exchange.client_name
                            );
                            return Err(ErrorInitialState::Snapshot(snapshot_error.to_string()));
                        }
                    }
                };
                orderbook_snapshot_tasks.push(snapshot_future);
            }
            let snap_shot_results: Result<Vec<_>, ErrorInitialState> =
                try_join_all(orderbook_snapshot_tasks).await;
            let _ = snap_shot_results.unwrap();
        }

        Ok(())
    }
    pub fn pop_exchange(&mut self) -> Option<Box<ExchangeWS>> {
        return self.exchanges.pop();
    }
}
