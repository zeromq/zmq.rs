use zeromq::prelude::*;
use zeromq::{RepSocket, ZmqMessage};
use zeromq::__async_rt as async_rt;

use futures::StreamExt;
use std::error::Error;
use std::time::Duration;
use std::str;

async fn run_rep_server(mut rep_socket: RepSocket) -> Result<(), Box<dyn Error>> {
    println!("Started rep server on tcp://127.0.0.1:5557");

    for i in 0..10i32 {
        let mess = rep_socket.recv().await?;
	let m = format!("{} Rep - {}", str::from_utf8(mess.get(0).unwrap().as_ref()).unwrap(),i);
	let repl = ZmqMessage::from(m);
        rep_socket.send(repl).await?;
    }
    // yield for a moment to ensure that server has some time to flush socket
    let errs = rep_socket.close().await;
    if !errs.is_empty() {
        panic!("Could not unbind socket: {:?}", errs);
    }
    Ok(())
}

#[async_rt::test]
async fn test_req_rep_sockets() -> Result<(), Box<dyn Error>> {
    pretty_env_logger::try_init().ok();

    let mut rep_socket = zeromq::RepSocket::new();
    let monitor = rep_socket.monitor();
    let endpoint = rep_socket.bind("tcp://localhost:0").await?;
    println!("Started rep server on {}", endpoint);

    async_rt::task::spawn(async {
        run_rep_server(rep_socket).await.unwrap();
    });

    let mut req_socket = zeromq::ReqSocket::new();
    req_socket.connect(endpoint.to_string().as_str()).await?;

    for i in 0..10i32 {
	let ms: String = format!("Req - {}", i);
	let m = ZmqMessage::from(ms);
        req_socket.send(m).await?;
        let repl = req_socket.recv().await?;
        assert_eq!(format!("Req - {} Rep - {}", i, i), String::from_utf8(repl.get(0).unwrap().to_vec()).unwrap())
    }
    req_socket.close().await;
    let events: Vec<_> = monitor.collect().await;
    assert_eq!(2, events.len(), "{:?}", &events);
    Ok(())
}

#[async_rt::test]
async fn test_many_req_rep_sockets() -> Result<(), Box<dyn Error>> {
    pretty_env_logger::try_init().ok();

    let mut rep_socket = zeromq::RepSocket::new();
    let endpoint = rep_socket.bind("tcp://localhost:0").await?;
    println!("Started rep server on {}", endpoint);

    for i in 0..100i32 {
        let cloned_endpoint = endpoint.to_string();
        async_rt::task::spawn(async move {
            // yield for a moment to ensure that server has some time to open socket
            async_rt::task::sleep(Duration::from_millis(100)).await;
            let mut req_socket = zeromq::ReqSocket::new();
            req_socket.connect(&cloned_endpoint).await.unwrap();

            for j in 0..100i32 {
		let ms: String = format!("Socket {} Req - {}", i, j);
		let m = ZmqMessage::from(ms);
                req_socket.send(m).await.unwrap();
                let repl = req_socket.recv().await.unwrap();
                assert_eq!(format!("Socket {} Req - {} Rep", i, j), String::from_utf8(repl.get(0).unwrap().to_vec()).unwrap());
            }
            drop(req_socket);
        });
    }

    for _ in 0..10000i32 {
        let mess = rep_socket.recv().await?;
	let mut payload = String::from_utf8(mess.get(0).unwrap().to_vec()).unwrap();
	println!("{}",payload);
	payload.push_str(" Rep");
	println!("{}",payload);
	let repl = ZmqMessage::from(payload);
        rep_socket.send(repl).await?;
    }
    Ok(())
}
