//! Companion-side bounded multiplexing; GitHub waits never occupy control handlers.
use super::{Result, context::send, replica::invalid};
use hey_gh::shared_read::{MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, Request, Response, wire};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    net::{UnixListener, UnixStream},
    runtime::Runtime,
    sync::{OwnedSemaphorePermit, Semaphore, oneshot},
    time::{Instant, timeout, timeout_at},
};

const LIMIT: usize = 4;

pub(super) struct Relay {
    runtime: Option<Runtime>,
    replies: Replies,
    _socket: SocketGuard,
}
#[derive(Clone)]
pub(super) struct Replies(Arc<Mutex<State>>);
struct State {
    supported: bool,
    alive: bool,
    probe_after: Option<Instant>,
    pending: BTreeMap<String, oneshot::Sender<Response>>,
}
impl Replies {
    pub fn close(&self) {
        let mut state = self.0.lock().unwrap();
        state.alive = false;
        state.pending.clear();
    }
    pub fn configure(&self, message: &Value) {
        self.0.lock().unwrap().supported = message["capabilities"]["github_reads_v1"] == true;
    }
    pub fn receive(&self, message: &Value) -> Result<()> {
        let id = message["id"]
            .as_str()
            .ok_or_else(|| invalid("Missing GitHub reply ID"))?;
        let Some(reply) = self.0.lock().unwrap().pending.remove(id) else {
            // Canceled/previous requests never occupy a reply queue or a future ID.
            return Ok(());
        };
        if super::context::encode_frame(message, MAX_RESPONSE_BYTES)?.is_none() {
            return Err(invalid("GitHub relay reply exceeds size limit"));
        }
        let response: Response = serde_json::from_value(message["response"].clone())?;
        if matches!(response, Response::TooLarge) {
            // Preserve a bootstrap window across short-lived CLI processes.
            // Native local cursors then keep that feed local after the window
            // expires. This changes relay discovery, never GitHub quota backoff.
            self.0.lock().unwrap().probe_after = Some(Instant::now() + Duration::from_secs(60));
        }
        let _ = reply.send(response);
        Ok(())
    }
}
struct Bridge<W> {
    output: Arc<Mutex<W>>,
    replies: Replies,
    serial: AtomicU64,
    canceled: Mutex<VecDeque<Canceled>>,
    flushing: AtomicBool,
}
struct Canceled {
    id: String,
    _slot: Arc<OwnedSemaphorePermit>,
}
struct Pending<W: Write> {
    id: String,
    bridge: Arc<Bridge<W>>,
    slot: Arc<OwnedSemaphorePermit>,
}
impl<W: Write> Drop for Pending<W> {
    fn drop(&mut self) {
        let active = self
            .bridge
            .replies
            .0
            .lock()
            .unwrap()
            .pending
            .remove(&self.id)
            .is_some();
        if active {
            // Never wait for control output in Drop or on the async runtime.
            // Retaining the connection permit bounds canceled work too.
            self.bridge.canceled.lock().unwrap().push_back(Canceled {
                id: self.id.clone(),
                _slot: self.slot.clone(),
            });
        }
    }
}
impl<W: Write> Bridge<W> {
    fn submit(
        self: &Arc<Self>,
        mut request: Request,
        deadline: Instant,
        slot: Arc<OwnedSemaphorePermit>,
    ) -> Result<Option<(Pending<W>, oneshot::Receiver<Response>)>> {
        // ID allocation, insertion, and sending are ordered with the fleet output.
        // Never hold the pending-map lock across a potentially blocked write.
        let Ok(mut output) = self.output.try_lock() else {
            return Ok(None);
        };
        let mut state = self.replies.0.lock().unwrap();
        if !state.alive || !state.supported || state.pending.len() >= LIMIT {
            return Ok(None);
        }
        if matches!(request, Request::Probe { .. })
            && state.probe_after.is_some_and(|at| Instant::now() < at)
        {
            return Ok(None);
        }
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u64;
        if remaining == 0 {
            return Err(hey_gh::Error::Deadline.into());
        }
        if let Request::Read { read } = &mut request {
            read.timeout_ms = remaining;
        }
        let id = self
            .serial
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| invalid("GitHub relay request IDs exhausted"))?
            .checked_add(1)
            .unwrap()
            .to_string();
        let (reply, received) = oneshot::channel();
        state.pending.insert(id.clone(), reply);
        drop(state);
        let pending = Pending {
            id: id.clone(),
            bridge: self.clone(),
            slot,
        };
        let result = send(
            &mut *output,
            json!({"kind":"github_read","id":id,"request":request}),
        );
        drop(output);
        result?;
        Ok(Some((pending, received)))
    }
    fn flush_canceled(&self) {
        if !self.replies.0.lock().unwrap().alive {
            self.canceled.lock().unwrap().clear();
            return;
        }
        let Ok(mut output) = self.output.try_lock() else {
            return;
        };
        loop {
            let Some(canceled) = self.canceled.lock().unwrap().pop_front() else {
                break;
            };
            if send(
                &mut *output,
                json!({"kind":"github_cancel","id":canceled.id}),
            )
            .is_err()
            {
                self.replies.close();
                self.canceled.lock().unwrap().clear();
                break;
            }
        }
    }
}

struct Flushing<'a>(&'a AtomicBool);
impl Drop for Flushing<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct SocketGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
    _lock: File,
}
impl Drop for SocketGuard {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|meta| {
            meta.file_type().is_socket() && meta.dev() == self.device && meta.ino() == self.inode
        }) {
            let _ = fs::remove_file(&self.path);
        }
    }
}
impl Relay {
    pub fn start<W: Write + Send + 'static>(output: Arc<Mutex<W>>) -> Result<Self> {
        Self::start_at(hey_gh::shared_read::socket_path()?, output)
    }
    pub(super) fn start_at<W: Write + Send + 'static>(
        socket: PathBuf,
        output: Arc<Mutex<W>>,
    ) -> Result<Self> {
        let owner = unsafe { libc::geteuid() };
        let directory = socket
            .parent()
            .ok_or_else(|| invalid("Missing GitHub relay directory"))?;
        fs::create_dir_all(directory)?;
        let metadata = fs::symlink_metadata(directory)?;
        if !metadata.is_dir() || metadata.uid() != owner {
            return Err(invalid(
                "GitHub relay directory must belong to this OS user",
            ));
        }
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        let lock_path = directory.join("lock");
        if let Ok(meta) = fs::symlink_metadata(&lock_path)
            && (!meta.is_file() || meta.uid() != owner)
        {
            return Err(invalid("Invalid GitHub relay lock"));
        }
        let lock = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(lock_path)?;
        lock.try_lock()
            .map_err(|_| invalid("Another connection owns the GitHub relay"))?;
        match fs::symlink_metadata(&socket) {
            Ok(meta) if meta.file_type().is_socket() && meta.uid() == owner => {
                // A noncooperating live listener must not be unlinked either.
                match std::os::unix::net::UnixStream::connect(&socket) {
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                        fs::remove_file(&socket)?
                    }
                    _ => return Err(invalid("GitHub relay socket is already in use")),
                }
            }
            Ok(_) => return Err(invalid("Invalid GitHub relay socket")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let listener = std::os::unix::net::UnixListener::bind(&socket)?;
        let meta = fs::symlink_metadata(&socket)?;
        let guard = SocketGuard {
            path: socket,
            device: meta.dev(),
            inode: meta.ino(),
            _lock: lock,
        };
        fs::set_permissions(&guard.path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("github-companion-relay")
            .enable_all()
            .build()?;
        let listener = {
            let _entered = runtime.enter();
            UnixListener::from_std(listener)?
        };
        let replies = Replies(Arc::new(Mutex::new(State {
            supported: false,
            alive: true,
            probe_after: None,
            pending: BTreeMap::new(),
        })));
        let bridge = Arc::new(Bridge {
            output,
            replies: replies.clone(),
            serial: AtomicU64::new(0),
            canceled: Mutex::new(VecDeque::new()),
            flushing: AtomicBool::new(false),
        });
        runtime.spawn(async move {
            let slots = Arc::new(Semaphore::new(LIMIT));
            let mut flush = tokio::time::interval(Duration::from_millis(100));
            loop {
                let accepted = tokio::select! {
                    result = listener.accept() => result,
                    _ = flush.tick() => {
                        if !bridge.canceled.lock().unwrap().is_empty()
                            && !bridge.flushing.swap(true, Ordering::AcqRel) {
                            let bridge=bridge.clone();
                            tokio::task::spawn_blocking(move || {
                                let _flushing = Flushing(&bridge.flushing);
                                bridge.flush_canceled();
                            });
                        }
                        continue;
                    }
                };
                let Ok((mut stream, _)) = accepted else {
                    break;
                };
                if !stream.peer_cred().is_ok_and(|peer| peer.uid() == owner) {
                    continue;
                }
                let started = Instant::now();
                let Ok(slot) = slots.clone().try_acquire_owned() else {
                    let _ = timeout(
                        Duration::from_millis(100),
                        wire::write(&mut stream, &Response::Unavailable, MAX_RESPONSE_BYTES),
                    )
                    .await;
                    continue;
                };
                let bridge = bridge.clone();
                tokio::spawn(async move {
                    let slot = Arc::new(slot);
                    let _ = handle(&mut stream, &bridge, started, &slot).await;
                });
            }
            bridge.replies.close();
        });
        Ok(Self {
            runtime: Some(runtime),
            replies,
            _socket: guard,
        })
    }
    pub fn replies(&self) -> Replies {
        self.replies.clone()
    }
}
impl Drop for Relay {
    fn drop(&mut self) {
        self.replies.close();
        self.runtime.take().unwrap().shutdown_background();
    }
}
async fn handle<W: Write + Send + 'static>(
    stream: &mut UnixStream,
    bridge: &Arc<Bridge<W>>,
    started: Instant,
    slot: &Arc<OwnedSemaphorePermit>,
) -> Result<()> {
    let request: Request = timeout(
        Duration::from_secs(2),
        wire::read(stream, MAX_REQUEST_BYTES),
    )
    .await??;
    let validation = match &request {
        Request::Probe { identity } => identity.validate(),
        Request::Read { read } => read.validate(),
    };
    let response = if let Err(error) = validation {
        Response::Reply {
            reply: error.into(),
        }
    } else {
        let deadline = started
            + Duration::from_millis(match &request {
                Request::Probe { .. } => 5000,
                Request::Read { read } => read.timeout_ms,
            });
        let Some(response) = forward(stream, bridge, request, deadline, slot).await else {
            return Ok(());
        };
        response
    };
    timeout(
        Duration::from_secs(2),
        wire::write(stream, &response, MAX_RESPONSE_BYTES),
    )
    .await??;
    Ok(())
}

async fn forward<W: Write + Send + 'static>(
    stream: &mut UnixStream,
    bridge: &Arc<Bridge<W>>,
    request: Request,
    deadline: Instant,
    slot: &Arc<OwnedSemaphorePermit>,
) -> Option<Response> {
    let writer = bridge.clone();
    let slot = slot.clone();
    // A pipe write itself can block even after its mutex is acquired. At most
    // LIMIT writers can exist, each retaining its permit until it really ends.
    let admission = tokio::task::spawn_blocking(move || writer.submit(request, deadline, slot));
    let _admission = AbortAdmission(admission.abort_handle());
    let mut byte = [0];
    let admitted = tokio::select! {
        result=timeout_at(deadline,admission)=>match result {
            Ok(Ok(Ok(Some(admitted))))=>admitted,
            Ok(Ok(Err(error))) if error.downcast_ref::<hey_gh::Error>().is_some_and(|e|matches!(e,hey_gh::Error::Deadline)) => return Some(deadline_reply()),
            Ok(_)=>return Some(Response::Unavailable),
            Err(_)=>return Some(deadline_reply()),
        },
        _=stream.read(&mut byte)=>return None,
    };
    let (_pending, received) = admitted;
    tokio::select! {
        result=timeout_at(deadline,received)=>Some(match result {
            Ok(Ok(response))=>response,
            Ok(Err(_))=>Response::Unavailable,
            Err(_)=>deadline_reply(),
        }),
        // EOF or any extra input cancels this single-request socket.
        _=stream.read(&mut byte)=>None,
    }
}
fn deadline_reply() -> Response {
    Response::Reply {
        reply: hey_gh::Error::Deadline.into(),
    }
}

// Abort a not-yet-started writer when its caller disconnects. An already
// started pipe write retains its permit and arranges cancellation on completion.
struct AbortAdmission(tokio::task::AbortHandle);
impl Drop for AbortAdmission {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests;
