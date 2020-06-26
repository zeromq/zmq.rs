use crate::{BlockingRecv, BlockingSend, Socket, SocketFrontend};
use std::convert::TryInto;
use std::error::Error;
use std::time::Duration;

#[tokio::test]
async fn test_pub_sub_sockets() {
    tokio::spawn(async move {
        let mut pub_socket = crate::PubSocket::new();
        pub_socket
            .bind("127.0.0.1:5556")
            .await
            .expect("Failed to bind socket");

        for i in 0..10i32 {
            let message = format!("Message - {}", i);
            pub_socket
                .send(message.into())
                .await
                .expect("Failed to send");
        }
    });

    tokio::spawn(async move {
        let mut sub_socket = crate::SubSocket::connect("127.0.0.1:5556")
            .await
            .expect("Failed to connect");

        sub_socket.subscribe("").await.expect("Failed to subscribe");

        for i in 0..10i32 {
            let repl: String = sub_socket
                .recv()
                .await
                .expect("Failed to recv")
                .try_into()
                .expect("Malformed string");
            assert_eq!(format!("Message - {}", i), repl)
        }
    });
}

async fn run_rep_server() -> Result<(), Box<dyn Error>> {
    let mut rep_socket = crate::RepSocket::new();
    rep_socket.bind("127.0.0.1:5557").await?;
    println!("Started rep server on 127.0.0.1:5557");

    for i in 0..10i32 {
        let mess: String = rep_socket.recv().await?.try_into()?;
        rep_socket
            .send(format!("{} Rep - {}", mess, i).into())
            .await?;
    }
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
    println!("Connected to server");

    for i in 0..10i32 {
        req_socket.send(format!("Req - {}", i).into()).await?;
        let repl: String = req_socket.recv().await?.try_into()?;
        assert_eq!(format!("Req - {} Rep - {}", i, i), repl)
    }
    Ok(())
}
