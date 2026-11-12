//! Process-level checks against a synthetic status snapshot; never access a real worker.
#![cfg(any(target_os = "linux", target_os = "macos"))]
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use terminal_pilot_rust::{
    pty::{Options, Pty},
    session::Session,
    strip_ansi,
};
use terminal_png_rust::{ansi, png, raster, svg};

struct Terminal {
    pty: Pty,
    screen: Session,
    pending: Vec<u8>,
    start: Instant,
    output: PathBuf,
    dir: tempfile::TempDir,
}
impl Terminal {
    fn launch(suite: &str, cols: usize, rows: usize, snapshot: &Value, projects: bool) -> Self {
        let output = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(suite);
        fs::create_dir_all(&output).unwrap();
        let dir = tempfile::tempdir_in(&output).unwrap();
        fs::write(dir.path().join("snapshot.json"), snapshot.to_string()).unwrap();
        let backend = dir.path().join("backend");
        fs::write(
            &backend,
            r#"#!/bin/sh
for last do :; done
if [ "$last" != status ]; then touch unexpected-command; exit 2; fi
if [ -f offline ]; then echo 'Synthetic queue offline' >&2; exit 1; fi
exec /bin/cat snapshot.json
"#,
        )
        .unwrap();
        fs::set_permissions(&backend, fs::Permissions::from_mode(0o700)).unwrap();
        let mut args = vec![
            "-c".into(),
            r#"before=$(stty -g) || exit 98
"$@"
code=$?
[ "$(stty -g)" = "$before" ] || exit 99
printf 'TERMINAL_RESTORED\n'
exit "$code""#
                .into(),
            "terminal".into(),
            env!("CARGO_BIN_EXE_hey-boss-worker-tui").into(),
            "--binary".into(),
            backend.to_str().unwrap().into(),
        ];
        if projects {
            args.push("--projects".into());
        }
        let pty = Pty::spawn(Options {
            command: "/bin/sh".into(),
            args,
            cwd: Some(dir.path().to_str().unwrap().into()),
            env: vec![
                ("TERM".into(), "xterm-256color".into()),
                ("COLORTERM".into(), "truecolor".into()),
                ("NO_COLOR".into(), "".into()),
                ("PATH".into(), std::env::var("PATH").unwrap()),
            ],
            cols,
            rows,
        })
        .unwrap();
        Self {
            pty,
            screen: Session::new(vec![], cols as f64, rows as f64, 0.).unwrap(),
            pending: vec![],
            start: Instant::now(),
            output,
            dir,
        }
    }
    fn pump(&mut self) {
        loop {
            let chunk = self.pty.read().unwrap();
            if chunk.is_empty() {
                break;
            }
            self.pending.extend(chunk);
            // A PTY read may end in the middle of a UTF-8 glyph.
            let n = match std::str::from_utf8(&self.pending) {
                Ok(s) => s.len(),
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                Err(e) => panic!("invalid terminal output: {e}"),
            };
            let text = std::str::from_utf8(&self.pending[..n])
                .unwrap()
                .encode_utf16()
                .collect::<Vec<_>>();
            self.screen
                .data(&text, self.start.elapsed().as_secs_f64() * 1000.)
                .unwrap();
            self.pending.drain(..n);
        }
    }
    fn wait_for(&mut self, predicate: impl Fn(&str) -> bool) -> String {
        let until = Instant::now() + Duration::from_secs(12);
        loop {
            self.pump();
            let text = self
                .screen
                .screen_lines()
                .iter()
                .map(|l| String::from_utf16_lossy(&strip_ansi(l)))
                .collect::<Vec<_>>()
                .join("\n");
            if predicate(&text) {
                return text;
            }
            if Instant::now() >= until || self.pty.exit_code().unwrap().is_some() {
                self.save_frame("failure");
                panic!("unexpected terminal screen:\n{text}");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn wait(&mut self, text: &str) {
        self.wait_for(|s| s.contains(text));
    }
    fn key(&mut self, key: &str) {
        self.pty.write(key.as_bytes()).unwrap();
    }
    fn resize(&mut self, cols: usize, rows: usize) {
        self.pty.resize(cols, rows).unwrap();
        self.screen.resize(cols as f64, rows as f64).unwrap();
    }
    fn capture(&mut self, name: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            self.pump();
            if self
                .screen
                .quiet_remaining(100., self.start.elapsed().as_secs_f64() * 1000.)
                .unwrap()
                == 0.
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "screen did not settle for {name}"
            );
            thread::sleep(Duration::from_millis(10));
        }
        self.save_frame(name)
    }
    fn save_frame(&self, name: &str) -> String {
        let raw = self.screen.screen_lines().join(&10);
        let image =
            raster::render(&svg::render(&ansi::parse(&raw), svg::Options::default())).unwrap();
        fs::write(
            self.output.join(format!("{name}.png")),
            png::encode(image.width, image.height, &image.rgba).unwrap(),
        )
        .unwrap();
        let text = String::from_utf16_lossy(&strip_ansi(&raw));
        fs::write(self.output.join(format!("{name}.txt")), &text).unwrap();
        text
    }
    fn assert_accent(&self) {
        assert!(
            String::from_utf16_lossy(&self.screen.screen_lines().join(&10))
                .contains("38;2;161;175;255")
        );
    }
    fn quit(&mut self) {
        assert!(
            !self.dir.path().join("unexpected-command").exists(),
            "read-only walkthrough attempted a mutation"
        );
        self.key("q");
        let until = Instant::now() + Duration::from_secs(4);
        loop {
            self.pump();
            if let Some(code) = self.pty.exit_code().unwrap() {
                assert_eq!(code, 0);
                break;
            }
            assert!(Instant::now() < until, "TUI did not exit");
            thread::sleep(Duration::from_millis(20));
        }
        self.pump();
        assert!(
            self.screen
                .history(None)
                .unwrap()
                .iter()
                .any(|l| String::from_utf16_lossy(l).contains("TERMINAL_RESTORED"))
        );
    }
}

#[test]
fn project_tab_order_survives_refresh_resize_and_unicode() {
    let names = [
        "Alpha tools",
        "Beta build",
        "Gamma docs",
        "Omega app",
        "Zeta web",
    ];
    let workers = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            json!({"id":name,"pid":42+i,"active":1,
            "config":{"name":name,"enabled":true,
            "concurrency":2,
            "projects":[format!("named:{name}")],
            "directory":format!("/work/{name}")},
            "runs":[{"id":format!("run-{name}"),"project_id":format!("named:{name}"),
            "number":i+1,"title":format!("{name} agent"),
            "state":"running",
            "finished_at":null}],
            "chiefs":[]})
        })
        .collect::<Vec<_>>();
    let mut snapshot = json!({"ok":true,"project_tabs":true,
            "workers":workers,
            "store":{"host":"fixture"},
            "fleet":{"supervisor_connection":{"state":"local"}}});
    let mut t = Terminal::launch("tab-order-qa", 110, 30, &snapshot, true);
    let file = t.dir.path().join("snapshot.json");
    let header = |t: &mut Terminal, name: &str| {
        let s = t.capture(name);
        t.assert_accent();
        s.lines().next().unwrap().to_owned()
    };
    t.wait("Alpha tools agent");
    let initial = header(&mut t, "wide-alpha");
    for name in &names[1..] {
        t.key("\t");
        t.wait(&format!("{name} agent"));
        let h = header(&mut t, &format!("wide-{}", name.to_lowercase()));
        assert!(h.contains(&format!("[{name}]")));
        for n in names {
            assert_eq!(h.find(n), initial.find(n));
        }
    }
    let before = header(&mut t, "wide-before-refresh");
    snapshot["workers"].as_array_mut().unwrap().reverse();
    snapshot["workers"][0]["config"]["directory"] = json!("/work/Zeta web/refreshed");
    fs::write(&file, snapshot.to_string()).unwrap();
    t.key("r");
    t.wait("/refreshed");
    assert_eq!(header(&mut t, "wide-refreshed"), before);
    t.resize(48, 16);
    t.wait("‹");
    let narrow = header(&mut t, "narrow-last");
    assert!(
        narrow.starts_with("‹ ")
            && narrow.find("Omega app").unwrap() < narrow.find("Zeta web").unwrap()
    );
    t.key("\t");
    t.wait("Alpha tools agent");
    let first = header(&mut t, "narrow-first");
    assert!(first.contains("[Alpha tools]") && first.trim_end().ends_with('›'));
    t.key("\x1b[Z");
    t.wait("Zeta web agent");
    assert_eq!(header(&mut t, "narrow-backward"), narrow);
    t.resize(80, 24);
    t.wait("Alpha tools");
    let medium = header(&mut t, "medium");
    assert!(names.iter().all(|n| medium.contains(n)));
    assert!(medium.find("Alpha") < medium.find("Zeta"));
    snapshot["projects"] = json!([{"id":"named:Zeta web","name":"界é👩‍💻".repeat(30)}]);
    fs::write(&file, snapshot.to_string()).unwrap();
    t.key("r");
    t.wait("…]");
    let h = header(&mut t, "unicode");
    assert!(h.contains("[界é👩‍💻") && h.contains("…]"));
    t.quit();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(file).unwrap()).unwrap(),
        snapshot
    );
}

#[test]
fn runtime_labels_in_active_and_completed_runs() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let runs=[("days",2861,2110*60+1,"Improve command performance"),("hours",3614,669*60+51,"Unify built-in commands"),("minutes",3579,124,"Check package exports"),("finished",3578,2*86400+3*3600,"Completed runtime check")]
    .map(|(id,number,seconds,title)|json!({"id":id,
            "number":number,"title":title,"project_name":"poe-code",
            "started_at":now-seconds*1000,
            "finished_at":(id=="finished").then_some(now),
            "state":if id=="finished"{"completed"}else{"running"},
            "events":[{"at":now,"text":"Verifying runtime labels at wide and narrow terminal sizes."}]}));
    let snapshot = json!({"ok":true,"worker_id":"runtime",
            "workers":[{"id":"runtime","pid":42,"active":3,
            "config":{"enabled":true,
            "concurrency":4,
            "projects":["named:poe-code"],
            "directory":"/workspace/poe-code"}}],
            "runs":runs});
    let mut t = Terminal::launch("runtime-qa", 160, 36, &snapshot, false);
    let capture = |t: &mut Terminal, n: &str| {
        let s = t.capture(n);
        assert!(!s.contains("2110m"));
        s
    };
    let resize = |t: &mut Terminal, c, r| {
        t.resize(c, r);
        t.wait_for(|s| {
            if r < 12 {
                s.contains("Resize to at least")
            } else {
                s.lines()
                    .nth(5)
                    .is_some_and(|l| l.trim_end().ends_with('┐'))
                    && s.lines().nth(r - 1).is_some_and(|l| l.contains("q quit"))
            }
        });
    };
    t.wait("1d 11h");
    let wide = capture(&mut t, "wide-active");
    t.assert_accent();
    assert_eq!(wide.matches("1d 11h").count(), 2);
    assert!(wide.contains("11h 09m"));
    t.key("\x1b[B");
    t.wait("poe-code #3614 · Unify built-in commands");
    capture(&mut t, "hours-selected");
    t.key("\x1b[A");
    t.wait("poe-code #2861 · Improve command performance");
    for (c, r) in [(120, 36), (80, 24), (64, 18), (48, 12)] {
        resize(&mut t, c, r);
        t.wait(if r == 12 { "#2861 running" } else { "1d 11h" });
        capture(&mut t, &format!("active-{c}x{r}"));
    }
    resize(&mut t, 120, 36);
    t.key("h");
    t.wait("2d 03h");
    assert_eq!(capture(&mut t, "finished").matches("2d 03h").count(), 2);
    t.key("r");
    t.wait("2d 03h");
    resize(&mut t, 48, 12);
    t.wait("#3578 completed");
    capture(&mut t, "finished-minimum");
    resize(&mut t, 30, 8);
    t.wait("Resize to at least");
    capture(&mut t, "too-small");
    t.quit();
}

#[test]
fn shared_project_tab_filters_workers_and_sessions() {
    let run = |id: &str, project: &str, done: bool| {
        json!({"id":id,"project_id":format!("named:{project}"),"project_name":project,
            "number":7,"title":format!("{project} task"),
            "state":if done{"completed"}else{"running"},
            "finished_at":done.then_some(42),
            "events":[{"text":format!("Checking {project} without changing the live fleet.")}]})
    };
    let worker = |id: &str, projects: &[&str], runs: Vec<Value>| {
        json!({"id":id,"pid":42,"active":runs.iter().filter(|r|r["finished_at"].is_null()).count(),
            "config":{"name":id,"enabled":true,
            "concurrency":3,
            "projects":projects.iter().map(|p|format!("named:{p}")).collect::<Vec<_>>(),
            "directories":projects.iter().map(|p|(format!("named:{p}"),json!(format!("/work/{p}")))).collect::<serde_json::Map<_,_>>()},
            "runs":runs,
            "chiefs":[]})
    };
    let mut snapshot = json!({"ok":true,"project_tabs":true,
            "fleet":{"supervisor_connection":{"state":"local"}},
            "workers":[worker("Dedicated Alpha",&["Alpha"],vec![run("alpha","Alpha",false)]),
            worker("Dedicated Beta",&["Beta"],vec![run("beta","Beta",false)]),
            worker("Shared one",&["Tools","Docs","Proxy","Search","Notes"],vec![run("tools","Tools",false),run("docs","Docs",true)]),
            worker("Shared two",&["Notes","Proxy","Search","Docs","Tools"],vec![run("proxy","Proxy",false)])]});
    let mut t = Terminal::launch("project-tabs-qa", 120, 36, &snapshot, true);
    let state = t.dir.path().join("snapshot.json");
    let shared = "[Shared · 2 workers · 5 projects]";
    let capture = |t: &mut Terminal, name: &str| {
        let s = t.capture(&format!("shared-{name}"));
        t.assert_accent();
        s
    };
    t.wait("[Alpha]");
    t.key("\t");
    t.wait("[Beta]");
    t.key("\t");
    t.wait(shared);
    t.wait("Tools task");
    t.wait("Proxy task");
    assert!(!capture(&mut t, "wide").contains("Alpha task"));
    t.key("\x1b[B");
    t.wait("Shared two");
    t.key("p");
    t.wait("Pause worker?");
    assert!(capture(&mut t, "confirm").contains("Shared two"));
    t.key("\x1b");
    t.wait_for(|s| !s.contains("Pause worker?") && s.contains("Tools task"));
    t.key("h");
    t.wait("Docs task");
    assert!(!capture(&mut t, "history").contains("Proxy task"));
    t.key("h");
    t.wait("Tools task");
    for (c, r) in [(100, 28), (80, 24), (64, 18), (48, 12)] {
        t.resize(c, r);
        t.wait(shared);
        t.wait(if c >= 64 {
            "Checking Tools without changing the live fleet."
        } else {
            "Tools task"
        });
        capture(&mut t, &format!("{c}x{r}"));
    }
    t.resize(120, 36);
    t.wait("Activity · PgUp/PgDn");
    t.key("w");
    t.wait("Shared workers");
    assert!(!capture(&mut t, "workers").contains("Dedicated Alpha"));
    t.key("\x1b");
    t.wait_for(|s| !s.contains("Shared workers"));
    snapshot["workers"].as_array_mut().unwrap().reverse();
    fs::write(&state, snapshot.to_string()).unwrap();
    t.key("r");
    t.wait(shared);
    t.key("\t");
    t.wait("[Alpha]");
    t.key("\x1b[Z");
    t.wait(shared);
    snapshot["workers"]
        .as_array_mut()
        .unwrap()
        .retain(|w| !w["id"].as_str().unwrap().starts_with("Shared"));
    fs::write(&state, snapshot.to_string()).unwrap();
    t.key("r");
    let s = t.wait_for(|s| s.contains("[Alpha]"));
    assert!(!s.contains("Shared ·"));
    t.quit();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(state).unwrap()).unwrap(),
        snapshot
    );
}

#[test]
fn activity_history_help_and_recovery() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let texts = [
        "The blue theme is in place. Checking keyboard navigation and narrow terminals.\nThe selected agent stays visible while reading its activity.",
        "Command: cargo test --locked --manifest-path worker-tui/Cargo.toml\nAll dashboard regression checks passed.",
        "Preserved multiline messages and removed duplicate latest updates.\nOlder updates remain available with Page Down.",
        "Reviewing the app palette and activity hierarchy.",
    ];
    let events = texts
        .into_iter()
        .map(str::to_owned)
        .chain((1..=8).map(|n| {
            format!(
                "Earlier update {n}: checking layout, responsiveness, and retained session context."
            )
        }))
        .enumerate()
        .map(|(i, text)| json!({"at":now-(i as u64+1)*30000,"text":text}))
        .collect::<Vec<_>>();
    let snapshot = json!({"ok":true,"worker_id":"design",
            "fleet":{"supervisor_connection":{"state":"connected"}},
            "workers":[{"id":"design","pid":42,"active":2,
            "config":{"name":"MacBook builder","enabled":true,
            "concurrency":3,
            "projects":["named:hey-boss"],
            "directory":"/workspace/hey-boss"}}],
            "runs":[
        {"id":"design-live","project_name":"hey-boss",
            "number":86,"title":"Improve terminal dashboard",
            "state":"running",
            "started_at":now-387000,
            "finished_at":null,
            "last_event":events[0]["text"],
            "events":events,
            "goal":{"status":"active","objective":"Improve TUI"}},
        {"id":"second","project_name":"hey-boss",
            "number":91,"title":"Verify companion connectivity",
            "state":"running",
            "started_at":now-94000,
            "finished_at":null,
            "events":[]},
        {"id":"failed","project_name":"hey-boss",
            "number":85,"title":"Upgrade connected device",
            "state":"failed",
            "started_at":now-900000,
            "finished_at":now-450000,
            "summary":"The device disconnected during verification.\nReconnect the device, then retry the upgrade.",
            "events":[{"at":now-450000,"text":"SSH connection closed before verification finished."}]}
    ]});
    let mut t = Terminal::launch("activity-qa", 120, 36, &snapshot, false);
    t.wait("Improve terminal dashboard");
    t.capture("01-wide");
    t.assert_accent();
    t.key("\x1b[6~");
    t.wait("Older");
    t.capture("02-older");
    t.key("\x1b[B");
    t.wait("Waiting for agent activity");
    t.wait("Latest");
    assert!(!t.capture("03-waiting").contains("Older"));
    t.key("h");
    t.wait("Reconnect the device");
    t.capture("04-failure-history");
    t.key("h");
    t.wait("The blue theme is in place");
    for (c, r, name) in [
        (80, 24, "05-standard"),
        (64, 18, "06-narrow"),
        (48, 12, "07-minimum"),
        (30, 8, "08-too-small"),
    ] {
        t.resize(c, r);
        t.wait(if c < 48 {
            "Resize to at least"
        } else {
            "↑↓ session  h history  ? help  q quit"
        });
        let s = t.capture(name);
        if c >= 64 {
            assert!(s.contains("The blue theme is in place"));
        }
        if c == 48 {
            t.key("?");
            t.wait("Close help");
            t.capture("07-minimum-help");
            t.key("\x1b");
            t.wait("Supervisor: connected");
            t.wait_for(|s| s.contains("#86 running") && !s.contains("Keyboard"));
        }
    }
    t.resize(120, 36);
    t.wait("The blue theme is in place");
    t.key("?");
    t.wait("Close help");
    t.capture("09-help");
    t.key("\x1b");
    t.wait_for(|s| !s.contains("Close help"));
    let offline = t.dir.path().join("offline");
    fs::write(&offline, "").unwrap();
    t.key("r");
    t.wait("Synthetic queue offline");
    assert!(
        t.capture("10-disconnected")
            .contains("Improve terminal dashboard")
    );
    fs::remove_file(offline).unwrap();
    t.key("r");
    t.wait_for(|s| s.contains("Supervisor: connected") && !s.contains("Synthetic queue offline"));
    t.quit();
}
