//! Reusable worker dashboard and cancellable terminal runtime.
//! The main CLI and this library compile the same source.
#[path = "../../src/worker_tui/mod.rs"]
mod dashboard;
pub use dashboard::*;
