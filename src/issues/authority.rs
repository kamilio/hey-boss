//! Deliberately narrow policy for metadata sent through the fleet connection.
use super::{BatchAssignment, Error, Operation, Request, Result};

pub(crate) fn validate(request: &Request) -> Result<()> {
    match &request.operation {
        Operation::View { .. } | Operation::Allocation { .. } => return Ok(()),
        Operation::Edit {
            draft: None | Some(true),
            if_version: Some(version),
            ..
        } if *version > 0 => {}
        Operation::Batch { edits }
            if edits
                .iter()
                .all(|edit| matches!(edit.assignment, BatchAssignment::Keep)) => {}
        _ => {
            return Err(Error::invalid(
                "--supervisor supports view, allocation, version-guarded title/body/label edits and drafting eligible issues, and label-only batches with assignment: keep. Other lifecycle, claim, assignment and reservation changes are not supported; nothing was saved. Inspect support with hey-boss fleet capabilities",
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
