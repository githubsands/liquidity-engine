use std::error::Error;

use num_cpus;
use rayon::ThreadPool;
use tokio::runtime::{Builder, Runtime};

use std::env;

use crossbeam_channel::bounded;

use tracing::{error, info};
use tracing_subscriber;

// use orderbook::OrderBook;
use exchange_controller::ExchangeController;

use config::{read_yaml_config, Config};

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

fn f64(num: usize) -> f64 {
    num as f64
}

fn usize(float_num: f64) -> usize {
    float_num as usize
}

fn orderbook_quoter_server(
    config: &Config,
    async_runtime: &mut Runtime,
    thread_pool: &mut ThreadPool,
) -> Result<(), Box<dyn Error>> {
    let async_handle = async_runtime.handle();
    // let (orderbook, orders_producer) = OrderBook::new(&config.orderbook);
    let (orderbook_producer, _) = bounded(4);
    let mut exchange_controller_result =
        ExchangeController::new(&config.exchanges, orderbook_producer).unwrap();
    info!("created exchange controller");
    async_runtime.block_on(async move {
        exchange_controller.boot_exchanges().await;
        async_sleep(async_duration::from_secs(10)).await;
        while let Some(mut exchange) = exchange_controller.exchange() {
            info!("spawning async handler for");
            tokio::spawn(async move { exchange.next().await });
        }
    });
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
