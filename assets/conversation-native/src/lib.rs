pub mod agent;
pub mod context;
pub mod media;
pub mod plugins;
pub mod provider;

use anyhow::{Context, Result};
use serde_json::Value;
use std::io::{self, Write};

pub fn emit(value: Value) -> Result<()> {
    writeln!(io::stdout().lock(), "{value}")?;
    Ok(())
}
pub fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .with_context(|| format!("Missing {key}"))
}
