//! Side-by-side latency comparison against libzmq through the `zmq2` bindings.
//!
//! This master-side port intentionally keeps only workloads supported by both
//! this branch and master: PUB/SUB, REQ/REP, PUSH/PULL, and DEALER/ROUTER over
//! TCP and IPC. Branch-only socket families, inproc, security, and engine-level
//! tests are omitted so criterion group names can be compared directly.

mod bench_runtime;

use bench_runtime::BenchRuntime;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures::future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use zeromq::{
    __async_rt::task, prelude::*, DealerSocket, PubSocket, PullSocket, PushSocket, RepSocket,
    ReqSocket, RouterSocket, SubSocket, ZmqMessage,
};

const MSG_SIZES: &[usize] = &[16, 256, 4096, 65536];
const SUB_COUNTS: &[usize] = &[1, 8, 64];
const TRANSPORTS: &[&str] = &["tcp", "ipc"];

static IPC_SEQ: AtomicU64 = AtomicU64::new(0);

fn ipc_path(tag: &str) -> String {
    let n = IPC_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("ipc:///tmp/zmq-bench-{tag}-{}-{n}.sock", std::process::id())
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

fn bench_libzmq_pub_sub(c: &mut Criterion) {
    for &transport in TRANSPORTS {
        for &n_subs in SUB_COUNTS {
            let mut group = c.benchmark_group(format!("libzmq/pub_sub/{transport}/subs={n_subs}"));
            group.sample_size(10);
            group.measurement_time(Duration::from_secs(10));
            group.warm_up_time(Duration::from_secs(2));
            for &msg_size in MSG_SIZES {
                group.throughput(Throughput::Bytes((msg_size * n_subs) as u64));
                group.bench_with_input(
                    BenchmarkId::from_parameter(msg_size),
                    &msg_size,
                    |b, &msg_size| {
                        bench_libzmq_pub_sub_one(
                            b,
                            n_subs,
                            msg_size,
                            &endpoint(&format!("libzmq-pubsub-{n_subs}-{msg_size}"), transport),
                        );
                    },
                );
            }
            group.finish();
        }
    }
}

fn bench_libzmq_pub_sub_one(
    b: &mut criterion::Bencher<'_>,
    n_subs: usize,
    msg_size: usize,
    endpoint: &str,
) {
    let ctx = zmq2::Context::new();
    let pub_sock = ctx.socket(zmq2::PUB).expect("pub socket");
    pub_sock.bind(endpoint).expect("pub bind");
    let bound = pub_sock
        .get_last_endpoint()
        .expect("last_endpoint")
        .unwrap();

    struct SubHandle {
        tx_drive: mpsc::Sender<()>,
        rx_done: mpsc::Receiver<Vec<u8>>,
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
            sub.connect(&bound).expect("sub connect");
            sub.set_subscribe(b"").expect("subscribe");
            while rx_drive.recv().is_ok() {
                let got = sub.recv_bytes(0).expect("sub recv");
                if tx_done.send(got).is_err() {
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

    thread::sleep(Duration::from_millis(100));
    let payload = vec![0xAB; msg_size];
    b.iter(|| {
        for sub in &subs {
            sub.tx_drive.send(()).expect("drive sub");
        }
        pub_sock.send(&payload, 0).expect("pub send");
        for sub in &subs {
            black_box(sub.rx_done.recv().expect("sub done"));
        }
    });
}

fn bench_zmqrs_pub_sub(c: &mut Criterion) {
    let rt = build_rt();
    for &transport in TRANSPORTS {
        for &n_subs in SUB_COUNTS {
            let mut group = c.benchmark_group(format!("zmqrs/pub_sub/{transport}/subs={n_subs}"));
            group.sample_size(10);
            group.measurement_time(Duration::from_secs(10));
            group.warm_up_time(Duration::from_secs(2));
            for &msg_size in MSG_SIZES {
                if n_subs == 64 && msg_size == 65536 {
                    continue;
                }
                group.throughput(Throughput::Bytes((msg_size * n_subs) as u64));
                group.bench_with_input(
                    BenchmarkId::from_parameter(msg_size),
                    &msg_size,
                    |b, &msg_size| {
                        bench_zmqrs_pub_sub_one(
                            b,
                            &rt,
                            n_subs,
                            msg_size,
                            &endpoint(&format!("zmqrs-pubsub-{n_subs}-{msg_size}"), transport),
                        );
                    },
                );
            }
            group.finish();
        }
    }
}

fn bench_zmqrs_pub_sub_one(
    b: &mut criterion::Bencher<'_>,
    rt: &BenchRuntime,
    n_subs: usize,
    msg_size: usize,
    endpoint: &str,
) {
    let (mut pub_sock, mut subs) = rt.block_on(async {
        let mut p = PubSocket::new();
        let bound = p.bind(endpoint).await.expect("pub bind").to_string();
        let mut subs = Vec::with_capacity(n_subs);
        for _ in 0..n_subs {
            let mut s = SubSocket::new();
            s.connect(bound.as_str()).await.expect("sub connect");
            s.subscribe("").await.expect("subscribe");
            subs.push(s);
        }
        task::sleep(Duration::from_millis(100)).await;
        (p, subs)
    });

    let payload = vec![0xAB; msg_size];
    b.iter(|| {
        rt.block_on(async {
            pub_sock
                .send(ZmqMessage::from(payload.clone()))
                .await
                .expect("pub send");

            let recv_futures: Vec<_> = subs.iter_mut().map(|s| s.recv()).collect();
            black_box(future::join_all(recv_futures).await);
        });
    });
}

fn bench_libzmq_req_rep(c: &mut Criterion) {
    for &transport in TRANSPORTS {
        let mut group = c.benchmark_group(format!("libzmq/req_rep/{transport}"));
        group.sample_size(10);
        group.measurement_time(Duration::from_secs(10));
        group.warm_up_time(Duration::from_secs(2));
        for &msg_size in MSG_SIZES {
            group.throughput(Throughput::Bytes(msg_size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
                bench_libzmq_req_rep_one(b, s, &endpoint(&format!("libzmq-reqrep-{s}"), transport));
            });
        }
        group.finish();
    }
}

fn bench_libzmq_req_rep_one(b: &mut criterion::Bencher<'_>, msg_size: usize, endpoint: &str) {
    let ctx = zmq2::Context::new();
    let rep = ctx.socket(zmq2::REP).expect("rep socket");
    rep.bind(endpoint).expect("rep bind");
    let bound = rep.get_last_endpoint().expect("last_endpoint").unwrap();
    rep.set_rcvtimeo(100).expect("rep timeout");

    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = stop.clone();
    let reply = vec![0xEF; msg_size];
    let thread = thread::spawn(move || loop {
        match rep.recv_bytes(0) {
            Ok(_) => {
                if rep.send(&reply, 0).is_err() {
                    break;
                }
            }
            Err(zmq2::Error::EAGAIN) if stop_t.load(Ordering::Relaxed) => break,
            Err(zmq2::Error::EAGAIN) => {}
            Err(_) => break,
        }
    });

    let req = ctx.socket(zmq2::REQ).expect("req socket");
    req.connect(&bound).expect("req connect");
    thread::sleep(Duration::from_millis(50));
    let request = vec![0xCD; msg_size];
    b.iter(|| {
        req.send(&request, 0).expect("req send");
        black_box(req.recv_bytes(0).expect("req recv"));
    });
    stop.store(true, Ordering::Relaxed);
    drop(req);
    thread.join().ok();
}

fn bench_zmqrs_req_rep(c: &mut Criterion) {
    let rt = build_rt();
    for &transport in TRANSPORTS {
        let mut group = c.benchmark_group(format!("zmqrs/req_rep/{transport}"));
        group.sample_size(10);
        group.measurement_time(Duration::from_secs(10));
        group.warm_up_time(Duration::from_secs(2));
        for &msg_size in MSG_SIZES {
            group.throughput(Throughput::Bytes(msg_size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
                bench_zmqrs_req_rep_one(
                    b,
                    &rt,
                    s,
                    &endpoint(&format!("zmqrs-reqrep-{s}"), transport),
                );
            });
        }
        group.finish();
    }
}

fn bench_zmqrs_req_rep_one(
    b: &mut criterion::Bencher<'_>,
    rt: &BenchRuntime,
    msg_size: usize,
    endpoint: &str,
) {
    let (mut req, mut rep) = rt.block_on(async {
        let mut r = RepSocket::new();
        let bound = r.bind(endpoint).await.expect("rep bind").to_string();
        let mut q = ReqSocket::new();
        q.connect(bound.as_str()).await.expect("req connect");
        task::sleep(Duration::from_millis(50)).await;
        (q, r)
    });
    let request = vec![0xCD; msg_size];
    let reply = vec![0xEF; msg_size];
    b.iter(|| {
        rt.block_on(async {
            req.send(ZmqMessage::from(request.clone()))
                .await
                .expect("req send");
            black_box(rep.recv().await.expect("rep recv"));
            rep.send(ZmqMessage::from(reply.clone()))
                .await
                .expect("rep send");
            black_box(req.recv().await.expect("req recv"));
        });
    });
}

fn bench_libzmq_push_pull(c: &mut Criterion) {
    for &transport in TRANSPORTS {
        let mut group = c.benchmark_group(format!("libzmq/push_pull/{transport}"));
        group.sample_size(10);
        group.measurement_time(Duration::from_secs(10));
        group.warm_up_time(Duration::from_secs(2));
        for &msg_size in MSG_SIZES {
            group.throughput(Throughput::Bytes(msg_size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
                bench_libzmq_push_pull_one(
                    b,
                    s,
                    &endpoint(&format!("libzmq-pushpull-{s}"), transport),
                );
            });
        }
        group.finish();
    }
}

fn bench_libzmq_push_pull_one(b: &mut criterion::Bencher<'_>, msg_size: usize, endpoint: &str) {
    let ctx = zmq2::Context::new();
    let pull = ctx.socket(zmq2::PULL).expect("pull socket");
    pull.bind(endpoint).expect("pull bind");
    let bound = pull.get_last_endpoint().expect("last_endpoint").unwrap();
    let push = ctx.socket(zmq2::PUSH).expect("push socket");
    push.connect(&bound).expect("push connect");
    thread::sleep(Duration::from_millis(50));
    let payload = vec![0xCD; msg_size];
    b.iter(|| {
        push.send(&payload, 0).expect("push send");
        black_box(pull.recv_bytes(0).expect("pull recv"));
    });
}

fn bench_zmqrs_push_pull(c: &mut Criterion) {
    let rt = build_rt();
    for &transport in TRANSPORTS {
        let mut group = c.benchmark_group(format!("zmqrs/push_pull/{transport}"));
        group.sample_size(10);
        group.measurement_time(Duration::from_secs(10));
        group.warm_up_time(Duration::from_secs(2));
        for &msg_size in MSG_SIZES {
            group.throughput(Throughput::Bytes(msg_size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
                bench_zmqrs_push_pull_one(
                    b,
                    &rt,
                    s,
                    &endpoint(&format!("zmqrs-pushpull-{s}"), transport),
                );
            });
        }
        group.finish();
    }
}

fn bench_zmqrs_push_pull_one(
    b: &mut criterion::Bencher<'_>,
    rt: &BenchRuntime,
    msg_size: usize,
    endpoint: &str,
) {
    let (mut push, mut pull) = rt.block_on(async {
        let mut p = PullSocket::new();
        let bound = p.bind(endpoint).await.expect("pull bind").to_string();
        let mut s = PushSocket::new();
        s.connect(bound.as_str()).await.expect("push connect");
        task::sleep(Duration::from_millis(50)).await;
        (s, p)
    });
    let payload = vec![0xCD; msg_size];
    b.iter(|| {
        rt.block_on(async {
            push.send(ZmqMessage::from(payload.clone()))
                .await
                .expect("push send");
            black_box(pull.recv().await.expect("pull recv"));
        });
    });
}

fn bench_libzmq_dealer_router(c: &mut Criterion) {
    for &transport in TRANSPORTS {
        let mut group = c.benchmark_group(format!("libzmq/dealer_router/{transport}"));
        group.sample_size(10);
        group.measurement_time(Duration::from_secs(10));
        group.warm_up_time(Duration::from_secs(2));
        for &msg_size in MSG_SIZES {
            group.throughput(Throughput::Bytes(msg_size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
                bench_libzmq_dealer_router_one(
                    b,
                    s,
                    &endpoint(&format!("libzmq-dealerrouter-{s}"), transport),
                );
            });
        }
        group.finish();
    }
}

fn bench_libzmq_dealer_router_one(b: &mut criterion::Bencher<'_>, msg_size: usize, endpoint: &str) {
    let ctx = zmq2::Context::new();
    let router = ctx.socket(zmq2::ROUTER).expect("router socket");
    router.bind(endpoint).expect("router bind");
    let bound = router.get_last_endpoint().expect("last_endpoint").unwrap();
    router.set_rcvtimeo(100).expect("router timeout");
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
    dealer.connect(&bound).expect("dealer connect");
    thread::sleep(Duration::from_millis(50));
    let payload = vec![0xCD; msg_size];
    b.iter(|| {
        dealer.send(&payload, 0).expect("dealer send");
        black_box(dealer.recv_bytes(0).expect("dealer recv"));
    });
    stop.store(true, Ordering::Relaxed);
    drop(dealer);
    thread.join().ok();
}

fn bench_zmqrs_dealer_router(c: &mut Criterion) {
    let rt = build_rt();
    for &transport in TRANSPORTS {
        let mut group = c.benchmark_group(format!("zmqrs/dealer_router/{transport}"));
        group.sample_size(10);
        group.measurement_time(Duration::from_secs(10));
        group.warm_up_time(Duration::from_secs(2));
        for &msg_size in MSG_SIZES {
            group.throughput(Throughput::Bytes(msg_size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
                bench_zmqrs_dealer_router_one(
                    b,
                    &rt,
                    s,
                    &endpoint(&format!("zmqrs-dealerrouter-{s}"), transport),
                );
            });
        }
        group.finish();
    }
}

fn bench_zmqrs_dealer_router_one(
    b: &mut criterion::Bencher<'_>,
    rt: &BenchRuntime,
    msg_size: usize,
    endpoint: &str,
) {
    let (mut dealer, mut router) = rt.block_on(async {
        let mut r = RouterSocket::new();
        let bound = r.bind(endpoint).await.expect("router bind").to_string();
        let mut d = DealerSocket::new();
        d.connect(bound.as_str()).await.expect("dealer connect");
        task::sleep(Duration::from_millis(50)).await;
        (d, r)
    });
    let payload = vec![0xCD; msg_size];
    b.iter(|| {
        rt.block_on(async {
            dealer
                .send(ZmqMessage::from(payload.clone()))
                .await
                .expect("dealer send");
            let m = router.recv().await.expect("router recv");
            router.send(m).await.expect("router send");
            black_box(dealer.recv().await.expect("dealer recv"));
        });
    });
}

criterion_group!(
    benches,
    bench_libzmq_pub_sub,
    bench_libzmq_req_rep,
    bench_libzmq_push_pull,
    bench_libzmq_dealer_router,
    bench_zmqrs_pub_sub,
    bench_zmqrs_req_rep,
    bench_zmqrs_push_pull,
    bench_zmqrs_dealer_router,
);
criterion_main!(benches);
