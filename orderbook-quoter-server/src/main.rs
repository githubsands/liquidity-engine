use futures::future::join_all;
use futures::StreamExt;
use rayon::prelude::*;
use rayon::{ThreadPool, ThreadPoolBuilder};
use tokio::runtime::{Builder, Runtime};
use tokio::time::{sleep as async_sleep, Duration as async_duration};

use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use awaitgroup::WaitGroup;
use crossbeam_channel::bounded;

use num_cpus;
use tracing::{debug, error, info, warn};

use exchange_controller::ExchangeController;
// use orderbook::OrderBook;

use config::{read_yaml_config, Config};

use std::error::Error;

use serde::{Deserialize, Serialize};
use tracing_subscriber;

fn main() {
    tracing_subscriber::fmt::init();
    info!("starting orderbook-quoter-server");
    let current_dir_result = env::current_dir();
    match current_dir_result {
        Ok(mut file) => {
            file.push("quoter-config.yaml");
            let config_file = read_yaml_config(file);
            info!("found configuration file");
            let num_cpus = num_cpus::get();
            let mut async_rt = Builder::new_multi_thread()
                .enable_io()
                .enable_time()
                .worker_threads(usize(
                    f64(num_cpus) * config_file.as_ref().unwrap().io_thread_percentage,
                ))
                .build()
                .unwrap();
            let mut tp = rayon::ThreadPoolBuilder::new()
                .num_threads(usize(
                    f64(num_cpus)
                        - (f64(num_cpus) * config_file.as_ref().unwrap().io_thread_percentage),
                ))
                .build()
                .unwrap();
            let res = orderbook_quoter_server(&config_file.unwrap(), &mut async_rt, &mut tp);
            match res {
                Ok(_) => info!("shutting down orderbook_quoter_server"),
                Err(error) => error!("failed to run orderbook quoter server {}", error),
            }
        }
        Err(error) => {
            error!("failed to start orderbook quoter server: {}", error);
        }
    }
}

fn orderbook_quoter_server(
    config: &Config,
    async_runtime: &mut Runtime,
    thread_pool: &mut ThreadPool,
) -> Result<(), Box<dyn Error>> {
    let async_handle = async_runtime.handle();
    // let (orderbook, orders_producer) = OrderBook::new(&config.orderbook);
    let (orderbook_producer, _) = bounded(4);
    let mut exchange_controller =
        ExchangeController::new(&config.exchanges, orderbook_producer).unwrap();
    info!("created exchange controller");
    let handle = async_handle.block_on(async move {
        exchange_controller.boot_exchanges().await;
        async_sleep(async_duration::from_secs(10)).await;
        while let Some(mut exchange_stream) = exchange_controller.exchange() {
            info!("spawning async handler for");
            tokio::spawn(async move {
                exchange_stream.next().await;
            });
        }
    });

    async_runtime.shutdown_timeout(Duration::from_millis(100));

    // TODO: wait for tasks to finish

    info!("quoter server is shutting down");
    Ok(())
}

/*
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = QuoterServer {};
    Server::builder()
        .add_service(pb::echo_server::QuoterServer::new(server))
        .serve("[::1]:50051".to_socket_addrs().unwrap().next().unwrap())
        .await
        .unwrap();

    Ok(())
}
*/
