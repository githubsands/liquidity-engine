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

use crate::pb::quoter_server::Quoter;
use crate::pb::quoter_server::QuoterServer;

use std::{error::Error, net::ToSocketAddrs, path::PathBuf, thread};

use io_context::Context;

use clap::{App, Arg};

use tokio::runtime::{Builder, Runtime};
use tonic::transport::Server;

use config::{read_yaml_config, Config};
use tracing::{error, info};
use tracing_subscriber;

use internal_objects::Deals;

use depth_driver::DepthDriver;
use orderbook::OrderBook;

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
    orderbook.send_deals = false; // TODO: this will be removed in the future, a hack.
    let orderbook = Box::new(orderbook);

    let orderbook_depth_processor_core = core_ids[0];
    let mut orderbook_clone = orderbook.clone();
    let t1 = thread::spawn(move || {
        info!("starting orderbook, depth processor, writer thread");
        _ = orderbook_clone.process_all_depths(&process_depths_ctx);
        let _ = core_affinity::set_for_current(orderbook_depth_processor_core);
    });

    let mut orderbook_clone = orderbook.clone();
    let t2 = thread::spawn(move || {
        info!("starting orderbook, deal packer, reader thread");
        let package_deals = orderbook_clone.package_deals(&package_deals_ctx);
        if package_deals.is_err() {
            panic!("failed to package deals")
        }
    });
    let io_grpc_core = core_ids[1];
    let config_clone = config.clone();
    let t3 = thread::spawn(move || {
        info!("starting grpc io");
        let async_grpc_io_rt = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .thread_name("grpc io")
            .worker_threads(1)
            .build()
            .unwrap();
        let _ = core_affinity::set_for_current(io_grpc_core);
        let grpc_io_handler = async_grpc_io_rt.handle();
        let grpc_server = grpc_io_handler.spawn(async move {
            let quoter_grpc_server = OrderBookQuoterServer::new(quotes_producer);
            Server::builder()
                .add_service(QuoterServer::new(quoter_grpc_server.clone()))
                .serve(
                    config_clone
                        .grpc_server
                        .host_uri
                        .to_socket_addrs()
                        .unwrap()
                        .next()
                        .unwrap(),
                )
                .await
                .unwrap();
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
        let _ = core_affinity::set_for_current(io_ws_core);
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
            let _ = depth_driver.subscribe_depths().await;
            let _ = depth_driver.build_orderbook().await;
            let stream_result = depth_driver.run_streams(&mut async_ctx).await;
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

type QuoterResult<T> = Result<Response<T>, Status>;
type ResponseStream = Pin<Box<dyn Stream<Item = Result<QuoterResponse, Status>> + Send>>;

#[derive(Debug, Clone)]
pub struct OrderBookQuoterServer {
    quote_broadcaster: tokio::sync::broadcast::Sender<QuoterResponse>,
}

impl OrderBookQuoterServer {
    pub fn new(
        quote_broadcaster: tokio::sync::broadcast::Sender<orderbook::pb::QuoterResponse>,
    ) -> OrderBookQuoterServer {
        let (tx, _) = broadcast::channel(16);
        OrderBookQuoterServer {
            quote_broadcaster: tx,
        }
    }
}

#[tonic::async_trait]
impl pb::quoter_server::Quoter for OrderBookQuoterServer {
    type ServerStreamingQuoterStream = ResponseStream;

    async fn server_streaming_quoter(
        &self,
        req: Request<QuoterRequest>,
    ) -> QuoterResult<Self::ServerStreamingQuoterStream> {
        info!("\tclient connected from: {:?}", req.remote_addr());

        let consumer = self.quote_broadcaster.subscribe();
        let mut quote_stream = Box::pin(tokio_stream::wrappers::BroadcastStream::new(consumer));

        let (tx, rx) = mpsc::channel(128);
        tokio::spawn(async move {
            while let Some(quote_result) = quote_stream.next().await {
                match quote_result {
                    Ok(quote) => {
                        match tx.send(Result::<_, Status>::Ok(quote)).await {
                            Ok(_) => {
                                // item (server response) was queued to be send to client
                            }
                            Err(_item) => {
                                // output_stream was build from rx and both are dropped
                                break;
                            }
                        }
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
            info!("\tclient disconnected");
        });

        let output_stream = ReceiverStream::new(rx);
        Ok(Response::new(
            Box::pin(output_stream) as Self::ServerStreamingQuoterStream
        ))
    }
}
