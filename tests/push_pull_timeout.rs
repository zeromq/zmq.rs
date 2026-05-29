#![cfg(feature = "tokio-runtime")]

use bytes::Bytes;
use zeromq::prelude::*;
use zeromq::ZmqMessage;

use std::net::TcpListener;
use std::process::Stdio;
use std::time::{Duration, Instant};

const REPRO_CHILD_TEST_NAME: &str = "issue_248_repro_child_process";
const REPRO_ROLE_ENV: &str = "ZMQ_RS_ISSUE_248_ROLE";
const REPRO_ENDPOINT_ENV: &str = "ZMQ_RS_ISSUE_248_ENDPOINT";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pull_recv_with_per_call_timeout_keeps_making_progress() {
    const TEST_WINDOW: Duration = Duration::from_secs(3);
    const WATCHDOG: Duration = Duration::from_secs(5);

    let mut push = zeromq::PushSocket::new();
    let endpoint = push.bind("tcp://127.0.0.1:0").await.unwrap().to_string();

    let mut pull = zeromq::PullSocket::new();
    pull.connect(&endpoint).await.unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;

    let sender = tokio::spawn(async move {
        let payload = Bytes::from(vec![b'x'; 128]);
        loop {
            match push.send(ZmqMessage::from(payload.clone())).await {
                Ok(()) => {}
                Err(_) => tokio::task::yield_now().await,
            }
        }
    });

    let received = tokio::time::timeout(WATCHDOG, async {
        let start = Instant::now();
        let deadline = start + TEST_WINDOW;
        let mut count = 0;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break count;
            }
            match tokio::time::timeout(remaining, pull.recv()).await {
                Ok(Ok(_)) => count += 1,
                Ok(Err(err)) => panic!("recv failed after {count} messages: {err:?}"),
                Err(err) => {
                    if Instant::now() >= deadline {
                        break count;
                    }
                    panic!("per-call timeout fired after {count} messages: {err:?}");
                }
            }
        }
    })
    .await;

    sender.abort();
    let _ = sender.await;

    let count = received.expect("PullSocket::recv stopped making progress when wrapped in timeout");
    println!("received {count} messages");
    assert!(
        count > 10_000,
        "received too few messages before deadline: {count}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pull_recv_with_per_call_timeout_keeps_making_progress_across_processes() {
    let endpoint = unused_tcp_endpoint();
    let test_exe = std::env::current_exe().unwrap();
    let child_args = ["--exact", REPRO_CHILD_TEST_NAME, "--nocapture"];

    let mut push = tokio::process::Command::new(&test_exe)
        .args(child_args)
        .env(REPRO_ROLE_ENV, "push")
        .env(REPRO_ENDPOINT_ENV, &endpoint)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut pull = tokio::process::Command::new(&test_exe)
        .args(child_args)
        .env(REPRO_ROLE_ENV, "pull")
        .env(REPRO_ENDPOINT_ENV, &endpoint)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let pull_status = tokio::time::timeout(Duration::from_secs(6), pull.wait()).await;
    push.start_kill().unwrap();
    let _ = push.wait().await;

    let pull_status = pull_status
        .expect("PullSocket::recv stopped making progress when wrapped in timeout")
        .unwrap();
    assert!(
        pull_status.success(),
        "pull child exited with {pull_status}"
    );
}

#[test]
fn issue_248_repro_child_process() {
    let Some(role) = std::env::var(REPRO_ROLE_ENV).ok() else {
        return;
    };
    let endpoint = std::env::var(REPRO_ENDPOINT_ENV).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async move {
        match role.as_str() {
            "push" => run_issue_248_push_child(&endpoint).await,
            "pull" => run_issue_248_pull_child(&endpoint).await,
            other => panic!("unknown repro role: {other}"),
        }
    });
}

async fn run_issue_248_push_child(endpoint: &str) {
    let mut socket = zeromq::PushSocket::new();
    socket.bind(endpoint).await.unwrap();
    let payload = Bytes::from(vec![b'x'; 128]);
    loop {
        if socket
            .send(ZmqMessage::from(payload.clone()))
            .await
            .is_err()
        {
            tokio::task::yield_now().await;
        }
    }
}

async fn run_issue_248_pull_child(endpoint: &str) {
    let mut socket = zeromq::PullSocket::new();
    socket.connect(endpoint).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let start = Instant::now();
    let deadline = start + Duration::from_secs(3);
    let mut count = 0;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.recv()).await {
            Ok(Ok(_)) => count += 1,
            Ok(Err(err)) => panic!("recv failed after {count} messages: {err:?}"),
            Err(err) => {
                if Instant::now() >= deadline {
                    break;
                }
                panic!("per-call timeout fired after {count} messages: {err:?}");
            }
        }
    }
    assert!(
        count > 10_000,
        "received too few messages before deadline: {count}"
    );
}

fn unused_tcp_endpoint() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("tcp://127.0.0.1:{port}")
}
