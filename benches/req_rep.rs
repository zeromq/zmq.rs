use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::convert::TryInto;
use std::time::Duration;
use tokio::runtime::Runtime;

use zeromq::{prelude::*, RepSocket, ReqSocket};

async fn setup() -> (ReqSocket, RepSocket) {
    let mut rep_socket = RepSocket::new();
    let bind_endpoint = rep_socket
        .bind("tcp://localhost:0")
        .await
        .expect("failed to bind rep");
    println!("Bound rep socket to {}", &bind_endpoint);

    let mut req_socket = ReqSocket::new();
    req_socket
        .connect(bind_endpoint)
        .await
        .expect("Failed to connect req");

    (req_socket, rep_socket)
}

fn criterion_benchmark(c: &mut Criterion) {
    let mut rt = Runtime::new().unwrap();

    let (req, rep) = rt.block_on(setup());
    let (mut req, mut rep) = (Some(req), Some(rep));

    const N_MSG: u32 = 512;

    c.bench_function("1 req and 1 rep messaging", |b| {
        b.iter(|| rt.block_on(f(&mut req, &mut rep)))
    });

    async fn f(req: &mut Option<ReqSocket>, rep: &mut Option<RepSocket>) {
        let mut req_owned = req.take().unwrap();
        let mut rep_owned = rep.take().unwrap();
        let rep_handle = tokio::spawn(async move {
            for i in 0..N_MSG {
                let mess: String = rep_owned
                    .recv()
                    .await
                    .expect("Rep failed to receive")
                    .try_into()
                    .unwrap();
                rep_owned
                    .send(format!("{} Rep - {}", mess, i).into())
                    .expect("Rep failed to send");
            }
            // yield for a moment to ensure that server has some time to flush socket
            // tokio::time::delay_for(Duration::from_millis(100)).await;
            rep_owned
        });

        for i in 0..N_MSG {
            req_owned
                .send(format!("Req - {}", i).into())
                .await
                .expect("Req failed to send");
            let repl: String = req_owned
                .recv()
                .await
                .expect("Req failed to recv")
                .try_into()
                .unwrap();
            assert_eq!(format!("Req - {0} Rep - {0}", i), repl);
            black_box(repl);
        }

        let rep_owned = rep_handle.await.expect("Rep task failed");
        req.replace(req_owned);
        rep.replace(rep_owned);
    }
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(128)
        .measurement_time(Duration::from_secs(32))
        .warm_up_time(Duration::from_secs(5));
    targets = criterion_benchmark
}
criterion_main!(benches);
