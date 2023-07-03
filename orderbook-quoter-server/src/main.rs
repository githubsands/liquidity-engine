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
use tokio::sync::mpsc::{
    channel as mpsc_channel, Receiver as TokioReceiver, Sender as TokioSender,
};

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

use internal_objects::Deals;

use exchange_controller::ExchangeController;
use orderbook::OrderBook;

use tokio::sync::watch::channel as watchChannel;

use tokio::sync::broadcast;

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

    let res = orderbook_quoter_server(config_file.unwrap());
    match res {
        Ok(_) => info!("shutting down orderbook_quoter_server"),
        Err(error) => error!("failed to run orderbook quoter server {}", error),
    }
}

fn orderbook_quoter_server(config: Config) -> Result<(), Box<dyn Error>> {
    let core_ids = core_affinity::get_core_ids().unwrap();
    let mut ctx = Context::background();
    // TODO: Handle the context
    let _ = ctx.add_cancel_signal();
    let parent_ctx = ctx.freeze();
    let process_depths_ctx = Context::create_child(&parent_ctx);
    let package_deals_ctx = Context::create_child(&parent_ctx);
    let (deal_producer, deal_consumer) = mpsc_channel(10);

    info!("starting orderbook-quoter-server");

    // TODO: Handle the snapshot_depth_producer
    let (_, snapshot_depth_consumer) = watchChannel(());
    let (orderbook, depth_producer) = OrderBook::new(deal_producer, &config.orderbook);
    let orderbook = Box::new(orderbook);

    let orderbook_depth_processor_core = core_ids[0];
    let mut orderbook_clone = orderbook.clone();
    let t1 = thread::spawn(move || {
        info!("starting orderbook depth processor");
        _ = orderbook_clone.process_all_depths(&process_depths_ctx);
        let _ = core_affinity::set_for_current(orderbook_depth_processor_core);
    });

    let orderbook_package_deals_core = core_ids[1];
    let mut orderbook_clone = orderbook.clone();
    let t2 = thread::spawn(move || {
        info!("starting orderbook deal packer");
        _ = orderbook_clone.package_deals(&package_deals_ctx);
        let _ = core_affinity::set_for_current(orderbook_package_deals_core);
    });

    let io_grpc_core = core_ids[2];
    let t3 = thread::spawn(move || {
        info!("starting ws io");
        let async_grpc_io_rt = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .thread_name("grpc io")
            .worker_threads(1)
            .build()
            .unwrap();
        let _ = core_affinity::set_for_current(io_grpc_core);
        let grpc_io_handler = async_grpc_io_rt.handle();
        grpc_io_handler.spawn(async {
            let server = OrderBookQuoterServer::new(deal_consumer);
            Server::builder()
                .add_service(QuoterServer::new(server)) // pb generated QuotedServer consumes our
                // type OrderBookQuoterServer
                .serve(":9090".to_socket_addrs().unwrap().next().unwrap())
                .await
                .unwrap();
        });
    });

    let config = config.clone();
    let io_ws_core = core_ids[3];
    let depth_producer = depth_producer.clone();
    let snapshot_depth_consumer = snapshot_depth_consumer.clone();
    let config = config.clone();
    let t4 = thread::spawn(move || {
        info!("starting grpc io");
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
            let mut exchange_controller =
                ExchangeController::new(&config.exchanges, depth_producer, snapshot_depth_consumer)
                    .unwrap();
            exchange_controller.websocket_connect().await;
            exchange_controller.subscribe_depths().await;
            exchange_controller.stream_depths().await;
        });
        async_ws_io_rt.block_on(local);
    });
    t1.join();
    t2.join();
    t3.join();
    t4.join();
    info!("made it here");

    Ok(())
}

type QuoterResult<T> = Result<Response<T>, Status>;
type ResponseStream = Pin<Box<dyn Stream<Item = Result<QuoterResponse, Status>> + Send>>;

#[derive(Debug)]
pub struct OrderBookQuoterServer {
    deals_consumer: TokioReceiver<Deals>,
    quote_broadcaster: tokio::sync::broadcast::Sender<QuoterResponse>,
}

impl OrderBookQuoterServer {
    pub fn new(deal_receiver: TokioReceiver<Deals>) -> OrderBookQuoterServer {
        let (tx, _) = broadcast::channel(16);
        OrderBookQuoterServer {
            deals_consumer: deal_receiver,
            quote_broadcaster: tx,
        }
    }
    pub async fn fanout_quotes(&mut self) {
        while let Ok(deals) = self.deals_consumer.try_recv() {
            self.quote_broadcaster.send(QuoterResponse {
                ask_deals: deals
                    .asks
                    .into_iter()
                    .map(|preprocessed_deal| pb::Deal {
                        location: preprocessed_deal.l as i32,
                        price: preprocessed_deal.p,
                        quantity: preprocessed_deal.q,
                    })
                    .collect(),

                bid_deals: deals
                    .bids
                    .into_iter()
                    .map(|preprocessed_deal| pb::Deal {
                        location: preprocessed_deal.l as i32,
                        price: preprocessed_deal.p,
                        quantity: preprocessed_deal.q,
                    })
                    .collect(),

                spread: (deals.asks[0].p - deals.bids[0].p) as i32,
            });
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
        println!("\tclient connected from: {:?}", req.remote_addr());
        let consumer = self.quote_broadcaster.subscribe();
        let (client_response_producer, client_response_consumer) =
            mpsc_channel::<Result<QuoterResponse, Status>>(1000);
        let mut quote_stream = tokio_stream::wrappers::BroadcastStream::new(consumer);
        let output_stream = ReceiverStream::new(client_response_consumer);
        let input_stream = client_response_producer;

        // 1. receive quote from upstream componenets through quote_grpc_stream
        // 2. forward quote to the client
        // 3. receive a quoter response and status back from the client
        tokio::spawn(async move {
            while let Some(broadcast_receiver_result) = quote_stream.next().await {
                match broadcast_receiver_result {
                    Ok(quote) => match input_stream.send(Result::<_, Status>::Ok(quote)).await {
                        Ok(_) => {
                            info!("sending quote to client");
                        }
                        Err(_quote) => break,
                    },
                    Err(_) => continue,
                }
            }
            println!("\tclient disconnected");
        });
        Ok(Response::new(
            Box::pin(output_stream) as Self::ServerStreamingQuoterStream
        ))
    }
}
