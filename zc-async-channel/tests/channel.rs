use std::cell::Cell;
use std::future::poll_fn;
use std::pin::pin;
use std::task::Poll;
use std::time::{Duration, Instant};

use compio::io::{AsyncRead, AsyncWriteExt};
use compio::net::{TcpListener, TcpStream};
use compio::runtime::spawn;
use compio::time::{interval, sleep, timeout};
use compio_executor::JoinError;
use depth_generator::generator::DepthMessageGenerator;
use depth_pool::{DEPTH_SLOT_SIZE, DepthUpdate, access_depth_update};
use futures::future::{Either, select};
use futures::join;
use zc_async_channel::channel::Channel;

// Integration tests that drive the channel with compio's real runtime:
// reactor-backed timers, TCP I/O completions, spawn/JoinHandle semantics,
// cancellation via timeout/select/handle-drop, and runtime-per-thread.
//
// Every payload is the zero-copy `DepthUpdate` from depth-pool.

// ---------- helpers ----------

/// `spawn` needs `'static`. Tests use a leaked Box for brevity;
/// in production use `StaticCell` (no heap) or keep tasks joined instead.
fn leak<const N: usize>() -> &'static mut Channel<DepthUpdate, N> {
    Box::leak(Box::new(Channel::new()))
}

/// `n` random depths from the generator.
fn depths(n: usize) -> Vec<DepthUpdate> {
    let mut g = DepthMessageGenerator::default();
    (0..n).map(|_| g.depth_message_random()).collect()
}

/// `n` depths tagged with location `l`, so each sender's stream can be told apart.
fn depths_at(n: usize, l: u8) -> Vec<DepthUpdate> {
    let mut v = depths(n);
    v.iter_mut().for_each(|d| d.l = l);
    v
}

/// Fail fast instead of hanging forever if a wakeup is lost.
async fn guard<F: std::future::Future>(f: F) -> F::Output {
    timeout(Duration::from_secs(5), f).await.expect("test hung: lost wakeup?")
}

/// Yield to compio's scheduler once.
async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded { return Poll::Ready(()); }
        yielded = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }).await
}

// ---------- reactor-driven wakeups ----------

#[compio::test]
async fn recv_is_woken_by_timer_driven_send() {
    guard(async move {
        let depth = depths(1)[0];
        let mut ch: Channel<DepthUpdate, 2> = Channel::new();
        let (tx, rx) = ch.split();
        let start = Instant::now();
        let ((), got) = join!(
            async {
                sleep(Duration::from_millis(30)).await;
                tx.send(depth).await.unwrap();
            }, rx.recv(),
        );
        assert_eq!(got, Some(depth));
        assert!(start.elapsed() >= Duration::from_millis(30), "recv must actually park");
    }).await
}

#[compio::test]
async fn backpressure_slow_consumer_on_timers() {
    guard(async move {
        let expected = depths(20);
        let mut ch: Channel<DepthUpdate, 1> = Channel::new();
        let (tx, rx) = ch.split();
        let max_in_flight = Cell::new(0u32);
        let sent = Cell::new(0u32);
        let recvd = Cell::new(0u32);
        let ((), got) = join!(
            async {
                for &d in &expected {
                    tx.send(d).await.unwrap();
                    sent.set(sent.get() + 1);
                    max_in_flight.set(max_in_flight.get().max(sent.get() - recvd.get()));
                }
                drop(tx);
            },
            async {
                let mut v = Vec::new();
                while let Some(d) = rx.recv().await {
                    recvd.set(recvd.get() + 1);
                    sleep(Duration::from_millis(1)).await;
                    v.push(d);
                }
                v
            },
        );
        assert_eq!(got, expected);
        assert!(max_in_flight.get() <= 2, "capacity 1 (+1 being processed) must bound the producer, saw {}", max_in_flight.get());
    }).await
}

#[compio::test]
async fn interval_ticks_feed_channel() {
    guard(async move {
        let expected = depths(10);
        let mut ch: Channel<DepthUpdate, 4> = Channel::new();
        let (tx, rx) = ch.split();
        let ((), got) = join!(
            async {
                let mut iv = interval(Duration::from_millis(5));
                for &d in &expected {
                    iv.tick().await;
                    tx.send(d).await.unwrap();
                }
                drop(tx);
            },
            async {
                let mut v = Vec::new();
                while let Some(d) = rx.recv().await {
                    v.push(d);
                }
                v
            },
        );
        assert_eq!(got, expected);
    }).await
}

#[compio::test]
async fn tcp_archived_depths_flow_through_channel() {
    guard(async move {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let expected = depths(4096);
        // back-to-back fixed-size rkyv archives
        let payload: Vec<u8> = expected
            .iter()
            .flat_map(|d| d.to_bytes().expect("serialize").to_vec())
            .collect();
        assert_eq!(payload.len(), expected.len() * DEPTH_SLOT_SIZE);

        let mut ch: Channel<DepthUpdate, 4> = Channel::new();
        let (tx, rx) = ch.split();

        let (client, server, got) = join!(
            // client: writes every archived depth then closes
            async move {
                let mut s = TcpStream::connect(addr).await.unwrap();
                let compio::BufResult(r, _) = s.write_all(payload).await;
                r.unwrap();
            },
            // server: socket reads -> whole archived frames -> depths -> channel
            async move {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut pending: Vec<u8> = Vec::with_capacity(2 * DEPTH_SLOT_SIZE);
                loop {
                    let compio::BufResult(r, buf) = s.read(Vec::with_capacity(4096)).await;
                    if r.unwrap() == 0 {
                        break;
                    }
                    pending.extend_from_slice(&buf);
                    let whole = pending.len() - pending.len() % DEPTH_SLOT_SIZE;
                    for f in pending[..whole].chunks_exact(DEPTH_SLOT_SIZE) {
                        let depth = access_depth_update(f).expect("valid archive").to_owned().unwrap();
                        tx.send(depth).await.unwrap();
                    }
                    pending.drain(..whole);
                }
                assert!(pending.is_empty(), "stream must hold whole archived depths");
            },
            async move {
                let mut out = Vec::new();
                while let Some(d) = rx.recv().await {
                    out.push(d);
                }
                out
            },
        );
        let ((), ()) = (client, server);
        assert_eq!(got, expected);
    }).await
}

#[compio::test]
async fn spawned_producer_and_consumer() {
    guard(async move {
        let expected = depths(1000);
        let sent = expected.clone();
        let (tx, rx) = leak::<4>().split();
        let p = spawn(async move { for d in sent { tx.send(d).await.unwrap(); } });
        let c = spawn(async move {
            let mut v = Vec::new();
            while let Some(d) = rx.recv().await { v.push(d); }
            v
        });
        p.await.unwrap();
        assert_eq!(c.await.unwrap(), expected);
    }).await
}

#[compio::test]
async fn dropping_join_handle_cancels_task_and_closes_channel() {
    guard(async move {
        let (tx, rx) = leak::<1>().split();
        let consumer = spawn(async move { while rx.recv().await.is_some() {} });
        yield_now().await;              // let the consumer park in recv()
        drop(consumer);                 // compio cancels the task -> its Receiver is dropped
        yield_now().await;
        let depth = depths(1)[0];
        assert_eq!(tx.send(depth).await, Err(depth), "cancelled task must release its Receiver");
    }).await
}

#[compio::test]
async fn detached_task_keeps_running() {
    guard(async move {
        let expected = depths(5);
        let sent = expected.clone();
        let (tx, rx) = leak::<2>().split();
        spawn(async move {
            for d in sent {
                sleep(Duration::from_millis(1)).await;
                tx.send(d).await.unwrap();
            }
        }).detach();                    // no handle, but it must still complete
        let mut got = Vec::new();
        while let Some(d) = rx.recv().await {
            got.push(d);
        }
        assert_eq!(got, expected);
    }).await
}

#[compio::test]
async fn panicking_producer_closes_channel() {
    guard(async move {
        let expected = depths(3);
        let sent = expected.clone();
        let (tx, rx) = leak::<8>().split();
        let p = spawn(async move {
            for d in sent { tx.send(d).await.unwrap(); }
            yield_now().await;
            panic!("producer blew up");   // tx dropped during unwind
        });
        let mut got = Vec::new();
        while let Some(d) = rx.recv().await { got.push(d); }
        assert_eq!(got, expected);
        assert!(matches!(p.await, Err(JoinError::Panicked(_))));
    }).await
}

#[compio::test]
async fn receiver_drop_wakes_sender_parked_in_other_task() {
    guard(async move {
        let d = depths(2);
        let (a, b) = (d[0], d[1]);
        let (tx, rx) = leak::<1>().split();
        let p = spawn(async move { tx.send(a).await.unwrap(); tx.send(b).await }); // parks on b
        sleep(Duration::from_millis(5)).await;
        drop(rx);
        let r = timeout(Duration::from_millis(500), p).await.expect("parked sender was never woken");
        assert_eq!(r.unwrap(), Err(b));
    }).await
}

#[compio::test]
async fn sender_drop_wakes_receiver_parked_in_other_task() {
    guard(async move {
        let (tx, rx) = leak::<1>().split();
        let c = spawn(async move { rx.recv().await });                           // parks: empty
        sleep(Duration::from_millis(5)).await;
        drop(tx);
        let r = timeout(Duration::from_millis(500), c).await.expect("parked receiver was never woken");
        assert_eq!(r.unwrap(), None);
    }).await
}

// ---------- cancellation via timeout / select ----------

#[compio::test]
async fn timeout_on_empty_recv_then_channel_still_works() {
    guard(async move {
        let depth = depths(1)[0];
        let mut ch: Channel<DepthUpdate, 1> = Channel::new();
        let (tx, rx) = ch.split();
        assert!(timeout(Duration::from_millis(10), rx.recv()).await.is_err(), "nothing sent -> Elapsed");
        tx.send(depth).await.unwrap();       // the cancelled recv left no stale state
        let got = timeout(Duration::from_millis(10), rx.recv()).await.unwrap();
        assert_eq!(got, Some(depth));
    }).await
}

#[compio::test]
async fn timeout_on_full_send_does_not_enqueue() {
    guard(async move {
        let expected = depths(2);
        let mut ch: Channel<DepthUpdate, 1> = Channel::new();
        let (tx, rx) = ch.split();
        tx.send(expected[0]).await.unwrap();
        assert!(timeout(Duration::from_millis(10), tx.send(expected[1])).await.is_err());
        assert_eq!(rx.try_recv(), Some(expected[0]));
        assert_eq!(rx.try_recv(), None, "timed-out send must not have enqueued the second depth");
    }).await
}

#[compio::test]
async fn select_data_vs_shutdown() {
    guard(async move {
        let expected = depths(3);
        let mut data: Channel<DepthUpdate, 4> = Channel::new();
        // shutdown is signalled with a depth as well
        let mut stop: Channel<DepthUpdate, 1> = Channel::new();
        let (dtx, drx) = data.split();
        let (stx, srx) = stop.split();

        let ((), seen) = join!(
            async {
                for &d in &expected { dtx.send(d).await.unwrap(); sleep(Duration::from_millis(2)).await; }
                sleep(Duration::from_millis(10)).await;
                stx.send(DepthUpdate::default()).await.unwrap(); // request shutdown; dtx still alive
                sleep(Duration::from_millis(10)).await;
                drop(dtx);
            },
            async {
                let mut seen = Vec::new();
                loop {
                    match select(pin!(drx.recv()), pin!(srx.recv())).await {
                        Either::Left((Some(d), _)) => seen.push(d),
                        Either::Left((None, _)) => panic!("shutdown should win before data closes"),
                        Either::Right((stop, _)) => {
                            assert_eq!(stop, Some(DepthUpdate::default()));
                            break;
                        }
                    }
                }
                seen
            },
        );
        assert_eq!(seen, expected);
    }).await
}


// ---------- stress ----------

#[compio::test]
async fn stress_capacity_one_spawned() {
    guard(async move {
        let expected = depths(20_000);
        let sent = expected.clone();
        let (tx, rx) = leak::<1>().split();
        let start = Instant::now();
        let p = spawn(async move { for d in sent { tx.send(d).await.unwrap(); } });
        let c = spawn(async move {
            let mut n = 0;
            while let Some(d) = rx.recv().await { assert_eq!(d, expected[n]); n += 1; }
            n
        });
        p.await.unwrap();
        assert_eq!(c.await.unwrap(), 20_000);
        eprintln!("20k depths through cap-1 channel across 2 compio tasks: {:?}", start.elapsed());
    }).await
}

// ---------- cloned senders (MPSC) ----------

#[compio::test]
async fn cloned_senders_all_deliver_and_close_after_last_drop() {
    guard(async move {
        let (from_a, from_b) = (depths_at(50, 1), depths_at(50, 2));
        let (want_a, want_b) = (from_a.clone(), from_b.clone());
        let mut ch: Channel<DepthUpdate, 2> = Channel::new();
        let (tx1, rx) = ch.split();
        let tx2 = tx1.clone();
        let ((), (), got) = join!(
            async move { for &d in &from_a { tx1.send(d).await.unwrap(); } },
            async move { for &d in &from_b { tx2.send(d).await.unwrap(); } },
            async {
                let mut v = Vec::new();
                while let Some(d) = rx.recv().await { v.push(d); sleep(Duration::from_millis(1)).await; }
                v
            },
        );
        let (a, b): (Vec<DepthUpdate>, Vec<DepthUpdate>) = got.into_iter().partition(|d| d.l == 1);
        assert_eq!(a, want_a, "per-sender order preserved");
        assert_eq!(b, want_b, "per-sender order preserved");
    }).await
}

#[compio::test]
async fn receiver_closes_only_after_last_sender_drops() {
    let expected = depths(2);
    let mut ch: Channel<DepthUpdate, 4, 2> = Channel::new();
    let (tx1, rx) = ch.split();
    let tx2 = tx1.clone();
    assert!(tx2.try_clone().is_none(), "S = 2 caps live senders");
    tx1.try_send(expected[0]).unwrap();
    drop(tx1);
    let tx3 = tx2.try_clone().expect("slot freed by tx1");
    drop(tx2);
    tx3.try_send(expected[1]).unwrap();
    drop(tx3);
    assert_eq!(rx.recv().await, Some(expected[0]));
    assert_eq!(rx.recv().await, Some(expected[1]));
    assert_eq!(rx.recv().await, None);
}
