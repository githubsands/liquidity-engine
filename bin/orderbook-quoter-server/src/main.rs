#[allow(unused_imports)]
pub mod pb {
    tonic::include_proto!("lib"); // lib (lib.rs) is the name where our generated proto is within
                                  // the OUR_DIR environmental variable.  be aware that we must
                                  // export OUT_DIR with the path of our generated proto stubs
                                  // before this program can correctly compile
}

use tokio::task::LocalSet;
use tokio_stream::StreamExt;

use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Arc;

use tonic::{Request, Response, Status};

use tokio::sync::broadcast::{channel, Sender};
use tokio::sync::mpsc::{
    channel as mpsc_channel, Receiver as TokioReceiver, Sender as TokioSender,
};

use tokio::sync::mpsc;

use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};

use pb::{QuoterRequest, QuoterResponse};

use std::{error::Error, net::ToSocketAddrs, path::PathBuf, thread};

use io_context::Context;

use clap::{App, Arg};

use tokio::runtime::{Builder, Runtime};
use tonic::transport::Server;

use tracing::{error, info};
use tracing_subscriber;

use config::{read_yaml_config, Config};
use depth_driver::DepthDriver;
use internal_objects::Deals;
use orderbook::OrderBook;
use quoter_grpc_server::run_server;

use tokio::sync::watch::channel as watchChannel;

use tokio::sync::broadcast;
use tokio::sync::Mutex;
use tokio_context::context::Context as asyncContext;

fn main() {
    tracing_subscriber::fmt().init();
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
    if config_file.is_err() {
        panic!("failed to load config {:?}", config_file)
    }
    let res = orderbook_quoter_server(config_file.unwrap());
    match res {
        Ok(_) => info!("shutting down orderbook_quoter_server"),
        Err(error) => error!("failed to run orderbook quoter server {}", error),
    }
}

fn orderbook_quoter_server(config: Config) -> Result<(), Box<dyn Error>> {
    info!("starting orderbook-quoter-server");
    let core_ids = core_affinity::get_core_ids().unwrap();

    let mut ctx = Context::background();
    let (mut async_ctx, _parent_handle) = asyncContext::new();
    let _ = ctx.add_cancel_signal();
    let parent_ctx = ctx.freeze();
    let process_depths_ctx = Context::create_child(&parent_ctx);
    let package_deals_ctx = Context::create_child(&parent_ctx);

    // TODO: Handle the snapshot_depth_producer
    let (_, snapshot_depth_consumer) = watchChannel(());
    let config = config.clone();
    let (quotes_producer, _) = tokio::sync::broadcast::channel(16);
    let (mut orderbook, depth_producer) =
        OrderBook::new(quotes_producer.clone(), &config.orderbook);
    orderbook.send_deals = true; // TODO: this will be removed in the future, a hack.
    
    // todo: update orderbook so bids is in its own thread and asks is in its own thread
    //       exchange stream(s) should possibly just be in each ask and bid thread as well
    let orderbook = Box::new(orderbook);

    let orderbook_depth_processor_core = core_ids[0];
    let mut orderbook_clone = orderbook.clone();
    let t1 = thread::spawn(move || {
        info!("starting orderbook, depth processor, writer thread");
        orderbook_clone.process_all_depths(&process_depths_ctx);
        core_affinity::set_for_current(orderbook_depth_processor_core);
    });

    let mut orderbook_clone = orderbook.clone();
    let t2 = thread::spawn(move || {
        // package deals runs three threads 1 to run a spin lock and 2 child threads
        // to traverse both sides of the book
        // TODO: pin these threads to cores
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(3)
            .build()
            .unwrap();
        pool.install(move || {
            info!("starting orderbook, deal packer, reader thread");
            let package_deals = orderbook_clone.package_deals(&package_deals_ctx);
            if package_deals.is_err() {
                panic!("failed to package deals")
            }
        });
    });
    let io_grpc_core = core_ids[1];
    let config_clone = config.clone();
    let quotes_producer = quotes_producer.clone();
    let t3 = thread::spawn(move || {
        info!("starting grpc io");
        let async_grpc_io_rt = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .thread_name("grpc io")
            .worker_threads(1)
            .build()
            .unwrap();
        core_affinity::set_for_current(io_grpc_core);
        let grpc_io_handler = async_grpc_io_rt.handle();
        let grpc_server = grpc_io_handler.spawn(async move {
            run_server(&config_clone, quotes_producer).await;
            info!("shutting down");
        });
        async_grpc_io_rt.block_on(grpc_server);
    });
    let io_ws_core = core_ids[2];
    let depth_producer = depth_producer.clone();
    let snapshot_depth_consumer = snapshot_depth_consumer.clone();
    let config_clone = config.clone();
    let t4 = thread::spawn(move || {
        info!("starting ws io");
        let async_ws_io_rt = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .thread_name("websocket io")
            .worker_threads(1)
            .build()
            .unwrap();
        let local = LocalSet::new();
        core_affinity::set_for_current(io_ws_core);
        local.spawn_local(async move {
            let mut depth_driver = DepthDriver::new(
                &config_clone.exchanges,
                depth_producer,
                snapshot_depth_consumer,
            )
            .unwrap();
            let start_result = depth_driver.websocket_connect().await;
            match start_result {
                Ok(_) => depth_driver.close_exchanges().await,
                Err(stream_error) => {
                    panic!("failed to stream exchanges: {:?}", stream_error);
                }
            }
            depth_driver.subscribe_depths().await;
            depth_driver.build_orderbook().await;
            if let match stream_result = depth_driver.run_streams(&mut async_ctx).await;
            match stream_result {
                Ok(_) => depth_driver.close_exchanges().await,
                Err(stream_error) => {
                    panic!("failed to stream exchanges: {:?}", stream_error);
                }
            }
        });
        async_ws_io_rt.block_on(local);
    });
    t1.join();
    t2.join();
    t3.join();
    t4.join();
    Ok(())
}
