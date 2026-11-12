//! Headless browser checks against the shipped UI and a private fixture.
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::io::Write;
#[path = "assignment_deadline.rs"]
mod assignment_deadline;
#[path = "../../tests/support/chrome.rs"]
mod chrome;
#[path = "../../tests/support/managed_worker.rs"]
mod managed_worker;
#[path = "pr_handoff.rs"]
mod pr_handoff;
#[path = "project_checkouts.rs"]
mod project_checkouts;
#[path = "settings_heading.rs"]
mod settings_heading;
#[path = "../../tests/support/worker_fixtures.rs"]
mod worker_fixtures;
use chrome::Page;

fn web(f: &Fixture, project: &str) -> (Service, String) {
    start_web(f.command(&[
        "issue",
        "--project",
        project,
        "web",
        "--port",
        "0",
        "--no-discovery",
        "--json",
    ]))
}

fn start_web(mut command: Command) -> (Service, String) {
    let mut service = Service(command.stdout(Stdio::piped()).spawn().unwrap());
    let (tx, rx) = mpsc::channel();
    let stdout = service.0.stdout.take().unwrap();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let _ = tx.send(reader.read_line(&mut line).map(|_| line));
        let _ = std::io::copy(&mut reader, &mut std::io::sink());
    });
    let ready: Value =
        serde_json::from_str(&rx.recv_timeout(Duration::from_secs(12)).unwrap().unwrap()).unwrap();
    (
        service,
        ready["url"].as_str().unwrap().trim_end_matches('/').into(),
    )
}

fn key(p: &mut Page, key: &str, code: &str, vk: i32, text: &str) {
    for kind in ["keyDown", "keyUp"] {
        p.command("Input.dispatchKeyEvent",json!({"type":kind,"key":key,"code":code,"windowsVirtualKeyCode":vk,"text":if kind=="keyDown"{text}else{""}})).unwrap();
    }
}

fn screenshot(page: &mut Page, directory: &str, name: &str) {
    capture(page, directory, name, json!({"format":"png"}));
}

fn capture(page: &mut Page, directory: &str, name: &str, params: Value) {
    let png = page.command("Page.captureScreenshot", params).unwrap();

    let dir = Path::new(directory);
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join(format!("{name}.png")),
        STANDARD.decode(png["data"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
}

fn fixture() -> Fixture {
    // Tests are built alongside Cargo's example binaries, one directory below the CLI.
    let bin = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("hey-boss");
    Fixture::new(&bin).unwrap()
}

// Point the install checks at a released CLI with HEY_BOSS_TEST_BINARY.
fn installed_fixture() -> Fixture {
    let f = std::env::var_os("HEY_BOSS_TEST_BINARY")
        .map_or_else(fixture, |bin| Fixture::new(Path::new(&bin)).unwrap());
    if let Ok(expected) = std::env::var("HEY_BOSS_EXPECTED_BUILD") {
        let r = f.command(&["--version"]).output().unwrap();
        assert!(
            r.status.success() && String::from_utf8_lossy(&r.stdout).contains(&expected),
            "Installed build mismatch"
        );
    }
    f
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn workers_open_saved_conversations_and_retain_reading_position() {
    let f = fixture();
    f.init("Atlas").unwrap();
    agents::conversations(&f).unwrap();
    let (_web, base) = web(&f, "Atlas");
    let mut page = Page::open(&format!("{base}/workers"));
    page.wait("document.querySelectorAll('.project-worker-task').length === 4");
    assert_eq!(
        page.eval("document.querySelectorAll('.project-worker-machine').length")
            .unwrap(),
        3
    );
    assert_eq!(page.eval("document.querySelector('[data-host=studio] .project-worker-task-state span').textContent").ok(),Some(json!("Last seen")));
    screenshot(&mut page, "out/worker-browser", "workers");
    page.eval("document.querySelector('[href*=\"run=atlas-1\"]').click()")
        .unwrap();
    page.wait("document.querySelectorAll('.chat-message').length === 5");
    assert_eq!(
        page.eval("Array.from(document.querySelectorAll('.chat-message'), n=>n.classList[1])")
            .unwrap(),
        json!(["user", "assistant", "tool", "tool", "assistant"])
    );
    assert_eq!(
        page.eval("document.querySelector('.message-body strong').textContent")
            .unwrap(),
        "8 reconnect tests passed"
    );
    page.eval("document.querySelector('.chat-message.tool summary').click()")
        .unwrap();
    page.wait(
        "document.querySelector('.chat-message.tool pre').getBoundingClientRect().height > 0",
    );
    assert_eq!(
        page.eval("new URL(document.querySelector('#back').href).hash")
            .unwrap(),
        "#project=named%3AAtlas&issue=1"
    );
    for (width, theme) in [(1440, "light"), (390, "dark"), (320, "light")] {
        page.command(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":844,"deviceScaleFactor":1,"mobile":false}),
        )
        .unwrap();
        page.command(
            "Emulation.setEmulatedMedia",
            json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
        )
        .unwrap();
        page.wait("document.documentElement.scrollWidth <= innerWidth");
        screenshot(
            &mut page,
            "out/worker-browser",
            &format!("conversation-{width}-{theme}"),
        );
    }

    // A real rollout across the 1 MiB history window, including a live append.
    let path = fs::read_dir(f.root.path().join("codex/sessions"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut log = fs::OpenOptions::new().append(true).open(&path).unwrap();
    let append = |log: &mut fs::File, text: &str| {
        writeln!(log,"{}",json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}})).unwrap()
    };
    // A large, non-message record separates pages without rendering a megabyte of text.
    writeln!(
        log,
        "{}",
        json!({"type":"metadata","padding":"x".repeat(1_050_000)})
    )
    .unwrap();
    for n in 0..24 {
        append(
            &mut log,
            &format!("Update {n}. {}", "Checking the connection. ".repeat(20)),
        );
    }
    page.command("Page.reload", json!({})).unwrap();
    page.wait("document.querySelectorAll('.chat-message').length === 24 && !document.querySelector('#load-earlier').hidden");
    page.eval("document.querySelector('#load-earlier').click()")
        .unwrap();
    page.wait("document.querySelectorAll('.chat-message').length === 29");
    assert_eq!(
        page.eval("document.querySelector('.chat-message').classList.contains('user')")
            .unwrap(),
        true
    );
    page.eval("new Promise(done => { window.addEventListener('scroll', () => done(true), {once:true}); scrollTo(0,300); })").unwrap();
    page.wait("scrollY === 300");
    append(&mut log, "A new update arrived.");
    page.eval("document.querySelector('#refresh').click()")
        .unwrap();
    page.wait("document.querySelectorAll('.chat-message').length === 30");
    assert_eq!(page.eval("scrollY").unwrap(), 300);
    page.wait("!document.querySelector('#jump-live').hidden");
    page.eval("document.querySelector('#jump-live').click()")
        .unwrap();
    page.wait("document.querySelector('#conversation-end').getBoundingClientRect().bottom <= innerHeight+5");
    page.command("Page.navigate",json!({"url":format!("{base}/agents/session#host=studio&run=studio-1&project=named%3AStudio")})).unwrap();
    page.wait("document.querySelector('#error')?.textContent.includes('disconnected')");
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn first_worker_uses_the_selected_checkout_and_capacity() {
    let f = fixture();
    f.init("Atlas").unwrap();
    let root = f.root.path();
    fs::write(
        root.join("fleet.yaml"),
        "machines:\n  local:\n    workers: []\n",
    )
    .unwrap();
    // Even if checkout discovery regresses, Git can only read repositories on disk.
    fs::write(
        root.join("home/.gitconfig"),
        "[protocol]\n allow = never\n[protocol \"file\"]\n allow = always\n",
    )
    .unwrap();
    let checkout = root.join("home/Workspace/example");
    fs::create_dir_all(&checkout).unwrap();
    for args in [
        &["init", "--quiet"][..],
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/fixture/example.git",
        ],
    ] {
        let git = Command::new("/usr/bin/git")
            .args(args)
            .current_dir(&checkout)
            .env_clear()
            .env("HOME", root.join("home"))
            .output()
            .unwrap();
        assert!(
            git.status.success(),
            "{}",
            String::from_utf8_lossy(&git.stderr)
        );
    }
    let _supervisor = agents::supervisor(&f).unwrap();
    let (_web, base) = web(&f, "Atlas");
    let mut page = Page::open(&format!("{base}/workers"));
    page.wait("document.querySelector('[data-machine-action=add]') != null");
    assert_eq!(
        page.eval("document.querySelectorAll('.project-worker').length")
            .unwrap(),
        0
    );
    let open = |p: &mut Page| {
        p.eval("document.querySelector('[data-machine-action=add]').click()")
            .unwrap();
        p.wait("document.querySelector('#machine-editor').open");
    };
    open(&mut page);
    assert_eq!(
        page.eval("document.querySelector('#machine-slots').value")
            .unwrap(),
        "1"
    );
    page.eval("document.querySelector('#machine-editor-cancel').click()")
        .unwrap();
    assert_eq!(
        page.eval("document.querySelectorAll('.project-worker').length")
            .unwrap(),
        0
    );
    open(&mut page);
    page.eval("document.querySelector('#machine-git').value='https://github.com/fixture/example.git'; document.querySelector('#machine-slots').value='5'; document.querySelector('#machine-editor-save').click()").unwrap();
    page.wait("document.querySelectorAll('.project-worker').length === 1 && !document.querySelector('#machine-editor').open");
    page.command("Page.reload", json!({})).unwrap();
    page.wait("document.querySelector('.worker-count')?.textContent === '5'");
    let config=page.eval("fetch('/api/fleet/configuration').then(r=>r.json()).then(v=>v.document.machines.local)").unwrap();
    let workers = config["workers"].as_array().unwrap();
    assert_eq!(workers.len(), 1);
    assert_eq!(
        workers[0]["config"]["projects"],
        json!(["github.com/fixture/example"])
    );
    assert_eq!(workers[0]["config"]["concurrency"], 5);
    page.eval("document.querySelector('[data-signal=pause]').click()")
        .unwrap();
    page.wait("document.querySelector('[data-signal=resume]') !== null");
    assert_eq!(page.eval("fetch('/api/fleet/configuration').then(r=>r.json()).then(v=>v.document.machines.local.workers[0].intent)").unwrap(),"pause");
    for width in [1440, 390, 320] {
        page.command(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":900,"deviceScaleFactor":1,"mobile":false}),
        )
        .unwrap();
        page.wait("document.documentElement.scrollWidth <= innerWidth");
        screenshot(
            &mut page,
            "out/worker-browser",
            &format!("first-worker-{width}"),
        );
    }
}

#[path = "issue_browser.rs"]
mod issues;

#[path = "jobs_browser.rs"]
mod jobs;

#[path = "skills_api.rs"]
mod skills_api;

#[path = "skills_browser.rs"]
mod skills;

#[path = "proxy_browser.rs"]
mod proxy;

#[path = "issue_links.rs"]
mod issue_links;

#[path = "navigation.rs"]
mod navigation;

#[path = "workers.rs"]
mod workers;

#[path = "quick_issue.rs"]
mod quick_issue;

#[path = "subtasks.rs"]
mod subtasks;

#[path = "dependency_scheduling.rs"]
mod dependency_scheduling;

#[path = "issue_transfer.rs"]
mod issue_transfer;

#[path = "assignment_states.rs"]
mod assignment_states;

#[path = "issue_list.rs"]
mod issue_list;

#[path = "artifact_reader.rs"]
mod artifact_reader;

#[path = "artifact_import_browser.rs"]
mod artifact_import;

#[test]
fn installed_reviewed_evidence_handoff_is_atomic_and_replayable() {
    let f = installed_fixture();
    let (project, _) = Scenario::ReviewedEvidence.destination();
    f.init(project).unwrap();
    reviewed_evidence::verify(&f, project).unwrap();
}
