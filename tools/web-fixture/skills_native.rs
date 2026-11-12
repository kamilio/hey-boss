//! Native launch payloads and save recovery. Set SKILL_NATIVE_AUDIT to also run the Cocoa editor.
use super::*;
use std::os::unix::net::UnixListener;

#[test]
#[ignore = "requires a built hey-boss CLI and the skills transport"]
fn native_skill_saves_recover_after_offline_rename() {
    let f = fixture();
    f.init("Skills QA").unwrap();
    let root = f.root.path();
    let homes = [root.join("home"), root.join("studio")];
    let text = |name: &str, body: &str| {
        format!("---\nname: {name}\ndescription: Fixture\n---\n# Example\n{body}\n")
    };
    let original = text("example", "Original");
    for home in &homes {
        let skill = home.join(".codex/skills/example");
        put(skill.join("SKILL.md"), &original);
        put(skill.join("references/notes.md"), "# Notes\n");
    }
    install_transport(root);
    put(root.join("config.yaml"), "ssh_hosts: [studio]\n");
    let api = Api::start(&f);
    api.post(&json!({"action":"scan"}), 200);
    api.idle();
    let desktop = UnixListener::bind(root.join("inbox.sock")).unwrap();
    let open = |file: &str| {
        thread::scope(|scope| {
            // Receive the same socket command the native editor receives.
            let client = scope.spawn(|| {
                api.post(
                    &json!({"action":"open_editor","skill":"example","file_path":file}),
                    200,
                )
            });
            desktop.set_nonblocking(true).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut socket = loop {
                match desktop.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            !client.is_finished() && Instant::now() < deadline,
                            "Native editor did not receive its launch"
                        );
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let request: Value = serde_json::from_reader(&socket).unwrap();
            assert_eq!(request["command"], "artifact_editor");
            let launch: Value =
                serde_json::from_str(request["question"].as_str().unwrap()).unwrap();
            serde_json::to_writer(
                &mut socket,
                &json!({"status":"ok","result":"{\"ok\":true}"}),
            )
            .unwrap();
            drop(socket);
            client.join().unwrap();
            launch
        })
    };
    let launch = open("SKILL.md");
    let filename = Path::new(launch["file"].as_str().unwrap());
    assert_eq!(fs::read_to_string(filename).unwrap(), original);
    assert_eq!(
        fs::read_to_string(filename.parent().unwrap().join("references/notes.md")).unwrap(),
        "# Notes\n"
    );
    let reference = open("references/notes.md");
    let save = |launch: &Value, content: &str, id: &str, success: bool| {
        let mut cmd = f.command(&[
            "skill",
            "--json",
            "editor-save",
            launch["skillSession"].as_str().unwrap(),
        ]);
        configure(&mut cmd, &f);
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        serde_json::to_writer(
            child.stdin.take().unwrap(),
            &json!({"content":content,"request_id":id}),
        )
        .unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.success(), success, "{out:?}");
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let verify = |name: &str, content: &str| {
        for home in &homes {
            for agent in [".codex", ".agents", ".claude", ".hey-boss"] {
                assert_eq!(
                    fs::read_to_string(home.join(agent).join("skills").join(name).join("SKILL.md"))
                        .unwrap(),
                    content
                );
            }
        }
    };
    let first = text("example", "First save");
    let second = text("example", "Second save");
    let drift = text("example", "Changed after scanning");
    put(homes[1].join(".codex/skills/example/SKILL.md"), &drift);
    save(&launch, &first, "first", true);
    verify("example", &first);
    save(&reference, "# Reference edited\n", "reference", true);
    verify("example", &first);
    save(&launch, &second, "second", true);
    verify("example", &second);
    for home in &homes {
        assert_eq!(
            fs::read_to_string(home.join(".codex/skills/example/references/notes.md")).unwrap(),
            "# Reference edited\n"
        );
    }
    save(&launch, &first, "first", true);
    verify("example", &second);
    assert!(
        fs::read_dir(homes[1].join(".hey-boss/skill-backups"))
            .unwrap()
            .any(
                |e| fs::read_to_string(e.unwrap().path().join(".codex/skills/example/SKILL.md"))
                    .ok()
                    .as_ref()
                    == Some(&drift)
            )
    );
    let journal = homes[0]
        .join(".hey-boss/skill-edits")
        .join(launch["skillSession"].as_str().unwrap())
        .join("session.json");
    let before = fs::read(&journal).unwrap();
    let offline = homes[1].join(".offline");
    put(offline.clone(), "");
    let renamed = text("renamed", "Offline rename");
    assert_eq!(
        save(&launch, &renamed, "rename", false)["error"]["code"],
        "sync_pending"
    );
    // Crash after local rename but before the editor journal recorded it.
    fs::write(journal, before).unwrap();
    fs::remove_file(offline).unwrap();
    save(&launch, &renamed, "rename", true);
    verify("renamed", &renamed);
    let last = text("renamed", "Next save after rename");
    save(&launch, &last, "after-rename", true);
    verify("renamed", &last);
    assert_eq!(fs::read_to_string(filename).unwrap(), last);
    if let Some(audit) = std::env::var_os("SKILL_NATIVE_AUDIT") {
        let path = root.join("launch.json");
        put(path.clone(), serde_json::to_vec(&launch).unwrap());
        let mut audit = Command::new(audit);
        let cli = f.command(&[]);
        audit
            .env_clear()
            .envs(cli.get_envs().filter_map(|(k, v)| v.map(|v| (k, v))))
            .current_dir(root);
        configure(&mut audit, &f);
        if let Some(dir) = std::env::var_os("SKILLS_SCREENSHOTS") {
            audit.env("SKILLS_SCREENSHOTS", dir);
        }
        assert!(
            audit
                .env("HEY_BOSS_AUDIT_SKILL_EDITOR", path)
                .env("HEY_BOSS_CLI_PATH", &f.binary)
                .status()
                .unwrap()
                .success()
        );
        verify("renamed", &(last + "\nNative / edit 🦀\n"));
    }
}
