use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn skills_reconnect_replays_only_a_rejected_write_for_the_same_actor() {
    let mut initial = inventory(
        &[skill(
            "AGENTS.md",
            "original",
            "# Example\nOriginal content",
        )],
        "AGENTS.md",
    );
    for m in initial["machines"].as_array_mut().unwrap() {
        m["copies"]
            .as_array_mut()
            .unwrap()
            .push(skill("example", "example", "Example skill"));
    }
    with_page(initial.clone(), |p, f, url| {
        let mut run = 0;
        let mut fresh = |p: &mut Page| {
            *f.lock().unwrap() = Fleet::new(initial.clone());
            run += 1;
            navigate(p, &format!("{url}?case={run}"));
        };
        let ready =
            |p: &mut Page| p.wait("document.querySelector('#skills-scan')?.disabled===false");
        let scan = |p: &mut Page| {
            click(p, "#skills-scan");
            ready(p);
        };
        fresh(p);
        click(p, "[data-select=example]");
        p.eval("document.querySelector('#skills-search').value='AGENTS';document.querySelector('#skills-search').dispatchEvent(new Event('input'))").unwrap();
        f.lock().unwrap().token = "after-restart";
        scan(p);
        shot(p, "restarted");
        assert_eq!(
            p.eval("document.querySelector('#skills-error').hidden")
                .unwrap(),
            true
        );
        {
            let s = f.lock().unwrap();
            assert_eq!(s.bootstraps, 2);
            assert_eq!(s.posts.len(), 2);
            assert_eq!(s.posts[0].0, s.posts[1].0);
            assert_eq!(s.posts[0].1, "initial");
            assert_eq!(s.posts[1].1, "after-restart");
            assert_eq!(s.applied.len(), 1);
        }
        assert_eq!(
            p.eval("document.querySelector('#skills-search').value")
                .unwrap(),
            "AGENTS"
        );
        let summary = p
            .eval("document.querySelector('#selection-summary').textContent")
            .unwrap();
        let summary = summary.as_str().unwrap();
        assert!(
            summary.contains("2 skills selected") && summary.contains("Unsaved changes"),
            "{summary}"
        );
        assert_eq!(
            p.eval("document.querySelector('.skill-detail-heading h2').textContent")
                .unwrap(),
            "AGENTS.md"
        );
        for failure in [
            "forbidden",
            "actor",
            "bootstrap",
            "server",
            "network",
            "conflict",
        ] {
            fresh(p);
            {
                let mut s = f.lock().unwrap();
                s.failure = failure;
                if matches!(failure, "actor" | "bootstrap" | "conflict") {
                    s.token = "rotated";
                }
                if failure == "actor" {
                    s.actor = "different";
                }
            }
            scan(p);
            p.wait("document.querySelector('#skills-error').hidden === false");
            assert_eq!(
                p.eval("document.querySelector('.skill-detail-heading h2').textContent")
                    .unwrap(),
                "AGENTS.md"
            );
            if failure == "forbidden" {
                shot(p, "unrecoverable");
            }
            {
                let mut s = f.lock().unwrap();
                assert_eq!(
                    s.posts.len(),
                    if failure == "conflict" { 2 } else { 1 },
                    "{failure}"
                );
                assert!(s.applied.is_empty(), "{failure}");
                s.failure = "";
                s.token = "retry";
                s.actor = "fixture";
            }
            click(p, "[data-refresh-inventory]");
            ready(p);
            assert_eq!(p.eval("document.querySelector('#skills-error').hidden && !document.querySelector('.skill-rollout-feedback.has-error')").unwrap(),true);
            assert_eq!(f.lock().unwrap().applied.len(), 1, "{failure}");
        }
        fresh(p);
        {
            let mut s = f.lock().unwrap();
            s.token = "second";
            s.failure = "forbidden";
        }
        scan(p);
        let reads = {
            let s = f.lock().unwrap();
            assert_eq!(s.posts.len(), 2);
            assert_eq!(s.bootstraps, 2);
            s.reads
        };
        until(f, |s| s.reads > reads);
        assert_eq!(
            p.eval("document.querySelector('#skills-error').hidden")
                .unwrap(),
            false
        );
        fresh(p);
        edit(p, "Unsaved Markdown draft");
        f.lock().unwrap().token = "editor-restart";
        scan(p);
        assert_eq!(
            p.eval(&format!("{EDITOR}.innerText")).unwrap(),
            "Unsaved Markdown draft"
        );
        click(p, "[data-save-markdown=distribute]");
        ready(p);
        {
            let s = f.lock().unwrap();
            let saved = s.applied.last().unwrap();
            assert_eq!(saved["content"], "Unsaved Markdown draft");
            assert_eq!(saved["base_digest"], "original");
            assert_eq!(saved["distribute"], true);
        }
        fresh(p);
        edit(p, "Saved after reconnect");
        f.lock().unwrap().token = "save-restart";
        click(p, "[data-save-markdown=distribute]");
        ready(p);
        {
            let s = f.lock().unwrap();
            assert_eq!(s.posts.len(), 2);
            assert_eq!(s.posts[0].0, s.posts[1].0);
            assert_eq!(s.applied.len(), 1);
            assert_eq!(s.applied[0]["content"], "Saved after reconnect");
        }
        for width in [1440, 768, 390, 320] {
            for theme in ["light", "dark"] {
                viewport(p, width, theme);
                p.wait("document.querySelector('.version-preview').getBoundingClientRect().width >= 180");
                shot(p, &format!("{width}-{theme}"));
            }
        }
    });
}
