use std::cell::RefCell;
use std::rc::Rc;

use crossbeam_channel::Sender;
use tracing::warn;

use tokio::sync::watch::{channel as watchChannel, Receiver as watchReceiver};
use tokio::task;
use tokio_context::context::Context;

use config::ExchangeConfig;
use exchange::exchange::Exchange;
use market_objects::DepthUpdate;
use quoter_errors::{ErrorHotPath, ErrorInitialState};

pub struct DepthDriver {
    exchanges: Vec<Rc<RefCell<Exchange>>>,
    // todo: implement syncs
    // orderbook_snapshot_sync: watchReceiver<()>,
    // exchange_snapshot_sync: watchSender<()>,
}

impl DepthDriver {
    pub fn new(
        exchange_configs: &Vec<ExchangeConfig>,
        depths_producer: Sender<DepthUpdate>,
        _: watchReceiver<()>,
    ) -> Result<DepthDriver, ErrorInitialState> {
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
        Ok(DepthDriver {
            exchanges,
            // orderbook_snapshot_trigger,
            // exchange_snapshot_trigger: snapshot_trigger,
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

    // todo: this can be done on multiplie threads?
    pub async fn build_orderbook(&mut self) -> Result<(), ErrorInitialState> {
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

    // todo: think about implementing context for graceful shutdowns
    pub async fn run_streams(&mut self, _: &mut Context) -> Result<(), ErrorHotPath> {
        let local = task::LocalSet::new();
        local
            .run_until(async move {
                for (idx, exchange) in self.exchanges.iter_mut().enumerate() {
                    let exchange = exchange.clone();
                    tokio::task::spawn_local(async move {
                        loop {
                            match exchange.as_ref().borrow_mut().stream_depths().await {
                                Ok(_) => continue,
                                Err(err) => {
                                    warn!(
                                        "received error {} for exchange when streaming depths{}",
                                        err, idx
                                    );
                                    // todo: don't return but reconnect - we don't want to end the
                                    //       entire system if one websocket fails
                                    return Err(ErrorHotPath::OrderBookDealSendFail);
                                }
                            }
                        }
                        // even though this code is unreachable we need to infer
                        // the return type
                        Ok::<(), ErrorHotPath>(())
                    });
                }
            })
            .await;
        Ok(())
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
