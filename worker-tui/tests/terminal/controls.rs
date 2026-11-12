use super::*;

fn smoke() -> Value {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    json!({
        "ok":true,"worker_id":"alpha","controls":{},
        "config":{"concurrency":2,"tags":["ready"]},"active":1,"free":1,"eligible":3,
        "store":{"host":"synthetic","database":"test-queue.db"},
        "workers":[
            {"id":"alpha","pid":42,"active":1,"config":{"name":"Builder","concurrency":2,"enabled":true}},
            {"id":"beta","pid":42,"active":0,"config":{"name":"Reviewer","concurrency":2,"enabled":true}}
        ],
        "runs":[
            {"id":"alpha-live","project_name":"demo","number":6,"title":"Build dashboard",
             "state":"running","finished_at":null,"started_at":now-62000,"session_id":"synthetic-session",
             "last_event":"Running checks","events":[{"text":"Synthetic activity 0"},{"text":"Synthetic activity 1"},{"text":"Synthetic activity 2"}]},
            {"id":"alpha-done","project_name":"demo","number":5,"title":"Previous attempt",
             "state":"completed","finished_at":now,"started_at":now-60000}
        ]
    })
}

#[test]
fn controls_and_disconnects_preserve_the_selected_worker() {
    let mut t = Terminal::launch("terminal-qa", 120, 36, &smoke(), false);
    t.wait("Build dashboard");
    assert!(!t.capture("wide").contains("Previous attempt"));
    t.key("h");
    let s = t.wait_for(|s| s.contains("Previous attempt"));
    assert!(!s.contains("Build dashboard"));
    t.key("\x1b[B\x1b[6~?");
    t.wait("Keyboard");
    t.key("\x1b");
    t.wait_for(|s| !s.contains("Keyboard"));
    t.resize(48, 12);
    t.wait("Completed attempts");
    assert!(t.capture("compact").contains("Previous attempt"));
    t.resize(30, 8);
    t.wait("Resize to at least");
    t.key("s\rp\r");
    t.quit();
    assert_eq!(t.state()["controls"], json!({}));
    let mut t = Terminal::launch("terminal-qa", 120, 36, &smoke(), false);
    t.wait("Build dashboard");
    t.wait("Live · refresh");
    let slow = t.dir.path().join("slow");
    fs::write(&slow, "").unwrap();
    t.key("r");
    t.wait("Refreshing");
    let start = Instant::now();
    t.key("?");
    t.wait("Keyboard");
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "help waited for a queue read"
    );
    t.key("\x1b");
    fs::remove_file(slow).unwrap();
    let s = t.wait_for(|s| s.contains("Live · refresh") && !s.contains("Keyboard"));
    assert!(!s.contains("Reviewer"));
    let offline = t.dir.path().join("offline");
    fs::write(&offline, "").unwrap();
    t.key("r");
    let s = t.wait_for(|s| s.contains("Synthetic queue offline"));
    assert!(s.contains("Build dashboard"));
    fs::remove_file(offline).unwrap();
    t.key("r");
    t.wait("Live · refresh");
    t.key("\x1b[200~sp\x1b[201~");
    thread::sleep(Duration::from_millis(100));
    assert_eq!(t.state()["controls"], json!({}));
    t.open_dialog("s", "Stop worker?");
    t.capture("confirmation");
    t.key("\x1b");
    t.wait_for(|s| !s.contains("Stop worker?"));
    assert_eq!(t.state()["controls"], json!({}));
    t.open_dialog("p", "Pause worker?");
    t.key("\r");
    t.wait("Finishing work before pause");
    assert_eq!(t.state()["controls"], json!({"alpha":"pause"}));
    t.open_dialog("s", "Stop worker?");
    t.key("\r");
    t.wait("stopped");
    assert_eq!(t.state()["controls"], json!({"alpha":"stop"}));
    t.quit();
}

#[test]
fn interruption_and_sigterm_restore_the_terminal() {
    let mut t = Terminal::launch("terminal-qa", 120, 36, &smoke(), false);
    fs::write(t.dir.path().join("slow"), "").unwrap();
    t.key("r");
    let until = Instant::now() + Duration::from_secs(8);
    while !t.dir.path().join("reading").exists() {
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(10));
    }
    t.key("\x03");
    t.wait_exit();
    let mut t = Terminal::launch("terminal-qa", 120, 36, &smoke(), false);
    t.wait("Build dashboard");
    let pid = fs::read_to_string(t.dir.path().join("dashboard.pid"))
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    assert!(pid > 1);
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    t.wait_exit();
}

#[test]
fn project_worker_creation_and_graceful_removal() {
    let worker = |id: &str, p: &str| {
        json!({
            "id":id,"pid":42,"active":1,"config":{"name":id,"enabled":true,"concurrency":1,"directory":format!("/work/{p}"),"projects":[format!("named:{p}")]},
            "runs":[{"id":format!("run-{id}"),"project_id":format!("named:{p}"),"project_name":p,"number":1,"title":format!("{p} agent"),"state":"running","finished_at":null}]
        })
    };
    let data = json!({"ok":true,"project_tabs":true,"workers":[worker("Alpha checkout","Alpha"),worker("Beta checkout","Beta")],"mutations":[],"store":{"host":"fixture"},"fleet":{"supervisor_connection":{"state":"local"}}});
    let mut t = Terminal::launch("project-tabs-qa", 110, 30, &data, true);
    t.wait("Alpha agent");
    t.key("\t");
    let s = t.wait_for(|s| s.contains("Beta agent"));
    assert!(!s.contains("Alpha agent"));
    t.key("\x1b[Z");
    t.wait("Alpha agent");
    t.capture("projects");
    t.open_dialog("a", "Add worker");
    t.key("Quiet tools\t\x7f2\t/work/Tool One\x0e/work/Tool Two");
    t.capture("add-worker");
    t.key("\r");
    t.wait_for(|s| s.contains("Alpha agent") && !s.contains("Add worker"));
    t.open_dialog("w", "Workers");
    t.wait("Quiet tools");
    assert_eq!(
        t.state()["mutations"][0],
        json!({"action":"add","name":"Quiet tools","slots":2,"directories":["/work/Tool One","/work/Tool Two"]})
    );
    t.open_dialog("d", "Remove worker?");
    t.key("\x1b");
    t.wait_for(|s| !s.contains("Remove worker?"));
    assert_eq!(t.state()["mutations"].as_array().unwrap().len(), 1);
    t.open_dialog("d", "Remove worker?");
    t.key("\r");
    t.wait("Removing after completion");
    assert_eq!(t.state()["workers"][0]["active"], 1);
    t.capture("draining");
    t.key("\x1b");
    t.wait("Alpha agent");
    let mut remaining = t.state();
    remaining["workers"].as_array_mut().unwrap().remove(0);
    let path = t.dir.path().join("snapshot.json");
    fs::write(path, remaining.to_string()).unwrap();
    t.key("r");
    t.wait("No active agents");
    t.resize(48, 16);
    t.wait("Shift+Tab");
    t.capture("narrow");
    t.quit();
    assert_eq!(t.state(), remaining);
}
