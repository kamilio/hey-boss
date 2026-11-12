//! Exercise the worker dashboard on the bundled web server with a private fleet.
use super::*;

fn dashboard(test: impl FnOnce(&mut Page)) {
    let f = fixture();
    let project = "github.com/poe-platform/poe-code";
    f.init(project).unwrap();
    agents::workers(&f, project).unwrap();
    let (_server, base) = web(&f, project);
    let mut page = Page::open(&format!("{base}/workers"));
    page.wait("!!document.querySelector('[data-worker=\"poe-code\"] .agent-runtime')");
    test(&mut page);
}
fn call(p: &mut Page, name: &str, args: Value) -> Value {
    p.eval(&format!("{name}(...{args})")).unwrap()
}
fn field(rows: &Value, pointer: &str) -> Vec<Value> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| r.pointer(pointer).unwrap().clone())
        .collect()
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn elapsed_time_tracks_each_active_agent_and_freezes_finished_attempts() {
    dashboard(|p| {
        for (s, expected) in [
            (0, "0s"),
            (59, "59s"),
            (60, "1m 00s"),
            (3599, "59m 59s"),
            (3600, "1h 00m"),
            (3661, "1h 01m"),
            (86399, "23h 59m"),
            (86400, "1d 00h"),
            (126601, "1d 11h"),
        ] {
            assert_eq!(
                call(p, "elapsed", json!([{"started_at":0},s*1000])),
                expected
            );
        }
        for (run, expected) in [
            (json!({"started_at":1000}), "1m 39s"),
            (json!({"started_at":1000,"finished_at":61000}), "1m 00s"),
            (json!({"started_at":101000}), "0s"),
            (json!({}), ""),
        ] {
            assert_eq!(call(p, "elapsed", json!([run, 100000])), expected);
        }
        p.eval("const clockSelector='[data-worker=\"poe-code\"] .project-worker-task .agent-runtime';const clocks=()=>[...document.querySelectorAll(clockSelector)]").unwrap();
        let snapshot = p
            .eval("fetch('/api/fleet/status').then(r=>r.json())")
            .unwrap();
        let tasks = &snapshot["machines"][0]["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["id"] == "poe-code")
            .unwrap()["runs"];
        p.wait("clocks().length===2 && clocks().every(t=>t.checkVisibility())");
        assert_eq!(
            p.eval("clocks().map(t=>Number(t.dataset.startedAt))")
                .unwrap(),
            json!(field(tasks, "/started_at"))
        );
        assert_eq!(
            p.eval("document.querySelectorAll('[data-worker=tools] .agent-runtime').length")
                .unwrap(),
            0
        );
        let initial = p.eval("clocks()[0].textContent").unwrap();
        p.wait(&format!("clocks()[0].textContent!=={initial} && clocks()[0].textContent===elapsed({{started_at:Number(clocks()[0].dataset.startedAt)}})"));
        for theme in ["light", "dark"] {
            p.command(
                "Emulation.setEmulatedMedia",
                json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
            )
            .unwrap();
            for width in [1440, 390, 320] {
                p.command(
                    "Emulation.setDeviceMetricsOverride",
                    json!({"width":width,"height":950,"deviceScaleFactor":1,"mobile":false}),
                )
                .unwrap();
                p.wait("document.documentElement.scrollWidth<=innerWidth && clocks().every(t=>t.checkVisibility() && t.scrollWidth<=t.clientWidth)");
                let mut clip =
                    p.command("Page.getLayoutMetrics", json!({})).unwrap()["cssContentSize"]
                        .clone();
                clip["scale"] = 1.into();
                capture(
                    p,
                    "out/worker-duration",
                    &format!("{theme}-{width}"),
                    json!({"format":"png","captureBeyondViewport":true,"clip":clip}),
                );
            }
        }
    });
}

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn fleet_snapshots_report_activity_capacity_and_sync_health() {
    dashboard(|p| {
        worker_membership_counts_only_live_capacity_and_excludes_retired_chiefs(p);
        assignment_lookup_never_opens_another_issue_or_owners_conversation(p);
        worker_holds_and_job_conversations_follow_the_correct_attempt_and_machine(p);
        worker_controls_distinguish_jobs_slots_and_pending_configuration(p);
        sync_attention_separates_machine_outages_from_saved_offline_edits(p);
    });
}

fn worker_membership_counts_only_live_capacity_and_excludes_retired_chiefs(p: &mut Page) {
    let run =
        json!({"id":"live-run","project_id":"atlas","project_name":"Atlas","started_at":1000});
    let done = json!({"id":"done","project_id":"atlas","project_name":"Atlas","finished_at":61000});
    let legacy =
        json!({"id":"chief:local:legacy","kind":"chief","project_id":"legacy","finished_at":null});
    let w = json!({"id":"live","pid":123,"config":{"projects":["atlas"],"concurrency":2},"runs":[run,done]});
    let machines = json!([
        {"host":"local","role":"supervisor","state":"connected","heartbeat":99,"workers":[w,{"id":"saved","config":{"projects":["atlas"],"concurrency":20}},{"id":"draining","pid":124,"config":{"projects":["beacon"],"enabled":false},"runs":[run]}]},
        {"host":"lost","state":"disconnected","heartbeat":98,"workers":[{"pid":2,"runs":[{"project_id":"beacon","project_name":"Beacon"}]}]},
        {"host":"late","state":"connected","heartbeat":80,"workers":[w]},
        {"host":"idle","state":"connected","heartbeat":99,"workers":[{"id":"idle","pid":1,"config":{"concurrency":2}}]}
    ]);
    let fleet = json!({"machines":machines});
    let view = call(p, "fleetView", json!([fleet, 100000]));
    assert_eq!(
        field(&view["live"], "/worker/id"),
        [json!("live"), json!("draining"), json!("idle")]
    );
    assert_eq!(field(&view["saved"], "/worker/id"), [json!("saved")]);
    assert_eq!(
        field(&view["offline"], "/host"),
        [json!("lost"), json!("late")]
    );
    assert_eq!(view["supervisor"]["host"], "local");
    assert_eq!(view["capacity"], 5);
    assert_eq!(view["active"], 2);
    assert_eq!(call(p, "fleetView", json!([{}, 100000]))["live"], json!([]));
    let projects = call(
        p,
        "projectView",
        json!([{"machines":[{"host":"local","state":"connected","heartbeat":99,"workers":[{"pid":1,"runs":[run,done,legacy],"chiefs":[legacy]}]},machines[1]]},100000]),
    );
    assert_eq!(field(&projects, "/name"), [json!("Atlas"), json!("Beacon")]);
    assert_eq!(projects[0]["active"].as_array().unwrap().len(), 1);
    assert_eq!(projects[0]["active"][0]["run"]["id"], "live-run");
    assert_eq!(projects[0]["history"].as_array().unwrap().len(), 1);
    assert_eq!(projects[1]["active"][0]["online"], false);
    assert_eq!(call(p, "projectView", json!([{"machines":[]} ])), json!([]));
    for path in ["/", "/project-settings.js", "/admin.js"] {
        let text = p
            .eval(&format!(
                "fetch({}).then(r=>{{if(!r.ok)throw Error(r.status);return r.text()}})",
                json!(path)
            ))
            .unwrap();
        assert!(
            !text.as_str().unwrap().to_lowercase().contains("chief"),
            "{path}"
        );
    }
    let mut data = fleet.clone();
    data["machines"][0]["workers"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"all","pid":3,"config":{"concurrency":3}}));
    let devices = call(p, "deviceView", json!([data, "atlas", 100000]));
    assert_eq!(
        field(&devices[0]["live"], "/id"),
        [json!("live"), json!("all")]
    );
    assert_eq!(field(&devices[0]["saved"], "/id"), [json!("saved")]);
    assert_eq!(devices[0]["active"], 1);
    assert_eq!(devices[0]["capacity"], 5);
    let late = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["machine"]["host"] == "late")
        .unwrap();
    assert_eq!(late["online"], false);
    assert_eq!(late["active"], 0);
    assert_eq!(late["capacity"], 0);
    for (data, project, n) in [
        (
            json!({"machines":[{"workers":[{"config":{"projects":["beacon"]}}]}]}),
            json!("atlas"),
            0,
        ),
        (json!({"machines":[{"workers":[]}]}), Value::Null, 1),
        (json!({}), Value::Null, 0),
    ] {
        assert_eq!(
            call(p, "deviceView", json!([data, project, 100000]))
                .as_array()
                .unwrap()
                .len(),
            n
        );
    }
}

fn assignment_lookup_never_opens_another_issue_or_owners_conversation(p: &mut Page) {
    let run = |id, project, n, actor, session, start| json!({"id":id,"project_id":project,"number":n,"actor_id":actor,"session_id":session,"started_at":start});
    let snapshot = json!({"machines":[{"host":"local","workers":[{"runs":[run("wrong-owner","named:QA",68,"other","other",100)]}]},
        {"host":"devbox","state":"disconnected","workers":[{"runs":[run("old","named:QA",68,"worker:old","assigned",1),run("latest","named:QA",68,"worker:old","assigned",3),run("wrong-issue","named:QA",69,"worker:old","assigned",4),run("wrong-project","other",68,"worker:old","assigned",5)]}]}]});
    let q = json!({"project":"named:QA","issue":"68","agent":"codex:assigned"});
    for (key, value, found) in [
        ("agent", "codex:assigned", true),
        ("agent", "worker:old", true),
        ("agent", "human:boss", false),
        ("agent", "codex:unknown", false),
        ("project", "named:Missing", false),
        ("issue", "0", false),
    ] {
        let mut q = q.clone();
        q[key] = value.into();
        let r = p
            .eval(&format!(
                "assignedAgentEntry({snapshot},new URLSearchParams({q}))"
            ))
            .unwrap();
        if found {
            assert_eq!(r["run"]["id"], "latest");
            assert_eq!(r["machine"]["host"], "devbox");
        } else {
            assert!(r.is_null(), "{q}");
        }
    }
    assert!(
        p.eval(&format!(
            "assignedAgentEntry({{}},new URLSearchParams({q}))"
        ))
        .unwrap()
        .is_null()
    );
    assert_eq!(p.eval(&format!("resolveAssignedAgent({snapshot},new URLSearchParams({q}),()=>{{throw Error('unexpected lookup')}})")).unwrap()["run"]["id"],"latest");
    let saved = json!({"machine":{"host":"local"},"run":{"id":"session:assigned","project_id":"named:QA","number":68,"standalone":true}});
    let r=p.eval(&format!("(async()=>{{let path;const entry=await resolveAssignedAgent({{}},new URLSearchParams({q}),v=>{{path=new URL(v,location.href);return {saved}}});return [entry,path.pathname,Object.fromEntries(path.searchParams)]}})()")).unwrap();
    assert_eq!(r, json!([saved, "/api/fleet/assignment", q]));
    assert!(p.eval(&format!("resolveAssignedAgent({{}},new URLSearchParams({q}),()=>{{throw Error('Assignment changed')}})")).unwrap_err().contains("Assignment changed"));
}

fn worker_holds_and_job_conversations_follow_the_correct_attempt_and_machine(p: &mut Page) {
    let future = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 60_000;
    let held = json!({"state":"infrastructure_blocked","started_at":1,"finished_at":2,"number":1});
    let mut retry = held.clone();
    retry["retry_at"] = future.into();
    for (run, online, label) in [
        (
            json!({"state":"attempt_held"}),
            true,
            "Task attempt protected",
        ),
        (
            json!({"state":"attempt_held","retry_at":future}),
            false,
            "Task attempt protected",
        ),
        (held.clone(), true, "Infrastructure unavailable"),
        (held.clone(), false, "Infrastructure unavailable"),
    ] {
        assert_eq!(
            call(p, "agentState", json!([{"run":run,"online":online}])),
            label
        );
    }
    assert!(
        call(p, "agentState", json!([{"run":retry,"online":true}]))
            .as_str()
            .unwrap()
            .starts_with("Retry in ")
    );
    for label in [
        "GitHub quota exhausted",
        "GitHub authentication failed",
        "GitHub permission denied",
        "GitHub request failed",
        "Worker environment check failed",
        "Database service unavailable",
        "Model proxy unavailable",
        "Approval service unavailable",
    ] {
        let mut run = held.clone();
        run["summary"] = format!("{label}. Automatic pickup is held.").into();
        assert_eq!(
            call(p, "agentState", json!([{"run":run,"online":true}])),
            label
        );
    }
    let later = json!({"run":{"number":1,"started_at":3,"state":"completed"}});
    for (active, history, count) in [
        (json!([]), json!([{"run":held}]), 0),
        (json!([]), json!([{"run":retry}]), 1),
        (json!([later]), json!([{"run":retry}]), 0),
        (json!([]), json!([{"run":retry},later]), 0),
        (
            json!([{"run":{"number":2,"started_at":3}}]),
            json!([{"run":retry}]),
            1,
        ),
    ] {
        assert_eq!(
            call(
                p,
                "scheduledRetries",
                json!([{"active":active,"history":history}])
            )
            .as_array()
            .unwrap()
            .len(),
            count
        );
    }
    let entry = json!({"machine":{"host":"local","state":"disconnected"},"run":{"id":"job:one","kind":"job","state":"running","machine":"node","standalone":true},"online":false});
    for (machines, online) in [
        (
            json!([{"host":"local","node":"node","state":"connected","heartbeat":99,"workers":[]}]),
            true,
        ),
        (
            json!([{"host":"local","node":"node","state":"connected","heartbeat":80}]),
            false,
        ),
        (json!([]), false),
    ] {
        assert_eq!(
            call(
                p,
                "jobConversation",
                json!([entry,{"machines":machines},100000])
            )["online"],
            online
        );
    }
    let mut ordinary = entry;
    ordinary["run"]["kind"] = "worker".into();
    assert_eq!(
        call(
            p,
            "jobConversation",
            json!([ordinary,{"machines":[]},100000])
        ),
        ordinary
    );
}

fn worker_controls_distinguish_jobs_slots_and_pending_configuration(p: &mut Page) {
    let machine = json!({"state":"connected","heartbeat":100});
    let offline = json!({"state":"disconnected"});
    let working = json!({"id":"a","intent":"running","pid":1,"active":1,"config":{"enabled":true,"concurrency":3}});
    let jobs = json!({"id":"a","intent":"running","pid":1,"active":0,"job_active":2,"config":{"enabled":true,"concurrency":1}});
    let mut paused = working.clone();
    paused["intent"] = "pause".into();
    let mut paused_jobs = jobs.clone();
    paused_jobs["intent"] = "pause".into();
    let mut problem = machine.clone();
    problem["configuration_error"] = "\"a\": Missing checkout; \"b\": Missing checkout".into();
    let mut unrelated = working.clone();
    unrelated["id"] = "c".into();
    let stopped = json!({"id":"a","intent":"stop","active":0,"pid":null});
    for (w, m, key, label) in [
        (&working, &machine, "group", "working"),
        (&jobs, &machine, "group", "working"),
        (&paused_jobs, &machine, "label", "Finishing work"),
        (&paused, &machine, "label", "Finishing work"),
        (&stopped, &machine, "group", "stopped"),
        (&working, &offline, "label", "Offline"),
        (&working, &problem, "group", "attention"),
        (&unrelated, &problem, "group", "working"),
    ] {
        assert_eq!(call(p, "workerPhase", json!([w, m, 100000]))[key], label);
    }
    let mut over = working.clone();
    over["active"] = 4.into();
    for (w, m, capacity, occupied, available, paused, live, running) in [
        (&working, &machine, 3, 1, 2, 0, true, true),
        (&jobs, &machine, 1, 0, 1, 0, true, true),
        (&paused, &machine, 3, 1, 0, 2, true, true),
        (&over, &machine, 3, 4, 0, 0, true, true),
        (&working, &offline, 0, 0, 0, 0, false, false),
        (&stopped, &machine, 0, 0, 0, 0, true, false),
    ] {
        assert_eq!(
            call(p, "slotUsage", json!([w, m, 100000])),
            json!({"capacity":capacity,"occupied":occupied,"available":available,"paused":paused,"online":live,"running":running})
        );
    }
    let workers = json!([{"id":"running","managed":true,"intent":"running","pid":12},{"id":"paused","managed":true,"intent":"pause"},{"id":"observed","pid":1,"active":1},{"id":"history","managed":false,"intent":"pause"},{"id":"stopped","managed":true,"intent":"stop"},{"id":"removed","managed":true,"intent":"drain","retiring":true,"pid":12},{"id":"finishing","managed":true,"intent":"drain","retiring":true,"pid":12,"active":1},{"id":"job","managed":true,"intent":"drain","retiring":true,"pid":12,"job_active":1}]);
    assert_eq!(
        field(
            &call(p, "projectWorkers", json!([{"workers":workers}])),
            "/id"
        ),
        [
            json!("running"),
            json!("paused"),
            json!("finishing"),
            json!("job")
        ]
    );
    let cfg = json!({"machines":{"local":{"workers":[{"id":"code","intent":"running","config":{"concurrency":5,"projects":["Code"],"provider":"claude"}},{"id":"other","intent":"pause","config":{"concurrency":37}},{"id":"retired","retiring":true},{"id":"small","config":{"concurrency":1}},{"id":"large","config":{"concurrency":1024}}]}}});
    p.eval(&format!("const capacityFixture={cfg};undefined"))
        .unwrap();
    for (id, delta, intent, desired) in [("code", 1, "running", 6), ("other", -1, "pause", 36)] {
        assert_eq!(
            p.eval(&format!(
                "workerCapacityUpdate(capacityFixture,'local',{},{delta})",
                json!(id)
            ))
            .unwrap(),
            json!({"host":"local","id":id,"intent":intent,"config":{"concurrency":desired}})
        );
    }
    assert_eq!(p.eval("capacityFixture").unwrap(), cfg);
    for (id, delta, why) in [
        ("small", -1, "between 1 and 1024"),
        ("large", 1, "between 1 and 1024"),
        ("retired", 1, "no longer"),
        ("missing", 1, "no longer"),
    ] {
        assert!(
            p.eval(&format!(
                "workerCapacityUpdate(capacityFixture,'local',{},{delta})",
                json!(id)
            ))
            .unwrap_err()
            .contains(why)
        );
    }
    let worker =
        json!({"id":"limit","active":0,"config":{"concurrency":3},"desired_concurrency":2});
    assert_eq!(
        call(p, "workerLimitState", json!([worker, machine, 100000])),
        json!({"limit":2,"applied":3,"status":"Pending","note":"Waiting for this worker to apply the new limit."})
    );
    assert_eq!(
        call(p, "workerLimitState", json!([worker, offline, 100000]))["status"],
        "Queued"
    );
    assert_eq!(
        call(p, "workerLimitState", json!([worker, problem, 100000]))["status"],
        "Blocked"
    );
    for (active, status) in [(0, ""), (3, "Finishing")] {
        let mut w = worker.clone();
        w["config"]["concurrency"] = 2.into();
        w["active"] = active.into();
        assert_eq!(
            call(p, "workerLimitState", json!([w, machine, 100000]))["status"],
            status
        );
    }
    for (detail, project, summary) in [
        (
            "github.com/acme/atlas: Git clone failed. Check repository access",
            Some("github.com/acme/atlas"),
            "Couldn’t clone repository",
        ),
        (
            "Database unavailable",
            None,
            "Worker settings need attention",
        ),
        (
            "github.com/acme/atlas: Git clone timed out; check connection",
            Some("github.com/acme/atlas"),
            "Repository connection timed out",
        ),
    ] {
        let v = call(
            p,
            "configurationProblems",
            json!([{"configuration_error":detail,"projects":{"github.com/acme/atlas":{"path":"~/Workspace/atlas"}}}]),
        );
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["project"].as_str(), project);
        assert_eq!(v[0]["summary"], summary);
    }
}

fn sync_attention_separates_machine_outages_from_saved_offline_edits(p: &mut Page) {
    let healthy = json!({"state":"connected","heartbeat":100,"sync_observed_at":100,"sync":{"state":"healthy","message":"Applied"}});
    let syncing = json!({"state":"catching_up","message":"Changes are waiting to be applied or sent","age_ms":500,"exchange_age_ms":1000,"pending_local":1,"pending_projection":4,"outgoing_pending":2});
    let problem = |p: &mut Page, patch: Value, now: i64| {
        let mut m = healthy.clone();
        for (key, v) in patch.as_object().unwrap() {
            m[key] = v.clone();
        }
        call(p, "syncProblem", json!([m, now]))
    };
    for patch in [
        json!({}),
        json!({"database_errors":{}}),
        json!({"sync":syncing}),
        json!({"conflicts":7}),
    ] {
        assert_eq!(problem(p, patch, 100000), Value::Null);
    }
    for (patch, text) in [
        (
            json!({"sync":{"state":"error","outgoing_pending":9000,"message":"old-mac: Incompatible sync protocol"}}),
            "old-mac: Incompatible sync protocol",
        ),
        (
            json!({"sync":{"state":"shadow","message":"Migration inactive"}}),
            "Migration inactive",
        ),
        (json!({"sync":null}), "not verified"),
        (json!({"sync_observed_at":69}), "not verified"),
        (json!({"peer_sync_error":"No SSH route"}), "No SSH route"),
        (
            json!({"database_errors":{"pull":"Database writer unavailable"}}),
            "Database busy; data sync will retry",
        ),
        (
            json!({"database_errors":{"configure":"Database writer unavailable","heartbeat":"locked"}}),
            "Database busy; settings and activity will retry",
        ),
    ] {
        assert!(
            problem(p, patch, 100000).as_str().unwrap().contains(text),
            "{text}"
        );
    }
    for (key, value) in [
        ("pending_reservations", json!(1)),
        ("conflicting_attempts", json!(1)),
        ("exchange_age_ms", Value::Null),
        ("exchange_age_ms", json!(30001)),
    ] {
        let mut s = syncing.clone();
        s[key] = value;
        assert!(
            problem(p, json!({"sync":s}), 100000)
                .as_str()
                .unwrap()
                .contains("waiting")
        );
    }
    let mut missing = syncing.clone();
    missing.as_object_mut().unwrap().remove("exchange_age_ms");
    assert!(
        problem(p, json!({"sync":missing}), 100000)
            .as_str()
            .unwrap()
            .contains("waiting")
    );
    assert!(
        problem(p, json!({"sync_observed_at":71,"sync":syncing}), 101000)
            .as_str()
            .unwrap()
            .contains("waiting")
    );
    let mut machines: Vec<_> = [
        ("local", "Main", 8),
        ("box", "Devbox", 7),
        ("mac", "Mac.lan", 1),
    ]
    .into_iter()
    .map(|(host, name, n)| {
        let mut m = healthy.clone();
        m["host"] = host.into();
        m["hostname"] = name.into();
        m["conflicts"] = n.into();
        m
    })
    .collect();
    machines[0]["role"] = "supervisor".into();
    let notices = |p: &mut Page, m: &[Value], n: usize| {
        call(
            p,
            "fleetAttention",
            json!([{"machines":m,"conflicts":vec![json!({});n]},100000]),
        )
    };
    let saved = notices(p, &machines, 8);
    assert_eq!(saved.as_array().unwrap().len(), 1);
    assert_eq!(saved[0]["key"], "saved-edits");
    assert_eq!(saved[0]["summary"], "8 saved offline edits need review");
    let note = saved[0]["note"].as_str().unwrap();
    assert!(note.contains("same edits") && !note.contains("Main: 8"));
    assert_eq!(
        notices(p, &machines, 0)[0]["summary"],
        "Saved offline edits reported"
    );
    assert_eq!(
        notices(p, &machines[..1], 1)[0]["summary"],
        "1 saved offline edit needs review"
    );
    assert!(
        notices(p, &machines, 100)[0]["summary"]
            .as_str()
            .unwrap()
            .contains("100+")
    );
    for m in &mut machines {
        m["sync"] = syncing.clone();
    }
    assert_eq!(notices(p, &machines, 8), saved);
    machines.reverse();
    assert_eq!(notices(p, &machines, 8), saved);
    machines[0]["state"] = "disconnected".into();
    let outage = notices(p, &machines, 8);
    assert_eq!(outage.as_array().unwrap().len(), 2);
    assert_eq!(outage[0]["summary"], "1 machine needs attention");
    assert!(
        outage[0]["note"]
            .as_str()
            .unwrap()
            .contains("Mac.lan: offline")
    );
    assert_eq!(outage[1], saved[0]);
}

#[path = "assigned_trace.rs"]
mod assigned_trace;
