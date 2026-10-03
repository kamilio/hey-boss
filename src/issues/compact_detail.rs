//! Full requirements and current guards without expanding historical evidence.
use super::*;

const COMMENT_BYTES: usize = 64 * 1024;

pub(super) fn read(
    db: &Connection,
    project: &Project,
    number: i64,
    limit: u32,
    offset: u32,
    actor: Option<&Actor>,
) -> Result<Value> {
    let issue = get_issue(db, &project.id, number, true)?;
    let mut page = comment_page_budget(
        db,
        project,
        number,
        limit,
        offset,
        super::super::CommentSort::Newest,
        (0, COMMENT_BYTES),
    )?;
    page["comments"].as_array_mut().unwrap().reverse();
    let returned = page["comments"].as_array().unwrap().len();
    let comment_bytes = serde_json::to_vec(&page["comments"])?.len();
    let prs = db.query_collect::<_, _, rusqlite::Error>(
        "SELECT url,added_by,created_at,purpose,status,checked_at,error FROM issue_pull_requests WHERE project_id=?1 AND issue_number=?2 ORDER BY created_at,url",
        params![project.id, number],
        |r| Ok(json!({"url":r.get::<_,String>(0)?,"added_by":r.get::<_,String>(1)?,"created_at":r.get::<_,i64>(2)?,"purpose":r.get::<_,String>(3)?,"status":r.get::<_,String>(4)?,"checked_at":r.get::<_,Option<i64>>(5)?,"error":r.get::<_,Option<String>>(6)?})),
    )?;
    let assignee: Option<Actor> = issue
        .assignee
        .as_ref()
        .map(|id| -> Result<Actor> {
            let raw: String =
                db.query_row("SELECT metadata FROM agents WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
            Ok(serde_json::from_str(&raw)?)
        })
        .transpose()?;
    let mut result = json!({
        "ok":true,"project":project,"issue":issue,
        "allocation":super::super::fleet::allocation(db,&project.id,number,actor.map(|a|a.machine.as_str()))?,
        "ready_guard":ready::snapshot(db,&project.id,&issue)?,
        "assignee_agent":assignee,
        "comments":page["comments"],"comment_count":page["comment_count"],
        "more_comments":!page["next_offset"].is_null(),"next_comment_offset":page["next_offset"],
        "projection":"compact_detail","projection_version":1,
        "completeness":{
            "body":true,"pull_requests":true,
            "comments":{"total":page["comment_count"],"returned":returned,"limit":limit,"offset":offset,
                "next_offset":page["next_offset"],"complete":offset==0 && page["next_offset"].is_null(),
                "selection_order":"newest_first","display_order":"oldest_first",
                "byte_target":COMMENT_BYTES,"byte_target_exceeded":comment_bytes>COMMENT_BYTES,
                "bodies_complete":true,"pagination":"offset; concurrent comments may shift pages"},
            "omitted":["issue.commits","issue.pull_requests[].origin","artifacts","attachment_history","history"],
            "omitted_evidence":"not_loaded; not evidence of absence"
        }
    });
    result["issue"]["pull_requests"] = json!(prs);
    if issue.deleted_at.is_some()
        && let Some(destination) = transfer::destination(db, &project.id, number)?
    {
        result["moved_to"] = destination;
    }
    let (role, machine): (String, String) =
        db.query_row("SELECT role,node FROM fleet_meta WHERE id=1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    result["health"] = json!({"authoritative":role!="agent","store_role":role,"store_machine":machine,"freshness":"stored_snapshot"});
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_detail_query_work_does_not_expand_with_commit_and_attachment_history() {
        let root = std::env::temp_dir().join(format!(
            "hb-detail-work-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        let path = root.join("issues.db");
        let store = Store::open(&path).unwrap();
        store.db.execute_batch("INSERT INTO projects(id,name,next_number) VALUES('named:Detail','Detail',2);
            INSERT INTO agents VALUES('creator','{}',0);
            INSERT INTO issues(project_id,number,title,body,state,created_by,created_at,updated_at,version,labels)
            VALUES('named:Detail',1,'Task','Requirements','open','creator',0,0,1,'[]');
            INSERT INTO issue_pull_requests(project_id,issue_number,url,added_by,created_at)
            VALUES('named:Detail',1,'https://github.com/o/r/pull/1','creator',0);").unwrap();
        let mut owner = crate::database::Owner::start(&path).unwrap().unwrap();
        let request: Request = serde_json::from_value(json!({"version":1,"project":{"id":"named:Detail","name":"Detail"},"operation":{"action":"view_compact","number":1,"limit":20,"offset":0}})).unwrap();
        let mut measurements = Vec::new();
        let mut first = None;
        for history in [false, true] {
            if history {
                store.db.execute_batch("WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<8192)
                    INSERT INTO issue_commits SELECT 'named:Detail',1,printf('%040x',id),'o/r','Old commit','creator',id,json_object('detail',printf('%4096s','provenance')) FROM n;
                    WITH RECURSIVE n(id) AS (VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<8192)
                    INSERT INTO events(project_id,issue_number,actor,action,created_at,data)
                    SELECT 'named:Detail',1,'creator','pr_attached',id,json_object('url','https://github.com/o/r/pull/1','origin',json_object('detail',printf('%4096s','provenance'))) FROM n;").unwrap();
            }
            let (db, transport) = crate::database::tests::measured_connection(&path);
            let mut reader = Store::open(&path).unwrap();
            reader.replace_connection_for_test(db);
            let result = reader.execute(&request).unwrap();
            if let Some(first) = &first {
                assert_eq!(&result, first);
            } else {
                first = Some(result);
            }
            drop(reader);
            measurements.push(transport.join().unwrap());
        }
        owner.stop();
        drop(store);
        fs::remove_dir_all(root).unwrap();
        eprintln!("Compact detail owner RPCs / SQL steps: {measurements:?}");
        assert_eq!(measurements[0].0, measurements[1].0);
        assert!(
            measurements[1].1 <= measurements[0].1 + 100,
            "History increased query work: {measurements:?}"
        );
    }
}
