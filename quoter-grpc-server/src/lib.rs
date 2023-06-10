#[allow(unused_imports)]
pub mod pb {
    tonic::include_proto!("lib");
}

use async_trait::async_trait;
use futures::stream::Stream;
use std::env;
use std::process;
use std::sync::Arc;
use std::time::Duration;
use std::{error::Error, io::ErrorKind, net::ToSocketAddrs, pin::Pin};
use tokio::sync::Mutex;

use tokio_stream::StreamExt;
use tonic::transport::Channel;
use tonic::{transport::Server, Request, Response, Status, Streaming};
use tracing::info;

use tokio::sync::broadcast::{channel, Receiver, Sender};
use tokio::sync::mpsc::{
    channel as mpsc_channel, Receiver as mpsc_receiver, Sender as mpsc_sender,
};

use internal_objects::{Deal, Quote};
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};

use pb::{quoter_server::QuoterServer, QuoterRequest, QuoterResponse};

type QuoterResult<T> = Result<Response<T>, Status>;
type ResponseStream = Pin<Box<dyn Stream<Item = Result<QuoterResponse, Status>> + Send>>;

#[derive(Debug)]
pub struct QuoterOrderBookServer {
    quote_producer: Arc<Sender<Quote>>,
}

impl QuoterOrderBookServer {
    pub fn new() -> (Self, Sender<Quote>) {
        let (quote_producer, _quote_consumer) = channel::<Quote>(16);
        let quote_producer_arc = Arc::new(quote_producer.clone());
        (
            QuoterOrderBookServer {
                quote_producer: quote_producer_arc,
            },
            quote_producer,
        )
    }
}

fn match_for_io_error(err_status: &Status) -> Option<&std::io::Error> {
    let mut err: &(dyn Error + 'static) = err_status;

    loop {
        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
            return Some(io_err);
        }

        // h2::Error do not expose std::io::Error with `source()`
        // https://github.com/hyperium/h2/pull/462
        if let Some(h2_err) = err.downcast_ref::<h2::Error>() {
            if let Some(io_err) = h2_err.get_io() {
                return Some(io_err);
            }
        }

        err = match err.source() {
            Some(err) => err,
            None => return None,
        };
    }
}

#[tonic::async_trait]
impl pb::quoter_server::Quoter for QuoterOrderBookServer {
    type ServerStreamingQuoterStream =
        Pin<Box<dyn Stream<Item = Result<QuoterResponse, Status>> + Send + Sync + 'static>>;

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
        let (client_response_producer, client_response_consumer) = mpsc_channel(100);
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
