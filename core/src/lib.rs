use compio::runtime::{JoinHandle, Runtime};
use kanal::*;

use config::Config;
use depth_driver::DepthDriver;
use market_objects::DepthUpdate;
use orderbook::orderbook::OrderBook;
use quoter_errors::ErrorInitialState;

enum ErrorCore {
    Startup(String)
}

pub struct Core {
    rt: Runtime,
    ob: OrderBook,
    depth_driver: Option<DepthDriver>,
    depth_consumer: AsyncReceiver<DepthUpdate>,
}

impl Core {
    pub fn new(config: &Config) -> Result<Self, ErrorCore> {
        let ob = OrderBook::new(&config.orderbook);

        let rt = Runtime::new().map_err(|e| ErrorCore::Startup(e.to_string()))?;

        let (_snapshot_producer, snapshot_consumer) = tokio::sync::watch::channel(());

        let (depth_driver, depth_consumer) = DepthDriver::new(&config.exchanges, snapshot_consumer)
            .map_err(|e| ErrorCore::Startup(e.to_string()))?;

        Ok(Core {
            rt,
            ob: OrderBook::new(&config.orderbook),
            depth_driver: Some(depth_driver),
            depth_consumer,
        })
    }

    pub fn run(&mut self) -> Result<JoinHandle<Result<(), ErrorInitialState>>, ErrorInitialState> {
        let depth_driver = self.depth_driver.take().ok_or_else(|| {
            ErrorInitialState::WSConnection("depth driver is already running".to_string())
        })?;
        Ok(depth_driver.run(&self.rt))
    }
}
