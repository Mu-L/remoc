use std::{io, sync::Arc};

use bytes::Bytes;
use futures::{SinkExt, StreamExt, channel::mpsc, future::join};
use remoc::{RemoteSend, prelude::*, rch::base, rtc::CallError};
use tokio::{
    sync::RwLock,
    time::{Duration, Instant, sleep_until},
};

const LATENCY: Duration = Duration::from_millis(150);
const ROUND_TRIP: Duration = LATENCY.saturating_mul(2);

#[rtc::remote]
trait Counter {
    async fn increase(&mut self, by: u32) -> Result<(), CallError>;
    async fn value(&self) -> Result<u32, CallError>;
}

struct CounterObj {
    value: u32,
}

impl Counter for CounterObj {
    async fn increase(&mut self, by: u32) -> Result<(), CallError> {
        self.value += by;
        Ok(())
    }

    async fn value(&self) -> Result<u32, CallError> {
        Ok(self.value)
    }
}

#[rtc::remote]
trait Directory {
    // This allows calls on the returned client to be sent before this call completes.
    #[pipelinable]
    async fn open_counter(&self) -> Result<CounterClient, CallError>;
}

struct DirectoryObj;

impl Directory for DirectoryObj {
    async fn open_counter(&self) -> Result<CounterClient, CallError> {
        let (server, client) = CounterServerSharedMut::new(Arc::new(RwLock::new(CounterObj { value: 0 })));
        tokio::spawn(server.serve());
        Ok(client)
    }
}

/// Forwards raw frames in one direction with [`LATENCY`] of simulated network delay.
///
/// Frames are queued as soon as they arrive so that the delay affects latency without
/// artificially limiting the link to one frame per [`LATENCY`].
async fn delayed_link(mut rx: mpsc::Receiver<Bytes>, mut tx: mpsc::Sender<Bytes>) {
    let (queued_tx, mut queued_rx) = mpsc::unbounded::<(Instant, Bytes)>();

    // Timestamp incoming frames independently, allowing several frames to be in flight.
    let receive = async move {
        while let Some(frame) = rx.next().await {
            if queued_tx.unbounded_send((Instant::now() + LATENCY, frame)).is_err() {
                break;
            }
        }
    };

    // Deliver each frame at its scheduled arrival time while preserving frame order.
    let send = async move {
        while let Some((arrival, frame)) = queued_rx.next().await {
            sleep_until(arrival).await;
            if tx.send(frame).await.is_err() {
                break;
            }
        }
    };

    join(receive, send).await;
}

/// Creates two connected Remoc endpoints over a simulated high-latency transport.
///
/// Each direction is delayed by [`LATENCY`], giving messages that require a response
/// the [`ROUND_TRIP`] delay printed by the example.
async fn delayed_loop_channel<T>() -> ((base::Sender<T>, base::Receiver<T>), (base::Sender<T>, base::Receiver<T>))
where
    T: RemoteSend,
{
    // Direction A -> B.
    let (a_tx, a_out) = mpsc::channel::<Bytes>(16);
    let (b_in, b_rx) = mpsc::channel::<Bytes>(16);
    tokio::spawn(delayed_link(a_out, b_in));

    // Direction B -> A.
    let (b_tx, b_out) = mpsc::channel::<Bytes>(16);
    let (a_in, a_rx) = mpsc::channel::<Bytes>(16);
    tokio::spawn(delayed_link(b_out, a_in));

    // Remoc expects fallible frame streams, while these in-memory streams cannot fail.
    let a_rx = a_rx.map(Ok::<_, io::Error>);
    let b_rx = b_rx.map(Ok::<_, io::Error>);
    let a_cfg = remoc::Cfg::default();
    let b_cfg = a_cfg.clone();

    // Establish both ends concurrently because each waits for its peer's handshake.
    let a = async move {
        let (connection, tx, rx) = remoc::Connect::framed(a_cfg, a_tx, a_rx).await.unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        (tx, rx)
    };
    let b = async move {
        let (connection, tx, rx) = remoc::Connect::framed(b_cfg, b_tx, b_rx).await.unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        (tx, rx)
    };

    join(a, b).await
}

async fn directory_client() -> DirectoryClient {
    let ((mut server_tx, _), (_, mut client_rx)) = delayed_loop_channel::<DirectoryClient>().await;
    let (server, client) = DirectoryServerShared::new(Arc::new(DirectoryObj));
    tokio::spawn(server.serve());
    server_tx.send(client).await.unwrap();
    client_rx.recv().await.unwrap().unwrap()
}

async fn sequential(dir: &DirectoryClient) -> Result<u32, CallError> {
    // Each await depends on the previous response, resulting in three round trips.
    let mut counter = dir.open_counter().await?;
    counter.increase(20).await?;
    counter.value().await
}

async fn pipelined(dir: &DirectoryClient) -> Result<u32, CallError> {
    // Create a placeholder client now; the directory will provide its receiver later.
    let (mut counter, counter_rx) = CounterClient::new();

    // Queue the dependent calls together. The remote side connects `counter_rx` to the
    // returned counter, so it can execute them without waiting for intermediate replies.
    let value = rtc::calls!(
        dir.open_counter_pipelined(counter_rx);
        counter.increase_call(20);
        counter.value_call()
    );
    
    Ok(value)
}

#[tokio::main]
async fn main() {
    println!("Simulated network round-trip time: {ROUND_TRIP:?}\n");

    let dir = directory_client().await;
    let started = Instant::now();
    let value = sequential(&dir).await.unwrap();
    let sequential_elapsed = started.elapsed();
    println!("Sequential: value = {value}, elapsed = {sequential_elapsed:?} (3 round trips)");

    let dir = directory_client().await;
    let started = Instant::now();
    let value = pipelined(&dir).await.unwrap();
    let pipelined_elapsed = started.elapsed();
    println!("Pipelined:  value = {value}, elapsed = {pipelined_elapsed:?} (1 round trip)");

    println!("\nPipelining saved about {:?}", sequential_elapsed.saturating_sub(pipelined_elapsed));
}
