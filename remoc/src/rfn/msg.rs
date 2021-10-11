//! Messages exchanged between remote functions and their providers.

use serde::{Deserialize, Serialize};

use crate::{
    codec::CodecT,
    rsync::{oneshot, RemoteSend},
};

/// Remote function call request.
#[derive(Serialize, Deserialize)]
#[serde(bound(serialize = "A: RemoteSend, R: RemoteSend, Codec: CodecT"))]
#[serde(bound(deserialize = "A: RemoteSend, R: RemoteSend, Codec: CodecT"))]
pub struct RFnRequest<A, R, Codec> {
    pub argument: A,
    pub result_tx: oneshot::Sender<R, Codec>,
}