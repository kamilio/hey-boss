use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn skills_library_reports_diffs_coverage_and_conflicts() {
    let f = fixture();
    f.init("Skills QA").unwrap();
    let (_server, base) = web(&f, "Skills QA");
    let mut page = Page::open("about:blank");
    page.command("Page.navigate", json!({"url":format!("{base}/skills")}))
        .unwrap();
    page.wait("typeof rolloutFeedback === 'function'");
    let copy = |name: &str, digest: &str, scope: &str, agent: &str| json!({"name":name,"digest":digest,"scope":scope,"agent":agent,"description":"Stacks","warnings":[]});
    let data = json!({"machines":[{"host":"laptop","state":"online","copies":[copy("stacked-prs","a","global","codex"),copy("stacked-prs","a","global","claude"),copy("AGENTS.md","m1","global","codex")]},{"host":"desktop","state":"attention","copies":[copy("stacked-prs","b","global","codex"),copy("remote-only","c","global","codex"),copy("local-task","p","project","agents")]}]});
    page.eval(&format!("const inventory={data}, fixtureSkills=library(inventory), archived=library({{...inventory,disabled:['stacked-prs','archived-only']}}), selectedStack=new Set(['stacked-prs']);")).unwrap();
    for (call, expected) in [
        ("fixtureSkills.length", json!(4)),
        ("fixtureSkills[0].name", json!("AGENTS.md")),
        (
            "fixtureSkills.find(s=>s.name==='stacked-prs').versions.length",
            json!(2),
        ),
        (
            "fixtureSkills.find(s=>s.name==='stacked-prs').copies.length",
            json!(3),
        ),
        (
            "fixtureSkills.find(s=>s.name==='stacked-prs').machines.length",
            json!(2),
        ),
        (
            "fixtureSkills.find(s=>s.name==='remote-only').copies[0].stale",
            json!(true),
        ),
        (
            "unresolved(fixtureSkills, selectedStack, {})",
            json!(["stacked-prs"]),
        ),
        (
            "unresolved(fixtureSkills, selectedStack, {'stacked-prs':'b'})",
            json!([]),
        ),
        (
            "unresolved(fixtureSkills, selectedStack, {'stacked-prs':'deleted'})",
            json!(["stacked-prs"]),
        ),
        (
            "visibleSkills(fixtureSkills,'desktop','all',new Set()).length",
            json!(2),
        ),
        (
            "visibleSkills(fixtureSkills,'','selected',selectedStack).length",
            json!(1),
        ),
        (
            "visibleSkills(fixtureSkills,'','project',new Set()).length",
            json!(1),
        ),
        (
            "visibleSkills(archived,'','disabled',new Set()).length",
            json!(2),
        ),
        (
            "visibleSkills(archived,'','selected',selectedStack).length",
            json!(0),
        ),
        ("unresolved(archived,selectedStack,{})", json!([])),
        (
            "archived.find(s=>s.name==='archived-only').copies.length",
            json!(0),
        ),
        (
            "library({machines:[{host:'devbox',state:'conflict',error:'Inventory refreshed',copies:[inventory.machines[0].copies[0]]}]})[0].copies[0].stale",
            json!(false),
        ),
        (
            "getMarkdownFiles({name:'AGENTS.md',text:'# Codex'})[0].path",
            json!("AGENTS.md"),
        ),
    ] {
        assert_eq!(page.eval(call).unwrap(), expected, "{call}");
    }
    let diff=page.eval(r"computeDiff('Line 1\nLine 2\nOld line\nLine 4','Line 1\nLine 2\nNew line\nExtra line\nLine 4')").unwrap();
    assert_eq!(diff["additions"], 2);
    assert_eq!(diff["deletions"], 1);
    assert_eq!(diff["hunks"].as_array().unwrap().len(), 1);
    assert!(
        diff["hunks"][0]["header"]
            .as_str()
            .unwrap()
            .starts_with("@@ -1,4 +1,5 @@")
    );
    let version = |guide| json!({"name":"stacked-prs","markdown_files":[{"path":"SKILL.md","text":"A\nB"},{"path":"references/guide.md","text":guide}]});
    let files = page
        .eval(&format!(
            "diffFiles({},{})",
            version("Old guide"),
            version("New guide")
        ))
        .unwrap();
    assert_eq!(files[0]["path"], "SKILL.md");
    assert_eq!(files[0]["status"], "unchanged");
    assert_eq!(files[1]["path"], "references/guide.md");
    assert_eq!(files[1]["status"], "modified");
    assert_eq!(files[1]["additions"], 1);
    assert_eq!(files[1]["deletions"], 1);
    let hosts = ["local", "devbox", "laptop.local"];
    let machines = hosts.map(|host| json!({"host":host}));
    for (present, versions, tone, labels) in [
        (&hosts[..], 1, "green", vec!["All machines"]),
        (&hosts[..2], 1, "orange", vec!["local", "devbox"]),
        (&hosts[2..], 1, "red", vec!["laptop"]),
        (&hosts[..], 2, "red", vec!["local", "devbox", "laptop"]),
    ] {
        let skill = json!({"machines":present,"versions":(0..versions).map(|digest|json!({"digest":digest})).collect::<Vec<_>>()});
        let result = page
            .eval(&format!("machineCoverage({skill},{})", json!(machines)))
            .unwrap();
        assert_eq!(result["allGreen"], tone == "green");
        assert_eq!(result["tone"], tone);
        let tags = result["tags"].as_array().unwrap();
        assert_eq!(
            tags.iter()
                .map(|t| t["label"].as_str().unwrap())
                .collect::<Vec<_>>(),
            labels
        );
        assert!(tags.iter().all(|t| t["tone"] == tone));
    }
    let failed = json!([{"host":"local"},{"host":"devbox","state":"attention","error":"AGENTS.md changed since scanning <remote>"}]);
    assert_eq!(page.eval(&format!("machineCoverage({{machines:['local','devbox'],versions:[{{digest:'a'}}]}},{failed}).allGreen")).unwrap(),false);
    let feedback = |p: &mut Page, report: Value| {
        p.eval(&format!("rolloutFeedback({report})"))
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    };
    let warning = feedback(
        &mut page,
        json!({"machines":failed,"message":"Distribution needs attention"}),
    );
    for expected in [
        "role=\"alert\"",
        "devbox",
        "changed since scanning &lt;remote&gt;",
        "data-refresh-inventory",
    ] {
        assert!(warning.contains(expected), "{warning}");
    }
    assert!(
        feedback(
            &mut page,
            json!({"busy":true,"machines":[],"message":"Unifying AGENTS.md…"})
        )
        .contains("role=\"status\"")
    );
    assert!(
        feedback(&mut page, json!({"error":"Request failed","machines":[]}))
            .contains("Request failed")
    );
    assert!(
        !feedback(&mut page, json!({"message":"Distributed","machines":[]}))
            .contains("data-refresh-inventory")
    );
}
