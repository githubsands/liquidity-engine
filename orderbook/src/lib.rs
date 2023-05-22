#[allow(unused_variables)]
use crossbeam_channel::Sender;
use ordered_float::OrderedFloat;
use rustc_hash::FxHashMap;
use std::time::Duration;
use tokio::sync::mpsc::Sender as TokioSender;
use tokio::sync::watch::{
    channel as watchChannel, Receiver as watchReceiver, Sender as WatchSender,
};

use std::cell::Cell;
use std::thread;

use std::cell::UnsafeCell;
use std::hash::Hash;
use std::ops::{Index, IndexMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use config::OrderbookConfig;
use internal_objects::{Deal, Deals};

use market_objects::DepthUpdate;

use quoter_errors::ErrorHotPath;
use ring_buffer::RingBuffer;
use tracing::{debug, warn};

use io_context::Context;

pub struct AtomicMap<K, V>
where
    K: Hash + Eq,
{
    data: UnsafeCell<FxHashMap<K, V>>,
}

unsafe impl<K: Hash + Eq + Send, V: Send> Send for AtomicMap<K, V> {}
unsafe impl<K: Hash + Eq + Sync, V: Sync> Sync for AtomicMap<K, V> {}

impl<K: Hash + Eq, V> AtomicMap<K, V> {
    pub fn new() -> Self {
        AtomicMap {
            data: UnsafeCell::new(FxHashMap::default()),
        }
    }

    pub fn insert(&self, key: K, value: V) {
        unsafe {
            (*self.data.get()).insert(key, value);
        }
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        let result;
        unsafe {
            result = (*self.data.get()).get(key);
        }
        result
    }

    pub fn get_mut(&self, key: &K) -> Option<&mut V> {
        let result;
        unsafe {
            result = (*self.data.get()).get_mut(key);
        }
        result
    }
}

impl<K: Hash + Eq, V> Index<K> for AtomicMap<K, V> {
    type Output = V;

    fn index(&self, index: K) -> &Self::Output {
        let result;
        unsafe {
            result = (*self.data.get())
                .get(&index)
                .expect("no entry found for key");
        }
        result
    }
}

impl<K: Hash + Eq, V> IndexMut<K> for AtomicMap<K, V> {
    fn index_mut(&mut self, index: K) -> &mut Self::Output {
        let result;
        unsafe {
            result = (*self.data.get())
                .get_mut(&index)
                .expect("no entry found for key");
        }
        result
    }
}

unsafe impl<T: Copy + Clone + Send + Sync> Send for AtomicVal<T> {}
unsafe impl<T: Copy + Clone + Send + Sync> Sync for AtomicVal<T> {}

pub struct AtomicVal<T: Copy + Clone + Send + Sync> {
    data: Cell<T>,
}

impl<T: Copy + Clone + Send + Sync> Clone for AtomicVal<T> {
    fn clone(&self) -> Self {
        AtomicVal {
            data: Cell::new(self.data.get()).clone(),
        }
    }
}

impl AtomicVal<f64> {
    pub fn new(current_best: f64) -> Self {
        AtomicVal {
            data: Cell::new(current_best),
        }
    }

    pub fn set(&self, val: f64) {
        self.data.set(val)
    }

    pub fn get(&self) -> f64 {
        self.data.get()
    }
}

#[derive(Clone)]
struct Level<LiquidityNode> {
    price: f64,
    deque: [LiquidityNode; 2],
}

#[derive(Copy, Clone, Debug)]
pub struct LiquidityNode {
    q: f64,
    l: u8,
}

impl Level<LiquidityNode> {
    fn new(price_level: f64) -> Self {
        Level {
            price: price_level,
            deque: [
                LiquidityNode { q: 0.0, l: 1 },
                LiquidityNode { q: 0.0, l: 2 },
            ],
        }
    }
}

#[derive(Clone)]
struct OrderBook {
    write_lock: Arc<AtomicBool>,
    read_lock: Arc<AtomicBool>,

    ring_buffer: RingBuffer,

    best_deal_asks_level_price: Arc<AtomicVal<f64>>,
    best_deal_asks_level_quantity: Arc<AtomicVal<f64>>,
    best_deal_asks_level_location: Arc<AtomicVal<f64>>,

    best_deal_bids_level_price: Arc<AtomicVal<f64>>,
    best_deal_bids_level_quantity: Arc<AtomicVal<f64>>,
    best_deal_bids_level_location: Arc<AtomicVal<f64>>,

    asks: Arc<AtomicMap<OrderedFloat<f64>, Level<LiquidityNode>>>,
    bids: Arc<AtomicMap<OrderedFloat<f64>, Level<LiquidityNode>>>,

    max_ask_level: f64,
    min_ask_level: f64,
    max_bid_level: f64,
    min_bid_level: f64,

    tick_size: f64,
    deal_producer: TokioSender<Deals>,
    send_deals: bool,
}

fn round(num: f64, place: f64) -> f64 {
    (num * place).round() / (place)
}

impl OrderBook {
    pub fn new(
        deal_producer: TokioSender<Deals>,
        config: OrderbookConfig,
    ) -> (OrderBook, Sender<DepthUpdate>) {
        let (asks, bids, max_ask_level, min_ask_level, max_bid_level, min_bid_level) =
            OrderBook::build_orderbook(
                config.tick_size,
                config.mid_price as f64,
                config.depth as f64,
            );
        let (ring_buffer, depth_producers) = RingBuffer::new(config.ring_buffer);
        let orderbook: OrderBook = OrderBook {
            write_lock: Arc::new(AtomicBool::new(false)),
            read_lock: Arc::new(AtomicBool::new(false)),

            ring_buffer,

            best_deal_asks_level_price: Arc::new(AtomicVal::new(f64::INFINITY)),
            best_deal_asks_level_quantity: Arc::new(AtomicVal::new(0.0)),
            best_deal_asks_level_location: Arc::new(AtomicVal::new(0.0)),

            best_deal_bids_level_price: Arc::new(AtomicVal::new(0.0)),
            best_deal_bids_level_quantity: Arc::new(AtomicVal::new(0.0)),
            best_deal_bids_level_location: Arc::new(AtomicVal::new(0.0)),

            asks: Arc::new(asks),
            bids: Arc::new(bids),

            max_ask_level,
            min_ask_level,
            max_bid_level,
            min_bid_level,

            tick_size: config.tick_size,

            deal_producer,
            send_deals: false,
        };
        (orderbook, depth_producers)
    }
    fn build_orderbook(
        tick_size: f64,
        mid_level: f64,
        depth: f64,
    ) -> (
        AtomicMap<OrderedFloat<f64>, Level<LiquidityNode>>,
        AtomicMap<OrderedFloat<f64>, Level<LiquidityNode>>,
        f64,
        f64,
        f64,
        f64,
    ) {
        // building ask side
        let mut asks: AtomicMap<OrderedFloat<f64>, Level<LiquidityNode>> = AtomicMap::new();
        let mut current_level = mid_level;
        let mut max_ask_level = 0.0;
        let mut min_ask_level = 0.0;
        for i in 0..=depth as u64 {
            if i == 0 {
                min_ask_level = current_level;
                let level = Level::new(current_level);
                asks.insert(OrderedFloat(round(current_level, 100.0)), level);
            } else {
                current_level = current_level + tick_size;
                let level = Level::new(current_level);
                asks.insert(OrderedFloat(round(current_level, 100.0)), level);
                if i == depth as u64 {
                    max_ask_level = round(current_level, 100.0);
                }
            }
        }

        // building bid side
        let mut bids: AtomicMap<OrderedFloat<f64>, Level<LiquidityNode>> = AtomicMap::new();
        current_level = mid_level;
        let mut max_bid_level = 0.0;
        let mut min_bid_level = 0.0;
        for i in 0..=depth as u64 {
            if i == 0 {
                max_bid_level = current_level;
                let level = Level::new(current_level);
                bids.insert(OrderedFloat(round(current_level, 100.0)), level);
            } else {
                current_level = current_level - tick_size;
                let level = Level::new(current_level);
                bids.insert(OrderedFloat(round(current_level, 100.0)), level);
                if i == depth as u64 {
                    min_bid_level = round(current_level, 100.0);
                }
            }
        }

        return (
            asks,
            bids,
            max_ask_level as f64,
            min_ask_level as f64,
            max_bid_level as f64,
            min_bid_level as f64,
        );
    }

    #[inline]
    pub fn consume_depths(&mut self) -> Result<(), ErrorHotPath> {
        let buffer_consume_result = self.ring_buffer.consume();
        match buffer_consume_result {
            Ok(()) => {
                return Ok(());
            }
            // TODO: Work out this buffer error
            Err(_) => return Err(ErrorHotPath::OrderBook("buffer error".to_string())),
        }
    }

    #[inline]
    // TODO: This wasn't made to be real time or performant. its just for TDD for now
    pub fn local_snapshot(
        &mut self,
        mid_point: f64,
        depth: f64,
    ) -> Result<(Vec<DepthUpdate>, Vec<DepthUpdate>), ErrorHotPath> {
        let mut asks: Vec<DepthUpdate> = vec![];
        let mut bids: Vec<DepthUpdate> = vec![];
        let mut depth_counter: f64 = depth;
        let mut current_level = mid_point;
        while depth_counter != 0.0 {
            if let Some(ask_level) = self.asks.get_mut(&OrderedFloat(current_level)) {
                debug!("ID 1 4 2 found snapshot level asks {:?}", asks);
                let mut vec: Vec<DepthUpdate> = ask_level
                    .deque
                    .as_mut()
                    .iter_mut()
                    .filter(|liquidity_node| liquidity_node.q != 0.0)
                    .map(|liquidity_node| DepthUpdate {
                        k: 0,
                        p: current_level,
                        q: liquidity_node.q,
                        l: liquidity_node.l,
                        s: true,
                    })
                    .collect();
                asks.append(&mut vec);
            }
            current_level = round(current_level + self.tick_size, 100.0);
            depth_counter -= 1.0;
        }
        current_level = mid_point;
        depth_counter = depth;
        while depth_counter != 0.0 {
            if let Some(bid_level) = self.bids.get_mut(&OrderedFloat(current_level)) {
                let mut vec: Vec<DepthUpdate> = bid_level
                    .deque
                    .as_mut()
                    .iter_mut()
                    .filter(|liquidity_node| liquidity_node.q != 0.0)
                    .map(|liquidity_node| DepthUpdate {
                        k: 1,
                        p: current_level,
                        q: liquidity_node.q,
                        l: liquidity_node.l,
                        s: true,
                    })
                    .collect();
                bids.append(&mut vec);
            }
            current_level = round(current_level - self.tick_size, 100.0);
            depth_counter -= 1.0;
        }
        Ok((asks, bids))
    }

    #[inline]
    pub fn process_all_depths(&mut self, ctx: &Context) -> Result<(), ErrorHotPath> {
        loop {
            if let Some(reason) = ctx.done() {
                debug!("context was triggered: {}", reason);
                return Err(ErrorHotPath::OrderBook(
                    "shutting down through context".to_string(),
                ));
            }

            while self.write_lock.load(Ordering::Relaxed) {
                let buffer_consume_result = self.ring_buffer.consume();
                if let Some(reason) = ctx.done() {
                    debug!("context was triggered: {}", reason);
                    return Err(ErrorHotPath::OrderBook(
                        "shutting down through context".to_string(),
                    ));
                }

                match buffer_consume_result {
                    Ok(()) => match self.ring_buffer.pop_depth() {
                        Some(depth_update) => {
                            if depth_update.s {
                                if let Err(update_book_err) =
                                    self.update_book_snapshots(depth_update)
                                {
                                    warn!("failed to update the book: {}", update_book_err)
                                }
                            } else {
                                self.write_lock.store(false, Ordering::SeqCst);
                                if let Err(update_book_err) = self.update_book_depths(depth_update)
                                {
                                    warn!("failed to update the book: {}", update_book_err)
                                }
                                if self.send_deals {
                                    self.read_lock.store(true, Ordering::SeqCst);
                                } else {
                                    thread::sleep(Duration::from_millis(1));
                                    self.write_lock.store(true, Ordering::SeqCst);
                                }
                                continue;
                            }
                        }
                        None => continue,
                    },
                    Err(_) => continue,
                }
            }
        }
    }

    #[inline]
    fn update_book_snapshots(&mut self, depth_update: DepthUpdate) -> Result<(), ErrorHotPath> {
        match depth_update.k {
            0 => {
                self.cache_best_deal_snapshot(depth_update);
                let update_result = self.ask_update(depth_update);
                match update_result {
                    Ok(_) => Ok(()),
                    Err(e) => return Err(e),
                }
            }
            1 => {
                self.cache_best_deal_snapshot(depth_update);
                let update_result = self.bid_update(depth_update);
                match update_result {
                    Ok(_) => Ok(()),
                    Err(e) => return Err(e),
                }
            }
            _ => {
                warn!("poor depth update direction received: {}", depth_update.k);
                return Err(ErrorHotPath::OrderBook(
                    "0 1 ASKS or 1 BIDS given by depth update".to_string(),
                ));
            }
        }
    }

    #[inline]
    fn cache_best_deal_snapshot(&mut self, depth_update: DepthUpdate) {
        match depth_update.k {
            0 => {
                if depth_update.p < self.best_deal_asks_level_price.get()
                    && depth_update.q > self.best_deal_asks_level_quantity.get()
                {
                    self.best_deal_asks_level_price.set(depth_update.p);
                    self.best_deal_asks_level_quantity.set(depth_update.q);
                    self.best_deal_asks_level_location
                        .set(depth_update.l as f64);
                    return;
                }

                if depth_update.p > self.best_deal_asks_level_price.get() {
                    return;
                }
            }
            1 => {
                if depth_update.p < self.best_deal_bids_level_price.get() {
                    return;
                }

                if depth_update.p > self.best_deal_bids_level_price.get()
                    && depth_update.q > self.best_deal_bids_level_quantity.get()
                {
                    self.best_deal_bids_level_price.set(depth_update.p);
                    self.best_deal_bids_level_quantity.set(depth_update.q);
                    self.best_deal_bids_level_location
                        .set(depth_update.l as f64);

                    return;
                }
            }
            _ => {
                warn!("invalid k type from depth_update")
            }
        }
    }

    #[inline]
    fn cache_best_deal_update(&mut self, depth_update: DepthUpdate) {
        match depth_update.k {
            0 => {
                if depth_update.p > self.best_deal_asks_level_price.get() {
                    return;
                }
                // incoming ask update is at our current known best deal but does it 0
                // out our liquidity?
                if depth_update.p == self.best_deal_asks_level_price.get()
                    && depth_update.p.is_sign_positive()
                {
                    return;
                }

                if depth_update.p < self.best_deal_asks_level_price.get()
                    && depth_update.q.is_sign_positive()
                {
                    self.best_deal_asks_level_price.set(depth_update.p);
                    return;
                }

                if depth_update.p < self.best_deal_asks_level_price.get()
                    && depth_update.q.is_sign_negative()
                    && depth_update.l as f64 == self.best_deal_asks_level_location.get()
                    && self.best_deal_asks_level_quantity.get() + depth_update.q < 0.0
                {
                    let mut first_liquid_node = true;
                    loop {
                        let mut current_level = depth_update.p;
                        let current_level_asks = self.asks.get(&OrderedFloat(depth_update.p));
                        if first_liquid_node {
                            let mut liquid_asks_levels = current_level_asks
                                .unwrap()
                                .deque
                                .into_iter()
                                .skip(1)
                                .filter(|&liquidity_node| liquidity_node.q != 0.0)
                                .peekable();
                            if liquid_asks_levels.peek().is_some() {
                                // we found a liquid nodes at the current price point.
                                // ignore the first node sense we already know it has 0 liquidity
                                let liquidity_node = liquid_asks_levels.next().unwrap();
                                self.best_deal_asks_level_price.set(depth_update.p);
                                self.best_deal_asks_level_quantity.set(liquidity_node.q);
                                self.best_deal_asks_level_location
                                    .set(liquidity_node.l as f64);
                                return;
                            } else {
                                first_liquid_node = false;
                                current_level = round(current_level + self.tick_size, 100.0);
                                continue;
                            }
                        } else {
                            // check other price levels as usual - not skipping the first node
                            let mut liquid_asks_levels = current_level_asks
                                .unwrap()
                                .deque
                                .into_iter()
                                .filter(|&liquidity_node| liquidity_node.q != 0.0)
                                .peekable();
                            if liquid_asks_levels.peek().is_some() {
                                let liquidity_node = liquid_asks_levels.next().unwrap();
                                self.best_deal_asks_level_price.set(depth_update.p);
                                self.best_deal_asks_level_quantity.set(liquidity_node.q);
                                self.best_deal_asks_level_location
                                    .set(liquidity_node.l as f64);
                                return;
                            } else {
                                current_level = round(current_level + self.tick_size, 100.0);
                                continue;
                            }
                        }
                    }
                }
            }
            1 => {
                if depth_update.p < self.best_deal_bids_level_price.get() {
                    return;
                }

                if depth_update.p == self.best_deal_bids_level_price.get()
                    && depth_update.p.is_sign_positive()
                {
                    return;
                }

                if depth_update.p > self.best_deal_bids_level_price.get()
                    && depth_update.q.is_sign_positive()
                {
                    self.best_deal_bids_level_price.set(depth_update.p);
                    return;
                }
                if depth_update.p > self.best_deal_asks_level_price.get()
                    && depth_update.q.is_sign_negative()
                    && depth_update.l as f64 == self.best_deal_asks_level_location.get()
                    && self.best_deal_bids_level_quantity.get() + depth_update.q < 0.0
                {
                    let mut first_liquid_node = true;
                    loop {
                        let mut current_level = depth_update.p;
                        let current_level_asks = self.asks.get(&OrderedFloat(depth_update.p));
                        if first_liquid_node {
                            // ignore the first node sense we already know it has 0 liquidity
                            // (skip)
                            let mut liquid_asks_levels = current_level_asks
                                .unwrap()
                                .deque
                                .into_iter()
                                .skip(1)
                                .filter(|&liquidity_node| liquidity_node.q != 0.0)
                                .peekable();
                            if liquid_asks_levels.peek().is_some() {
                                // we found a liquid nodes at the current price point.
                                let liquidity_node = liquid_asks_levels.next().unwrap();
                                self.best_deal_asks_level_price.set(depth_update.p);
                                self.best_deal_asks_level_quantity.set(liquidity_node.q);
                                self.best_deal_asks_level_location
                                    .set(liquidity_node.l as f64);
                                return;
                            } else {
                                first_liquid_node = false;
                                current_level = round(current_level - self.tick_size, 100.0);
                                continue;
                            }
                        } else {
                            // check other price levels as usual - not skipping the first node
                            let mut liquid_asks_levels = current_level_asks
                                .unwrap()
                                .deque
                                .into_iter()
                                .filter(|&liquidity_node| liquidity_node.q != 0.0)
                                .peekable();
                            if liquid_asks_levels.peek().is_some() {
                                let liquidity_node = liquid_asks_levels.next().unwrap();
                                self.best_deal_asks_level_price.set(depth_update.p);
                                self.best_deal_asks_level_quantity.set(liquidity_node.q);
                                self.best_deal_asks_level_location
                                    .set(liquidity_node.l as f64);
                                return;
                            } else {
                                current_level = round(current_level - self.tick_size, 100.0);
                                continue;
                            }
                        }
                    }
                }
            }
            _ => {
                warn!("invalid k type from depth_update")
            }
        }
    }

    #[inline]
    fn update_book_depths(&mut self, depth_update: DepthUpdate) -> Result<(), ErrorHotPath> {
        match depth_update.k {
            0 => {
                self.cache_best_deal_update(depth_update);
                let update_result = self.ask_update(depth_update);
                match update_result {
                    Ok(_) => Ok(()),
                    Err(e) => return Err(e),
                }
            }
            1 => {
                self.cache_best_deal_update(depth_update);
                let update_result = self.bid_update(depth_update);
                match update_result {
                    Ok(_) => return Ok(()),
                    Err(e) => return Err(e),
                }
            }
            _ => {
                warn!("poor depth update direction received: {}", depth_update.k);
                return Err(ErrorHotPath::OrderBook(
                    "0 1 ASKS or 1 BIDS given by depth update".to_string(),
                ));
            }
        }
    }

    #[inline]
    fn ask_update(&mut self, depth_update: DepthUpdate) -> Result<(), ErrorHotPath> {
        if let Some(asks) = self.asks.get_mut(&OrderedFloat(depth_update.p)) {
            asks.deque
                .iter_mut()
                .find(|liquidity_node| liquidity_node.l == depth_update.l)
                .map(|liquidity_node| {
                    let liquidity = liquidity_node.q + depth_update.q;
                    if liquidity < 0.0 {
                        return Err(ErrorHotPath::OrderBookNegativeLiquidity);
                    }
                    liquidity_node.q = liquidity;
                    Ok(())
                });
            asks.deque
                .sort_by(|prev, next| next.q.partial_cmp(&prev.q).unwrap());
        } else {
            return Err(ErrorHotPath::OrderBook("no level found".to_string()));
        }
        Ok(())
    }

    #[inline]
    fn bid_update(&mut self, depth_update: DepthUpdate) -> Result<(), ErrorHotPath> {
        if let Some(bids) = self.bids.get_mut(&OrderedFloat(depth_update.p)) {
            bids.deque
                .iter_mut()
                .find(|liquidity_node| liquidity_node.l == depth_update.l)
                .map(|liquidity_node| {
                    let liquidity = liquidity_node.q + depth_update.q;
                    if liquidity < 0.0 {
                        return Err(ErrorHotPath::OrderBookNegativeLiquidity);
                    }
                    liquidity_node.q = liquidity;
                    Ok(())
                });
            bids.deque
                .sort_by(|prev, next| next.q.partial_cmp(&prev.q).unwrap());
        } else {
            return Err(ErrorHotPath::OrderBook("no level found".to_string()));
        }
        Ok(())
    }

    #[inline]
    // TODO: Get rid of heap allocations here
    fn package_deals(&mut self, ctx: &Context) -> Result<Deals, ErrorHotPath> {
        loop {
            if let Some(reason) = ctx.done() {
                debug!("context was triggered: {}", reason);
                return Err(ErrorHotPath::OrderBook(
                    // TODO: This should not be an error
                    "shutting down through context".to_string(),
                ));
            }
            while self.read_lock.load(Ordering::Relaxed) {
                self.read_lock.store(false, Ordering::SeqCst);
                let mut traverse_result: Result<Deals, ErrorHotPath> = thread::scope(|s| {
                    let ask_traverser = s.spawn(|| self.traverse_asks());
                    let bid_traverser = s.spawn(|| self.traverse_bids());
                    let asks = ask_traverser.join();
                    let bids = bid_traverser.join();
                    Ok(Deals {
                        asks: asks.unwrap().unwrap(),
                        bids: bids.unwrap().unwrap(),
                    })
                });
                self.write_lock.store(true, Ordering::SeqCst);
                if self.send_deals {
                    let _ = self.deal_producer.try_send(traverse_result.unwrap());
                }
            }
        }
    }

    fn traverse_asks(&self) -> Result<[Deal; 10], ErrorHotPath> {
        let mut deals: [Deal; 10] = [Deal {
            p: 0.0,
            q: 0.0,
            l: 0,
        }; 10];
        let mut current_ask_deal_counter: usize = 0;
        let mut current_level: f64 = self.best_deal_asks_level_price.get();
        while current_ask_deal_counter < 10 {
            if current_level < self.max_ask_level + self.tick_size {
                let current_level_asks = self.asks.get(&OrderedFloat(current_level));
                if current_level_asks.is_none() {
                    current_level = round(current_level + self.tick_size, 100.0);
                    warn!("level {} does not exist in the ASKS book", current_level);
                    continue;
                }
                let mut liquid_asks_levels = current_level_asks
                    .unwrap()
                    .deque
                    .into_iter()
                    .filter(|&liquidity_node| liquidity_node.q != 0.0)
                    .peekable();
                if liquid_asks_levels.peek().is_none() {
                    current_level = round(current_level + self.tick_size, 100.0);
                    continue;
                }
                while let Some(ask) = liquid_asks_levels.next() {
                    deals[current_ask_deal_counter].p = current_level; // price level
                    deals[current_ask_deal_counter].l = ask.l; // the exchange id
                    deals[current_ask_deal_counter].q = ask.q; // quantity/volume/liquidity
                    current_ask_deal_counter += 1;
                    if current_ask_deal_counter == 10 {
                        break;
                    }
                }
                current_level = round(current_level + self.tick_size, 100.0);
                continue;
            } else {
                return Err(ErrorHotPath::OrderBookMaxTraversedReached);
            }
        }
        return Ok(deals);
    }

    #[inline]
    fn traverse_bids(&self) -> Result<[Deal; 10], ErrorHotPath> {
        let mut deals: [Deal; 10] = [Deal {
            p: 0.0,
            q: 0.0,
            l: 0,
        }; 10];
        let mut current_bid_deal_counter: usize = 0;
        let mut current_level: f64 = self.best_deal_bids_level_price.get();
        while current_bid_deal_counter < 10 {
            if !(current_level < self.min_bid_level - self.tick_size) {
                let current_level_bids = self.bids.get(&OrderedFloat(current_level));
                if current_level_bids.is_none() {
                    warn!("level {} does not exist in the BIDS book", current_level);
                    current_level = round(current_level - self.tick_size, 100.0);
                    continue;
                }
                let mut liquid_bids_levels = current_level_bids
                    .unwrap()
                    .deque
                    .into_iter()
                    .filter(|&liquidity_node| liquidity_node.q != 0.0)
                    .peekable();
                if liquid_bids_levels.peek().is_none() {
                    current_level = round(current_level - self.tick_size, 100.0);
                    continue;
                }
                while let Some(bid) = liquid_bids_levels.next() {
                    deals[current_bid_deal_counter].p = current_level; // price level
                    deals[current_bid_deal_counter].l = bid.l; // the exchange id
                    deals[current_bid_deal_counter].q = bid.q; // quantity/volume/liquidity
                    current_bid_deal_counter += 1;
                    if current_bid_deal_counter == 10 {
                        break;
                    }
                }
                current_level = round(current_level - self.tick_size, 100.0);
                continue;
            } else {
                return Err(ErrorHotPath::OrderBookMaxTraversedReached);
            }
        }
        return Ok(deals);
    }

    #[inline]
    fn send_deals(&self, deals: Deals) -> Result<(), ErrorHotPath> {
        while let Err(send_error) = self.deal_producer.try_send(deals) {
            warn!("failed to send deal {:?}", send_error);
            continue;
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(unused_variables)]
mod tests {
    use super::*;
    use config::RingBufferConfig;
    use crossbeam_channel::{unbounded, Sender};
    use depth_generator::DepthMessageGenerator;
    use itertools::interleave;
    use std::thread;
    use test_log::test;
    use testing_traits::ConsumerDefault;
    use tokio::sync::mpsc::{channel as asyncChannel, Sender as asyncProducer};
    use tokio::time::Duration;
    use tracing::info;
    use tracing_test::traced_test;

    impl<'a> ConsumerDefault<'a, OrderBook, DepthUpdate> for OrderBook {
        fn consumer_default() -> (Box<Self>, Sender<DepthUpdate>) {
            let (depth_trigger, trigger_receiver) = watchChannel(());
            let tick_size: f64 = 0.01;
            let mid_level: f64 = 2700.0;
            let depth: f64 = 5000.0;
            let (asks, bids, max_ask_level, min_ask_level, max_bid_level, min_bid_level) =
                OrderBook::build_orderbook(tick_size, mid_level, depth);
            let (ring_buffer, depth_producer) = RingBuffer::new(RingBufferConfig {
                ring_buffer_size: 3000000,
                channel_buffer_size: 30000000, // NOTE BACK PRESSURE HERE IS SIGNIFICANT
            });
            let (deal_producer, _) = asyncChannel(10);
            let orderbook = OrderBook {
                write_lock: Arc::new(AtomicBool::new(true)),
                read_lock: Arc::new(AtomicBool::new(false)),
                ring_buffer,

                best_deal_asks_level_price: Arc::new(AtomicVal::new(f64::INFINITY)),
                best_deal_asks_level_quantity: Arc::new(AtomicVal::new(0.0)),
                best_deal_asks_level_location: Arc::new(AtomicVal::new(0.0)),

                best_deal_bids_level_price: Arc::new(AtomicVal::new(0.0)),
                best_deal_bids_level_quantity: Arc::new(AtomicVal::new(0.0)),
                best_deal_bids_level_location: Arc::new(AtomicVal::new(0.0)),

                asks: Arc::new(asks),
                bids: Arc::new(bids),

                max_ask_level,
                min_ask_level,
                max_bid_level,
                min_bid_level,

                tick_size: 0.01,

                deal_producer,
                send_deals: false,
            };
            (Box::new(orderbook), depth_producer)
        }
    }

    #[test]
    fn test_cross_thread_boundary_hashmap() {
        let map: Arc<AtomicMap<OrderedFloat<f64>, Level<LiquidityNode>>> =
            Arc::new(AtomicMap::new());
        let map_clone = map.clone();
        thread::spawn(move || {
            map_clone.insert(OrderedFloat(10.0), Level::new(10.0));
        });
        let map_clone = map.clone();
        let t2 = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            map_clone.insert(OrderedFloat(10.0), Level::new(11.0));
        });
        let _ = t2.join();
        let val = map.get(&OrderedFloat(10.0));

        // test that an inner hashmap value has changed
        assert!(val.unwrap().price == 11.0)
    }

    #[traced_test]
    #[test]
    fn test_build_orderbook() {
        let tick_size = 0.01;
        let mid_level = 2700.0;
        let depth = 5000.0;
        // 1. test that orderbook was built with appropriate price levels given the tick size, mid
        //    level, and depth
        let (mut asks, mut bids, max_ask_level, min_ask_level, max_bid_level, min_bid_level) =
            OrderBook::build_orderbook(tick_size, mid_level, depth);
        info!("max ask: {}", max_ask_level);
        info!("min ask: {}", min_ask_level);
        info!("max bid: {}", max_bid_level);
        info!("min bid: {}", min_bid_level);
        assert!(max_ask_level == 2750.0);
        assert!(min_ask_level == 2700.0);
        assert!(max_bid_level == 2700.0);
        assert!(min_bid_level == 2650.0);
        // 2. test that each level of the order book has initial incremental exchange locations
        let mut current_level = mid_level;
        for i in 0..=depth as u64 {
            let results: Vec<bool> = asks
                .get_mut(&OrderedFloat(current_level))
                .unwrap()
                .deque
                .into_iter()
                .scan(1, |exchange_id, liquidity_node| {
                    if liquidity_node.l != *exchange_id {
                        *exchange_id = *exchange_id + 1;
                        return Some(false);
                    } else {
                        *exchange_id = *exchange_id + 1;
                        return Some(true);
                    }
                })
                .collect();
            for result in results {
                assert!(result == true)
            }
            current_level = round(current_level + tick_size, 100.0);
        }
        current_level = mid_level;
        for i in 0..=depth as u64 {
            let results: Vec<bool> = bids
                .get_mut(&OrderedFloat(min_bid_level))
                .unwrap()
                .deque
                .into_iter()
                .scan(1, |exchange_id, liquidity_node| {
                    if liquidity_node.l != *exchange_id {
                        info!("liquidity node is {}", liquidity_node.l);
                        *exchange_id = *exchange_id + 1;
                        return Some(false);
                    } else {
                        *exchange_id = *exchange_id + 1;
                        return Some(true);
                    }
                })
                .collect();
            for result in results {
                assert!(result == true)
            }
            current_level = round(current_level - tick_size, 100.0);
        }
    }

    #[traced_test]
    #[test]
    fn test_sorted_by_greatest_volume_bids_and_asks() {
        // test that we correctly sort each price level's liquidity nodes by decaying volume
        let (mut orderbook, _) = OrderBook::consumer_default();

        let price_level = 2690.63;
        let _ = orderbook.bid_update(DepthUpdate {
            k: 0,
            p: price_level,
            q: 30.0,
            l: 1,
            s: false,
        });
        let _ = orderbook.bid_update(DepthUpdate {
            k: 0,
            p: price_level,
            q: 40.0,
            l: 2,
            s: false,
        });
        let level = orderbook.bids.get_mut(&OrderedFloat(price_level)).unwrap();
        let decaying_volumes_check = level.deque.iter().scan(100.0, |past_value, deal| {
            let result = *past_value < deal.q;
            if !result {
                *past_value = deal.q;
            }
            info!(past_value);
            Some(result)
        });
        for boolean in decaying_volumes_check {
            assert!(boolean == false)
        }
        let (mut orderbook, _) = OrderBook::consumer_default();
        let _ = orderbook.bid_update(DepthUpdate {
            k: 0,
            p: price_level,
            q: 77.0,
            l: 1,
            s: false,
        });
        let _ = orderbook.bid_update(DepthUpdate {
            k: 0,
            p: price_level,
            q: 134.0,
            l: 2,
            s: false,
        });
        let decaying_volumes_check = level.deque.iter().scan(100.0, |past_value, deal| {
            let result = *past_value < deal.q;
            if !result {
                *past_value = deal.q;
            }
            info!(past_value);
            Some(result)
        });
        for boolean in decaying_volumes_check {
            debug!(boolean);
            assert!(boolean == false)
        }
    }

    #[traced_test]
    #[test]
    #[rustfmt::skip]
    fn test_traverse_asks() {
        info!("testing traverse asks");
        let (mut orderbook, _) = OrderBook::consumer_default();
        let best_deal = 2700.00;
        let _ = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: best_deal,
            q: 40.0,
            l: 1,
            s: false,
        });
        let _ = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2700.63, // skip some levels
            q: 1.0,
            l: 2,
            s: false,
        });
        let _ = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2700.63,
            q: 77.0,
            l: 1, // add liquidty at another location (exchange)
            s: false,
        });
        let _ = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2700.64,
            q: 13.0,
            l: 1,
            s: false,
        });
        let _ = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2705.65,
            q: 25.0,
            l: 2,
            s: false,
        });
        let _ = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2705.66,
            q: 34.0,
            l: 1,
            s: false,
        });
        let _ = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2705.67,
            q: 23.0,
            l: 1,
            s: false,
        });
        let _ = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2705.68,
            q: 99.0,
            l: 1,
            s: false,
        });
        let _ = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2705.69,
            q: 70.0,
            l: 2,
            s: false,
        });
        let _ = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2709.70,
            q: 47.0,
            l: 1,
            s: false,
        });
        let _ = orderbook.ask_update(DepthUpdate {
            k: 0,
            p: 2709.71, // TODO: BUG this value returns a 2709.7
            q: 23.0,
            l: 1,
            s: false,
        });

        let expected_deals: [Deal; 10] = [
            Deal { p: 2700.00, q: 40.0, l: 1 },
            Deal { p: 2700.63, q: 77.0, l: 1 },
            Deal { p: 2700.63, q: 1.0, l: 2 },
            Deal { p: 2700.64, q: 13.0, l: 1 },
            Deal { p: 2705.65, q: 25.0, l: 2 },
            Deal { p: 2705.66, q: 34.0, l: 1 },
            Deal { p: 2705.67, q: 23.0, l: 1 },
            Deal { p: 2705.68, q: 99.0, l: 1 },
            Deal { p: 2705.69, q: 70.0, l: 2 },
            Deal { p: 2709.70, q: 47.0, l: 1 },
        ];
        
        // (1) test that our traverse asks return deals with no errors
        orderbook.best_deal_asks_level_price.set(best_deal);
        orderbook.tick_size = 0.01;
        let actual_deals = orderbook.traverse_asks().unwrap();
        let result = actual_deals
            .into_iter()
            .zip(expected_deals)
            .all(|(actual_deal, expected_deal)| actual_deal == expected_deal);
        assert!(result == true);
    }

    #[test]
    #[traced_test]
    #[rustfmt::skip]
    fn test_traverse_bids() {
        info!("testing traverse bids");
        let (mut orderbook, _) = OrderBook::consumer_default();
        let best_deal = 2700.00;
        let _ = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: best_deal,
            q: 30.0,
            l: 1,
            s: false,
        });
        let _ = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2699.63, // skip some levels
            q: 77.0,
            l: 2,
            s: false,
        });
        let _ = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2699.63,
            q: 50.0,
            l: 1, // add liquidty at another location (exchange)
            s: false,
        });
        let _ = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2690.33,
            q: 13.09,
            l: 1,
            s: false,
        });
        let _ = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2689.39,
            q: 25.22,
            l: 2,
            s: false,
        });
        let _ = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2687.20,
            q: 34.88,
            l: 1,
            s: false,
        });
        let _ = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2685.20,
            q: 23.0,
            l: 1,
            s: false,
        });
        let _ = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2684.33,
            q: 99.1,
            l: 1,
            s: false,
        });
        let _ = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2682.99,
            q: 70.22,
            l: 1,
            s: false,
        });
        let _ = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2680.33,
            q: 47.20,
            l: 1,
            s: false,
        });
        let best_deal = 2700.00;
        let _ = orderbook.bid_update(DepthUpdate {
            k: 1,
            p: 2679.22,
            q: 23.33,
            l: 1,
            s: false,
        });

        let expected_deals: [Deal; 10] = [
            Deal { p: 2700.00, q: 30.0, l: 1 },
            Deal { p: 2699.63, q: 77.0, l: 2 },
            Deal { p: 2699.63, q: 50.0, l: 1 },
            Deal { p: 2690.33, q: 13.09, l: 1 },
            Deal { p: 2689.39, q: 25.22, l: 2 },
            Deal { p: 2687.20, q: 34.88, l: 1 },
            Deal { p: 2685.20, q: 23.0, l: 1 },
            Deal { p: 2684.33, q: 99.1, l: 1 },
            Deal { p: 2682.99, q: 70.22, l: 1 },
            Deal { p: 2680.33, q: 47.20, l: 1 },
        ];

        orderbook.best_deal_bids_level_price.set(2700.0);
        let actual_deals = orderbook.traverse_bids().unwrap();

        let result = actual_deals
            .into_iter()
            .zip(expected_deals)
            .all(|(actual_deal, expected_deal)| actual_deal == expected_deal);
        assert!(result == true);
    }

    // DepthMachine provides us with tooling to inject depths into the orderbook its currently used
    // for three tests
    //
    // 1. test_orderbook_snapshot_updates
    // 2. test_orderbook_depth_updates
    // 3. test_orderbook_deal_output
    //
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

    use io_context::Context;
    #[tokio::test]
    #[traced_test]
    async fn test_orderbook_snapshot_updates() {
        let test_length_seconds = 10;
        let (depth_trigger, trigger_receiver) = watchChannel(());
        let (mut orderbook, depth_producer) = OrderBook::consumer_default();
        let tick_size = 0.01;
        let mid_point = 2700.0;
        let depth = 300.0;
        let mut dmg = DepthMessageGenerator::default();
        let mid_point = mid_point;
        let exchanges = 2;
        dmg.tick_size = tick_size;
        dmg.price = mid_point as f64;
        dmg.price_std = 3.0;
        let mut depth_machine = DepthMachine {
            trigger_snapshot: trigger_receiver,
            depth_producer,
            dmg,
            snapshot: None,
            sequence: false,
            depth: depth as usize,
            mid_point: mid_point as usize,
            exchanges,
        };
        let expected_snapshot = depth_machine.build_snapshot();
        let mut ctx = Context::background();
        let cancel_ctx = ctx.add_cancel_signal();
        let parent_ctx = ctx.freeze();
        let process_depths_ctx = Context::create_child(&parent_ctx);
        let (tx, rx_depth) = unbounded::<(Vec<DepthUpdate>, Vec<DepthUpdate>)>();

        thread::spawn(move || {
            //  wait for snapshot depths to be produced upstream by task on L1268
            thread::sleep(Duration::from_millis(100));
            orderbook.process_all_depths(&process_depths_ctx);
            debug!("process depths has ended through context's done");
            let snapshot = orderbook.local_snapshot(mid_point as f64, depth as f64); // grab the book between
                                                                                     // [2650, 2750]
                                                                                     //
            _ = tx.send(snapshot.unwrap());
            return;
        });

        let depth_machine_task = tokio::spawn(async move {
            depth_trigger.send(());
            let result = depth_machine.produce_snapshot_depths().await;
        });
        _ = depth_machine_task.await;
        cancel_ctx.cancel();
        tokio::time::sleep(Duration::from_secs(test_length_seconds)).await;
        if let Ok(actual_snapshot) = rx_depth.recv() {
            // (1) first test that our snapshots are equal size on both the ask and bid sides
            assert!(actual_snapshot.0.len() == expected_snapshot.0.len());
            assert!(actual_snapshot.1.len() == expected_snapshot.1.len());

            // (2) squeeze our books to get the actual and expected realized usd liquidities on
            // both sides
            let ask_liquidity_usd_actual: f64 = actual_snapshot
                .0
                .iter()
                .map(|depth_update| depth_update.p * depth_update.q)
                .sum();
            let ask_liquidity_usd_expected: f64 = actual_snapshot
                .0
                .iter()
                .map(|depth_update| depth_update.p * depth_update.q)
                .sum();
            let bid_liquidity_usd_actual: f64 = actual_snapshot
                .1
                .iter()
                .map(|depth_update| depth_update.p * depth_update.q)
                .sum();
            let bid_liquidity_usd_expected: f64 = actual_snapshot
                .1
                .iter()
                .map(|depth_update| depth_update.p * depth_update.q)
                .sum();
            assert!(ask_liquidity_usd_expected == ask_liquidity_usd_actual);
            assert!(bid_liquidity_usd_expected == bid_liquidity_usd_actual)
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn test_orderbook_depth_updates() {
        let test_length_seconds = 30;
        let (depth_trigger, trigger_receiver) = watchChannel(());
        let (mut orderbook, depth_producer) = OrderBook::consumer_default();
        orderbook.send_deals = false;

        let tick_size = 0.01;
        let mid_point = 2700.0;
        let depth = 150.0;
        let mut dmg = DepthMessageGenerator::default();
        let mid_point = mid_point;
        let exchanges = 2;
        dmg.tick_size = tick_size;
        dmg.price = mid_point as f64;
        dmg.price_std = 3.0;
        let mut depth_machine = DepthMachine {
            trigger_snapshot: trigger_receiver,
            depth_producer,
            dmg,
            snapshot: None,
            sequence: false,
            depth: depth as usize,
            mid_point: mid_point as usize,
            exchanges,
        };
        let snapshot = depth_machine.build_snapshot();
        let mut expected_snapshot = snapshot.clone();

        let mut ctx = Context::background();
        let cancel_ctx = ctx.add_cancel_signal();
        let parent_ctx = ctx.freeze();
        let process_depths_ctx = Context::create_child(&parent_ctx);

        let (tx, rx_depth) = unbounded::<(Vec<DepthUpdate>, Vec<DepthUpdate>)>();
        thread::spawn(move || {
            //  wait for snapshot depths to be produced upstream by task on L1268
            thread::sleep(Duration::from_millis(100));
            _ = orderbook.process_all_depths(&process_depths_ctx);
            debug!("process depths has ended through context's done"); // [2650, 2750]
            let snapshot = orderbook.local_snapshot(mid_point as f64, depth as f64); // grab the book between
                                                                                     // [2650, 2750]
                                                                                     //
            _ = tx.send(snapshot.unwrap());
        });

        let depth_machine_task = tokio::spawn(async move {
            let send_result = depth_trigger.send(());
            let _ = depth_machine.produce_snapshot_depths().await;
            tokio::time::sleep(Duration::from_millis(20)).await;
            let continuity_load = depth_machine
                .dmg
                .depth_market_continuity_from_existing_book(snapshot);
            tokio::time::sleep(Duration::from_secs(5)).await;
            let _ = depth_machine.push_depth_load(continuity_load).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
            return;
        });

        let _ = depth_machine_task.await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        cancel_ctx.cancel();
        tokio::time::sleep(Duration::from_secs(test_length_seconds)).await;

        // test that our orderbooks liquidities got cut in half by our continuity depth load
        if let Ok(mut actual_snapshot) = rx_depth.recv() {
            actual_snapshot
                .0
                .sort_by(|a, b| a.p.partial_cmp(&b.p).unwrap());
            expected_snapshot
                .0
                .sort_by(|a, b| a.p.partial_cmp(&b.p).unwrap());
            actual_snapshot
                .0
                .sort_by(|a, b| a.l.partial_cmp(&b.l).unwrap());
            expected_snapshot
                .0
                .sort_by(|a, b| a.l.partial_cmp(&b.l).unwrap());
            let depth_change_result =
                actual_snapshot.0.into_iter().zip(expected_snapshot.0).all(
                    |(updated_depth, original_depth)| updated_depth.q * 2.0 == original_depth.q,
                );
            assert!(depth_change_result == true);

            actual_snapshot
                .1
                .sort_by(|a, b| a.p.partial_cmp(&b.p).unwrap());
            expected_snapshot
                .1
                .sort_by(|a, b| a.p.partial_cmp(&b.p).unwrap());
            actual_snapshot
                .1
                .sort_by(|a, b| a.l.partial_cmp(&b.l).unwrap());
            expected_snapshot
                .1
                .sort_by(|a, b| a.l.partial_cmp(&b.l).unwrap());
            let depth_change_result =
                actual_snapshot.1.into_iter().zip(expected_snapshot.1).all(
                    |(updated_depth, original_depth)| updated_depth.q * 2.0 == original_depth.q,
                );
        }
    }

    #[tokio::test]
    #[traced_test]
    #[rustfmt::skip]
    async fn test_orderbook_deal_output() {
        let test_length_seconds = 30;
        let (depth_trigger, trigger_receiver) = watchChannel(());
        let (deal_producer, mut deal_consumer) = asyncChannel::<Deals>(100);
        let (mut orderbook, depth_producer) = OrderBook::consumer_default();
        let tick_size = 0.01;
        let mid_point = 2700.0;
        let depth = 150.0;
        orderbook.write_lock = Arc::new(AtomicBool::new(true));
        orderbook.read_lock = Arc::new(AtomicBool::new(false));
        orderbook.deal_producer = deal_producer;
        orderbook.send_deals = true;

        let mut dmg = DepthMessageGenerator::default();
        let mid_point = mid_point;
        let exchanges = 2;
        dmg.tick_size = tick_size;
        dmg.price = mid_point as f64;
        dmg.price_std = 3.0;
        let mut depth_machine = DepthMachine {
            trigger_snapshot: trigger_receiver,
            depth_producer,
            dmg,
            snapshot: None,
            sequence: false,
            depth: depth as usize,
            mid_point: mid_point as usize,
            exchanges,
        };
        let snapshot = depth_machine.build_snapshot();

        let mut ctx = Context::background();
        let cancel_ctx = ctx.add_cancel_signal();
        let parent_ctx = ctx.freeze();
        let process_depths_ctx = Context::create_child(&parent_ctx);
        let package_deals_ctx = Context::create_child(&parent_ctx);

        let mut orderbook_clone = orderbook.clone();
        thread::spawn(move || {
            //  wait for snapshot depths to be produced upstream by task on L1268
            thread::sleep(Duration::from_millis(100));
            _ = orderbook_clone.process_all_depths(&process_depths_ctx);
            debug!("process depths has ended through context's done"); // [2650, 2750]
                                                                       // _ = tx.send(snapshot.unwrap());
        });

        let mut orderbook_clone = orderbook.clone();
        thread::spawn(move || {
            //  wait for snapshot depths to be produced upstream by task on L1268
            thread::sleep(Duration::from_millis(100));
            _ = orderbook_clone.package_deals(&package_deals_ctx);
            debug!("process depths has ended through context's done");
        });
        
        let injected_depth_snapshots_asks: Vec<DepthUpdate> = vec![
            DepthUpdate { k: 0, p: 2700.01, q: 42.33, l: 1, s: true },
            DepthUpdate { k: 0, p: 2700.01, q: 30.29, l: 2, s: true },
            DepthUpdate { k: 0, p: 2700.02, q: 1.0, l: 1, s: true },
            DepthUpdate { k: 0, p: 2700.02, q: 1.0, l: 2, s: true },
            DepthUpdate { k: 0, p: 2700.03, q: 1.0, l: 1, s: true },
            DepthUpdate { k: 0, p: 2700.03, q: 1.0, l: 2, s: true },
            DepthUpdate { k: 0, p: 2700.04, q: 1.0, l: 1, s: true },
            DepthUpdate { k: 0, p: 2700.04, q: 1.0, l: 2, s: true },
            DepthUpdate { k: 0, p: 2700.05, q: 1.0, l: 1, s: true },
            DepthUpdate { k: 0, p: 2700.05, q: 1.0, l: 2, s: true },
            DepthUpdate { k: 0, p: 2700.06, q: 1.0, l: 1, s: true },
            DepthUpdate { k: 0, p: 2700.06, q: 1.0, l: 2, s: true },
            DepthUpdate { k: 0, p: 2700.07, q: 1.0, l: 1, s: true },
            DepthUpdate { k: 0, p: 2700.07, q: 1.0, l: 2, s: true },
            DepthUpdate { k: 0, p: 2700.08, q: 1.0, l: 1, s: true },
        ];

        let injected_depth_snapshots_bids: Vec<DepthUpdate> = vec![
            DepthUpdate { k: 1, p: 2699.99, q: 303.02, l: 1, s: true },
            DepthUpdate { k: 1, p: 2699.99, q: 144.33, l: 2, s: true },
            DepthUpdate { k: 1, p: 2699.98, q: 1.0, l: 1, s: true },
            DepthUpdate { k: 1, p: 2699.98, q: 1.0, l: 2, s: true },
            DepthUpdate { k: 1, p: 2699.97, q: 1.0, l: 1, s: true },
            DepthUpdate { k: 1, p: 2699.97, q: 1.0, l: 2, s: true },
            DepthUpdate { k: 1, p: 2699.96, q: 1.0, l: 1, s: true },
            DepthUpdate { k: 1, p: 2699.96, q: 1.0, l: 2, s: true },
            DepthUpdate { k: 1, p: 2699.95, q: 1.0, l: 1, s: true },
            DepthUpdate { k: 1, p: 2699.95, q: 1.0, l: 2, s: true },
            DepthUpdate { k: 1, p: 2699.94, q: 1.0, l: 1, s: true },
            DepthUpdate { k: 1, p: 2699.94, q: 1.0, l: 2, s: true },
            DepthUpdate { k: 1, p: 2699.93, q: 1.0, l: 1, s: true },
            DepthUpdate { k: 1, p: 2699.93, q: 1.0, l: 2, s: true },
            DepthUpdate { k: 1, p: 2699.92, q: 1.0, l: 1, s: true },
        ];

        let prices = [2700.01, 2700.01, 2700.02, 2700.02, 2700.03];
        let locations = [1, 2, 1, 2, 1];

        // e1: take liquidity at l1
        // e2: take liqudiity at l2 - zeroing out this liquidity level
        // e3: add liquidity at 2700.00 - leaving jump in the orderbook at 2700.01 between 2700.00
        // and 2700.02
        // e4: add liquidity to end of the best 10 deals leaving the quote unchanged
        
        // start producing deals through updates.  every update produces a deal
        let injected_depth_updates_asks: Vec<DepthUpdate> = vec![
            DepthUpdate { k: 0, p: 2700.01, q: -42.33, l: 1, s: false}, 
            DepthUpdate { k: 0, p: 2700.01, q: -30.29, l: 2, s: false},         
            DepthUpdate { k: 0, p: 2700.00, q: 99.99, l: 2, s: false},         
            DepthUpdate { k: 0, p: 2700.07, q: 10.304, l: 2, s: false},         
            DepthUpdate { k: 1, p: 2699.99, q: -303.02, l: 1, s: false},         
            DepthUpdate { k: 1, p: 2699.99, q: -144.33, l: 2, s: false},         
            DepthUpdate { k: 1, p: 2700.00, q: 788.0, l: 2, s: false},         
            DepthUpdate { k: 1, p: 2690.01, q: 233.0, l: 2, s: false},         
        ];

        let snapshots: (Vec<DepthUpdate>, Vec<DepthUpdate>) = (injected_depth_snapshots_asks, injected_depth_snapshots_bids);
        let updates: (Vec<DepthUpdate>, Vec<DepthUpdate>) = (injected_depth_updates_asks, vec![]);

        let depth_machine_task = tokio::spawn(async move {
            debug!("pushing depth snapshots");
            let _ = depth_machine.push_depth_load(snapshots).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
            debug!("pushing depth updates");
            let _ = depth_machine.push_depth_load(updates).await;
            return;
        });

        let _ = depth_machine_task.await;
        tokio::time::sleep(Duration::from_secs(10)).await;
        cancel_ctx.cancel();
        
        let e1_deals_expected = Deals { 
            asks: [
                Deal { p: 2700.01, q: 30.29, l: 2 },
                Deal { p: 2700.02, q: 1.0, l: 1 },
                Deal { p: 2700.02, q: 1.0, l: 2 },
                Deal { p: 2700.03, q: 1.0, l: 1 },
                Deal { p: 2700.03, q: 1.0, l: 2 },
                Deal { p: 2700.04, q: 1.0, l: 1 },
                Deal { p: 2700.04, q: 1.0, l: 2 },
                Deal { p: 2700.05, q: 1.0, l: 1 },
                Deal { p: 2700.05, q: 1.0, l: 2 },
                Deal { p: 2700.06, q: 1.0, l: 1 },
            ],
            bids: [
                Deal { p: 2699.99, q: 303.02, l: 1 },
                Deal { p: 2699.99, q: 144.33, l: 2 },
                Deal { p: 2699.98, q: 1.0, l: 1 },
                Deal { p: 2699.98, q: 1.0, l: 2 },
                Deal { p: 2699.97, q: 1.0, l: 1 },
                Deal { p: 2699.97, q: 1.0, l: 2 },
                Deal { p: 2699.96, q: 1.0, l: 1 },
                Deal { p: 2699.96, q: 1.0, l: 2 },
                Deal { p: 2699.95, q: 1.0, l: 1 },
                Deal { p: 2699.95, q: 1.0, l: 2 },
            ],
        };
        
        let e2_deals_expected = Deals {
        asks: [
            Deal { p: 2700.02, q: 1.0, l: 1 },
            Deal { p: 2700.02, q: 1.0, l: 2 },
            Deal { p: 2700.03, q: 1.0, l: 1 },
            Deal { p: 2700.03, q: 1.0, l: 2 },
            Deal { p: 2700.04, q: 1.0, l: 1 },
            Deal { p: 2700.04, q: 1.0, l: 2 },
            Deal { p: 2700.05, q: 1.0, l: 1 },
            Deal { p: 2700.05, q: 1.0, l: 2 },
            Deal { p: 2700.06, q: 1.0, l: 1 },
            Deal { p: 2700.06, q: 1.0, l: 2 },
        ],
        bids: [
            Deal { p: 2699.99, q: 303.02, l: 1 },
            Deal { p: 2699.99, q: 144.33, l: 2 },
            Deal { p: 2699.98, q: 1.0, l: 1 },
            Deal { p: 2699.98, q: 1.0, l: 2 },
            Deal { p: 2699.97, q: 1.0, l: 1 },
            Deal { p: 2699.97, q: 1.0, l: 2 },
            Deal { p: 2699.96, q: 1.0, l: 1 },
            Deal { p: 2699.96, q: 1.0, l: 2 },
            Deal { p: 2699.95, q: 1.0, l: 1 },
            Deal { p: 2699.95, q: 1.0, l: 2 },
        ],
    };

    let e3_deals_expected = Deals {
        asks: [
            Deal { p: 2700.00, q: 99.99, l: 2 },
            Deal { p: 2700.02, q: 1.0, l: 1 },
            Deal { p: 2700.02, q: 1.0, l: 2 },
            Deal { p: 2700.03, q: 1.0, l: 1 },
            Deal { p: 2700.03, q: 1.0, l: 2 },
            Deal { p: 2700.04, q: 1.0, l: 1 },
            Deal { p: 2700.04, q: 1.0, l: 2 },
            Deal { p: 2700.05, q: 1.0, l: 1 },
            Deal { p: 2700.05, q: 1.0, l: 2 },
            Deal { p: 2700.06, q: 1.0, l: 1 },
        ],
        bids: [
            Deal { p: 2699.99, q: 303.02, l: 1 },
            Deal { p: 2699.99, q: 144.33, l: 2 },
            Deal { p: 2699.98, q: 1.0, l: 1 },
            Deal { p: 2699.98, q: 1.0, l: 2 },
            Deal { p: 2699.97, q: 1.0, l: 1 },
            Deal { p: 2699.97, q: 1.0, l: 2 },
            Deal { p: 2699.96, q: 1.0, l: 1 },
            Deal { p: 2699.96, q: 1.0, l: 2 },
            Deal { p: 2699.95, q: 1.0, l: 1 },
            Deal { p: 2699.95, q: 1.0, l: 2 },
        ],
    };

    
    let e4_deals_expected = Deals {
        asks: [
            Deal { p: 2700.00, q: 99.99, l: 2 },
            Deal { p: 2700.02, q: 1.0, l: 1 },
            Deal { p: 2700.02, q: 1.0, l: 2 },
            Deal { p: 2700.03, q: 1.0, l: 1 },
            Deal { p: 2700.03, q: 1.0, l: 2 },
            Deal { p: 2700.04, q: 1.0, l: 1 },
            Deal { p: 2700.04, q: 1.0, l: 2 },
            Deal { p: 2700.05, q: 1.0, l: 1 },
            Deal { p: 2700.05, q: 1.0, l: 2 },
            Deal { p: 2700.06, q: 1.0, l: 1 },
        ],
        bids: [
            Deal { p: 2699.99, q: 303.02, l: 1 },
            Deal { p: 2699.99, q: 144.33, l: 2 },
            Deal { p: 2699.98, q: 1.0, l: 1 },
            Deal { p: 2699.98, q: 1.0, l: 2 },
            Deal { p: 2699.97, q: 1.0, l: 1 },
            Deal { p: 2699.97, q: 1.0, l: 2 },
            Deal { p: 2699.96, q: 1.0, l: 1 },
            Deal { p: 2699.96, q: 1.0, l: 2 },
            Deal { p: 2699.95, q: 1.0, l: 1 },
            Deal { p: 2699.95, q: 1.0, l: 2 },
        ],
    };

    let e5_deals_expected = Deals {
        asks: [
            Deal { p: 2700.00, q: 99.99, l: 2 },
            Deal { p: 2700.02, q: 1.0, l: 1 },
            Deal { p: 2700.02, q: 1.0, l: 2 },
            Deal { p: 2700.03, q: 1.0, l: 1 },
            Deal { p: 2700.03, q: 1.0, l: 2 },
            Deal { p: 2700.04, q: 1.0, l: 1 },
            Deal { p: 2700.04, q: 1.0, l: 2 },
            Deal { p: 2700.05, q: 1.0, l: 1 },
            Deal { p: 2700.05, q: 1.0, l: 2 },
            Deal { p: 2700.06, q: 1.0, l: 1 },
        ],
        bids: [
            Deal { p: 2699.99, q: 144.33, l: 2 },
            Deal { p: 2699.98, q: 1.0, l: 1 },
            Deal { p: 2699.98, q: 1.0, l: 2 },
            Deal { p: 2699.97, q: 1.0, l: 1 },
            Deal { p: 2699.97, q: 1.0, l: 2 },
            Deal { p: 2699.96, q: 1.0, l: 1 },
            Deal { p: 2699.96, q: 1.0, l: 2 },
            Deal { p: 2699.95, q: 1.0, l: 1 },
            Deal { p: 2699.95, q: 1.0, l: 2 },
            Deal { p: 2699.94, q: 1.0, l: 1 },
        ],
    };
        
    let e6_deals_expected = Deals {
        asks: [
            Deal { p: 2700.00, q: 99.99, l: 2 },
            Deal { p: 2700.02, q: 1.0, l: 1 },
            Deal { p: 2700.02, q: 1.0, l: 2 },
            Deal { p: 2700.03, q: 1.0, l: 1 },
            Deal { p: 2700.03, q: 1.0, l: 2 },
            Deal { p: 2700.04, q: 1.0, l: 1 },
            Deal { p: 2700.04, q: 1.0, l: 2 },
            Deal { p: 2700.05, q: 1.0, l: 1 },
            Deal { p: 2700.05, q: 1.0, l: 2 },
            Deal { p: 2700.06, q: 1.0, l: 1 },
        ],
        bids: [
            Deal { p: 2699.98, q: 1.0, l: 1 },
            Deal { p: 2699.98, q: 1.0, l: 2 },
            Deal { p: 2699.97, q: 1.0, l: 1 },
            Deal { p: 2699.97, q: 1.0, l: 2 },
            Deal { p: 2699.96, q: 1.0, l: 1 },
            Deal { p: 2699.96, q: 1.0, l: 2 },
            Deal { p: 2699.95, q: 1.0, l: 1 },
            Deal { p: 2699.95, q: 1.0, l: 2 },
            Deal { p: 2699.94, q: 1.0, l: 1 },
            Deal { p: 2699.94, q: 1.0, l: 2 },
        ],
    };

    let e7_deals_expected = Deals {
        asks: [
            Deal { p: 2700.00, q: 99.99, l: 2 },
            Deal { p: 2700.02, q: 1.0, l: 1 },
            Deal { p: 2700.02, q: 1.0, l: 2 },
            Deal { p: 2700.03, q: 1.0, l: 1 },
            Deal { p: 2700.03, q: 1.0, l: 2 },
            Deal { p: 2700.04, q: 1.0, l: 1 },
            Deal { p: 2700.04, q: 1.0, l: 2 },
            Deal { p: 2700.05, q: 1.0, l: 1 },
            Deal { p: 2700.05, q: 1.0, l: 2 },
            Deal { p: 2700.06, q: 1.0, l: 1 },
        ],
        bids: [
            Deal { p: 2700.00, q: 788.0, l: 2},
            Deal { p: 2699.98, q: 1.0, l: 1 },
            Deal { p: 2699.98, q: 1.0, l: 2 },
            Deal { p: 2699.97, q: 1.0, l: 1 },
            Deal { p: 2699.97, q: 1.0, l: 2 },
            Deal { p: 2699.96, q: 1.0, l: 1 },
            Deal { p: 2699.96, q: 1.0, l: 2 },
            Deal { p: 2699.95, q: 1.0, l: 1 },
            Deal { p: 2699.95, q: 1.0, l: 2 },
            Deal { p: 2699.94, q: 1.0, l: 1 },
        ],
    };

     let e8_deals_expected = Deals {
        asks: [
            Deal { p: 2700.00, q: 99.99, l: 2 },
            Deal { p: 2700.02, q: 1.0, l: 1 },
            Deal { p: 2700.02, q: 1.0, l: 2 },
            Deal { p: 2700.03, q: 1.0, l: 1 },
            Deal { p: 2700.03, q: 1.0, l: 2 },
            Deal { p: 2700.04, q: 1.0, l: 1 },
            Deal { p: 2700.04, q: 1.0, l: 2 },
            Deal { p: 2700.05, q: 1.0, l: 1 },
            Deal { p: 2700.05, q: 1.0, l: 2 },
            Deal { p: 2700.06, q: 1.0, l: 1 },
        ],
        bids: [
            Deal { p: 2700.00, q: 788.0, l: 2},
            Deal { p: 2699.98, q: 1.0, l: 1 },
            Deal { p: 2699.98, q: 1.0, l: 2 },
            Deal { p: 2699.97, q: 1.0, l: 1 },
            Deal { p: 2699.97, q: 1.0, l: 2 },
            Deal { p: 2699.96, q: 1.0, l: 1 },
            Deal { p: 2699.96, q: 1.0, l: 2 },
            Deal { p: 2699.95, q: 1.0, l: 1 },
            Deal { p: 2699.95, q: 1.0, l: 2 },
            Deal { p: 2699.94, q: 1.0, l: 1 },
        ],
    };

    let expected_deals: Vec<Deals> = vec![e1_deals_expected, e2_deals_expected, e3_deals_expected, e4_deals_expected, e5_deals_expected, e6_deals_expected, e7_deals_expected, e8_deals_expected];
    let mut actual_deals: Vec<Deals>  = vec![];

    let mut deal_counter = 0;
    while deal_counter < expected_deals.len() {
        if let Some(deal) = deal_consumer.recv().await {
            debug!("deal counter is {:}", deal_counter);
            assert!(deal.asks.into_iter().eq(expected_deals[deal_counter].asks.into_iter()));
            assert!(deal.bids.into_iter().eq(expected_deals[deal_counter].bids.into_iter()));
            deal_counter += 1;
        }
    }
    }
}
