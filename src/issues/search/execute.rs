use super::*;
use crate::issues::search::Options;
use std::collections::{HashMap, HashSet};

impl Store {
    pub(super) fn execute_search(&mut self, request: &Request, options: &Options) -> Result<Value> {
        let project = request_project(&self.db, request)?;
        let owner = if options.mine {
            request.actor.as_ref().map(|a| a.id.as_str())
        } else {
            options.assignee.as_deref()
        };
        let labels = serde_json::to_string(&options.labels)?;
        let eligible_sql = format!(
            "SELECT number,title,coalesce((SELECT generation FROM issue_search_dirty d WHERE d.project=issues.project_id AND d.number=issues.number),0),coalesce((SELECT dirty FROM issue_search_dirty d WHERE d.project=issues.project_id AND d.number=issues.number),0) FROM issues WHERE {}",
            list_predicate(false, owner, options.unassigned)
        );
        let args = params![
            project.id,
            options.state,
            owner,
            options.unassigned,
            Option::<String>::None,
            labels,
            0,
            0,
            "[]"
        ];
        let read = |r: &crate::database::Row| {
            Ok::<_, rusqlite::Error>((
                r.get::<_, i64>(0)?,
                (
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, bool>(3)?,
                ),
            ))
        };
        let before: HashMap<_, _> = self
            .db
            .query_collect(&eligible_sql, args, read)?
            .into_iter()
            .collect();
        let allowed = before
            .iter()
            .map(|(n, (title, _, _))| (*n, title.clone()))
            .collect();
        let dirty: HashSet<_> = before
            .iter()
            .filter(|(_, v)| v.2)
            .map(|(n, _)| *n)
            .collect();
        let path =
            self.db.path().map(Path::new).ok_or_else(|| {
                Error::invalid("Ranked search requires a persistent issue database")
            })?;
        let (mut matches, status) = super::super::search::retrieve(
            path,
            &project.id,
            &options.query,
            options.semantic,
            &allowed,
            &dirty,
        )?;
        // The same final snapshot controls revision checks, filters and hydration.
        // Generations persist after indexing, so an edit cannot hide in a cleared queue.
        let tx = self.db.read_transaction()?;
        let current: HashMap<_, _> = tx
            .query_collect(&eligible_sql, args, read)?
            .into_iter()
            .collect();
        matches.retain(|m| {
            current
                .get(&m.number)
                .zip(before.get(&m.number))
                .is_some_and(|(a, b)| a.0 == b.0 && a.1 == b.1)
        });
        let total = matches.len();
        let offset = if options.all {
            0
        } else {
            options.offset as usize
        };
        let limit = if options.all {
            total
        } else {
            options.limit as usize
        };
        let matches: Vec<_> = matches.into_iter().skip(offset).take(limit).collect();
        let numbers = serde_json::to_string(&matches.iter().map(|m| m.number).collect::<Vec<_>>())?;
        let columns = if options.compact {
            compact::COLUMNS.to_owned()
        } else {
            COLUMNS.replacen("body,", "'' AS body,", 1)
        };
        let sql = format!(
            "SELECT {columns},CASE WHEN archive_key IS NOT NULL THEN archived_comments ELSE (SELECT count(*) FROM comments c WHERE c.project_id=issues.project_id AND c.issue_number=issues.number) END AS comment_count FROM json_each(?2) picked CROSS JOIN issues WHERE project_id=?1 AND number=picked.value"
        );
        let rows = tx.query_collect(&sql, params![project.id, numbers], |r| {
            let mut v = if options.compact {
                compact::row(r)?
            } else {
                serde_json::to_value(row_issue(r)?)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
            };
            v.as_object_mut().unwrap().remove("body");
            v["comment_count"] = json!(r.get::<_, i64>("comment_count")?);
            Ok::<_, rusqlite::Error>(v)
        })?;
        let mut rows: HashMap<_, _> = rows
            .into_iter()
            .map(|v| (v["number"].as_i64().unwrap(), v))
            .collect();
        let mut items = Vec::new();
        for m in matches {
            if let Some(mut v) = rows.remove(&m.number) {
                v["match"] = json!({"field":m.field,"excerpt":m.text,"comment_id":if m.source>0 {Some(m.source)}else{None},"kind":if m.semantic {"semantic"}else{"keyword"}});
                items.push(v);
            }
        }
        let order_version: i64 = tx.query_row(
            "SELECT issue_order_version FROM projects WHERE id=?1",
            [&project.id],
            |r| r.get(0),
        )?;
        let mut result = json!({"ok":true,"project":project,"issues":items,"order_version":order_version,"search":status,"total":total,"next_offset":if !options.all && offset+limit<total {Some(offset+limit)}else{None}});
        if options.compact {
            compact::enrich(&tx, &project.id, result["issues"].as_array_mut().unwrap())?;
            compact::metadata(&tx, &mut result)?;
        } else {
            let numbers: Vec<_> = result["issues"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["number"].as_i64().unwrap())
                .collect();
            let mut prs = registry::pull_requests_for_issues(&tx, &project.id, &numbers)?;
            let mut commits = super::super::commits::list_for_issues(&tx, &project.id, &numbers)?;
            for issue in result["issues"].as_array_mut().unwrap() {
                let n = issue["number"].as_i64().unwrap();
                issue["pull_requests"] = json!(prs.remove(&n).unwrap_or_default());
                issue["commits"] = json!(commits.remove(&n).unwrap_or_default());
            }
            subtasks::enrich(&tx, &project.id, &mut result)?;
            super::super::blockers::enrich(&tx, &project.id, &mut result)?;
            assignments::enrich_result(&tx, &project.id, &mut result, request.actor.as_ref())?;
            actor_labels::enrich(&tx, &mut result)?;
        }
        let settings = registry::project_settings(&tx, &project)?;
        for key in [
            "prs_enabled",
            "drafts_enabled",
            "artifacts_enabled",
            "mindmaps_enabled",
        ] {
            result[key] = settings[key].clone();
        }
        let boss = super::super::global_settings::read(&tx)?;
        result["boss"] =
            json!({"id":"human:boss","name":boss["boss_name"],"version":boss["version"]});
        tx.commit()?;
        Ok(result)
    }
}
