//! Exercise the skills API across two private homes and an unreachable machine.
use super::*;
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    time::Instant,
};

fn put(path: PathBuf, bytes: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn install_transport(root: &Path) {
    fs::create_dir(root.join("bin")).unwrap();
    symlink(worker_fixtures::binary("skill-ssh"), root.join("bin/ssh")).unwrap();
    symlink("/usr/bin/true", root.join("bin/open")).unwrap();
}
fn configure(c: &mut Command, f: &Fixture) {
    let root = f.root.path();
    c.env("TMPDIR", root)
        .env("SKILL_TEST_CLI", &f.binary)
        .env("SKILL_TEST_STUDIO", root.join("studio"))
        .env(
            "PATH",
            std::env::join_paths([root.join("bin"), "/usr/bin".into(), "/bin".into()]).unwrap(),
        );
}
struct Api {
    _server: Service,
    url: String,
    client: reqwest::blocking::Client,
    csrf: String,
}
impl Api {
    fn start(f: &Fixture) -> Self {
        let mut c = f.command(&[
            "issue",
            "--project",
            "Skills QA",
            "web",
            "--port",
            "0",
            "--no-discovery",
            "--json",
        ]);
        configure(&mut c, f);
        let (server, base) = start_web(c);
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let bootstrap: Value = client
            .get(format!("{base}/api/bootstrap"))
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap();
        Self {
            _server: server,
            url: format!("{base}/api/skills"),
            client,
            csrf: bootstrap["csrf"].as_str().unwrap().into(),
        }
    }
    fn post(&self, request: &Value, status: u16) -> Value {
        let response = self
            .client
            .post(&self.url)
            .header("X-Hey-Boss-CSRF", &self.csrf)
            .json(request)
            .send()
            .unwrap();
        let actual = response.status().as_u16();
        let value = response.json().unwrap();
        assert_eq!(actual, status, "{request}: {value}");
        value
    }
    fn report(&self) -> Value {
        self.client
            .get(&self.url)
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap()
    }
    fn idle(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let report = self.report();
            if report["busy"] == false {
                return report;
            }
            assert!(
                Instant::now() < deadline,
                "Skills job stayed busy: {report}"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}
fn machine<'a>(report: &'a Value, host: &str) -> &'a Value {
    report["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["host"] == host)
        .unwrap()
}
fn copy<'a>(report: &'a Value, host: &str, skill: &str) -> &'a Value {
    machine(report, host)["copies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == skill)
        .unwrap()
}

#[test]
#[ignore = "requires a built hey-boss CLI and the skills transport"]
fn skills_distribution_survives_conflicts_and_server_restart() {
    let f = fixture();
    f.init("Skills QA").unwrap();
    let root = f.root.path();
    let local = root.join("home");
    let studio = root.join("studio");
    let homes = [&local, &studio];
    let agents = [".codex", ".agents", ".claude", ".hey-boss"];
    let instructions = "# Fixture instructions\n";
    for (host, home) in [("local", &local), ("studio", &studio)] {
        put(home.join(".hey-boss/skill-sync.json"), r#"{"selected":[]}"#);
        put(
            home.join(".claude/skills/stacked-prs/SKILL.md"),
            format!("---\nname: stacked-prs\ndescription: Fixture\n---\n{host} version\n"),
        );
        put(
            home.join(".codex/AGENTS.md"),
            if host == "local" { instructions } else { "" },
        );
    }
    let remote = studio.join(".agents/skills/remote-only");
    put(
        remote.join("SKILL.md"),
        "---\nname: remote-only\ndescription: Fixture\n---\nRun the helper.",
    );
    let script = remote.join("scripts/nested/run.sh");
    put(script.clone(), "#!/bin/sh\necho hello\n");
    fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
    put(remote.join("references/notes.md"), "# Companion notes\n");
    let bytes: Vec<u8> = (0..=255).collect();
    put(remote.join("asset.bin"), &bytes);
    let junk = studio.join(".claude/skills/bad-junk");
    put(junk.join("SKILL.md"), "---\nname: bad-junk\n---\nFixture");
    put(root.join("config.yaml"), "ssh_hosts: [studio, offline]\n");
    install_transport(root);
    let api = Api::start(&f);
    let scan = json!({"action":"scan"});
    assert_eq!(
        api.client
            .post(&api.url)
            .json(&scan)
            .send()
            .unwrap()
            .status(),
        403
    );
    assert_eq!(api.post(&scan, 200)["busy"], true);
    let report = api.idle();
    assert_eq!(report["machines"].as_array().unwrap().len(), 3);
    assert_eq!(machine(&report, "offline")["state"], "attention");
    assert!(
        machine(&report, "studio")["copies"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c.get("files").is_none())
    );
    let source = &copy(&report, "studio", "stacked-prs")["digest"];
    let remote_copy = copy(&report, "studio", "remote-only");
    assert_eq!(
        remote_copy["markdown_files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["SKILL.md", "references/notes.md"]
    );
    assert!(
        remote_copy["ignored_files"]
            .as_array()
            .unwrap()
            .contains(&json!("scripts/nested/run.sh"))
    );
    let mut payload = json!({"action":"distribute","revision":report["revision"],"selected":["stacked-prs","remote-only"],"choices":{},"max_words":300});
    api.post(&payload, 409);
    payload["choices"] = json!({"stacked-prs":source});
    payload["revision"] = json!(report["revision"].as_i64().unwrap() - 1);
    api.post(&payload, 409);
    assert_eq!(api.report()["selected"], json!(["hey-boss"]));
    payload["revision"] = report["revision"].clone();
    api.post(&payload, 200);
    let done = api.idle();
    assert_eq!(done["max_words"], 300);
    for home in homes {
        for agent in agents {
            let dir = home.join(agent).join("skills");
            assert!(
                fs::read_to_string(dir.join("stacked-prs/SKILL.md"))
                    .unwrap()
                    .contains("studio version")
            );
            assert_eq!(fs::read(dir.join("remote-only/asset.bin")).unwrap(), bytes);
            assert_ne!(
                fs::metadata(dir.join("remote-only/scripts/nested/run.sh"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0
            );
        }
    }
    let changed = local.join(".claude/skills/stacked-prs/SKILL.md");
    fs::write(&changed, "Edited after scanning. Keep this.").unwrap();
    payload["revision"] = done["revision"].clone();
    api.post(&payload, 200);
    assert!(
        machine(&api.idle(), "local")["error"]
            .as_str()
            .unwrap()
            .contains("changed since")
    );
    assert_eq!(
        fs::read_to_string(changed).unwrap(),
        "Edited after scanning. Keep this."
    );
    drop(api);
    let api = Api::start(&f);
    let saved = api.report();
    assert_eq!(&saved["choices"]["stacked-prs"], source);
    assert!(
        saved["selected"]
            .as_array()
            .unwrap()
            .contains(&json!("remote-only"))
    );
    assert_eq!(saved["busy"], false);
    api.post(&json!({"action":"distribute","revision":saved["revision"],"skill":"AGENTS.md","selected":["AGENTS.md"],"choices":{"AGENTS.md":copy(&saved,"local","AGENTS.md")["digest"]},"max_words":400}),200);
    api.idle();
    for home in homes {
        for path in [
            ".codex/AGENTS.md",
            ".agents/AGENTS.md",
            ".claude/CLAUDE.md",
            ".hey-boss/skills/AGENTS.md/SKILL.md",
        ] {
            assert_eq!(fs::read_to_string(home.join(path)).unwrap(), instructions);
        }
    }
    let notes = "# Updated companion notes\n";
    api.post(&json!({"action":"save_file","skill":"remote-only","file_path":"references/notes.md","content":notes,"base_digest":remote_copy["digest"],"distribute":true}),200);
    api.idle();
    for home in homes {
        for agent in agents {
            assert_eq!(
                fs::read_to_string(
                    home.join(agent)
                        .join("skills/remote-only/references/notes.md")
                )
                .unwrap(),
                notes
            );
        }
    }
    let opened = api.post(&json!({"action":"open_dir","skill":"remote-only"}), 200);
    assert!(Path::new(opened["opened_dir"].as_str().unwrap()).is_dir());
    assert!(Path::new(opened["opened_file"].as_str().unwrap()).is_file());
    api.post(&json!({"action":"delete","skill":"bad-junk"}), 200);
    let end = api.idle();
    assert!(!junk.exists());
    assert!(
        end["machines"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|m| m["copies"].as_array().unwrap())
            .all(|c| c["name"] != "bad-junk")
    );
}

#[path = "skills_native.rs"]
mod native;

#[path = "skills_disable.rs"]
mod disable;

#[path = "skills_rename.rs"]
mod rename;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn skill_folders_move_import_and_remember_directories() {
    let f = fixture();
    f.init("Skills QA").unwrap();
    let root = f.root.path();
    let source = root.join("home/incoming/calendar");
    let destination = root.join("home/moved");
    put(
        source.join("SKILL.md"),
        "---\nname: calendar\ndescription: Calendar fixture\n---\nCalendar\n",
    );
    put(source.join("references/notes.md"), "Notes\n");
    fs::create_dir(&destination).unwrap();
    put(root.join("config.yaml"), "ssh_hosts: []\n");
    let api = Api::start(&f);
    api.post(&json!({"action":"scan"}), 200);
    api.idle();
    let url = api.url.replace("/api/skills", "/skills");
    let mut page = Page::open("about:blank");
    page.command("Page.navigate", json!({"url":url})).unwrap();
    page.wait("document.querySelector('#skills-import')?.disabled === false");
    page.eval("document.querySelector('#skills-import').click()")
        .unwrap();
    page.eval(&format!(
        "document.querySelector('[name=directory]').value={}",
        json!(source)
    ))
    .unwrap();
    page.eval("document.querySelector('#skill-directory-dialog form').requestSubmit()")
        .unwrap();
    page.wait("!document.querySelector('#skill-directory-dialog').open && !!document.querySelector('[data-move-skill]')");
    assert_eq!(api.report()["last_import_directory"], json!(source));
    assert!(source.join("SKILL.md").exists());
    let imported = root.join("home/.hey-boss/skills/calendar");
    assert!(imported.join("references/notes.md").exists());

    page.eval("document.querySelector('[data-move-skill]').click()")
        .unwrap();
    assert_eq!(
        page.eval("document.querySelector('[name=source]').value")
            .unwrap(),
        json!(imported)
    );
    page.eval(&format!(
        "document.querySelector('[name=directory]').value={}",
        json!(destination)
    ))
    .unwrap();
    assert!(
        page.eval("document.querySelector('#skill-directory-dialog').textContent")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("Destination project folder")
    );
    for (width, theme) in [(1440, "light"), (390, "dark")] {
        page.command(
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":900,"deviceScaleFactor":1,"mobile":width<500}),
        )
        .unwrap();
        page.command(
            "Emulation.setEmulatedMedia",
            json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
        )
        .unwrap();
        assert_eq!(
            page.eval("document.documentElement.scrollWidth <= innerWidth")
                .unwrap(),
            true
        );
        screenshot(&mut page, "out/skill-directories", &format!("move-{width}"));
    }
    page.eval("document.querySelector('#skill-directory-dialog form').requestSubmit()")
        .unwrap();
    page.wait("!document.querySelector('#skill-directory-dialog').open");
    assert!(!imported.exists());
    assert!(
        destination
            .join(".agents/skills/calendar/references/notes.md")
            .exists()
    );
    assert_eq!(api.report()["last_move_directory"], json!(destination));
    drop(api);

    let api = Api::start(&f);
    assert_eq!(api.report()["last_move_directory"], json!(destination));
    assert_eq!(api.report()["last_import_directory"], json!(source));
    page.command(
        "Page.navigate",
        json!({"url":api.url.replace("/api/skills", "/skills")}),
    )
    .unwrap();
    page.wait("document.querySelector('#skills-import')?.disabled === false");
    page.eval("document.querySelector('#skills-import').click()")
        .unwrap();
    assert_eq!(
        page.eval("document.querySelector('[name=directory]').value")
            .unwrap(),
        json!(source)
    );
    page.eval("document.querySelector('#skill-directory-dialog form').requestSubmit()")
        .unwrap();
    page.wait("!document.querySelector('#skill-directory-dialog').open && !!document.querySelector('[data-move-skill]')");
    page.eval("document.querySelector('[data-move-skill]').click()")
        .unwrap();
    assert_eq!(
        page.eval("document.querySelector('[name=directory]').value")
            .unwrap(),
        json!(destination)
    );
    page.eval("document.querySelector('#skill-directory-dialog form').requestSubmit()")
        .unwrap();
    page.wait("document.querySelector('#skill-directory-dialog [role=alert]')?.hidden === false");
    assert!(
        page.eval("document.querySelector('#skill-directory-dialog [role=alert]').textContent")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("already exists")
    );
    assert!(imported.join("SKILL.md").exists());
    assert!(
        destination
            .join(".agents/skills/calendar/SKILL.md")
            .exists()
    );
}
