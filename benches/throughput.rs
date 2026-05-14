//! Pipelined-throughput benches for workloads master can run.

mod bench_runtime;

use bench_runtime::BenchRuntime;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use zeromq::{
    __async_rt::task, prelude::*, DealerSocket, PubSocket, RouterSocket, SubSocket, ZmqMessage,
};

const BATCH_SIZE: usize = 1024;
const PIPELINE_SIZES: &[usize] = &[256, 4096];
const SUB_COUNTS: &[usize] = &[1, 8, 64];
const TRANSPORTS: &[&str] = &["tcp", "ipc"];

static IPC_SEQ: AtomicU64 = AtomicU64::new(0);

fn ipc_path(tag: &str) -> String {
    let n = IPC_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("ipc:///tmp/zmq-tput-{tag}-{}-{n}.sock", std::process::id())
}

fn endpoint(tag: &str, transport: &str) -> String {
    match transport {
        "tcp" => "tcp://127.0.0.1:0".to_string(),
        "ipc" => ipc_path(tag),
        _ => unreachable!(),
    }
}

fn build_rt() -> BenchRuntime {
    BenchRuntime::new()
}

fn bench_zmqrs_pub_pipelined(c: &mut Criterion) {
    let rt = build_rt();
    for &transport in TRANSPORTS {
        for &n_subs in SUB_COUNTS {
            let mut group = c.benchmark_group(format!(
                "zmqrs/throughput/pub_fanout/{transport}/subs={n_subs}"
            ));
            bench_runtime::configure_group(&mut group);
            for &msg_size in PIPELINE_SIZES {
                let bytes = (BATCH_SIZE * msg_size * n_subs) as u64;
                group.throughput(Throughput::Bytes(bytes));
                group.bench_with_input(
                    BenchmarkId::from_parameter(msg_size),
                    &msg_size,
                    |b, &s| {
                        bench_zmqrs_pub_pipelined_one(b, &rt, n_subs, s, transport);
                    },
                );
            }
            group.finish();
        }
    }
}

fn bench_zmqrs_pub_pipelined_one(
    b: &mut criterion::Bencher<'_>,
    rt: &BenchRuntime,
    n_subs: usize,
    msg_size: usize,
    transport: &str,
) {
    let endpoint = endpoint(&format!("zmqrs-pub-{n_subs}-{msg_size}"), transport);
    let (mut pub_sock, mut subs) = rt.block_on(async {
        let mut p = PubSocket::new();
        let bound = p.bind(&endpoint).await.expect("pub bind").to_string();
        let mut subs = Vec::with_capacity(n_subs);
        for _ in 0..n_subs {
            let mut s = SubSocket::new();
            s.connect(bound.as_str()).await.expect("sub connect");
            s.subscribe("").await.expect("subscribe");
            subs.push(s);
        }

        let sync = ZmqMessage::from(vec![0xFF]);
        let mut remaining = subs;
        let mut ready = Vec::with_capacity(n_subs);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !remaining.is_empty() {
            if std::time::Instant::now() > deadline {
                panic!("zmqrs pub/sub sync timed out");
            }
            p.send(sync.clone()).await.expect("pub sync");
            let mut waiting = Vec::new();
            for mut s in remaining.drain(..) {
                match task::timeout(Duration::from_millis(5), s.recv()).await {
                    Ok(Ok(_)) => ready.push(s),
                    Ok(Err(e)) => panic!("sub sync recv: {e:?}"),
                    Err(_) => waiting.push(s),
                }
            }
            remaining = waiting;
        }
        (p, ready)
    });

    let payload = vec![0xAB; msg_size];
    b.iter(|| {
        rt.block_on(async {
            let sub_handles: Vec<_> = subs
                .drain(..)
                .map(|mut s| {
                    task::spawn(async move {
                        for _ in 0..BATCH_SIZE {
                            match task::timeout(Duration::from_millis(20), s.recv()).await {
                                Ok(Ok(m)) => {
                                    black_box(m);
                                }
                                Ok(Err(e)) => panic!("sub recv: {e:?}"),
                                Err(_) => break,
                            }
                        }
                        s
                    })
                })
                .collect();
            for _ in 0..BATCH_SIZE {
                pub_sock
                    .send(ZmqMessage::from(payload.clone()))
                    .await
                    .expect("pub send");
            }
            for h in sub_handles {
                subs.push(h.await.expect("sub task"));
            }
        });
    });
}

fn bench_libzmq_pub_pipelined(c: &mut Criterion) {
    for &transport in TRANSPORTS {
        for &n_subs in SUB_COUNTS {
            let mut group = c.benchmark_group(format!(
                "libzmq/throughput/pub_fanout/{transport}/subs={n_subs}"
            ));
            bench_runtime::configure_group(&mut group);
            for &msg_size in PIPELINE_SIZES {
                let bytes = (BATCH_SIZE * msg_size * n_subs) as u64;
                group.throughput(Throughput::Bytes(bytes));
                group.bench_with_input(
                    BenchmarkId::from_parameter(msg_size),
                    &msg_size,
                    |b, &s| {
                        bench_libzmq_pub_pipelined_one(b, n_subs, s, transport);
                    },
                );
            }
            group.finish();
        }
    }
}

fn bench_libzmq_pub_pipelined_one(
    b: &mut criterion::Bencher<'_>,
    n_subs: usize,
    msg_size: usize,
    transport: &str,
) {
    let endpoint = endpoint(&format!("libzmq-pub-{n_subs}-{msg_size}"), transport);
    let ctx = zmq2::Context::new();
    let pub_sock = ctx.socket(zmq2::PUB).expect("pub socket");
    pub_sock
        .set_sndhwm((BATCH_SIZE * 16) as i32)
        .expect("sndhwm");
    pub_sock.bind(&endpoint).expect("pub bind");
    let bound = pub_sock
        .get_last_endpoint()
        .expect("last_endpoint")
        .unwrap();

    struct SubHandle {
        tx_drive: mpsc::Sender<usize>,
        rx_done: mpsc::Receiver<()>,
        _thread: thread::JoinHandle<()>,
    }

    let mut subs = Vec::with_capacity(n_subs);
    for _ in 0..n_subs {
        let ctx = ctx.clone();
        let bound = bound.clone();
        let (tx_drive, rx_drive) = mpsc::channel();
        let (tx_done, rx_done) = mpsc::channel();
        let thread = thread::spawn(move || {
            let sub = ctx.socket(zmq2::SUB).expect("sub socket");
            sub.set_rcvhwm((BATCH_SIZE * 16) as i32).expect("rcvhwm");
            sub.set_rcvtimeo(20).expect("rcvtimeo");
            sub.connect(&bound).expect("sub connect");
            sub.set_subscribe(b"").expect("subscribe");
            while let Ok(n) = rx_drive.recv() {
                for _ in 0..n {
                    match sub.recv_bytes(0) {
                        Ok(m) => {
                            black_box(m);
                        }
                        Err(zmq2::Error::EAGAIN) => break,
                        Err(e) => panic!("sub recv: {e:?}"),
                    }
                }
                if tx_done.send(()).is_err() {
                    break;
                }
            }
        });
        subs.push(SubHandle {
            tx_drive,
            rx_done,
            _thread: thread,
        });
    }

    thread::sleep(Duration::from_millis(200));
    let payload = vec![0xAB; msg_size];
    b.iter(|| {
        for sub in &subs {
            sub.tx_drive.send(BATCH_SIZE).expect("drive sub");
        }
        for _ in 0..BATCH_SIZE {
            pub_sock.send(&payload, 0).expect("pub send");
        }
        for sub in &subs {
            sub.rx_done.recv().expect("sub done");
        }
    });
}

fn bench_zmqrs_dealer_router_pipelined(c: &mut Criterion) {
    let rt = build_rt();
    for &transport in TRANSPORTS {
        let mut group = c.benchmark_group(format!("zmqrs/throughput/dealer_router/{transport}"));
        bench_runtime::configure_group(&mut group);
        for &msg_size in PIPELINE_SIZES {
            group.throughput(Throughput::Bytes((BATCH_SIZE * msg_size) as u64));
            group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
                bench_zmqrs_dealer_router_one(b, &rt, s, transport);
            });
        }
        group.finish();
    }
}

fn bench_zmqrs_dealer_router_one(
    b: &mut criterion::Bencher<'_>,
    rt: &BenchRuntime,
    msg_size: usize,
    transport: &str,
) {
    let endpoint = endpoint(&format!("zmqrs-dr-{msg_size}"), transport);
    let (dealer_send, dealer_recv, router) = rt.block_on(async {
        let mut r = RouterSocket::new();
        let bound = r.bind(&endpoint).await.expect("router bind").to_string();
        let mut d = DealerSocket::new();
        d.connect(bound.as_str()).await.expect("dealer connect");
        task::sleep(Duration::from_millis(50)).await;
        let (send, recv) = d.split();
        (send, recv, r)
    });
    let mut dealer_send = Some(dealer_send);
    let mut dealer_recv = Some(dealer_recv);
    let mut router = Some(router);
    let payload = vec![0xCD; msg_size];

    b.iter(|| {
        let mut send = dealer_send.take().unwrap();
        let mut recv = dealer_recv.take().unwrap();
        let mut r = router.take().unwrap();
        rt.block_on(async {
            let router_task = task::spawn(async move {
                for _ in 0..BATCH_SIZE {
                    let m = r.recv().await.expect("router recv");
                    r.send(m).await.expect("router send");
                }
                r
            });
            let recv_task = task::spawn(async move {
                for _ in 0..BATCH_SIZE {
                    black_box(recv.recv().await.expect("dealer recv"));
                }
                recv
            });
            for _ in 0..BATCH_SIZE {
                send.send(ZmqMessage::from(payload.clone()))
                    .await
                    .expect("dealer send");
            }
            dealer_recv.replace(recv_task.await.expect("dealer recv task"));
            router.replace(router_task.await.expect("router task"));
            dealer_send.replace(send);
        });
    });
}

fn bench_libzmq_dealer_router_pipelined(c: &mut Criterion) {
    for &transport in TRANSPORTS {
        let mut group = c.benchmark_group(format!("libzmq/throughput/dealer_router/{transport}"));
        bench_runtime::configure_group(&mut group);
        for &msg_size in PIPELINE_SIZES {
            group.throughput(Throughput::Bytes((BATCH_SIZE * msg_size) as u64));
            group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
                bench_libzmq_dealer_router_one(b, s, transport);
            });
        }
        group.finish();
    }
}

fn bench_libzmq_dealer_router_one(
    b: &mut criterion::Bencher<'_>,
    msg_size: usize,
    transport: &str,
) {
    let endpoint = endpoint(&format!("libzmq-dr-{msg_size}"), transport);
    let ctx = zmq2::Context::new();
    let router = ctx.socket(zmq2::ROUTER).expect("router socket");
    let hwm = (BATCH_SIZE * 4) as i32;
    router.set_sndhwm(hwm).expect("router sndhwm");
    router.set_rcvhwm(hwm).expect("router rcvhwm");
    router.set_rcvtimeo(100).expect("router timeout");
    router.bind(&endpoint).expect("router bind");
    let bound = router.get_last_endpoint().expect("last_endpoint").unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = stop.clone();
    let thread = thread::spawn(move || loop {
        match router.recv_multipart(0) {
            Ok(parts) => {
                if router.send_multipart(parts, 0).is_err() {
                    break;
                }
            }
            Err(zmq2::Error::EAGAIN) if stop_t.load(Ordering::Relaxed) => break,
            Err(zmq2::Error::EAGAIN) => {}
            Err(_) => break,
        }
    });

    let dealer = ctx.socket(zmq2::DEALER).expect("dealer socket");
    dealer.set_sndhwm(hwm).expect("dealer sndhwm");
    dealer.set_rcvhwm(hwm).expect("dealer rcvhwm");
    dealer.connect(&bound).expect("dealer connect");
    thread::sleep(Duration::from_millis(50));
    let payload = vec![0xCD; msg_size];

    b.iter(|| {
        for _ in 0..BATCH_SIZE {
            dealer.send(&payload, 0).expect("dealer send");
        }
        for _ in 0..BATCH_SIZE {
            black_box(dealer.recv_bytes(0).expect("dealer recv"));
        }
    });

    stop.store(true, Ordering::Relaxed);
    drop(dealer);
    thread.join().ok();
}

criterion_group!(
    benches,
    bench_zmqrs_pub_pipelined,
    bench_zmqrs_dealer_router_pipelined,
    bench_libzmq_pub_pipelined,
    bench_libzmq_dealer_router_pipelined,
);
criterion_main!(benches);
