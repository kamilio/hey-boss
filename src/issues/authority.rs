//! Deliberately narrow policy for metadata sent through the fleet connection.
use super::{BatchAssignment, Error, Operation, Request, Result};

pub(crate) fn validate(request: &Request) -> Result<()> {
    match &request.operation {
        Operation::View { .. } | Operation::Allocation { .. } => return Ok(()),
        Operation::Edit {
            draft: None,
            if_version: Some(version),
            ..
        } if *version > 0 => {}
        Operation::Batch { edits }
            if edits
                .iter()
                .all(|edit| matches!(edit.assignment, BatchAssignment::Keep)) => {}
        _ => {
            return Err(Error::invalid(
                "--supervisor supports view, allocation, version-guarded title/body/label edits, and label-only batches with assignment: keep. Lifecycle, draft, claim, assignment and reservation changes are not supported; nothing was saved",
            ));
        }
    }
    if request.request_id.is_none() {
        return Err(Error::invalid(
            "--supervisor metadata changes require --request-id; retry uncertain writes with the same ID and identical operation",
        ));
    }
    Ok(())
}
