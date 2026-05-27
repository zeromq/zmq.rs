//! Side-by-side latency comparison against libzmq through the `zmq2` bindings.
//!
//! This master-side port intentionally keeps only workloads supported by both
//! this branch and master: PUB/SUB, REQ/REP, PUSH/PULL, and DEALER/ROUTER over
//! TCP and IPC. Branch-only socket families, inproc, security, and engine-level
//! tests are omitted so criterion group names can be compared directly.

mod bench_runtime;

use bench_runtime::BenchRuntime;
use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures::future;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use zeromq::{
    __async_rt::task, prelude::*, DealerSocket, PubSocket, PullSocket, PushSocket, RepSocket,
    ReqSocket, RouterSocket, SubSocket, XPubSocket, XSubSocket, ZmqMessage,
};

const MSG_SIZES: &[usize] = &[16, 256, 4096, 65536];
const SUB_COUNTS: &[usize] = &[1, 8, 64];
const HOTPATH_MSG_SIZES: &[usize] = &[64, 256, 1024];
const HOTPATH_PUB_SUB_COUNTS: &[usize] = &[0, 1, 4];
const HOTPATH_DELIVERED_PUB_SUB_COUNTS: &[usize] = &[1, 4];
const HOTPATH_ZMQ2_HWM: i32 = 100_000;

type NativeJoinHandle = task::JoinHandle<()>;

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

async fn drain_zmqrs_pub_sync_messages(subs: &mut [SubSocket]) {
    for sub in subs {
        loop {
            match task::timeout(Duration::from_millis(5), sub.recv()).await {
                Ok(Ok(message)) => {
                    black_box(message);
                }
                Ok(Err(error)) => panic!("zmqrs pub sync drain recv: {error:?}"),
                Err(_) => break,
            }
        }
    }
}

async fn sync_zmqrs_pub_subscribers(
    pub_sock: &mut PubSocket,
    mut subs: Vec<SubSocket>,
) -> Vec<SubSocket> {
    let sync = ZmqMessage::from(vec![0xFF]);
    let mut ready = Vec::with_capacity(subs.len());
    let deadline = Instant::now() + Duration::from_secs(10);
    while !subs.is_empty() {
        if Instant::now() > deadline {
            panic!("zmqrs pub/sub sync timed out");
        }
        pub_sock.send(sync.clone()).await.expect("zmqrs pub sync");
        let mut waiting = Vec::new();
        for mut sub in subs.drain(..) {
            match task::timeout(Duration::from_millis(5), sub.recv()).await {
                Ok(Ok(message)) => {
                    black_box(message);
                    ready.push(sub);
                }
                Ok(Err(error)) => panic!("zmqrs sub sync recv: {error:?}"),
                Err(_) => waiting.push(sub),
            }
        }
        subs = waiting;
    }

    // Subscribers that are already ready may receive later sync frames during subscription propagation; drain before timing.
    drain_zmqrs_pub_sync_messages(&mut ready).await;
    ready
}

fn hotpath_payload(size: usize, byte: u8) -> Bytes {
    Bytes::from(vec![byte; size])
}

fn hotpath_native_endpoint() -> &'static str {
    "tcp://127.0.0.1:0"
}

fn stop_native_drains(rt: &BenchRuntime, stop: Arc<AtomicBool>, handles: Vec<NativeJoinHandle>) {
    stop.store(true, Ordering::Relaxed);
    rt.block_on(async {
        for handle in handles {
            let _ = handle.await;
        }
    });
}

fn configure_libzmq_socket(socket: &zmq2::Socket) {
    socket.set_linger(0).expect("linger");
    socket.set_sndhwm(HOTPATH_ZMQ2_HWM).expect("sndhwm");
    socket.set_rcvhwm(HOTPATH_ZMQ2_HWM).expect("rcvhwm");
}

fn libzmq_send_retry(socket: &zmq2::Socket, payload: &[u8]) {
    loop {
        match socket.send(payload, zmq2::DONTWAIT) {
            Ok(()) => return,
            Err(zmq2::Error::EAGAIN) => std::hint::spin_loop(),
            Err(e) => panic!("libzmq send: {e:?}"),
        }
    }
}

fn join_libzmq_drains(stop: Arc<AtomicBool>, threads: Vec<thread::JoinHandle<()>>) {
    stop.store(true, Ordering::Relaxed);
    for thread in threads {
        thread.join().expect("drain thread");
    }
}

fn sync_libzmq_pub_subscribers(pub_sock: &zmq2::Socket, subs: &[zmq2::Socket]) {
    let sync_payload = [0xFFu8];
    let mut ready = vec![false; subs.len()];
    let deadline = Instant::now() + Duration::from_secs(10);
    while ready.iter().any(|is_ready| !*is_ready) {
        if Instant::now() > deadline {
            panic!("libzmq pub/sub sync timed out");
        }
        pub_sock
            .send(&sync_payload[..], 0)
            .expect("libzmq pub sync");
        for (index, sub) in subs.iter().enumerate() {
            if !ready[index] {
                match sub.recv_bytes(zmq2::DONTWAIT) {
                    Ok(message) => {
                        black_box(message);
                        ready[index] = true;
                    }
                    Err(zmq2::Error::EAGAIN) => {}
                    Err(error) => panic!("libzmq sub sync recv: {error:?}"),
                }
            }
        }
        thread::sleep(Duration::from_millis(1));
    }

    loop {
        let mut drained_any = false;
        for sub in subs {
            loop {
                match sub.recv_bytes(zmq2::DONTWAIT) {
                    Ok(message) => {
                        black_box(message);
                        drained_any = true;
                    }
                    Err(zmq2::Error::EAGAIN) => break,
                    Err(error) => panic!("libzmq sub sync drain recv: {error:?}"),
                }
            }
        }
        if !drained_any {
            break;
        }
    }
}

fn sync_libzmq_pub_drain_threads(pub_sock: &zmq2::Socket, counters: &[Arc<AtomicUsize>]) {
    let sync_payload = [0xFFu8];
    let deadline = Instant::now() + Duration::from_secs(10);
    while counters
        .iter()
        .any(|counter| counter.load(Ordering::Relaxed) == 0)
    {
        if Instant::now() > deadline {
            panic!("libzmq pub drain thread sync timed out");
        }
        pub_sock
            .send(&sync_payload[..], 0)
            .expect("libzmq pub drain sync");
        thread::sleep(Duration::from_millis(1));
    }

    let mut previous: usize = counters
        .iter()
        .map(|counter| counter.load(Ordering::Relaxed))
        .sum();
    loop {
        thread::sleep(Duration::from_millis(2));
        let current: usize = counters
            .iter()
            .map(|counter| counter.load(Ordering::Relaxed))
            .sum();
        if current == previous {
            break;
        }
        previous = current;
    }
}

fn bench_native_socket_send(c: &mut Criterion) {
    let rt = build_rt();

    for &n_subs in HOTPATH_PUB_SUB_COUNTS {
        let mut group = c.benchmark_group(format!("hotpath/native_socket_send/pub/subs={n_subs}"));
        bench_runtime::configure_group(&mut group);
        for &msg_size in HOTPATH_MSG_SIZES {
            group.throughput(Throughput::Bytes(msg_size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
                bench_native_pub_send_one(b, &rt, n_subs, s);
            });
        }
        group.finish();
    }

    let mut group = c.benchmark_group("hotpath/native_socket_send/push_pull");
    bench_runtime::configure_group(&mut group);
    for &msg_size in HOTPATH_MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            bench_native_push_send_one(b, &rt, s);
        });
    }
    group.finish();

    let mut group = c.benchmark_group("hotpath/native_socket_send/dealer_router");
    bench_runtime::configure_group(&mut group);
    for &msg_size in HOTPATH_MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            bench_native_dealer_send_one(b, &rt, s);
        });
    }
    group.finish();
}

fn bench_native_pub_send_one(
    b: &mut criterion::Bencher<'_>,
    rt: &BenchRuntime,
    n_subs: usize,
    msg_size: usize,
) {
    let (mut pub_sock, stop, handles) = rt.block_on(async {
        let mut p = PubSocket::new();
        let bound = p
            .bind(hotpath_native_endpoint())
            .await
            .expect("pub bind")
            .to_string();
        let mut subs = Vec::with_capacity(n_subs);
        for _ in 0..n_subs {
            let mut sub = SubSocket::new();
            sub.connect(bound.as_str()).await.expect("sub connect");
            sub.subscribe("").await.expect("subscribe");
            subs.push(sub);
        }

        let subs = sync_zmqrs_pub_subscribers(&mut p, subs).await;
        let stop = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::with_capacity(n_subs);

        for mut sub in subs {
            let stop_for_task = stop.clone();
            handles.push(task::spawn(async move {
                while !stop_for_task.load(Ordering::Relaxed) {
                    match task::timeout(Duration::from_millis(20), sub.recv()).await {
                        Ok(Ok(message)) => {
                            black_box(message);
                        }
                        Ok(Err(_)) => break,
                        Err(_) => {}
                    }
                }
            }));
        }

        (p, stop, handles)
    });
    let payload = hotpath_payload(msg_size, 0xAB);

    b.iter(|| {
        rt.block_on(async {
            pub_sock
                .send(ZmqMessage::from(payload.clone()))
                .await
                .expect("pub send");
        });
    });

    stop_native_drains(rt, stop, handles);
}

fn bench_native_push_send_one(b: &mut criterion::Bencher<'_>, rt: &BenchRuntime, msg_size: usize) {
    let (mut push, stop, handle) = rt.block_on(async {
        let mut pull = PullSocket::new();
        let bound = pull
            .bind(hotpath_native_endpoint())
            .await
            .expect("pull bind")
            .to_string();
        let mut push = PushSocket::new();
        push.connect(bound.as_str()).await.expect("push connect");
        task::sleep(Duration::from_millis(50)).await;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_task = stop.clone();
        let handle = task::spawn(async move {
            while !stop_for_task.load(Ordering::Relaxed) {
                match task::timeout(Duration::from_millis(20), pull.recv()).await {
                    Ok(Ok(message)) => {
                        black_box(message);
                    }
                    Ok(Err(_)) => break,
                    Err(_) => {}
                }
            }
        });
        (push, stop, handle)
    });
    let payload = hotpath_payload(msg_size, 0xCD);

    b.iter(|| {
        rt.block_on(async {
            push.send(ZmqMessage::from(payload.clone()))
                .await
                .expect("push send");
        });
    });

    stop_native_drains(rt, stop, vec![handle]);
}

fn bench_native_dealer_send_one(
    b: &mut criterion::Bencher<'_>,
    rt: &BenchRuntime,
    msg_size: usize,
) {
    let (mut dealer, stop, handle) = rt.block_on(async {
        let mut router = RouterSocket::new();
        let bound = router
            .bind(hotpath_native_endpoint())
            .await
            .expect("router bind")
            .to_string();
        let mut dealer = DealerSocket::new();
        dealer
            .connect(bound.as_str())
            .await
            .expect("dealer connect");
        task::sleep(Duration::from_millis(50)).await;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_task = stop.clone();
        let handle = task::spawn(async move {
            while !stop_for_task.load(Ordering::Relaxed) {
                match task::timeout(Duration::from_millis(20), router.recv()).await {
                    Ok(Ok(message)) => {
                        black_box(message);
                    }
                    Ok(Err(_)) => break,
                    Err(_) => {}
                }
            }
        });
        (dealer, stop, handle)
    });
    let payload = hotpath_payload(msg_size, 0xEF);

    b.iter(|| {
        rt.block_on(async {
            dealer
                .send(ZmqMessage::from(payload.clone()))
                .await
                .expect("dealer send");
        });
    });

    stop_native_drains(rt, stop, vec![handle]);
}

fn bench_libzmq_socket_send(c: &mut Criterion) {
    for &n_subs in HOTPATH_PUB_SUB_COUNTS {
        let mut group = c.benchmark_group(format!("hotpath/libzmq_socket_send/pub/subs={n_subs}"));
        bench_runtime::configure_group(&mut group);
        for &msg_size in HOTPATH_MSG_SIZES {
            group.throughput(Throughput::Bytes(msg_size as u64));
            group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
                bench_libzmq_pub_send_one(b, n_subs, s);
            });
        }
        group.finish();
    }

    let mut group = c.benchmark_group("hotpath/libzmq_socket_send/push_pull");
    bench_runtime::configure_group(&mut group);
    for &msg_size in HOTPATH_MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            bench_libzmq_push_send_one(b, s);
        });
    }
    group.finish();

    let mut group = c.benchmark_group("hotpath/libzmq_socket_send/dealer_router");
    bench_runtime::configure_group(&mut group);
    for &msg_size in HOTPATH_MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            bench_libzmq_dealer_send_one(b, s);
        });
    }
    group.finish();
}

fn bench_libzmq_pub_send_one(b: &mut criterion::Bencher<'_>, n_subs: usize, msg_size: usize) {
    let ctx = zmq2::Context::new();
    let pub_sock = ctx.socket(zmq2::PUB).expect("pub socket");
    configure_libzmq_socket(&pub_sock);
    pub_sock.bind(hotpath_native_endpoint()).expect("pub bind");
    let bound = pub_sock
        .get_last_endpoint()
        .expect("last_endpoint")
        .expect("bound endpoint");

    let stop = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::with_capacity(n_subs);
    let mut counters = Vec::with_capacity(n_subs);
    for _ in 0..n_subs {
        let ctx = ctx.clone();
        let bound = bound.clone();
        let stop_for_thread = stop.clone();
        let counter = Arc::new(AtomicUsize::new(0));
        counters.push(counter.clone());
        threads.push(thread::spawn(move || {
            let sub = ctx.socket(zmq2::SUB).expect("sub socket");
            configure_libzmq_socket(&sub);
            sub.set_rcvtimeo(20).expect("rcvtimeo");
            sub.connect(&bound).expect("sub connect");
            sub.set_subscribe(b"").expect("subscribe");
            while !stop_for_thread.load(Ordering::Relaxed) {
                match sub.recv_bytes(0) {
                    Ok(message) => {
                        black_box(message);
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(zmq2::Error::EAGAIN) => {}
                    Err(_) => break,
                }
            }
        }));
    }

    sync_libzmq_pub_drain_threads(&pub_sock, &counters);
    let payload = vec![0xAB; msg_size];
    b.iter(|| libzmq_send_retry(&pub_sock, &payload));

    join_libzmq_drains(stop, threads);
}

fn bench_libzmq_push_send_one(b: &mut criterion::Bencher<'_>, msg_size: usize) {
    let ctx = zmq2::Context::new();
    let pull = ctx.socket(zmq2::PULL).expect("pull socket");
    configure_libzmq_socket(&pull);
    pull.set_rcvtimeo(20).expect("rcvtimeo");
    pull.bind(hotpath_native_endpoint()).expect("pull bind");
    let bound = pull
        .get_last_endpoint()
        .expect("last_endpoint")
        .expect("bound endpoint");

    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = stop.clone();
    let thread = thread::spawn(move || {
        while !stop_for_thread.load(Ordering::Relaxed) {
            match pull.recv_bytes(0) {
                Ok(message) => {
                    black_box(message);
                }
                Err(zmq2::Error::EAGAIN) => {}
                Err(_) => break,
            }
        }
    });

    let push = ctx.socket(zmq2::PUSH).expect("push socket");
    configure_libzmq_socket(&push);
    push.connect(&bound).expect("push connect");
    thread::sleep(Duration::from_millis(50));

    let payload = vec![0xCD; msg_size];
    b.iter(|| libzmq_send_retry(&push, &payload));

    join_libzmq_drains(stop, vec![thread]);
}

fn bench_libzmq_dealer_send_one(b: &mut criterion::Bencher<'_>, msg_size: usize) {
    let ctx = zmq2::Context::new();
    let router = ctx.socket(zmq2::ROUTER).expect("router socket");
    configure_libzmq_socket(&router);
    router.set_rcvtimeo(20).expect("rcvtimeo");
    router.bind(hotpath_native_endpoint()).expect("router bind");
    let bound = router
        .get_last_endpoint()
        .expect("last_endpoint")
        .expect("bound endpoint");

    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = stop.clone();
    let thread = thread::spawn(move || {
        while !stop_for_thread.load(Ordering::Relaxed) {
            match router.recv_multipart(0) {
                Ok(message) => {
                    black_box(message);
                }
                Err(zmq2::Error::EAGAIN) => {}
                Err(_) => break,
            }
        }
    });

    let dealer = ctx.socket(zmq2::DEALER).expect("dealer socket");
    configure_libzmq_socket(&dealer);
    dealer.connect(&bound).expect("dealer connect");
    thread::sleep(Duration::from_millis(50));

    let payload = vec![0xEF; msg_size];
    b.iter(|| libzmq_send_retry(&dealer, &payload));

    join_libzmq_drains(stop, vec![thread]);
}

fn bench_native_delivered_latency(c: &mut Criterion) {
    let rt = build_rt();

    for &n_subs in HOTPATH_DELIVERED_PUB_SUB_COUNTS {
        let mut group = c.benchmark_group(format!(
            "hotpath/native_delivered_latency/pub_fanout/subs={n_subs}"
        ));
        bench_runtime::configure_group(&mut group);
        for &msg_size in HOTPATH_MSG_SIZES {
            group.throughput(Throughput::Bytes((msg_size * n_subs) as u64));
            group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
                bench_native_pub_delivered_one(b, &rt, n_subs, s);
            });
        }
        group.finish();
    }

    let mut group = c.benchmark_group("hotpath/native_delivered_latency/push_pull");
    bench_runtime::configure_group(&mut group);
    for &msg_size in HOTPATH_MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            bench_native_push_delivered_one(b, &rt, s);
        });
    }
    group.finish();

    let mut group = c.benchmark_group("hotpath/native_delivered_latency/dealer_router");
    bench_runtime::configure_group(&mut group);
    for &msg_size in HOTPATH_MSG_SIZES {
        group.throughput(Throughput::Bytes((msg_size * 2) as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            bench_native_dealer_delivered_one(b, &rt, s);
        });
    }
    group.finish();

    let mut group = c.benchmark_group("hotpath/native_delivered_latency/xpub_xsub_downstream");
    bench_runtime::configure_group(&mut group);
    for &msg_size in HOTPATH_MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            bench_native_xpub_to_xsub_delivered_one(b, &rt, s);
        });
    }
    group.finish();

    let mut group = c.benchmark_group("hotpath/native_delivered_latency/xsub_xpub_upstream");
    bench_runtime::configure_group(&mut group);
    for &msg_size in HOTPATH_MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            bench_native_xsub_to_xpub_delivered_one(b, &rt, s);
        });
    }
    group.finish();
}

fn bench_native_pub_delivered_one(
    b: &mut criterion::Bencher<'_>,
    rt: &BenchRuntime,
    n_subs: usize,
    msg_size: usize,
) {
    let (mut pub_sock, mut subs) = rt.block_on(async {
        let mut p = PubSocket::new();
        let bound = p
            .bind(hotpath_native_endpoint())
            .await
            .expect("pub bind")
            .to_string();
        let mut subs = Vec::with_capacity(n_subs);
        for _ in 0..n_subs {
            let mut sub = SubSocket::new();
            sub.connect(bound.as_str()).await.expect("sub connect");
            sub.subscribe("").await.expect("subscribe");
            subs.push(sub);
        }
        let subs = sync_zmqrs_pub_subscribers(&mut p, subs).await;
        (p, subs)
    });
    let payload = hotpath_payload(msg_size, 0xAB);

    b.iter(|| {
        rt.block_on(async {
            pub_sock
                .send(ZmqMessage::from(payload.clone()))
                .await
                .expect("pub send");
            for sub in &mut subs {
                let message = task::timeout(Duration::from_secs(1), sub.recv())
                    .await
                    .expect("sub timeout")
                    .expect("sub recv");
                black_box(message);
            }
        });
    });
}

fn bench_native_push_delivered_one(
    b: &mut criterion::Bencher<'_>,
    rt: &BenchRuntime,
    msg_size: usize,
) {
    let (mut push, mut pull) = rt.block_on(async {
        let mut pull = PullSocket::new();
        let bound = pull
            .bind(hotpath_native_endpoint())
            .await
            .expect("pull bind")
            .to_string();
        let mut push = PushSocket::new();
        push.connect(bound.as_str()).await.expect("push connect");
        task::sleep(Duration::from_millis(50)).await;
        (push, pull)
    });
    let payload = hotpath_payload(msg_size, 0xCD);

    b.iter(|| {
        rt.block_on(async {
            push.send(ZmqMessage::from(payload.clone()))
                .await
                .expect("push send");
            black_box(pull.recv().await.expect("pull recv"));
        });
    });
}

fn bench_native_dealer_delivered_one(
    b: &mut criterion::Bencher<'_>,
    rt: &BenchRuntime,
    msg_size: usize,
) {
    let (mut dealer, mut router) = rt.block_on(async {
        let mut router = RouterSocket::new();
        let bound = router
            .bind(hotpath_native_endpoint())
            .await
            .expect("router bind")
            .to_string();
        let mut dealer = DealerSocket::new();
        dealer
            .connect(bound.as_str())
            .await
            .expect("dealer connect");
        task::sleep(Duration::from_millis(50)).await;
        (dealer, router)
    });
    let payload = hotpath_payload(msg_size, 0xEF);

    b.iter(|| {
        rt.block_on(async {
            dealer
                .send(ZmqMessage::from(payload.clone()))
                .await
                .expect("dealer send");
            let message = router.recv().await.expect("router recv");
            router.send(message).await.expect("router send");
            black_box(dealer.recv().await.expect("dealer recv"));
        });
    });
}

fn bench_native_xpub_to_xsub_delivered_one(
    b: &mut criterion::Bencher<'_>,
    rt: &BenchRuntime,
    msg_size: usize,
) {
    let (mut xpub, mut xsub) = rt.block_on(async {
        let mut xpub = XPubSocket::new();
        let bound = xpub
            .bind(hotpath_native_endpoint())
            .await
            .expect("xpub bind")
            .to_string();
        let mut xsub = XSubSocket::new();
        xsub.connect(bound.as_str()).await.expect("xsub connect");
        xsub.subscribe("").await.expect("xsub subscribe");
        task::timeout(Duration::from_secs(2), xpub.recv())
            .await
            .expect("xpub subscription timeout")
            .expect("xpub subscription recv");
        task::sleep(Duration::from_millis(50)).await;
        (xpub, xsub)
    });
    let payload = hotpath_payload(msg_size, 0xA7);

    b.iter(|| {
        rt.block_on(async {
            xpub.send(ZmqMessage::from(payload.clone()))
                .await
                .expect("xpub send");
            black_box(xsub.recv().await.expect("xsub recv"));
        });
    });
}

fn bench_native_xsub_to_xpub_delivered_one(
    b: &mut criterion::Bencher<'_>,
    rt: &BenchRuntime,
    msg_size: usize,
) {
    let (mut xsub, mut xpub) = rt.block_on(async {
        let mut xpub = XPubSocket::new();
        let bound = xpub
            .bind(hotpath_native_endpoint())
            .await
            .expect("xpub bind")
            .to_string();
        let mut xsub = XSubSocket::new();
        xsub.connect(bound.as_str()).await.expect("xsub connect");
        task::sleep(Duration::from_millis(50)).await;
        (xsub, xpub)
    });
    let payload = hotpath_payload(msg_size, 0xA8);

    b.iter(|| {
        rt.block_on(async {
            xsub.send(ZmqMessage::from(payload.clone()))
                .await
                .expect("xsub send");
            black_box(xpub.recv().await.expect("xpub recv"));
        });
    });
}

fn bench_libzmq_delivered_latency(c: &mut Criterion) {
    for &n_subs in HOTPATH_DELIVERED_PUB_SUB_COUNTS {
        let mut group = c.benchmark_group(format!(
            "hotpath/libzmq_delivered_latency/pub_fanout/subs={n_subs}"
        ));
        bench_runtime::configure_group(&mut group);
        for &msg_size in HOTPATH_MSG_SIZES {
            group.throughput(Throughput::Bytes((msg_size * n_subs) as u64));
            group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
                bench_libzmq_pub_delivered_one(b, n_subs, s);
            });
        }
        group.finish();
    }

    let mut group = c.benchmark_group("hotpath/libzmq_delivered_latency/push_pull");
    bench_runtime::configure_group(&mut group);
    for &msg_size in HOTPATH_MSG_SIZES {
        group.throughput(Throughput::Bytes(msg_size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            bench_libzmq_push_delivered_one(b, s);
        });
    }
    group.finish();

    let mut group = c.benchmark_group("hotpath/libzmq_delivered_latency/dealer_router");
    bench_runtime::configure_group(&mut group);
    for &msg_size in HOTPATH_MSG_SIZES {
        group.throughput(Throughput::Bytes((msg_size * 2) as u64));
        group.bench_with_input(BenchmarkId::from_parameter(msg_size), &msg_size, |b, &s| {
            bench_libzmq_dealer_delivered_one(b, s);
        });
    }
    group.finish();
}

fn bench_libzmq_pub_delivered_one(b: &mut criterion::Bencher<'_>, n_subs: usize, msg_size: usize) {
    let ctx = zmq2::Context::new();
    let pub_sock = ctx.socket(zmq2::PUB).expect("pub socket");
    configure_libzmq_socket(&pub_sock);
    pub_sock.bind(hotpath_native_endpoint()).expect("pub bind");
    let bound = pub_sock
        .get_last_endpoint()
        .expect("last_endpoint")
        .expect("bound endpoint");

    let mut subs = Vec::with_capacity(n_subs);
    for _ in 0..n_subs {
        let sub = ctx.socket(zmq2::SUB).expect("sub socket");
        configure_libzmq_socket(&sub);
        sub.set_rcvtimeo(1000).expect("rcvtimeo");
        sub.connect(&bound).expect("sub connect");
        sub.set_subscribe(b"").expect("subscribe");
        subs.push(sub);
    }

    sync_libzmq_pub_subscribers(&pub_sock, &subs);
    let payload = vec![0xAB; msg_size];
    b.iter(|| {
        pub_sock.send(&payload, 0).expect("pub send");
        for sub in &subs {
            black_box(sub.recv_bytes(0).expect("sub recv"));
        }
    });
}

fn bench_libzmq_push_delivered_one(b: &mut criterion::Bencher<'_>, msg_size: usize) {
    let ctx = zmq2::Context::new();
    let pull = ctx.socket(zmq2::PULL).expect("pull socket");
    configure_libzmq_socket(&pull);
    pull.bind(hotpath_native_endpoint()).expect("pull bind");
    let bound = pull
        .get_last_endpoint()
        .expect("last_endpoint")
        .expect("bound endpoint");
    let push = ctx.socket(zmq2::PUSH).expect("push socket");
    configure_libzmq_socket(&push);
    push.connect(&bound).expect("push connect");
    thread::sleep(Duration::from_millis(50));

    let payload = vec![0xCD; msg_size];
    b.iter(|| {
        push.send(&payload, 0).expect("push send");
        black_box(pull.recv_bytes(0).expect("pull recv"));
    });
}

fn bench_libzmq_dealer_delivered_one(b: &mut criterion::Bencher<'_>, msg_size: usize) {
    let ctx = zmq2::Context::new();
    let router = ctx.socket(zmq2::ROUTER).expect("router socket");
    configure_libzmq_socket(&router);
    router.bind(hotpath_native_endpoint()).expect("router bind");
    let bound = router
        .get_last_endpoint()
        .expect("last_endpoint")
        .expect("bound endpoint");
    let dealer = ctx.socket(zmq2::DEALER).expect("dealer socket");
    configure_libzmq_socket(&dealer);
    dealer.connect(&bound).expect("dealer connect");
    thread::sleep(Duration::from_millis(50));

    let payload = vec![0xEF; msg_size];
    b.iter(|| {
        dealer.send(&payload, 0).expect("dealer send");
        let message = router.recv_multipart(0).expect("router recv");
        router
            .send_multipart(message, 0)
            .expect("router echo multipart");
        black_box(dealer.recv_bytes(0).expect("dealer recv"));
    });
}

fn bench_libzmq_pub_sub(c: &mut Criterion) {
    for transport in bench_runtime::selected_transports(bench_runtime::DEFAULT_TRANSPORTS) {
        for &n_subs in SUB_COUNTS {
            let mut group = c.benchmark_group(format!("libzmq/pub_sub/{transport}/subs={n_subs}"));
            bench_runtime::configure_group(&mut group);
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
        rx_done: mpsc::Receiver<Option<Vec<u8>>>,
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
            sub.set_rcvtimeo(20).expect("sub rcvtimeo");
            sub.connect(&bound).expect("sub connect");
            sub.set_subscribe(b"").expect("subscribe");
            while rx_drive.recv().is_ok() {
                let got = match sub.recv_bytes(0) {
                    Ok(message) => Some(message),
                    Err(zmq2::Error::EAGAIN) => None,
                    Err(error) => panic!("sub recv: {error:?}"),
                };
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

    let sync_payload = vec![0xFF];
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut ready = vec![false; subs.len()];
    while ready.iter().any(|is_ready| !*is_ready) {
        if Instant::now() > deadline {
            panic!("libzmq pub/sub sync timed out");
        }
        for (index, sub) in subs.iter().enumerate() {
            if !ready[index] {
                sub.tx_drive.send(()).expect("drive sync sub");
            }
        }
        pub_sock.send(&sync_payload, 0).expect("libzmq pub sync");
        for (index, sub) in subs.iter().enumerate() {
            if !ready[index] {
                if let Some(message) = sub.rx_done.recv().expect("sync sub done") {
                    black_box(message);
                    ready[index] = true;
                }
            }
        }
    }

    let payload = vec![0xAB; msg_size];
    b.iter(|| {
        for sub in &subs {
            sub.tx_drive.send(()).expect("drive sub");
        }
        pub_sock.send(&payload, 0).expect("pub send");
        for sub in &subs {
            let message = sub.rx_done.recv().expect("sub done").expect("sub payload");
            black_box(message);
        }
    });
}

fn bench_zmqrs_pub_sub(c: &mut Criterion) {
    let rt = build_rt();
    for transport in bench_runtime::selected_transports(bench_runtime::DEFAULT_TRANSPORTS) {
        for &n_subs in SUB_COUNTS {
            let mut group = c.benchmark_group(format!("zmqrs/pub_sub/{transport}/subs={n_subs}"));
            bench_runtime::configure_group(&mut group);
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
        let subs = sync_zmqrs_pub_subscribers(&mut p, subs).await;
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

    drop(subs);
    let errors = rt.block_on(pub_sock.close());
    assert!(errors.is_empty(), "pub close errors: {errors:?}");
}

fn bench_libzmq_req_rep(c: &mut Criterion) {
    for transport in bench_runtime::selected_transports(bench_runtime::DEFAULT_TRANSPORTS) {
        let mut group = c.benchmark_group(format!("libzmq/req_rep/{transport}"));
        bench_runtime::configure_group(&mut group);
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
    for transport in bench_runtime::selected_transports(bench_runtime::DEFAULT_TRANSPORTS) {
        let mut group = c.benchmark_group(format!("zmqrs/req_rep/{transport}"));
        bench_runtime::configure_group(&mut group);
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

    drop(req);
    let errors = rt.block_on(rep.close());
    assert!(errors.is_empty(), "rep close errors: {errors:?}");
}

fn bench_libzmq_push_pull(c: &mut Criterion) {
    for transport in bench_runtime::selected_transports(bench_runtime::DEFAULT_TRANSPORTS) {
        let mut group = c.benchmark_group(format!("libzmq/push_pull/{transport}"));
        bench_runtime::configure_group(&mut group);
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
    for transport in bench_runtime::selected_transports(bench_runtime::DEFAULT_TRANSPORTS) {
        let mut group = c.benchmark_group(format!("zmqrs/push_pull/{transport}"));
        bench_runtime::configure_group(&mut group);
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

    drop(push);
    let errors = rt.block_on(pull.close());
    assert!(errors.is_empty(), "pull close errors: {errors:?}");
}

fn bench_libzmq_dealer_router(c: &mut Criterion) {
    for transport in bench_runtime::selected_transports(bench_runtime::DEFAULT_TRANSPORTS) {
        let mut group = c.benchmark_group(format!("libzmq/dealer_router/{transport}"));
        bench_runtime::configure_group(&mut group);
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
    for transport in bench_runtime::selected_transports(bench_runtime::DEFAULT_TRANSPORTS) {
        let mut group = c.benchmark_group(format!("zmqrs/dealer_router/{transport}"));
        bench_runtime::configure_group(&mut group);
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

    drop(dealer);
    let errors = rt.block_on(router.close());
    assert!(errors.is_empty(), "router close errors: {errors:?}");
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
    bench_native_socket_send,
    bench_libzmq_socket_send,
    bench_native_delivered_latency,
    bench_libzmq_delivered_latency,
);
criterion_main!(benches);
