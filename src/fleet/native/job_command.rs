//! One-shot commands run outside project checkouts and retain their output.
use super::*;
use crate::jobs::{Operation, command};
use std::fs;

fn call(ctx: &Context, d: &Dispatch, operation: Operation) -> crate::issues::Result<Value> {
    let project = &d.run.snapshot.project_id;
    let request = crate::issues::Request {
        version: 1,
        project: crate::issues::Project {
            id: project.clone(),
            name: project.clone(),
        },
        project_override: None,
        actor: Some(
            ctx.actor()
                .map_err(|e| crate::issues::Error::invalid(e.to_string()))?,
        ),
        request_id: Some(format!("exec-file-{}", d.run.id)),
        operation: crate::issues::Operation::Job { operation },
    };
    loop {
        match Store::open(&ctx.path)?.execute(&request) {
            Ok(v) => return Ok(v),
            Err(e)
                if ["fleet_unavailable", "fleet_error"].contains(&e.code.as_str())
                    && !ctx.stopped() =>
            {
                std::thread::sleep(Duration::from_secs(2))
            }
            Err(e) => return Err(e),
        }
    }
}
pub(super) fn run(
    ctx: &Context,
    d: &Dispatch,
    report: &mut Report,
) -> Result<(String, Option<String>)> {
    let spec = d.run.snapshot.definition.execution.as_ref().unwrap();
    let root = ctx.state.join("executions").join(&d.run.id);
    fs::create_dir_all(root.parent().unwrap())?;
    fs::create_dir(&root)?;
    if let Some(blob) = &spec.input {
        command::download(blob, &root.join("input.tar.gz"), |op| call(ctx, d, op))?;
    }
    fs::create_dir(root.join("result"))?;
    report.cwd = Some(root.to_string_lossy().into_owned());
    let outcome = super::script::run(
        ctx,
        d,
        report,
        root.to_str()
            .ok_or_else(|| replica::invalid("Non UTF-8 execution directory"))?,
        &spec.program,
    )?;
    let archive = root.join("result.tar.gz");
    command::pack(&root.join("result"), &archive)?;
    let blob = command::upload(&archive, |op| call(ctx, d, op))?;
    report.result.get_or_insert_with(|| json!({}))["output"] = serde_json::to_value(blob)?;
    Ok(outcome)
}
