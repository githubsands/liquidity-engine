use crossbeam_channel::{Receiver, Sender, unbounded};

use tokio::sync::watch::channel as watch_channel;

use config::Config;
use depth_driver::DepthDriver;
use market_objects::DepthUpdate;
use orderbook::OrderBook;
use quoter_errors::ErrorInitialState;

pub struct Core {
    orderbook: OrderBook,
    depth_driver: DepthDriver,
    // depths flow from the driver's exchanges into the orderbook through this channel.
    // todo: feed these updates into `orderbook` once it exposes a depth processor.
    depth_consumer: Receiver<DepthUpdate>,
}

impl Core {
    pub fn new(config: &Config) -> Result<Self, ErrorInitialState> {
        let (orderbook, _, _, _, _) = OrderBook::new(&config.orderbook);
        let (depth_producer, depth_consumer): (Sender<DepthUpdate>, Receiver<DepthUpdate>) =
            unbounded();
        // todo: drive snapshot rebuilds through this watch channel
        let (_snapshot_producer, snapshot_consumer) = watch_channel(());
        let depth_driver = DepthDriver::new(&config.exchanges, depth_producer, snapshot_consumer)?;
        Ok(Core {
            orderbook,
            depth_driver,
            depth_consumer,
        })
    }

    pub fn run(&mut self) -> Result<(), ErrorInitialState> {
        self.depth_driver.run()
    }
}
