//! Pulse core: provider session readers, cost models, usage probes and Discord presence.
//!
//! The `cc-discord-presence` daemon and the `pulse` desktop application both build on this crate.

pub mod activity_target;
pub mod app;
pub use codex_presence_core as presence_core;
pub mod chrome_session;
pub mod cli;
pub mod codex;
pub mod commandcode;
pub mod config;
pub mod cost;
pub mod discord;
pub mod discord_identity;
pub mod foreign_sqlite;
pub mod git_head;
pub mod http;
pub mod metrics;
pub mod opencode;
pub mod orion;
pub mod owned_child;
pub mod plan;
pub mod power;
pub mod process_guard;
pub mod process_list;
pub mod provider;
pub mod session;
pub mod sound;
pub mod storage;
pub mod usage;
pub mod util;
pub mod workflow_state;
