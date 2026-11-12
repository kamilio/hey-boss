use serde_json::{Value, json};
use std::{
    fs,
    net::TcpStream,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket};

struct Browser {
    child: Child,
    _profile: tempfile::TempDir,
}
impl Drop for Browser {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
            let start = Instant::now();
            while self.child.try_wait().ok().flatten().is_none() {
                if start.elapsed() > Duration::from_secs(3) {
                    let _ = self.child.kill();
                    break;
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
        let _ = self.child.wait();
    }
}

pub struct Page {
    socket: WebSocket<TcpStream>,
    _browser: Browser,
    pub beforeunload_dialogs: usize,
}
impl Page {
    pub fn open(url: &str) -> Self {
        let out = std::env::current_dir().unwrap().join("out");
        fs::create_dir_all(&out).unwrap();
        let profile = tempfile::Builder::new()
            .prefix("chrome-")
            .tempdir_in(out)
            .unwrap();
        let binary = std::env::var_os("CHROME_BIN").unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
            } else {
                "google-chrome"
            }
            .into()
        });
        let child = Command::new(binary)
            .args([
                "--headless",
                "--no-first-run",
                "--disable-background-networking",
                "--no-proxy-server",
                "--remote-debugging-port=0",
                "--host-resolver-rules=MAP hey-boss.test 127.0.0.1",
            ])
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg(url)
            .env("TMPDIR", profile.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start Chrome (or set CHROME_BIN)");
        let active = profile.path().join("DevToolsActivePort");
        let mut browser = Browser {
            child,
            _profile: profile,
        };
        let start = Instant::now();
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(3))
            .no_proxy()
            .build()
            .unwrap();
        let endpoint = loop {
            if let Ok(contents) = fs::read_to_string(&active)
                && let Some(port) = contents.lines().next()
                && let Ok(resp) = client
                    .get(format!("http://127.0.0.1:{port}/json/list"))
                    .send()
                && let Ok(targets) = resp.json::<Vec<Value>>()
                && let Some(socket) = targets
                    .iter()
                    .find(|t| t["type"] == "page" && t["url"] == url)
                    .and_then(|t| t["webSocketDebuggerUrl"].as_str())
            {
                break socket.to_owned();
            }
            assert!(
                browser.child.try_wait().unwrap().is_none(),
                "Chrome exited before opening {url}"
            );
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "Chrome did not open {url}"
            );
            thread::sleep(Duration::from_millis(50));
        };
        let addr = reqwest::Url::parse(&endpoint).unwrap();
        let tcp = TcpStream::connect(("127.0.0.1", addr.port().unwrap())).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        tcp.set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let (socket, _) = tungstenite::client(endpoint.as_str(), tcp).unwrap();
        let mut page = Self {
            socket,
            _browser: browser,
            beforeunload_dialogs: 0,
        };
        page.wait(&format!(
            "location.href === {} && document.readyState === 'complete'",
            json!(url)
        ));
        page
    }
    pub fn eval(&mut self, expr: &str) -> Result<Value, String> {
        let result = self.command(
            "Runtime.evaluate",
            json!({"expression":expr,"awaitPromise":true,"returnByValue":true}),
        )?;
        if let Some(e) = result.get("exceptionDetails") {
            return Err(e.to_string());
        }
        Ok(result["result"]["value"].clone())
    }
    pub fn wait(&mut self, condition: &str) {
        let start = Instant::now();
        while self.eval(condition).ok() != Some(json!(true)) {
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "timed out waiting for {condition}"
            );
            thread::sleep(Duration::from_millis(30));
        }
    }
    pub fn command(&mut self, method: &str, params: Value) -> Result<Value, String> {
        // One command is outstanding at a time. Browser events have no request ID.
        self.socket
            .send(Message::Text(
                json!({"id":1,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        loop {
            let msg = self.socket.read().unwrap();
            if !msg.is_text() {
                continue;
            }
            let reply: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            // Page-enabled tests reload dirty editors to verify recovery after confirming "Leave".
            if reply["method"] == "Page.javascriptDialogOpening"
                && reply["params"]["type"] == "beforeunload"
            {
                self.beforeunload_dialogs += 1;
                self.socket.send(Message::Text(json!({"id":2,"method":"Page.handleJavaScriptDialog","params":{"accept":true}}).to_string().into())).unwrap();
            }
            if reply["id"] != 1 {
                continue;
            }
            if let Some(e) = reply.get("error") {
                return Err(e.to_string());
            }
            return Ok(reply["result"].clone());
        }
    }
}
