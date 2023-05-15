use futures::future::join_all;
use rayon::prelude::*;
use rayon::{ThreadPool, ThreadPoolBuilder};
use tokio::runtime::{Builder, Runtime};

use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crossbeam_channel::bounded;

use num_cpus;
use tracing::{debug, error, info, warn};

use exchange_controller::ExchangeController;

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
            let mut async_rt = Builder::new_multi_thread()
                .enable_io()
                .worker_threads(4)
                .thread_stack_size(3 * 1024 * 1024)
                .build()
                .unwrap();
            let num_threads = num_cpus::get();
            let mut tp = rayon::ThreadPoolBuilder::new()
                .num_threads(num_threads)
                .build()
                .unwrap();
            let res = orderbook_quoter_server(&config_file.unwrap(), &mut async_rt, &mut tp);
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
    let (s1, _) = bounded(4);
    let (s2, _) = bounded(4);
    let exchange_controller_result = ExchangeController::new(&config.exchanges, s1, s2);
    info!("created exchange controller");

    async_runtime.block_on(exchange_controller_result.unwrap().boot_exchanges());
    info!("quoter server is shutting down");

    Ok(())
    /*
    let (quoter_server, quote_producer) = QuoterServer::new(config.quoter_server_config)
    let (orderbook, order_buy_producer, order_bid_producer) = Orderbook::new(config.orderbook_config, quote_producer);

    let exchange_controller =  ExchangeController::new(&config.exchanges, order_buy_producer, order_bid_producer);
    async_handle.spawn(move async {
        if let Err(exchange_boot_result) = exchange_controller.boot_exchanges.await() {
            error!("failed to boot exchanges")
        }
        exchange_controller.receive_orderbooks.await()
    });
    */
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
