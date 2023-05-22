#[allow(unused_imports)]
pub mod pb {
    tonic::include_proto!("lib"); // lib (lib.rs) is the name where our generated proto is within
                                  // the OUR_DIR environemntal variable.  be aware that we must
                                  // export OUT_DIR with the path of our generated proto stubs
                                  // before this program can correctly compile
}

use tokio::task::LocalSet;

use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Arc;

use tokio_stream::StreamExt;
use tonic::{Request, Response, Status};

use tokio::sync::broadcast::{channel, Sender};
use tokio::sync::mpsc::channel as mpsc_channel;

use internal_objects::Quote;
use tokio_stream::wrappers::ReceiverStream;

use pb::{QuoterRequest, QuoterResponse};

use crate::pb::quoter_server::QuoterServer;

use std::{error::Error, net::ToSocketAddrs, path::PathBuf, thread};

use io_context::Context;

use clap::{App, Arg};

use tokio::runtime::{Builder, Runtime};
use tonic::transport::Server;

use config::{read_yaml_config, Config};
use tracing::{error, info};
use tracing_subscriber;

use crossbeam_channel::{bounded, Receiver as syncReceiver, Sender as syncChannel};
use exchange_controller::ExchangeController;
use orderbook::OrderBook;

use tokio::sync::watch::{
    channel as watchChannel, Receiver as watchReceiver, Sender as watchSender,
};

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

    let mut ctx = Context::background();
    let res = orderbook_quoter_server(&ctx, config_file.unwrap());
    match res {
        Ok(_) => info!("shutting down orderbook_quoter_server"),
        Err(error) => error!("failed to run orderbook quoter server {}", error),
    }
}

fn orderbook_quoter_server(ctx: &Context, config: Config) -> Result<(), Box<dyn Error>> {
    let core_ids = core_affinity::get_core_ids().unwrap();
    let cancel_ctx = ctx.add_cancel_signal();
    let parent_ctx = ctx.freeze();
    let process_depths_ctx = Context::create_child(&parent_ctx);
    let package_deals_ctx = Context::create_child(&parent_ctx);

    info!("starting orderbook-quoter-server");

    let (snapshot_depth_producer, snapshot_depth_consumer) = watchChannel(());
    let (orderbook, depth_producer) = OrderBook::new(config.orderbook);
    let orderbook = Arc::new(Box::new(orderbook));

    let orderbook_depth_processor_core = core_ids[0];
    let orderbook_clone = orderbook.clone();
    thread::spawn(move || {
        _ = orderbook_clone.process_all_depths(&package_deals_ctx);
        let core_id = core_affinity::set_for_current(orderbook_depth_processor_core);
    });

    let orderbook_package_deals_core = core_ids[1];
    let orderbook_clone = orderbook.clone();
    thread::spawn(move || {
        _ = orderbook_clone.package_deals(&package_deals_ctx);
        let core_id = core_affinity::set_for_current(orderbook_package_deals_core);
    });

    let io_grpc_core = core_ids[2];
    let config = config.clone();
    thread::spawn(move || {
        let mut async_grpc_io_rt = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .thread_name("grpc io")
            .worker_threads(1)
            .build()
            .unwrap();
        let core_id = core_affinity::set_for_current(io_grpc_core);
        let grpc_io_handler = async_grpc_io_rt.handle();
        grpc_io_handler.spawn(async {
            let (server, quote_producer) = OrderBookQuoterServer::new();
            Server::builder()
                .add_service(QuoterServer::new(server)) // pb generated QuotedServer consumes our
                // type OrderBookQuoterServer
                .serve(":9090".to_socket_addrs().unwrap().next().unwrap())
                .await
                .unwrap();
        });
    });

    let io_ws_core = core_ids[3];
    let depth_producer = depth_producer.clone();
    let snapshot_depth_consumer = snapshot_depth_consumer.clone();
    let config = config.clone();
    thread::spawn(move || {
        let mut async_ws_io_rt = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .thread_name("websocket io")
            .worker_threads(1)
            .build()
            .unwrap();
        let local = LocalSet::new();
        let core_id = core_affinity::set_for_current(io_ws_core);
        local.spawn_local(async move {
            let mut exchange_controller =
                ExchangeController::new(&config.exchanges, depth_producer, snapshot_depth_consumer)
                    .unwrap();
            exchange_controller.websocket_connect().await;
            exchange_controller.subscribe_depths().await;
            exchange_controller.stream_depths().await;
        });
        async_ws_io_rt.block_on(local);
    });

    Ok(())
}

type QuoterResult<T> = Result<Response<T>, Status>;
type ResponseStream = Pin<Box<dyn Stream<Item = Result<QuoterResponse, Status>> + Send>>;

#[derive(Debug, Clone)]
pub struct OrderBookQuoterServer {
    pub quote_producer: Arc<Sender<Quote>>,
}

impl OrderBookQuoterServer {
    pub fn new() -> (Self, Sender<Quote>) {
        let (quote_producer, _quote_consumer) = channel::<Quote>(16);
        let quote_producer_arc = Arc::new(quote_producer.clone());
        (
            OrderBookQuoterServer {
                quote_producer: quote_producer_arc,
            },
            quote_producer,
        )
    }
}

#[tonic::async_trait]
impl pb::quoter_server::Quoter for OrderBookQuoterServer {
    type ServerStreamingQuoterStream = ResponseStream;

    async fn server_streaming_quoter(
        &self,
        req: Request<QuoterRequest>,
    ) -> QuoterResult<Self::ServerStreamingQuoterStream> {
        println!("\tclient connected from: {:?}", req.remote_addr());
        let producer = self.quote_producer.clone();
        let receiver = producer.subscribe();
        let output_stream = tokio_stream::wrappers::BroadcastStream::new(receiver);
        let mut quote_grpc_stream =
            output_stream.map(|upstream_quote_result| match upstream_quote_result {
                Ok(upstream_quote) => Ok(QuoterResponse {
                    ask_deals: upstream_quote
                        .ask_deals
                        .into_iter()
                        .map(|preprocessed_deal| pb::Deal {
                            location: preprocessed_deal.l as i32,
                            price: preprocessed_deal.p,
                            quantity: preprocessed_deal.q,
                        })
                        .collect(),
                    bid_deals: upstream_quote
                        .bid_deals
                        .into_iter()
                        .map(|preprocessed_deal| pb::Deal {
                            location: preprocessed_deal.l as i32,
                            price: preprocessed_deal.p,
                            quantity: preprocessed_deal.q,
                        })
                        .collect(),
                    spread: upstream_quote.spread as i32,
                }),
                Err(upstream_quote_error) => {
                    info!("failed");
                    return Err(upstream_quote_error);
                }
            });
        // 1. receive quote from upstream componenets through quote_grpc_stream
        // 2. forward quote to the client
        // 3. receive a quoter response and status back from the client
        let (client_response_producer, client_response_consumer) = mpsc_channel(1000);
        tokio::spawn(async move {
            while let Some(broadcast_receiver_result) = quote_grpc_stream.next().await {
                match broadcast_receiver_result {
                    Ok(quote) => {
                        match client_response_producer
                            .send(Result::<_, Status>::Ok(quote))
                            .await
                        {
                            Ok(_) => {
                                info!("sending quote to client");
                            }
                            Err(_quote) => break,
                        }
                    }
                    Err(_) => continue,
                }
            }
            println!("\tclient disconnected");
        });
        let output_stream = ReceiverStream::new(client_response_consumer);
        Ok(Response::new(
            Box::pin(output_stream) as Self::ServerStreamingQuoterStream
        ))
    }
}
