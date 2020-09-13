use crate::{BlockingRecv, BlockingSend, NonBlockingSend, Socket};
use chrono::Utc;
use futures::channel::{mpsc, oneshot};
use futures::{SinkExt, StreamExt};
use std::convert::TryInto;
use std::error::Error;
use std::time::Duration;

#[tokio::test]
async fn test_pub_sub_sockets() {
    let (server_stop_sender, mut server_stop) = oneshot::channel::<()>();
    let (results_sender, results) = mpsc::channel(100);
    tokio::spawn(async move {
        let mut pub_socket = crate::PubSocket::new();
        pub_socket
            .bind("127.0.0.1:5556")
            .await
            .expect("Failed to bind socket");

        loop {
            if let Ok(Some(_)) = server_stop.try_recv() {
                break;
            }
            pub_socket
                .send(Utc::now().to_rfc2822().into())
                .expect("Failed to send");
            tokio::time::delay_for(Duration::from_millis(100)).await;
        }
    });

    for _ in 0..10 {
        let mut client_sender = results_sender.clone();
        tokio::spawn(async move {
            let mut sub_socket = crate::SubSocket::new();
            sub_socket
                .connect("127.0.0.1:5556")
                .await
                .expect("Failed to connect");

            sub_socket.subscribe("").await.expect("Failed to subscribe");

            for _ in 0..10i32 {
                let repl: String = sub_socket
                    .recv()
                    .await
                    .expect("Failed to recv")
                    .try_into()
                    .expect("Malformed string");
                assert_eq!(Utc::now().to_rfc2822(), repl);
                client_sender.send(true).await.unwrap();
            }
        });
    }
    drop(results_sender);

    let res_vec: Vec<bool> = results.collect().await;
    server_stop_sender.send(()).unwrap();
    assert_eq!(100, res_vec.len());
}

async fn run_rep_server() -> Result<(), Box<dyn Error>> {
    let mut rep_socket = crate::RepSocket::new();
    rep_socket.bind("127.0.0.1:5557").await?;
    println!("Started rep server on 127.0.0.1:5557");

    for i in 0..10i32 {
        let mess: String = rep_socket.recv().await?.try_into()?;
        rep_socket.send(format!("{} Rep - {}", mess, i).into())?;
    }
    // yield for a moment to ensure that server has some time to flush socket
    tokio::time::delay_for(Duration::from_millis(100)).await;
    Ok(())
}

#[tokio::test]
async fn test_req_rep_sockets() -> Result<(), Box<dyn Error>> {
    tokio::spawn(async move {
        run_rep_server().await.unwrap();
    });

    // yield for a moment to ensure that server has some time to open socket
    tokio::time::delay_for(Duration::from_millis(10)).await;
    let mut req_socket = crate::ReqSocket::new();
    req_socket.connect("127.0.0.1:5557").await?;

    for i in 0..10i32 {
        req_socket.send(format!("Req - {}", i).into()).await?;
        let repl: String = req_socket.recv().await?.try_into()?;
        assert_eq!(format!("Req - {} Rep - {}", i, i), repl)
    }
    Ok(())
}

#[tokio::test]
async fn test_many_req_rep_sockets() -> Result<(), Box<dyn Error>> {
    for i in 0..100i32 {
        tokio::spawn(async move {
            // yield for a moment to ensure that server has some time to open socket
            tokio::time::delay_for(Duration::from_millis(100)).await;
            let mut req_socket = crate::ReqSocket::new();
            req_socket.connect("127.0.0.1:5558").await.unwrap();

            for j in 0..100i32 {
                req_socket
                    .send(format!("Socket {} Req - {}", i, j).into())
                    .await
                    .unwrap();
                let repl: String = req_socket.recv().await.unwrap().try_into().unwrap();
                assert_eq!(format!("Socket {} Req - {} Rep", i, j), repl)
            }
            drop(req_socket);
        });
    }

    let mut rep_socket = crate::RepSocket::new();
    rep_socket.bind("127.0.0.1:5558").await?;
    println!("Started rep server on 127.0.0.1:5558");

    for _ in 0..10000i32 {
        let mess: String = rep_socket.recv().await?.try_into()?;
        rep_socket.send(format!("{} Rep", mess).into())?;
    }
    Ok(())
}
