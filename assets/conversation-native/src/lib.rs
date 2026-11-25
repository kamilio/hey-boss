pub mod agent;
pub mod context;
pub mod delegate;
pub mod media;
pub mod notebook;
pub mod plugins;
pub mod provider;
pub mod t3;

use anyhow::{Context, Result};
use serde_json::Value;
use std::io::{self, Write};

tokio::task_local! {
    pub static EVENTS: tokio::sync::mpsc::UnboundedSender<Value>;
}

pub fn emit(value: Value) -> Result<()> {
    if let Ok(result) = EVENTS.try_with(|events| events.send(value.clone())) {
        result?;
        return Ok(());
    }
    writeln!(io::stdout().lock(), "{value}")?;
    Ok(())
}
pub fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .with_context(|| format!("Missing {key}"))
}
