use criterion::{black_box, criterion_group, criterion_main, Bencher, Criterion};
use tokio::runtime::Runtime;

use zeromq::{prelude::*, PubSocket, SubSocket};

/// Binds n pubs and connects n subs to them
async fn setup(m_pubs: u8, n_subs: u8) -> (Vec<zeromq::PubSocket>, Vec<zeromq::SubSocket>) {
    let mut pubs = Vec::new();
    let mut subs: Vec<_> = (0..n_subs).map(|_| SubSocket::new()).collect();

    for _ in 0..m_pubs {
        let mut p = PubSocket::new();
        let bind_endpoint = p.bind("tcp://localhost:0").await.unwrap();
        for s in subs.iter_mut() {
            s.connect(bind_endpoint.clone()).await.unwrap();
        }
        pubs.push(p);
    }
    (pubs, subs)
}

fn criterion_benchmark(c: &mut Criterion) {
    let mut rt = Runtime::new().unwrap();
    let (mut pubs, mut subs) = rt.block_on(setup(4, 16));

    c.bench_function("m pubs and n subs messaging", |b| {
        b.iter(|| rt.block_on(f(&mut pubs, &mut subs)))
    });

    async fn f(pubs: &mut [PubSocket], subs: &mut [SubSocket]) {
        // Send/Recv messages
    }
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
