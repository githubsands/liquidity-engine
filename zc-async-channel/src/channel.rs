use core::cell::Cell;
use core::future::poll_fn;
use core::task::{Context, Poll, Waker};

pub struct Channel<T, const N: usize, const S: usize = 4> {
    slots: [Cell<Option<T>>; N],
    head: Cell<usize>,
    len: Cell<usize>,
    rx_waker: Cell<Option<Waker>>,
    tx_wakers: [Cell<Option<Waker>>; S],
    /// Bit `i` set => waker slot `i` is owned by a live `Sender`.
    tx_ids: Cell<u64>,
    rx_alive: Cell<bool>,
}

impl<T, const N: usize, const S: usize> Default for Channel<T, N, S> {
    fn default() -> Self { 
        Self::new() 
    }
}

impl<T, const N: usize, const S: usize> Channel<T, N, S> {
    pub fn new() -> Self {
        // todo: get rid of this ... 
        const { core::assert!(N > 0, "capacity must be > 0") };
        const { core::assert!(S > 0 && S <= 64, "sender limit must be in 1..=64") };
        Self {
            slots: core::array::from_fn(|_| Cell::new(None)),
            head: Cell::new(0),
            len: Cell::new(0),
            rx_waker: Cell::new(None),
            tx_wakers: core::array::from_fn(|_| Cell::new(None)),
            tx_ids: Cell::new(0),
            rx_alive: Cell::new(false),
        }
    }

    pub fn split(&mut self) -> (Sender<'_, T, N, S>, Receiver<'_, T, N, S>) {
        self.rx_alive.set(true);
        self.tx_ids.set(0);
        let id = self.alloc_tx().unwrap();
        (Sender { 
            ch: self,
            id,
        }, Receiver { 
            ch: self 
        })
    }

    fn tx_alive(&self) -> bool {
        self.tx_ids.get() != 0
    }

    fn alloc_tx(&self) -> Option<usize> {
        let all = if S == 64 { u64::MAX } else { (1u64 << S) - 1 };
        let free = !self.tx_ids.get() & all;
        if free == 0 { return None; }
        let id = free.trailing_zeros() as usize;
        self.tx_ids.set(self.tx_ids.get() | (1 << id));
        Some(id)
    }

    fn wake_tx(&self) {
        // Wake every parked sender: if only one were woken and its send future was then
        // cancelled, the freed slot would go unused while the others stay parked.
        for w in &self.tx_wakers { 
            wake(w);
        }
    }

    fn push(&self, v: T) -> Result<(), T> {
        let len = self.len.get();
        if len == N { return Err(v); }
        self.slots[(self.head.get() + len) % N].set(Some(v));
        self.len.set(len + 1);
        wake(&self.rx_waker);
        Ok(())
    }

    fn pop(&self) -> Option<T> {
        let len = self.len.get();
        if len == 0 { return None; }
        let head = self.head.get();
        let v = self.slots[head].take();
        self.head.set((head + 1) % N);
        self.len.set(len - 1);
        self.wake_tx();
        v
    }
}

fn register(slot: &Cell<Option<Waker>>, cx: &Context<'_>) {
    match slot.take() {
        Some(w) if w.will_wake(cx.waker()) => slot.set(Some(w)),
        _ => slot.set(Some(cx.waker().clone())),
    }
}

fn wake(slot: &Cell<Option<Waker>>) {
    if let Some(w) = slot.take() { 
        w.wake()
    };
}

pub struct Sender<'a, T, const N: usize, const S: usize = 4> { 
    ch: &'a Channel<T, N, S>,
    id: usize,
}
pub struct Receiver<'a, T, const N: usize, const S: usize = 4> { 
    ch: &'a Channel<T, N, S>
}

impl<T, const N: usize, const S: usize> Sender<'_, T, N, S> {
    /// Waits while full. Returns Err(v) if the receiver was dropped.
    pub async fn send(&self, v: T) -> Result<(), T> {
        let mut v = Some(v);
        poll_fn(|cx| {
            let val = v.take().unwrap();
            if !self.ch.rx_alive.get() { 
                return Poll::Ready(Err(val));
            }
            match self.ch.push(val) {
                Ok(()) => Poll::Ready(Ok(())),
                Err(val) => { 
                    v = Some(val); 
                    register(&self.ch.tx_wakers[self.id], cx); 
                    Poll::Pending 
                }
            }
        }).await
    }

    pub fn try_send(&self, v: T) -> Result<(), T> {
        if !self.ch.rx_alive.get() { 
            return Err(v);
        }
        self.ch.push(v)
    }

    /// Clones this sender, or returns None if `S` senders are already live.
    pub fn try_clone(&self) -> Option<Self> {
        Some(Sender { 
            ch: self.ch, 
            id: self.ch.alloc_tx()?,
        })
    }
}

impl<T, const N: usize, const S: usize> Clone for Sender<'_, T, N, S> {
    /// Panics if `S` senders are already live; use `try_clone` to handle that case.
    fn clone(&self) -> Self {
        self.try_clone().expect("sender limit S reached")
    }
}

impl<T, const N: usize, const S: usize> Receiver<'_, T, N, S> {
    /// Waits while empty. Returns None once every sender is dropped and the buffer is drained.
    pub async fn recv(&self) -> Option<T> {
        poll_fn(|cx| {
            if let Some(v) = self.ch.pop() {
                return Poll::Ready(Some(v));
            }
            if !self.ch.tx_alive() { 
                return Poll::Ready(None);
            }
            register(&self.ch.rx_waker, cx);
            Poll::Pending
        }).await
    }

    pub fn try_recv(&self) -> Option<T> { 
        self.ch.pop() 
    }
}

impl<T, const N: usize, const S: usize> Drop for Sender<'_, T, N, S> {
    fn drop(&mut self) { 
        self.ch.tx_wakers[self.id].take();
        self.ch.tx_ids.set(self.ch.tx_ids.get() & !(1 << self.id));
        if !self.ch.tx_alive() {
            wake(&self.ch.rx_waker);
        }
    }
}
impl<T, const N: usize, const S: usize> Drop for Receiver<'_, T, N, S> {
    fn drop(&mut self) { 
        self.ch.rx_alive.set(false); 
        self.ch.wake_tx();
    }
}
