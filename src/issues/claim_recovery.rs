//! Release abandoned local claims without treating idle or remote agents as dead.
use super::*;

const DEAD_SESSION_GRACE_MS: i64 = 60_000;

impl Store {
    pub(crate) fn release_stale_claims(&mut self, machine: &str, now: i64) -> Result<usize> {
        let mut stmt = self.db.prepare(
            "SELECT p.id,p.name,i.number,a.metadata,a.last_seen
             FROM issues i JOIN projects p ON p.id=i.project_id JOIN agents a ON a.id=i.assignee
             WHERE i.state='open' AND i.deleted_at IS NULL AND a.last_seen<=?1
             AND json_extract(a.metadata,'$.machine')=?2
             AND (i.assignee LIKE 'codex:%' OR i.assignee LIKE 'claude:%')
             AND NOT EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=i.project_id AND r.issue_number=i.number AND r.finished_at IS NULL)",
        )?;
        let claims = stmt
            .query_map(
                rusqlite::params![now - DEAD_SESSION_GRACE_MS, machine],
                |r| {
                    Ok((
                        Project {
                            id: r.get(0)?,
                            name: r.get(1)?,
                        },
                        r.get::<_, i64>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let mut released = 0;
        for (project, number, metadata, last_seen) in claims {
            let actor: Actor = serde_json::from_str(&metadata)?;
            if super::super::identity::presence(&actor, machine) != "stale" {
                continue;
            }
            let tx = self
                .db
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Recheck ownership and activity after obtaining the writer lock.
            let unchanged: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM issues i JOIN agents a ON a.id=i.assignee
                 WHERE i.project_id=?1 AND i.number=?2 AND i.state='open' AND i.deleted_at IS NULL
                 AND i.assignee=?3 AND a.metadata=?4 AND a.last_seen=?5
                 AND NOT EXISTS(SELECT 1 FROM worker_runs r WHERE r.project_id=i.project_id AND r.issue_number=i.number AND r.finished_at IS NULL))",
                rusqlite::params![project.id, number, actor.id, metadata, last_seen], |r| r.get(0),
            )?;
            if unchanged && super::super::identity::presence(&actor, machine) == "stale" {
                mutate(
                    &tx,
                    &project,
                    &actor,
                    &Operation::Unassign {
                        number,
                        force: false,
                    },
                    now,
                )?;
                event(
                    &tx,
                    &project.id,
                    number,
                    &actor.id,
                    "claim_expired",
                    now,
                    &json!({"reason":"agent session is no longer alive"}),
                )?;
                tx.execute(
                    "UPDATE projects SET activity_at=max(activity_at,?2) WHERE id=?1",
                    rusqlite::params![project.id, now],
                )?;
                released += 1;
            }
            tx.commit()?;
        }
        Ok(released)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_claims_expire_after_grace_but_live_unknown_remote_and_human_claims_remain() {
        let root = std::env::temp_dir().join(format!(
            "hb-claim-recovery-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut store = Store::open(&root.join("issues.db")).unwrap();
        let project = Project {
            id: "named:Claim recovery".into(),
            name: "Claim recovery".into(),
        };
        let clock = 200_000;
        for (index, (id, machine, pid, start, seen)) in [
            ("codex:dead", "local", Some(999_999), Some("dead".into()), 0),
            (
                "codex:recent",
                "local",
                Some(999_999),
                Some("dead".into()),
                clock - 59_999,
            ),
            (
                "codex:live",
                "local",
                Some(std::process::id()),
                crate::agents::process_identity(std::process::id()),
                0,
            ),
            ("codex:unknown", "local", None, None, 0),
            (
                "codex:remote",
                "remote",
                Some(999_999),
                Some("dead".into()),
                0,
            ),
            ("human:boss", "local", Some(999_999), Some("dead".into()), 0),
            (
                "codex:supervised",
                "local",
                Some(999_999),
                Some("dead".into()),
                0,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let actor = Actor {
                id: id.into(),
                kind: "codex".into(),
                session_id: Some(id.into()),
                machine: machine.into(),
                host: machine.into(),
                pid,
                process_start: start,
                cwd: root.clone(),
                source: "synthetic recovery test".into(),
            };
            let request = |operation| Request {
                version: 1,
                project: project.clone(),
                project_override: None,
                actor: Some(actor.clone()),
                operation,
                request_id: None,
            };
            let number = index as i64 + 1;
            store
                .execute(&request(Operation::Create {
                    title: id.into(),
                    body: String::new(),
                    labels: vec![],
                    at_top: false,
                }))
                .unwrap();
            store
                .execute(&request(Operation::Claim {
                    number,
                    force: false,
                }))
                .unwrap();
            store
                .db
                .execute(
                    "UPDATE agents SET last_seen=?2 WHERE id=?1",
                    rusqlite::params![id, seen],
                )
                .unwrap();
        }
        store.db.execute("INSERT INTO worker_runs(id,project_id,issue_number,job,actor_id,state,owner_pid,owner_start,machine,started_at,updated_at) VALUES('active',?1,7,'{}','codex:supervised','running',1,'start','local',0,0)", [&project.id]).unwrap();
        assert_eq!(store.release_stale_claims("local", clock).unwrap(), 1);
        assert_eq!(store.release_stale_claims("local", clock).unwrap(), 0);
        assert!(
            get_issue(&store.db, &project.id, 1, true)
                .unwrap()
                .assignee
                .is_none()
        );
        for number in 2..=7 {
            assert!(
                get_issue(&store.db, &project.id, number, true)
                    .unwrap()
                    .assignee
                    .is_some(),
                "claim {number} should remain"
            );
        }
        assert_eq!(store.release_stale_claims("local", clock + 1).unwrap(), 1);
        let expired: i64 = store
            .db
            .query_row(
                "SELECT count(*) FROM events WHERE action='claim_expired'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(expired, 2);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
