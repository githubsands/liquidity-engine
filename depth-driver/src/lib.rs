use kanal::*;

use std::cell::RefCell;
use std::rc::Rc;

use tracing::warn;

use compio::runtime::{spawn, JoinHandle, Runtime};
use futures::future::join_all;
use tokio::sync::watch::{channel as watchChannel, Receiver as watchReceiver};

use config::ExchangeConfig;
use exchange::exchange::Exchange;
use market_objects::DepthUpdate;
use quoter_errors::{ErrorHotPath, ErrorInitialState};

const DEPTH_CHANNEL_SIZE: usize = 10_000;

pub struct DepthDriver {
    exchanges: Vec<Rc<RefCell<Exchange>>>,
}

impl DepthDriver {
    // new owns the depth channel: each exchange gets a clone of the producer and the
    // consumer is handed back to the caller to drain depth updates from.
    pub fn new(
        exchange_configs: &Vec<ExchangeConfig>,
        _: watchReceiver<()>,
    ) -> Result<(DepthDriver, AsyncReceiver<DepthUpdate>), ErrorInitialState> {
        let (depths_producer, depths_consumer) = bounded_async(DEPTH_CHANNEL_SIZE);
        let mut exchanges: Vec<Rc<RefCell<Exchange>>> = Vec::new();
        let (_, inner_snapshot_consumer) = watchChannel(());
        for exchange_config in exchange_configs {
            let exchange = Exchange::new(
                exchange_config,
                depths_producer.clone(),
                inner_snapshot_consumer.clone(),
            );
            exchanges.push(Rc::new(RefCell::new(exchange?)));
        }
        Ok((DepthDriver { exchanges }, depths_consumer))
    }

    pub fn run(self, rt: &Runtime) -> JoinHandle<Result<(), ErrorInitialState>> {
        rt.spawn(async move {
            let mut driver = self;
            driver.websocket_connect().await?;
            driver.subscribe_depths().await?;
            driver.sync_orderbook().await?;
            driver
                .run_streams()
                .await
                .map_err(|e| ErrorInitialState::WSConnection(e.to_string()))?;
            Ok::<(), ErrorInitialState>(())
        })
    }

    pub async fn websocket_connect(&mut self) -> Result<(), ErrorInitialState> {
        for exchange in self.exchanges.iter_mut() {
            exchange.as_ref().borrow_mut().start().await?;
        }
        Ok(())
    }

    pub async fn subscribe_depths(&mut self) -> Result<(), ErrorInitialState> {
        for exchange in self.exchanges.iter_mut() {
            exchange
                .as_ref()
                .borrow_mut()
                .subscribe_orderbooks()
                .await?;
        }
        Ok(())
    }

    pub async fn sync_orderbook(&mut self) -> Result<(), ErrorInitialState> {
        for exchange in self.exchanges.iter_mut() {
            exchange.as_ref().borrow_mut().run_snapshot().await?;
        }
        for exchange in self.exchanges.iter_mut() {
            exchange
                .as_ref()
                .borrow_mut()
                .push_buffered_ws_depths()
                .await;
        }
        Ok(())
    }

    // todo: graceful shutdown - cancelling the task awaiting run_streams cancels every stream
    // run_streams spawns one task per exchange on the current compio runtime and waits for
    // all of them. streams run independently: one failing doesn't stop the others.
    pub async fn run_streams(&mut self) -> Result<(), ErrorHotPath> {
        let streams: Vec<JoinHandle<Result<(), ErrorHotPath>>> = self
            .exchanges
            .iter()
            .enumerate()
            .map(|(idx, exchange)| {
                let exchange = exchange.clone();
                spawn(async move {
                    loop {
                        if let Err(err) = exchange.as_ref().borrow_mut().stream_depths().await {
                            warn!(
                                "received error {} for exchange when streaming depths{}",
                                err, idx
                            );
                            // todo: don't return but reconnect - we don't want to end the
                            //       entire system if one websocket fails
                            return Err(ErrorHotPath::OrderBookDealSendFail);
                        }
                    }
                })
            })
            .collect();
        let mut result = Ok(());
        for stream in join_all(streams).await {
            match stream {
                Ok(Ok(())) => {}
                Ok(Err(err)) => result = Err(err),
                Err(join_err) => {
                    warn!("exchange stream task did not complete: {:?}", join_err);
                    result = Err(ErrorHotPath::OrderBookDealSendFail);
                }
            }
        }
        result
    }

    // todo: update the error here
    pub async fn close_exchanges(&mut self) -> Result<(), ErrorHotPath> {
        for exchange in &mut self.exchanges {
            let mut exchange = exchange.borrow_mut();
            exchange
                .close()
                .await
                .map_err(|_| ErrorHotPath::OrderBookDealSendFail)?;
        }
        Ok(())
    }
}
