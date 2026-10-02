//! Deliberately narrow policy for issue operations sent through the fleet connection.
use super::{BatchAssignment, Error, Operation, Request, Result};

pub(crate) fn validate(request: &Request) -> Result<()> {
    match &request.operation {
        Operation::View { .. }
        | Operation::Allocation { .. }
        | Operation::RequestStatus { .. }
        | Operation::PullRequests { .. } => {
            return Ok(());
        }
        // Additive metadata does not depend on the issue's current owner or
        // version. INSERT ON CONFLICT preserves an existing link's purpose.
        // Keep the local CLI's commit-URL alias outside this PR capability.
        Operation::AddPullRequest { url, .. } if !super::commits::is_commit_url(url) => {}
        Operation::Edit {
            draft: None | Some(true),
            if_version: Some(version),
            ..
        } if *version > 0 => {}
        Operation::Reopen {
            if_version: Some(version),
            ..
        } if *version > 0 => {}
        Operation::SetBlockers {
            if_version: Some(version),
            force: false,
            ..
        } if *version > 0 => {}
        Operation::RefreshGithub { .. } => {}
        Operation::Ready { guard: Some(_), .. } => {}
        Operation::Close {
            guard: Some(_),
            force: false,
            ..
        } => {}
        Operation::Assign { if_version, .. } if *if_version > 0 => {}
        Operation::Batch { edits }
            if edits
                .iter()
                .all(|edit| matches!(edit.assignment, BatchAssignment::Keep)) => {}
        _ => {
            return Err(Error::invalid(
                "--supervisor supports view, allocation, PR add/list, version-guarded title/body/label edits, drafting, reopening and blocked-by edits on unassigned, unreserved issues, guarded Ready handoffs and close, and label-only batches with assignment: keep. PR add preserves existing purposes and ownership; PR classify/remove and commit URLs are not supported. Dependency edits and guarded close do not support --force. Other lifecycle, claim, assignment and reservation changes are not supported; nothing was saved. Inspect support with hey-boss fleet capabilities",
            ));
        }
    }
    if request.request_id.is_none() {
        return Err(Error::invalid(
            "--supervisor changes require --request-id; retry uncertain writes with the same ID and identical operation",
        ));
    }
    Ok(())
}

/// Called under the authority's write lock, after checking durable retry receipts.
/// No expiry or missing heartbeat proves an unfinished attempt safe to release.
pub(super) fn guard_unowned(
    db: &crate::database::Connection,
    project: &str,
    number: i64,
) -> Result<()> {
    let protected: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM issues WHERE project_id=?1 AND number=?2 AND assignee IS NOT NULL) OR EXISTS(SELECT 1 FROM fleet_allocations WHERE project_id=?1 AND issue_number=?2) OR EXISTS(SELECT 1 FROM worker_runs WHERE project_id=?1 AND issue_number=?2 AND finished_at IS NULL)",
        rusqlite::params![project, number], |row| row.get(0),
    )?;
    if protected {
        return Err(Error::conflict(
            "Supervisor reopen and dependency edits require an unassigned, unreserved issue with no unfinished worker attempt. Ownership and reservations were preserved; inspect issue allocation through --supervisor",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pr_capability_is_additive_and_requires_mutation_receipts() {
        let mut request: Request = serde_json::from_value(json!({
            "version":1, "project":{"id":"named:Test","name":"Test"},
            "operation":{"action":"add_pull_request","number":1,
                "url":"https://github.com/example/repo/pull/123","purpose":"prerequisite"}
        }))
        .unwrap();
        assert!(
            validate(&request)
                .unwrap_err()
                .message
                .contains("--request-id")
        );
        request.request_id = Some("pr-retry".into());
        validate(&request).unwrap();
        for operation in [
            json!({"action":"classify_pull_request","number":1,"url":"https://github.com/example/repo/pull/123","purpose":"fix"}),
            json!({"action":"remove_pull_request","number":1,"url":"https://github.com/example/repo/pull/123"}),
            json!({"action":"add_pull_request","number":1,"url":"https://github.com/example/repo/commit/0123456789012345678901234567890123456789"}),
        ] {
            request.operation = serde_json::from_value(operation).unwrap();
            assert_eq!(validate(&request).unwrap_err().code, "invalid_input");
        }
        request.operation = Operation::PullRequests { number: 1 };
        request.request_id = None;
        validate(&request).unwrap();
    }
}
