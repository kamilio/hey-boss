//! Preserve browser drafts and remote files across renames, failures and reviewed retries.
use super::super::skills::{EDITOR, click, replace_editor, shot, viewport};
use super::*;

fn text(name: &str) -> String {
    format!("---\nname: {name}\ndescription: Fixture\n---\n[Reference](references/notes.md)\n")
}
fn save(name: &str, body: &str, digest: &Value) -> Value {
    json!({"action":"save_file","skill":name,"file_path":"SKILL.md","content":body,"base_digest":digest})
}
fn reload(p: &mut Page) {
    // This test deliberately reloads with unsaved edits, as if the user confirmed the dialog.
    p.eval("window.oldSkillPage=true").unwrap();
    p.command("Page.reload", json!({})).unwrap();
    p.wait("!window.oldSkillPage && !!document.querySelector('.skill-detail-heading')");
}
fn draft_contains(p: &mut Page, expected: &str) {
    p.wait(&format!(
        "{EDITOR}?.innerText.includes({}) === true",
        json!(expected)
    ));
}
fn healthy(report: &Value) {
    assert!(
        report["machines"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["error"].as_str().unwrap_or("").is_empty()),
        "{report}"
    );
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn renames_preserve_browser_drafts_and_update_both_machines() {
    let f = fixture();
    f.init("Skills QA").unwrap();
    let root = f.root.path();
    let homes = [root.join("home"), root.join("studio")];
    let dirs: Vec<_> = homes
        .iter()
        .flat_map(|home| {
            [".codex", ".agents", ".claude", ".hey-boss"]
                .map(|agent| home.join(agent).join("skills"))
        })
        .collect();
    let (before, after, last, taken) = (
        "rename-before",
        "rename-after",
        "rename-final",
        "rename-taken",
    );
    for home in &homes {
        let dir = home.join(".codex/skills").join(before);
        put(dir.join("SKILL.md"), text(before));
        put(dir.join("references/notes.md"), "# Reference\n");
        put(dir.join("helper.sh"), "#!/bin/sh\nexit 0\n");
        fs::set_permissions(dir.join("helper.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        put(
            home.join(".hey-boss/skill-sync.json"),
            json!({"selected":[before]}).to_string(),
        );
    }
    install_transport(root);
    put(root.join("config.yaml"), "ssh_hosts: [studio]\n");
    let api = Api::start(&f);
    api.post(&json!({"action":"scan"}), 200);
    let scan = api.idle();
    healthy(&scan);
    let digest = &copy(&scan, "local", before)["digest"];
    let old = homes[0].join(".codex/skills").join(before);
    let remote = homes[1].join(".codex/skills").join(before);
    put(
        homes[0].join(".agents/skills/rename-taken/SKILL.md"),
        text(taken),
    );
    assert!(
        api.post(&save(before, &text(taken), digest), 409)
            .to_string()
            .contains("already exists")
    );
    assert_eq!(
        fs::read_to_string(old.join("SKILL.md")).unwrap(),
        text(before)
    );
    assert!(
        api.post(&save(before, &text("../escape"), digest), 400)
            .to_string()
            .contains("name")
    );
    let saved = api.post(&save(before, &text(after), digest), 200);
    assert_eq!(saved["saved_skill"], after);
    let selected = saved["selected"].as_array().unwrap();
    assert!(selected.contains(&json!(after)) && !selected.contains(&json!(before)));
    assert!(!old.exists() && remote.exists());
    drop(api);
    let api = Api::start(&f);
    let distribute = |report: &Value| {
        api.post(&json!({"action":"distribute","revision":report["revision"],"skill":after,"selected":report["selected"],"choices":report["choices"]}),200);
        api.idle()
    };
    let drift = text(before) + "Remote edit\n";
    put(remote.join("SKILL.md"), &drift);
    let conflict = distribute(&api.report());
    assert!(
        machine(&conflict, "studio")["error"]
            .as_str()
            .unwrap()
            .contains("changed since")
    );
    assert_eq!(fs::read_to_string(remote.join("SKILL.md")).unwrap(), drift);
    healthy(&distribute(&conflict));
    let verify = |name: &str, body: &str| {
        for dir in &dirs {
            assert_eq!(
                fs::read_to_string(dir.join(name).join("SKILL.md")).unwrap(),
                body,
                "{}",
                dir.display()
            );
        }
    };
    verify(after, &text(after));
    for dir in &dirs {
        assert!(!dir.join(before).exists());
        assert_eq!(
            fs::read_to_string(dir.join(after).join("references/notes.md")).unwrap(),
            "# Reference\n"
        );
        assert_ne!(
            fs::metadata(dir.join(after).join("helper.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
    }
    let mut p = Page::open("about:blank");
    p.command("Page.enable", json!({})).unwrap();
    p.command("Page.addScriptToEvaluateOnNewDocument",json!({"source":"addEventListener('error',e=>sessionStorage.setItem('pageError',e.message));addEventListener('unhandledrejection',e=>sessionStorage.setItem('pageError',String(e.reason)))"})).unwrap();
    let url = reqwest::Url::parse(&api.url)
        .unwrap()
        .join("/skills#skill=global%3Arename-after")
        .unwrap();
    p.command("Page.navigate", json!({"url":url.as_str()}))
        .unwrap();
    let save_button = "[data-save-markdown=distribute]";
    click(&mut p, "[data-toggle-editor]");
    let draft = text(after) + "Unsaved browser draft\n";
    replace_editor(&mut p, &draft);
    reload(&mut p);
    draft_contains(&mut p, "Unsaved browser draft");
    click(&mut p, "[data-md-file='references/notes.md']");
    replace_editor(&mut p, "Unsaved reference draft");
    click(&mut p, "[data-md-file='SKILL.md']");
    draft_contains(&mut p, "Unsaved browser draft");
    click(&mut p, "[data-open='global:rename-taken']");
    click(&mut p, "[data-open='global:rename-after']");
    draft_contains(&mut p, "Unsaved browser draft");

    p.command("Network.enable", json!({})).unwrap();
    let network = |p: &mut Page, offline| {
        p.command(
            "Network.emulateNetworkConditions",
            json!({"offline":offline,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1}),
        )
        .unwrap()
    };
    network(&mut p, true);
    click(&mut p, save_button);
    p.wait(
        "document.querySelector('[data-editor-status]')?.textContent.includes('Could not save')",
    );
    network(&mut p, false);
    reload(&mut p);
    draft_contains(&mut p, "Unsaved browser draft");
    // Delay delivery of one real server reply so a user edit can occur after the write commits.
    p.eval("const realFetch=window.fetch;window.fetch=async (...args)=>{const response=await realFetch(...args);if(args[1]?.body && JSON.parse(args[1].body).action==='save_file'){window.fetch=realFetch;await new Promise(resolve=>window.releaseSave=resolve)}return response}").unwrap();
    click(&mut p, save_button);
    p.wait("typeof releaseSave === 'function'");
    replace_editor(&mut p, &(draft + "Typed while saving\n"));
    p.eval("releaseSave()").unwrap();
    p.wait("document.querySelector('[data-save-markdown]')?.disabled === false");
    draft_contains(&mut p, "Typed while saving");
    reload(&mut p);
    draft_contains(&mut p, "Typed while saving");
    replace_editor(&mut p, &text(last));
    shot(&mut p, "editing", Some("html"));
    click(&mut p, save_button);
    p.wait("document.querySelector('.skill-detail-heading h2')?.textContent === 'rename-final'");
    assert_eq!(
        p.eval("new URLSearchParams(location.hash.slice(1)).get('skill')")
            .unwrap(),
        "global:rename-final"
    );
    healthy(&api.idle());
    verify(last, &text(last));
    assert!(!homes[1].join(".codex/skills").join(after).exists());
    reload(&mut p);
    click(&mut p, "[data-toggle-editor]");
    let updated = text(last) + "Updated fixture content\n";
    replace_editor(&mut p, &updated);
    click(&mut p, save_button);
    p.wait("!document.querySelector('[data-save-markdown]')");
    healthy(&api.idle());
    verify(last, &updated);
    put(
        homes[1].join(".hey-boss/skills/rename-final/SKILL.md"),
        text(last),
    );
    api.post(&json!({"action":"scan"}), 200);
    let report = api.idle();
    reload(&mut p);
    let unify = |p: &mut Page, digest: &str| {
        p.wait("document.querySelectorAll('.skill-version').length===2");
        click(p, &format!("[data-choose='{digest}']"));
        click(p, &format!("[data-unify='{digest}']"));
        p.wait("document.querySelectorAll('.skill-version').length===1");
        api.idle()
    };
    p.wait("document.querySelector('.skill-versions').textContent.includes('Saved copy only')");
    shot(&mut p, "saved-copy-conflict", Some("html"));
    let saved_only = machine(&report, "studio")["copies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == last && c["agent"] == "library")
        .unwrap();
    let report = unify(&mut p, saved_only["digest"].as_str().unwrap());
    verify(last, &text(last));
    let saved = api.post(
        &save(last, &updated, &copy(&report, "local", last)["digest"]),
        200,
    );
    reload(&mut p);
    unify(&mut p, saved["choices"][last].as_str().unwrap());
    verify(last, &updated);

    reload(&mut p);
    click(&mut p, "[data-toggle-editor]");
    replace_editor(&mut p, &text(taken));
    click(&mut p, save_button);
    p.wait("!document.querySelector('#skills-error').hidden && document.querySelector('[data-save-markdown]').disabled===false");
    draft_contains(&mut p, taken);
    reload(&mut p);
    draft_contains(&mut p, taken);
    click(&mut p, "[data-md-file='references/notes.md']");
    draft_contains(&mut p, "Unsaved reference draft");
    click(&mut p, "[data-md-file='SKILL.md']");
    let recovered = updated.clone() + "Browser draft before external save\n";
    replace_editor(&mut p, &recovered);
    let mut external = save(
        last,
        &(updated + "External save\n"),
        &api.report()["choices"][last],
    );
    external["distribute"] = true.into();
    api.post(&external, 200);
    api.idle();
    reload(&mut p);
    draft_contains(&mut p, "Browser draft before external save");
    click(&mut p, save_button);
    p.wait(
        "document.querySelector('[data-editor-status]')?.textContent.includes('Could not save')",
    );
    click(&mut p, "[data-rebase-draft]");
    click(&mut p, save_button);
    p.wait("!document.querySelector('[data-save-markdown]')");
    api.idle();
    verify(last, &recovered);
    reload(&mut p);
    click(&mut p, "[data-toggle-editor]");
    replace_editor(&mut p, &text(taken));
    click(&mut p, save_button);
    p.wait(
        "document.querySelector('[data-editor-status]')?.textContent.includes('Could not save')",
    );
    viewport(&mut p, 390, "light");
    p.wait("Array.from(document.querySelectorAll('.skill-markdown-editor-actions button')).every(b=>{const r=b.getBoundingClientRect(),card=b.closest('.skill-markdown-editor-card').getBoundingClientRect();return r.left>=card.left && r.right<=card.right})");
    shot(&mut p, "rejected-mobile", Some("html"));
    assert_eq!(
        p.eval("sessionStorage.getItem('pageError')").unwrap(),
        Value::Null
    );
}
