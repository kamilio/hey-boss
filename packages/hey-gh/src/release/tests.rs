use super::*;
use serde_json::json;

fn gate() -> Gate {
    Gate {
        name: "main".into(),
        workflow: "release.yml".into(),
        purpose: Purpose::Validation,
        jobs: vec![Job {
            name: "validate / unit".into(),
            count: 1,
            steps: vec!["Run tests".into()],
            step_counts: Default::default(),
            unchanged_steps: Default::default(),
        }],
    }
}
fn run(status: &str, conclusion: Option<&str>) -> Value {
    json!({"id":10,"run_attempt":1,"head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","status":status,"conclusion":conclusion})
}
fn job(conclusion: &str) -> Value {
    json!({"id":11,"run_id":10,"run_attempt":1,"head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "name":"validate / unit","status":"completed","conclusion":conclusion,"completed_at":"2026-10-04T10:00:00Z",
        "steps":[{"name":"Run tests","status":"completed","conclusion":conclusion,"completed_at":"2026-10-04T09:59:00Z"}]})
}
#[test]
fn green_workflow_with_skipped_tests_does_not_validate_main() {
    assert_eq!(
        assess(
            &gate(),
            &run("completed", Some("success")),
            &[job("skipped")]
        )
        .state,
        RunState::Skipped
    );
}
#[test]
fn cancelled_without_jobs_is_not_a_missing_or_successful_build() {
    assert_eq!(
        assess(&gate(), &run("completed", Some("cancelled")), &[]).state,
        RunState::Cancelled
    );
}
#[test]
fn cancelled_run_retains_failed_jobs_without_claiming_main_broke() {
    let v = assess(
        &gate(),
        &run("completed", Some("cancelled")),
        &[job("failure")],
    );
    assert_eq!(v.state, RunState::Cancelled);
    assert_eq!(v.failed_jobs, vec!["validate / unit"]);
}
#[test]
fn publication_failure_does_not_erase_successful_validation() {
    assert_eq!(
        assess(
            &gate(),
            &run("completed", Some("failure")),
            &[job("success")]
        )
        .state,
        RunState::Passed
    );
}
#[test]
fn successful_step_in_skipped_job_does_not_count() {
    let mut j = job("success");
    j["conclusion"] = json!("skipped");
    assert_eq!(
        assess(&gate(), &run("completed", Some("success")), &[j]).state,
        RunState::Skipped
    );
}
#[test]
fn successful_job_with_skipped_validation_step_does_not_count() {
    let mut j = job("success");
    j["steps"][0]["conclusion"] = json!("skipped");
    assert_eq!(
        assess(&gate(), &run("completed", Some("success")), &[j]).state,
        RunState::Skipped
    );
}
#[test]
fn wrong_run_attempt_or_head_cannot_supply_success() {
    for (key, value) in [
        ("run_id", json!(9)),
        ("run_attempt", json!(2)),
        (
            "head_sha",
            json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        ),
    ] {
        let mut j = job("success");
        j[key] = value;
        assert_eq!(
            assess(&gate(), &run("completed", Some("success")), &[j]).state,
            RunState::Unknown
        );
    }
}
#[test]
fn all_matrix_shards_must_be_present_and_successful() {
    let mut g = gate();
    g.jobs[0].name = "validate / bash (".to_owned() + "*";
    g.jobs[0].count = 4;
    let mut jobs: Vec<_> = (1..=4)
        .map(|i| {
            let mut j = job("success");
            j["id"] = json!(20 + i);
            j["name"] = json!(format!("validate / bash ({i})"));
            j
        })
        .collect();
    assert_eq!(
        assess(&g, &run("completed", Some("success")), &jobs).state,
        RunState::Passed
    );
    jobs.pop();
    assert_eq!(
        assess(&g, &run("completed", Some("success")), &jobs).state,
        RunState::Unknown
    );
}
#[test]
fn completion_time_is_actual_selected_job_completion_not_run_updated_time() {
    let mut r = run("completed", Some("success"));
    r["updated_at"] = json!("2026-10-04T15:00:00Z");
    assert_eq!(
        assess(&gate(), &r, &[job("success")])
            .completed_at
            .as_deref(),
        Some("2026-10-04T10:00:00Z")
    );
}
#[test]
fn pending_build_and_unknown_conclusions_stay_unsatisfied() {
    assert_eq!(
        assess(&gate(), &run("pending", None), &[]).state,
        RunState::Pending
    );
    let mut j = job("new_github_conclusion");
    j["steps"][0]["conclusion"] = json!("success");
    assert_eq!(
        assess(&gate(), &run("completed", Some("success")), &[j]).state,
        RunState::Unknown
    );
}

#[test]
fn unknown_workflow_conclusion_and_duplicate_jobs_cannot_certify() {
    assert_eq!(
        assess(
            &gate(),
            &run("completed", Some("new_conclusion")),
            &[job("success")]
        )
        .state,
        RunState::Unknown
    );
    let mut g = gate();
    g.jobs[0].name = "validate / *".into();
    g.jobs[0].count = 2;
    assert_eq!(
        assess(
            &g,
            &run("completed", Some("success")),
            &[job("success"), job("success")]
        )
        .state,
        RunState::Unknown
    );
}

#[test]
fn comparison_requires_the_target_as_merge_base() {
    let a = "a".repeat(40);
    let b = "b".repeat(40);
    let c = json!({"status":"ahead","base_commit":{"sha":a},"merge_base_commit":{"sha":b}});
    assert!(!contains(&a, &b, &c));
    let c = json!({"status":"ahead","base_commit":{"sha":a},"merge_base_commit":{"sha":a}});
    assert!(contains(&a, &b, &c));
}

#[test]
fn queue_survives_restart_removal_and_out_of_order_results() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queue.sqlite");
    let p: Project = serde_json::from_str(include_str!("profiles/poe-code.json")).unwrap();
    let target = "a".repeat(40);
    let mut q = queue::Queue::open(&path).unwrap();
    q.add(&p, std::slice::from_ref(&target)).unwrap();
    let mut report = Report::new(&target);
    report.state = "failed".into();
    report.gates.push(GateReport {
        name: "main".into(),
        purpose: Purpose::Validation,
        satisfied: false,
        history_complete: true,
        confirmation: None,
        runs: vec![RunRecord {
            id: 10,
            attempt: 1,
            sha: target.clone(),
            url: "https://github.com/o/r/actions/runs/10".into(),
            coverage: "exact".into(),
            verdict: assess(
                &gate(),
                &run("completed", Some("failure")),
                &[job("failure")],
            ),
        }],
    });
    q.record(&Batch {
        validations: vec![],
        observed_at_ms: 10,
        reports: vec![report.clone()],
    })
    .unwrap();
    drop(q);
    let mut q = queue::Queue::open(&path).unwrap();
    report.state = "verified".into();
    report.gates.clear();
    q.record(&Batch {
        validations: vec![],
        observed_at_ms: 20,
        reports: vec![report.clone()],
    })
    .unwrap();
    q.record(&Batch {
        validations: vec![],
        observed_at_ms: 5,
        reports: vec![Report::new(&target)],
    })
    .unwrap();
    let rows = q.entries().unwrap();
    assert_eq!(rows[0].report.as_ref().unwrap().state, "recovered");
    assert_eq!(rows[0].failures.len(), 1);
    assert_eq!(rows[0].checked_at_ms, Some(20));
    q.remove(&target).unwrap();
    q.record(&Batch {
        validations: vec![],
        observed_at_ms: 30,
        reports: vec![report],
    })
    .unwrap();
    assert!(q.entries().unwrap().is_empty());
}

#[test]
fn queue_rejects_mode_and_policy_changes_without_erasing_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let mut q = queue::Queue::open(&dir.path().join("queue.sqlite")).unwrap();
    let mut p: Project = serde_json::from_str(include_str!("profiles/poe2.json")).unwrap();
    q.add(&p, &["123".into()]).unwrap();
    assert!(q.add(&p, &["abc1234".into()]).is_err());
    p.target = TargetKind::Commit;
    assert!(q.add(&p, &["abc1234".into()]).is_err());
    assert_eq!(q.entries().unwrap().len(), 1);
}

#[test]
fn a_configured_unchanged_deployment_is_explicit_and_never_called_deployed() {
    let mut g = gate();
    g.purpose = Purpose::Deployment;
    g.jobs[0]
        .unchanged_steps
        .insert("Run tests".into(), "Verified unchanged artifact".into());
    let mut j = job("success");
    j["steps"][0]["conclusion"] = json!("skipped");
    j["steps"].as_array_mut().unwrap().push(
        json!({"name":"Verified unchanged artifact","status":"completed","conclusion":"success"}),
    );
    assert_eq!(
        assess(&g, &run("completed", Some("success")), &[j.clone()]).state,
        RunState::Unchanged
    );
    j["steps"][1]["conclusion"] = json!("skipped");
    assert_eq!(
        assess(&g, &run("completed", Some("success")), &[j]).state,
        RunState::Skipped
    );
}

#[test]
fn queue_transport_error_withdraws_success_and_preserves_source_age() {
    let dir = tempfile::tempdir().unwrap();
    let mut q = queue::Queue::open(&dir.path().join("queue.sqlite")).unwrap();
    let p: Project = serde_json::from_str(include_str!("profiles/poe-code.json")).unwrap();
    let target = "a".repeat(40);
    q.add(&p, std::slice::from_ref(&target)).unwrap();
    let mut report = Report::new(&target);
    report.state = "verified".into();
    q.record(&Batch {
        observed_at_ms: 10,
        reports: vec![report],
        validations: vec![crate::ResourceValidation {
            resource: "synthetic".into(),
            validated_at_ms: 5,
            source: crate::Source::Cache,
        }],
    })
    .unwrap();
    q.record_error(&[target], "daemon unavailable").unwrap();
    let rows = q.entries().unwrap();
    assert_eq!(rows[0].oldest_source_validation_ms, Some(5));
    let report = rows[0].report.as_ref().unwrap();
    assert_eq!(report.state, "unknown");
    assert_eq!(report.errors, vec!["daemon unavailable"]);
}

#[test]
fn queue_keeps_historical_deployment_confirmation_when_a_later_poll_fails() {
    let dir = tempfile::tempdir().unwrap();
    let mut q = queue::Queue::open(&dir.path().join("queue.sqlite")).unwrap();
    let p: Project = serde_json::from_str(include_str!("profiles/poe-code.json")).unwrap();
    let target = "a".repeat(40);
    q.add(&p, std::slice::from_ref(&target)).unwrap();
    let record = RunRecord {
        id: 10,
        attempt: 1,
        sha: target.clone(),
        url: "https://github.com/o/r/actions/runs/10".into(),
        coverage: "exact".into(),
        verdict: assess(
            &gate(),
            &run("completed", Some("success")),
            &[job("success")],
        ),
    };
    let mut report = Report::new(&target);
    report.state = "verified".into();
    report.gates.push(GateReport {
        name: "deployed worker".into(),
        purpose: Purpose::Deployment,
        satisfied: true,
        history_complete: true,
        confirmation: Some(record.clone()),
        runs: vec![record],
    });
    q.record(&Batch {
        observed_at_ms: 10,
        reports: vec![report],
        validations: vec![],
    })
    .unwrap();
    q.record_error(&[target], "GitHub unavailable").unwrap();
    let rows = serde_json::to_value(q.entries().unwrap()).unwrap();
    assert_eq!(rows[0]["confirmations"][0]["gate"], "deployed worker");
    assert_eq!(
        rows[0]["confirmations"][0]["evidence"]["completed_at"],
        "2026-10-04T10:00:00Z"
    );
    assert_eq!(rows[0]["report"]["state"], "unknown");
}

#[test]
fn repeated_composite_steps_must_match_the_configured_count() {
    let mut g = gate();
    g.jobs[0].step_counts.insert("Run tests".into(), 2);
    assert_eq!(
        assess(&g, &run("completed", Some("success")), &[job("success")]).state,
        RunState::Unknown
    );
    let mut j = job("success");
    let step = j["steps"][0].clone();
    j["steps"].as_array_mut().unwrap().push(step);
    assert_eq!(
        assess(&g, &run("completed", Some("success")), &[j]).state,
        RunState::Passed
    );
}

#[test]
fn cancellation_after_all_configured_jobs_passed_does_not_erase_their_proof() {
    assert_eq!(
        assess(
            &gate(),
            &run("completed", Some("cancelled")),
            &[job("success")]
        )
        .state,
        RunState::Passed
    );
}

#[test]
fn repeated_step_counts_must_reference_required_steps_with_positive_bounds() {
    for (name, count) in [("missing", 2), ("Run tests", 0), ("Run tests", 101)] {
        let mut g = gate();
        g.jobs[0].step_counts.insert(name.into(), count);
        let project = Project {
            repository: "o/r".into(),
            branch: "main".into(),
            target: TargetKind::Commit,
            gates: vec![g],
        };
        assert!(project.validate().is_err());
    }
}
