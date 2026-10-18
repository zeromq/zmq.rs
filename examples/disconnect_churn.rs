//! Churns short-lived clients against a REP, a bound SUB and a bound PUB that
//! share one runtime, and reports whether the server keeps answering.
//!
//! Each REQ client connects, sends one request and leaves; one in eight
//! leaves with the reply unread, which resets the connection. Each PUB client
//! connects, sends one message and leaves. Each SUB client connects, waits
//! for one message from the server's PUB, which publishes without pause, and
//! leaves. A watchdog on an OS thread reports
//! a stall when no request was answered for 5 s, so it still fires when the
//! runtime itself is blocked.
//!
//! ```text
//! cargo run --release --example disconnect_churn -- [secs] [churners] [workers] [ipc|tcp]
//! ```

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use zeromq::prelude::*;
use zeromq::{PubSocket, RepSocket, ReqSocket, SocketOptions, SubSocket, ZmqMessage};

const STALL: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Counts {
    replies: AtomicU64,
    req_failed: AtomicU64,
    pub_clients: AtomicU64,
    sub_clients: AtomicU64,
}

fn options() -> SocketOptions {
    let mut options = SocketOptions::default();
    options.connect_timeout(Duration::from_secs(1));
    options
}

async fn req_client(endpoint: &str, read_reply: bool, counts: &Counts) {
    let mut req = ReqSocket::with_options(options());
    let sent =
        req.connect(endpoint).await.is_ok() && req.send(ZmqMessage::from("ping")).await.is_ok();
    if !sent {
        counts.req_failed.fetch_add(1, Ordering::Relaxed);
    } else if read_reply {
        match tokio::time::timeout(Duration::from_secs(2), req.recv()).await {
            Ok(Ok(_)) => counts.replies.fetch_add(1, Ordering::Relaxed),
            _ => counts.req_failed.fetch_add(1, Ordering::Relaxed),
        };
    }
}

async fn pub_client(endpoint: &str, counts: &Counts) {
    let mut publisher = PubSocket::with_options(options());
    if publisher.connect(endpoint).await.is_ok() {
        let _ = publisher.send(ZmqMessage::from("data")).await;
        counts.pub_clients.fetch_add(1, Ordering::Relaxed);
    }
}

async fn sub_client(endpoint: &str, counts: &Counts) {
    let mut subscriber = SubSocket::with_options(options());
    if subscriber.connect(endpoint).await.is_ok() && subscriber.subscribe("").await.is_ok() {
        let _ = tokio::time::timeout(Duration::from_secs(1), subscriber.recv()).await;
        counts.sub_clients.fetch_add(1, Ordering::Relaxed);
    }
}

fn endpoint(transport: &str, name: &str) -> String {
    if transport == "tcp" {
        return "tcp://127.0.0.1:0".into();
    }
    let path = std::env::temp_dir().join(format!("zmq-churn-{name}-{}.sock", std::process::id()));
    format!("ipc://{}", path.display())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let secs: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(30);
    let churners: usize = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(4);
    let workers: usize = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(2);
    let transport = args.get(4).cloned().unwrap_or_else(|| "ipc".into());

    let counts = Arc::new(Counts::default());
    let started = Instant::now();
    let watchdog_counts = counts.clone();
    let watchdog_transport = transport.clone();
    std::thread::spawn(move || {
        let mut last = 0;
        let mut last_progress = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let now = watchdog_counts.replies.load(Ordering::Relaxed);
            if now != last {
                last = now;
                last_progress = Instant::now();
            } else if last_progress.elapsed() > STALL {
                println!(
                    "transport={watchdog_transport} workers={workers} churners={churners} result=STALLED after {:.1}s, replies={now}",
                    started.elapsed().as_secs_f64(),
                );
                std::process::exit(2);
            }
        }
    });

    let runtime = |threads| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(threads)
            .enable_all()
            .build()
            .unwrap()
    };
    let server = runtime(workers);
    let clients = runtime(churners);
    let (rep_endpoint, sub_endpoint, pub_endpoint) = server.block_on(async {
        let mut rep = RepSocket::new();
        let rep_endpoint = rep
            .bind(&endpoint(&transport, "rep"))
            .await
            .unwrap()
            .to_string();
        let mut sub = SubSocket::new();
        sub.subscribe("").await.unwrap();
        let sub_endpoint = sub
            .bind(&endpoint(&transport, "sub"))
            .await
            .unwrap()
            .to_string();
        let mut publisher = PubSocket::new();
        let pub_endpoint = publisher
            .bind(&endpoint(&transport, "pub"))
            .await
            .unwrap()
            .to_string();

        tokio::spawn(async move {
            loop {
                if let Ok(m) = rep.recv().await {
                    let _ = rep.send(m).await;
                }
            }
        });
        tokio::spawn(async move {
            loop {
                let _ = sub.recv().await;
            }
        });
        tokio::spawn(async move {
            loop {
                let _ = publisher.send(ZmqMessage::from("tick")).await;
                tokio::task::yield_now().await;
            }
        });
        (rep_endpoint, sub_endpoint, pub_endpoint)
    });

    clients.block_on(async move {
        let until = Instant::now() + Duration::from_secs(secs);
        let mut tasks = Vec::new();
        for _ in 0..churners {
            let (endpoint, req_counts) = (rep_endpoint.clone(), counts.clone());
            tasks.push(tokio::spawn(async move {
                let mut n: u64 = 0;
                while Instant::now() < until {
                    n += 1;
                    req_client(&endpoint, n % 8 != 0, &req_counts).await;
                }
            }));
            let (endpoint, pub_counts) = (sub_endpoint.clone(), counts.clone());
            tasks.push(tokio::spawn(async move {
                while Instant::now() < until {
                    pub_client(&endpoint, &pub_counts).await;
                }
            }));
            let (endpoint, sub_counts) = (pub_endpoint.clone(), counts.clone());
            tasks.push(tokio::spawn(async move {
                while Instant::now() < until {
                    sub_client(&endpoint, &sub_counts).await;
                }
            }));
        }
        for task in tasks {
            let _ = task.await;
        }

        let final_reply = {
            let probe = Counts::default();
            req_client(&rep_endpoint, true, &probe).await;
            probe.replies.load(Ordering::Relaxed) == 1
        };
        let elapsed = started.elapsed().as_secs_f64();
        let replies = counts.replies.load(Ordering::Relaxed);
        println!(
            "transport={transport} workers={workers} churners={churners} secs={elapsed:.1} replies={replies} ({:.0}/s) req_failed={} pub_clients={} sub_clients={} final_reply={final_reply} result={}",
            replies as f64 / elapsed,
            counts.req_failed.load(Ordering::Relaxed),
            counts.pub_clients.load(Ordering::Relaxed),
            counts.sub_clients.load(Ordering::Relaxed),
            if final_reply { "ok" } else { "STALLED" },
        );
        std::process::exit(if final_reply { 0 } else { 2 });
    });
}
