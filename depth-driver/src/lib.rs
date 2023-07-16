use std::cell::RefCell;
use std::rc::Rc;

use crossbeam_channel::Sender;

use tokio::sync::watch::{
    channel as watchChannel, Receiver as watchReceiver, Sender as watchSender,
};
use tokio_context::context::Context;
use tracing::info;

use futures::pin_mut;
use futures::stream::FuturesOrdered;
use futures::StreamExt;

use config::ExchangeConfig;
use exchange::Exchange;
use market_objects::DepthUpdate;
use quoter_errors::{ErrorHotPath, ErrorInitialState};

pub struct DepthDriver {
    exchanges: Vec<Rc<RefCell<Exchange>>>,
    orderbook_snapshot_trigger: watchReceiver<()>,
    exchange_snapshot_trigger: watchSender<()>,
}

impl DepthDriver {
    pub fn new(
        exchange_configs: &Vec<ExchangeConfig>,
        depths_producer: Sender<DepthUpdate>,
        orderbook_snapshot_trigger: watchReceiver<()>,
    ) -> Result<DepthDriver, ErrorInitialState> {
        let mut exchanges: Vec<Rc<RefCell<Exchange>>> = Vec::new();
        let (snapshot_trigger, inner_snapshot_consumer) = watchChannel(());
        for exchange_config in exchange_configs {
            let exchange = Exchange::new(
                exchange_config,
                depths_producer.clone(),
                inner_snapshot_consumer.clone(),
            );
            exchanges.push(Rc::new(RefCell::new(exchange)));
        }
        Ok(DepthDriver {
            exchanges,
            orderbook_snapshot_trigger,
            exchange_snapshot_trigger: snapshot_trigger,
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

    // run_streams is the main driver the entire program that continously polls websocket depths from
    // N exchanges and also handles orderbook rebuilds
    pub async fn run_streams(&mut self, ctx: &mut Context) -> Result<(), ErrorHotPath> {
        info!("running streams");
        let inner_trigger = &mut self.orderbook_snapshot_trigger;
        let mut exchanges: Vec<*mut Exchange> = vec![];
        let mut streams = FuturesOrdered::new();
        for exchange in &mut self.exchanges {
            exchanges.push(exchange.as_ref().as_ptr())
        }

        unsafe {
            for stream in exchanges {
                streams.push_back(stream.as_mut().unwrap().stream_depths())
            }
        }

        pin_mut!(streams);

        loop {
            tokio::select! {
                    // TODO: This may not rebuild orderbook correctly but the trigger currently is not
                    // implemented within the orderbook. This future handles orderbook rebuilds.
                        _ = inner_trigger.changed()=> {
                            // send snapshot trigger to our N exchange streams
                            // this sets a streams run function to grab a http depth load from multiplie exchanges, and buffers
                            // the websocket depths.
                            let _ = self.exchange_snapshot_trigger.send(());
                        }
                        // this future here polls each exchange's websocket as usual
                        // TODO: Handle error conditions
                        _ = streams.next() => {
                            info!("streaming")
                        }

                        _ = ctx.done() => {
                            return Ok(());
                        }
            }
        }
    }

    pub async fn close_exchanges(&mut self) {
        for exchange in &mut self.exchanges {
            let mut exchange = exchange.borrow_mut();
            exchange.close().await
        }
    }
}
