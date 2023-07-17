#[allow(unused_imports)]
pub mod pb {
    tonic::include_proto!("lib"); // lib (lib.rs) is the name where our generated proto is within
                                  // the OUR_DIR environemntal variable.  be aware that we must
                                  // export OUT_DIR with the path of our generated proto stubs
                                  // before this program can correctly compile
}

use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Arc;

use tokio_stream::StreamExt;
use tonic::{Request, Response, Status};
use tracing::info;

use tokio::sync::broadcast::{channel, Sender};
use tokio::sync::mpsc::channel as tokioChannel;

use internal_objects::Quote;
use tokio_stream::wrappers::ReceiverStream;

use pb::{QuoterRequest, QuoterResponse};

use crate::pb::quoter_server::QuoterServer;

use tonic::transport::Server;

use config::{read_yaml_config, Config};

use std::{error::Error, net::ToSocketAddrs, path::PathBuf, thread};

type QuoterResult<T> = Result<Response<T>, Status>;
type ResponseStream = Pin<Box<dyn Stream<Item = Result<pb::QuoterResponse, Status>> + Send>>;

pub async fn run_server(config: &Config, quotes_producer: tokio::sync::broadcast::Sender<Quote>) {
    let quoter_grpc_server = OrderBookQuoterServer::new(quotes_producer);
    Server::builder()
        .add_service(QuoterServer::new(quoter_grpc_server.clone()))
        .serve(
            config
                .grpc_server
                .host_uri
                .to_socket_addrs()
                .unwrap()
                .next()
                .unwrap(),
        )
        .await
        .unwrap();
}

#[derive(Debug, Clone)]
pub struct OrderBookQuoterServer {
    quote_broadcaster: tokio::sync::broadcast::Sender<Quote>,
}

impl OrderBookQuoterServer {
    pub fn new(quote_broadcaster: tokio::sync::broadcast::Sender<Quote>) -> OrderBookQuoterServer {
        OrderBookQuoterServer {
            quote_broadcaster: quote_broadcaster,
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

        let (tx, rx) = tokioChannel(128);
        tokio::spawn(async move {
            info!("\tstarting quote stream for {:?}", req.remote_addr());
            while let Some(quote_result) = quote_stream.next().await {
                info!("sending quote to {:?}", req.remote_addr());
                match quote_result {
                    Ok(quote) => {
                        let vec_asks: Vec<pb::Deal> = quote
                            .ask_deals
                            .into_iter()
                            .map(|preprocessed_deal| pb::Deal {
                                location: preprocessed_deal.l as i32,
                                price: preprocessed_deal.p,
                                quantity: preprocessed_deal.q,
                            })
                            .collect();

                        let vec_bids: Vec<pb::Deal> = quote
                            .bid_deals
                            .into_iter()
                            .map(|preprocessed_deal| pb::Deal {
                                location: preprocessed_deal.l as i32,
                                price: preprocessed_deal.p,
                                quantity: preprocessed_deal.q,
                            })
                            .collect();
                        match tx
                            .send(Result::<_, Status>::Ok(QuoterResponse {
                                ask_deals: vec_asks,
                                bid_deals: vec_bids,
                                spread: quote.spread as i32,
                            }))
                            .await
                        {
                            Ok(_) => {
                                info!("grpc non error")
                                // item (server response) was queued to be send to client
                            }
                            Err(_item) => {
                                info!("grpc error");
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
