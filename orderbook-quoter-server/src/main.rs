use std::error::Error;
use std::path::PathBuf;

use futures::future::join_all;
use futures::StreamExt;
use num_cpus;
use rayon::ThreadPool;
use tokio::runtime::{Builder, Runtime};
use tokio::time::{sleep as async_sleep, Duration as async_duration};

use crossbeam_channel::bounded;

use clap::{App, Arg};
use tracing::{error, info};
use tracing_subscriber;

// use orderbook::OrderBook;
use exchange_controller::ExchangeController;

use config::{read_yaml_config, Config};

fn f64(num: usize) -> f64 {
    num as f64
}

fn usize(float_num: f64) -> usize {
    float_num as usize
}

fn main() {
    tracing_subscriber::fmt::init();
    let matches = App::new("orderbook quoter server")
        .arg(
            Arg::new("config")
                .long("config")
                .value_name("FILE")
                .takes_value(true),
        )
        .get_matches();
    let config_path = PathBuf::from(matches.value_of("config").unwrap_or("/etc/config.yaml"));
    info!("config path given: {:?}", config_path);
    let file = PathBuf::from(config_path);
    let config_file = read_yaml_config(file);
    info!("config loaded succesfully");
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
            f64(num_cpus) - (f64(num_cpus) * config_file.as_ref().unwrap().io_thread_percentage),
        ))
        .build()
        .unwrap();
    let res = orderbook_quoter_server(&config_file.unwrap(), &mut async_rt, &mut tp);
    match res {
        Ok(_) => info!("shutting down orderbook_quoter_server"),
        Err(error) => error!("failed to run orderbook quoter server {}", error),
    }
}

fn orderbook_quoter_server(
    config: &Config,
    async_runtime: &mut Runtime,
    thread_pool: &mut ThreadPool,
) -> Result<(), Box<dyn Error>> {
    info!("starting orderbook-quoter-server");
    let async_handle = async_runtime.handle();
    // let (orderbook, orders_producer) = OrderBook::new(&config.orderbook);
    let (orderbook_producer, _) = bounded(4);
    let mut exchange_controller =
        ExchangeController::new(&config.exchanges, orderbook_producer).unwrap();
    info!("created exchange controller");
    let handle = async_handle.block_on(async move {
        exchange_controller.boot_exchanges().await;
        async_sleep(async_duration::from_secs(10)).await;

        let mut exchange_handles = Vec::new();
        while let Some(mut exchange_stream) = exchange_controller.pop_exchange() {
            info!("spawning async handler for");
            let exchange_handler = tokio::spawn(async move {
                exchange_stream.next().await;
            });
            exchange_handles.push(exchange_handler);
        }
        let _ = join_all(exchange_handles).await;
    });

    info!("quoter server is shutting down");
    Ok(())
}
