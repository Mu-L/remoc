use std::{
    marker::PhantomData,
    sync::{Arc, Mutex},
};

use futures::FutureExt;
use serde::{ser, Deserialize, Serialize};

use super::{ConnectError, Interlock, Location};
use crate::remote::{self, PortDeserializer, PortSerializer};

enum ReceivableSender<T, Codec> {
    ToReceive(tokio::sync::mpsc::UnboundedReceiver<Result<remote::Sender<T, Codec>, ConnectError>>),
    Received(Result<remote::Sender<T, Codec>, ConnectError>),
}

impl<T, Codec> ReceivableSender<T, Codec> {
    async fn get(&mut self) -> Result<&mut remote::Sender<T, Codec>, ConnectError> {
        if let Self::ToReceive(rx) = self {
            *self = Self::Received(rx.recv().await.unwrap_or(Err(ConnectError::Dropped)));
        }

        if let Self::Received(sender) = self {
            sender.as_mut().map_err(|err| err.clone())
        } else {
            unreachable!()
        }
    }
}

/// A local-remote channel sender.
pub struct Sender<T, Codec> {
    pub(super) sender: ReceivableSender<T, Codec>,
    pub(super) receiver_tx:
        Option<tokio::sync::mpsc::UnboundedSender<Result<remote::Receiver<T, Codec>, ConnectError>>>,
    pub(super) interlock: Arc<Mutex<Interlock>>,
}

/// A local-remote channel sender in transport.
#[derive(Debug, Serialize, Deserialize)]
pub struct TransportedSender<T, Codec> {
    /// chmux port number.
    pub port: u32,
    /// Data type.
    pub data: PhantomData<T>,
    /// Data codec.
    pub codec: PhantomData<Codec>,
}

impl<T, Codec> Sender<T, Codec> {
    /// Sends a value over this channel to the remote endpoint.
    pub async fn send(&mut self, value: T) -> Result<(), SendError<T>> {
        self.sender.get().await?.send(value)
    }
}

impl Serialize for Sender {
    /// Serializes this sender for sending over a chmux channel.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let receiver_tx =
            self.receiver_tx.clone().ok_or_else(|| ser::Error::custom("cannot forward received sender"))?;

        let interlock_confirm = {
            let mut interlock = self.interlock.lock().unwrap();
            if !interlock.receiver.is_local() {
                return Err(ser::Error::custom("cannot send sender because receiver has been sent"));
            }
            interlock.receiver.start_send()
        };

        let port = PortSerializer::connect(|connect, _| {
            async move {
                let _ = interlock_confirm.send(());

                match connect.await {
                    Ok((_, raw_rx)) => {
                        let _ = receiver_tx.send(Ok(raw_rx));
                    }
                    Err(err) => {
                        let _ = receiver_tx.send(Err(ConnectError::Connect(err)));
                    }
                }
            }
            .boxed()
        })?;

        TransportedSender { port }.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Sender {
    /// Deserializes this sender after it has been received over a chmux channel.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let TransportedSender { port } = TransportedSender::deserialize(deserializer)?;

        let (sender_tx, sender_rx) = tokio::sync::mpsc::unbounded_channel();
        PortDeserializer::accept(port, |local_port, request, _| {
            async move {
                match request.accept_from(local_port).await {
                    Ok((raw_tx, _)) => {
                        let _ = sender_tx.send(Ok(raw_tx));
                    }
                    Err(err) => {
                        let _ = sender_tx.send(Err(ConnectError::Accept(err)));
                    }
                }
            }
            .boxed()
        })?;

        Ok(Self {
            sender_rx,
            receiver_tx: None,
            interlock: Arc::new(Mutex::new(Interlock { sender: Location::Local, receiver: Location::Remote })),
        })
    }
}
