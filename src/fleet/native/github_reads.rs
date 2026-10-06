//! Independent bounded GitHub work on the authenticated fleet connection.
use super::{Result, replica::invalid};
use hey_gh::{
    ApiClient, Error,
    shared_read::{Request, Response},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};
use tokio::{
    runtime::Runtime,
    sync::{OwnedSemaphorePermit, Semaphore},
    task::AbortHandle,
};

const PEER_LIMIT: usize = 4;
static SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(8)));

pub(super) struct Completed {
    pub id: String,
    pub response: Response,
    // Buffered replies still own their memory/concurrency slot until drained.
    _slot: OwnedSemaphorePermit,
    _peer: OwnedSemaphorePermit,
}

pub(super) struct Backend {
    runtime: Option<Runtime>,
    client: ApiClient,
    pending: Mutex<BTreeMap<String, AbortHandle>>,
    completed: mpsc::Receiver<Completed>,
    outgoing: mpsc::SyncSender<Completed>,
    slots: Arc<Semaphore>,
    peer_slots: Arc<Semaphore>,
    last_id: AtomicU64,
}
impl Backend {
    pub fn new(client: ApiClient) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("github-read-relay")
            .enable_all()
            .build()?;
        let (outgoing, completed) = mpsc::sync_channel(PEER_LIMIT);
        Ok(Self {
            runtime: Some(runtime),
            client,
            pending: Mutex::new(BTreeMap::new()),
            completed,
            outgoing,
            slots: SLOTS.clone(),
            peer_slots: Arc::new(Semaphore::new(PEER_LIMIT)),
            last_id: AtomicU64::new(0),
        })
    }
    pub fn receive(&self, message: &Value) -> Result<Option<Value>> {
        if super::context::encode_frame(message, 32 * 1024)?.is_none() {
            return Err(invalid("GitHub relay request exceeds 32 KiB"));
        }
        let id = message["id"]
            .as_str()
            .ok_or_else(|| invalid("Missing GitHub relay request ID"))?;
        if message["kind"] == "github_cancel" {
            self.cancel(id);
            return Ok(None);
        }
        if message["kind"] != "github_read" {
            return Err(invalid("Unsupported GitHub relay message"));
        }
        let request = serde_json::from_value(message["request"].clone())?;
        self.submit(id, request)?
            .as_ref()
            .map(|response| frame(id, response))
            .transpose()
    }
    pub fn submit(&self, id: &str, request: Request) -> Result<Option<Response>> {
        let sequence = id
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0 && n.to_string() == id);
        let Some(sequence) = sequence else {
            return Err(invalid("Invalid GitHub relay request ID"));
        };
        // IDs are minted under the companion's output lock. A monotonic scalar
        // fences canceled/completed work without an unbounded set of old IDs.
        if self
            .last_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |last| {
                (sequence > last).then_some(sequence)
            })
            .is_err()
        {
            return Err(invalid("Duplicate or reordered GitHub relay request ID"));
        }
        let validation = match &request {
            Request::Probe { identity } => identity.validate(),
            Request::Read { read } => read.validate(),
        };
        if let Err(error) = validation {
            return Ok(Some(Response::Reply {
                reply: error.into(),
            }));
        }
        let mut pending = self.pending.lock().unwrap();
        if pending.contains_key(id) {
            return Err(invalid("Duplicate GitHub relay request ID"));
        }
        if pending.len() >= PEER_LIMIT {
            return Ok(Some(Response::Unavailable));
        }
        let Ok(peer) = self.peer_slots.clone().try_acquire_owned() else {
            return Ok(Some(Response::Unavailable));
        };
        let Ok(slot) = self.slots.clone().try_acquire_owned() else {
            return Ok(Some(Response::Unavailable));
        };
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(match &request {
                Request::Probe { .. } => 5000,
                Request::Read { read } => read.timeout_ms,
            });
        let client = self.client.clone().with_read_deadline(deadline);
        let outgoing = self.outgoing.clone();
        let key = id.to_owned();
        let task = self.runtime.as_ref().unwrap().spawn(async move {
            let result = match request {
                Request::Probe { identity: expected } => {
                    client.shared_identity().await.map(|identity| {
                        if identity.user_id == expected.user_id
                            && identity.hostname.eq_ignore_ascii_case(&expected.hostname)
                        {
                            Response::Identity { identity }
                        } else {
                            Response::Unavailable
                        }
                    })
                }
                Request::Read { read } => client
                    .shared_read(&read)
                    .await
                    .map(|reply| Response::Reply { reply }),
            };
            let response = match result {
                Ok(response) => response,
                // These are failures of the direct local-daemon transport or
                // handshake. Upstream API errors are already inside Reply and
                // must never become availability fallback here.
                Err(Error::Transport(_) | Error::LocalAuth(_) | Error::CacheMiss) => {
                    Response::Unavailable
                }
                Err(Error::Invalid(message))
                    if matches!(
                        message.as_str(),
                        "shared daemon identity changed" | "shared response exceeds size limit"
                    ) =>
                {
                    Response::Unavailable
                }
                Err(error) => Response::Reply {
                    reply: error.into(),
                },
            };
            let _ = outgoing.try_send(Completed {
                id: key,
                response,
                _slot: slot,
                _peer: peer,
            });
        });
        pending.insert(id.to_owned(), task.abort_handle());
        Ok(None)
    }
    pub fn cancel(&self, id: &str) {
        if let Some(task) = self.pending.lock().unwrap().remove(id) {
            task.abort();
        }
    }
    pub fn drain(&self) -> Vec<Completed> {
        let mut output = Vec::new();
        while let Ok(done) = self.completed.try_recv() {
            if self.pending.lock().unwrap().remove(&done.id).is_some() {
                output.push(done);
            }
        }
        output
    }
}

pub(super) fn frame(id: &str, response: &Response) -> Result<Value> {
    let value = json!({"kind":"github_reply","id":id,"response":response});
    // Check the complete representation before any bytes reach the fleet wire.
    // Decoding/re-encoding JSON can enlarge the original HTTP representation.
    if super::context::encode_frame(&value, hey_gh::shared_read::MAX_RESPONSE_BYTES - 64)?.is_some()
    {
        Ok(value)
    } else {
        Ok(json!({"kind":"github_reply","id":id,"response":Response::Unavailable}))
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        for (_, task) in std::mem::take(self.pending.get_mut().unwrap()) {
            task.abort();
        }
        self.runtime.take().unwrap().shutdown_background();
    }
}

#[cfg(test)]
mod tests;
