use crate::generator::DepthMessageGenerator;
use crossbeam_channel::Sender;
use itertools::interleave;
use market_objects::DepthUpdate;
use quoter_errors::ErrorHotPath;
use tokio::sync::watch::{channel as watchChannel, Receiver as watchReceiver};
use tokio::time::Duration;
use tracing::{debug, info};

pub struct DepthMachine {
    pub trigger_snapshot: watchReceiver<()>,
    pub depth_producer: Sender<DepthUpdate>,
    pub dmg: DepthMessageGenerator,
    pub sequence: bool,
    pub snapshot: Option<(Vec<DepthUpdate>, Vec<DepthUpdate>)>,
    pub depth: usize,
    pub exchanges: usize,
    pub mid_point: usize,
}
impl DepthMachine {
    fn build_snapshot(&mut self) -> (Vec<DepthUpdate>, Vec<DepthUpdate>) {
        let (asks, bids) =
            self.dmg
                .depth_balanced_orderbook(self.depth, self.exchanges, self.mid_point);
        self.snapshot = Some((asks.clone(), bids.clone()));
        return (asks, bids);
    }
    fn snapshot(&mut self) -> (Vec<DepthUpdate>, Vec<DepthUpdate>) {
        return self.snapshot.as_ref().unwrap().clone();
    }
    async fn produce_snapshot_depths(&mut self) -> Result<(), ErrorHotPath> {
        debug!("waiting to be triggered");
        loop {
            tokio::select! {
                _ = self.trigger_snapshot.changed() => {
                    let books = self.snapshot.clone().unwrap();
                    let depths: Vec<DepthUpdate>= interleave(books.0, books.1).collect();
                    for i in 0..depths.len() {
                        tokio::time::sleep(Duration::from_nanos(3)).await;
                        let result = self.depth_producer.send(depths[i]);
                        match result {
                            Ok(_) => {
                                debug!("sendiongd epth")
                            }
                            Err(depth_send_error) => {
                                debug!("LOOK failed to send depth");
                                return Err(ErrorHotPath::ExchangeStreamSnapshot("failed to send depth to orderbook".to_string()))
                            }
                        }
                    }
                    debug!("done");
                    break;
                }
            }
        }
        return Ok(());
    }
    async fn produce_depths(&mut self) {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_nanos(1)) => {
                if self.sequence {
                    let depth_update = self.dmg.depth_message_random();
                    info!("sending depth");
                    _ = self.depth_producer.send(depth_update);
                }
            }
        }
    }
    async fn push_depth_load(&mut self, books: (Vec<DepthUpdate>, Vec<DepthUpdate>)) {
        let depths = interleave(books.0.into_iter(), books.1.into_iter());
        for depth in depths {
            let _ = self.depth_producer.send(depth);
        }
    }
}
