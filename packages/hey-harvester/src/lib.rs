//! Standalone machine maintenance, sharing Hey Boss configuration and inventory.
pub const BUILD_ID: &str = env!("HEY_HARVESTER_BUILD_ID");

pub mod agents;
pub mod cli;
pub mod health;
mod install;
pub mod tui;
