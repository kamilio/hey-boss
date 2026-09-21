//! Use the same bounded history reader for desktop and connected devices.
use super::{Result, context::Context};
use serde_json::Value;
use std::path::PathBuf;
pub(super) fn page(ctx: &Context, run_id: &str, cursor: &Value) -> Result<Value> {
    let window: crate::agent_conversations::Window = if let Some(cursor) = cursor.as_u64() {
        crate::agent_conversations::Window {
            cursor,
            ..Default::default()
        }
    } else {
        serde_json::from_value(cursor.clone())?
    };
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| ctx.home.join(".codex"));
    Ok(crate::agent_conversations::window_page(
        &ctx.db()?,
        &home,
        run_id,
        &window,
    )?)
}
