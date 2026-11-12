use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn skill_choice_survives_inflight_writes_and_inventory_polls() {
    let name = "AGENTS.md";
    let remote = skill(name, "remote", "# Fixture\nRemote version");
    let mut data = inventory(
        &[
            skill(name, "local", "# Fixture\nLocal version"),
            remote.clone(),
        ],
        name,
    );
    data["choices"] = json!({name:"local"});
    let initial = data.clone();
    data["busy"] = true.into();
    with_page(data, |p, f, url| {
        f.lock().unwrap().delay = true;
        p.command("Page.navigate", json!({"url":url})).unwrap();
        click(p, "[data-choose=remote]");
        chosen(p, "remote");
        assert_eq!(
            p.eval("document.querySelector('[data-unify]').disabled")
                .unwrap(),
            true
        );
        f.lock().unwrap().inventory["message"] = "Still distributing".into();
        p.wait(
            "document.querySelector('#skills-status').textContent.includes('Still distributing')",
        );
        chosen(p, "remote");
        assert!(f.lock().unwrap().posts.is_empty());
        for width in [1440, 768, 390, 320] {
            for theme in ["light", "dark"] {
                viewport(p, width, theme);
                p.wait("document.querySelector('[data-choose=local]').disabled === false && document.querySelector('.gh-diff-file-meta').getBoundingClientRect().right <= document.querySelector('.gh-diff-file').getBoundingClientRect().right+1");
                shot(p, &format!("busy-{width}-{theme}"), Some("#skills-detail"));
            }
        }
        {
            let mut s = f.lock().unwrap();
            s.inventory["busy"] = false.into();
            s.inventory["revision"] = 18.into();
            s.inventory["machines"][1]["error"] = "Machine timed out".into();
        }
        p.wait("!document.querySelector('[data-unify]').disabled");
        chosen(p, "remote");
        shot(p, "retry-ready", Some("#skills-detail"));
        click(p, "[data-unify]");
        until(f, |s| s.pending.is_some());
        {
            let s = f.lock().unwrap();
            assert_eq!(s.posts.len(), 1);
            assert_eq!(s.applied[0]["choices"][name], "remote");
            assert_eq!(s.applied[0]["revision"], 18);
        }
        p.eval("document.querySelector('[data-choose=local]').focus()")
            .unwrap();
        key(p, "Enter", "Enter", 13, "\r");
        chosen(p, "local");
        let before = {
            let mut s = f.lock().unwrap();
            s.release();
            s.reads
        };
        until(f, |s| s.reads > before);
        chosen(p, "local");
        {
            let mut s = f.lock().unwrap();
            s.inventory["busy"] = false.into();
            s.inventory["revision"] = 19.into();
            assert_eq!(s.posts.len(), 1);
        }
        click(p, "[data-unify]");
        until(f, |s| s.pending.is_some());
        {
            let mut s = f.lock().unwrap();
            assert_eq!(s.applied.len(), 2);
            assert_eq!(s.applied[1]["choices"][name], "local");
            s.release();
            s.inventory["busy"] = false.into();
            s.inventory["revision"] = 20.into();
            for m in s.inventory["machines"].as_array_mut().unwrap() {
                m["copies"] = json!([remote]);
            }
        }
        p.wait("document.querySelectorAll('.skill-version').length===1 && document.querySelector('[data-unify]').disabled");
        click(p, "[data-choose=remote]");
        p.wait("!document.querySelector('[data-unify]').disabled");
        shot(p, "reviewed", Some("#skills-detail"));
        f.lock().unwrap().inventory = initial;
        click(p, "#skills-reset");
        p.wait("document.querySelectorAll('.skill-version').length===2");
        navigate(p, &format!("{url}?save"));
        edit(p, "# Fixture\nEdited local version");
        click(p, "[data-save-markdown=distribute]");
        until(f, |s| s.pending.is_some());
        click(p, "[data-choose=remote]");
        chosen(p, "remote");
        let before = {
            let mut s = f.lock().unwrap();
            assert_eq!(s.applied.last().unwrap()["action"], "save_file");
            s.release();
            s.reads
        };
        until(f, |s| s.reads > before);
        chosen(p, "remote");
    });
}
