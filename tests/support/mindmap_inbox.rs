//! Synthetic desktop bridge for CLI/HTTP projection tests. Never touches a real Inbox.
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub struct Inbox {
    path: PathBuf,
    reply: Arc<Mutex<Value>>,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Inbox {
    pub fn start(path: impl AsRef<Path>, tasks: Value) -> Self {
        let path = path.as_ref().to_owned();
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let reply = Arc::new(Mutex::new(
            json!({"status":"ok","result":json!({"tasks":tasks}).to_string()}),
        ));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_reply, thread_requests, thread_stop) =
            (reply.clone(), requests.clone(), stop.clone());
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // macOS accepts inherit the nonblocking listener flag.
                        // The bridge protocol sends complete, potentially large replies.
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let mut bytes = Vec::new();
                        stream.read_to_end(&mut bytes).unwrap();
                        thread_requests
                            .lock()
                            .unwrap()
                            .push(serde_json::from_slice(&bytes).unwrap());
                        let bytes = serde_json::to_vec(&*thread_reply.lock().unwrap()).unwrap();
                        stream.write_all(&bytes).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("Synthetic Inbox accept failed: {error}"),
                }
            }
        });
        Self {
            path,
            reply,
            requests,
            stop,
            thread: Some(thread),
        }
    }
    pub fn tasks(&self, tasks: Value) {
        *self.reply.lock().unwrap() =
            json!({"status":"ok","result":json!({"tasks":tasks}).to_string()});
    }
    pub fn unavailable(&self) {
        *self.reply.lock().unwrap() =
            json!({"status":"error","error":"Synthetic Inbox temporarily unavailable"});
    }
    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for Inbox {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Err(error) = self.thread.take().unwrap().join() {
            if !std::thread::panicking() {
                std::panic::resume_unwind(error);
            }
        }
        std::fs::remove_file(&self.path).unwrap();
    }
}
