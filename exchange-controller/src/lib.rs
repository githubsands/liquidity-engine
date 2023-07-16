use config::ExchangeConfig;
use exchange_ws::{ExchangeWS, WSStreamState};

use tracing::{debug, error, info, warn};

use order::Order;
use quoter_errors::{ErrorHotPath, ErrorInitialState};

use crossbeam_channel::Sender;

use futures::future::{try_join_all, IntoFuture};
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
        let ws_stream_result = self.handle_orders().await;
        loop {
            info!("handling orders");
            self.handle_orders().await;
        }
    }
    pub async fn handle_orders(&mut self) {
        while let Some(exchange_streaming_state) = self.exchanges[0].try_next() {
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
    }

    /*
    pub async fn handle_orders(&mut self) -> Result<(), ErrorHotPath> {
        let mut tasks: Vec<Pin<Box<dyn Future<Output = Result<(), ErrorHotPath>>>>> = Vec::new();

        for exchange in &mut self.exchanges {
            let stream = Box::pin(async {
                while let Some(exchange_streaming_state) = exchange.next().await {
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
                        WSStreamState::WSError(ws_error) => break,
                    }
                }
                return Err(ErrorHotPath::ExchangeWSError("test".to_string()));
            });
            tasks.push(stream);
        }

        try_join_all(tasks).await?;

        Ok(())
    }
    */
}
