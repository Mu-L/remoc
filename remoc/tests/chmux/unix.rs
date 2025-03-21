use futures::stream::StreamExt;
use std::{fs, time::Duration};
use tokio::{
    io::split,
    net::{UnixListener, UnixStream},
    time::sleep,
};
use tokio_util::codec::{length_delimited::LengthDelimitedCodec, FramedRead, FramedWrite};

use remoc::{chmux, exec};

async fn uds_server() {
    let _ = fs::remove_file("/tmp/chmux_test");
    let listener = UnixListener::bind("/tmp/chmux_test").unwrap();

    let (socket, _) = listener.accept().await.unwrap();
    let (socket_rx, socket_tx) = split(socket);
    let framed_tx = FramedWrite::new(socket_tx, LengthDelimitedCodec::new());
    let framed_rx = FramedRead::new(socket_rx, LengthDelimitedCodec::new());
    let framed_rx = framed_rx.map(|data| data.map(|b| b.freeze()));

    let mux_cfg = chmux::Cfg::default();
    let (mux, _, mut server) = chmux::ChMux::new(mux_cfg, framed_tx, framed_rx).await.unwrap();

    let mux_run = exec::spawn(async move { mux.run().await.unwrap() });

    while let Some((mut tx, mut rx)) = server.accept().await.unwrap() {
        println!("Server accepting request");

        tx.send("Hi from server".into()).await.unwrap();
        println!("Server sent Hi message");

        println!("Server dropping sender");
        drop(tx);
        println!("Server dropped sender");

        while let Some(msg) = rx.recv().await.unwrap() {
            println!("Server received: {}", String::from_utf8_lossy(&Vec::from(msg)));
        }
    }

    println!("Waiting for server mux to terminate...");
    mux_run.await.unwrap();
}

async fn uds_client() {
    let socket = UnixStream::connect("/tmp/chmux_test").await.unwrap();

    let (socket_rx, socket_tx) = split(socket);
    let framed_tx = FramedWrite::new(socket_tx, LengthDelimitedCodec::new());
    let framed_rx = FramedRead::new(socket_rx, LengthDelimitedCodec::new());
    let framed_rx = framed_rx.map(|data| data.map(|b| b.freeze()));

    let mux_cfg = chmux::Cfg::default();
    let (mux, client, _) = chmux::ChMux::new(mux_cfg, framed_tx, framed_rx).await.unwrap();
    let mux_run = exec::spawn(async move { mux.run().await.unwrap() });

    {
        let client = client;

        println!("Client connecting...");
        let (mut tx, mut rx) = client.connect().await.unwrap();
        println!("Client connected");

        tx.send("Hi from client".into()).await.unwrap();
        println!("Client sent Hi message");

        println!("Client dropping sender");
        drop(tx);
        println!("Client dropped sender");

        while let Some(msg) = rx.recv().await.unwrap() {
            println!("Client received: {}", String::from_utf8_lossy(&Vec::from(msg)));
        }

        println!("Client closing connection...");
    }

    println!("Waiting for client mux to terminate...");
    mux_run.await.unwrap();
}

#[tokio::test]
async fn uds_test() {
    crate::init();

    println!("Starting server task...");
    let server_task = exec::spawn(uds_server());
    sleep(Duration::from_millis(100)).await;

    println!("String client thread...");
    let client_task = exec::spawn(uds_client());

    println!("Waiting for server task...");
    server_task.await.unwrap();
    println!("Waiting for client thread...");
    client_task.await.unwrap();
}
