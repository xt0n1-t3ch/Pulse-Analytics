//! Codex integration: model catalog, costs, session parsing, telemetry and Discord presence.

pub mod account_usage;
pub mod config;
pub mod cost;
pub mod discord;
pub mod model;
pub mod process;
pub mod session;
pub mod telemetry {
    pub mod limits;
    pub mod plan;
    pub mod service_tier;
}
pub mod util;

#[expect(
    dead_code,
    reason = "never called: it only pins the vendored sanitizer signatures at compile time"
)]
fn preserve_vendored_presence_sanitizer_contract() {
    let _ = session::sanitize_domain_target as fn(&str, usize) -> Option<String>;
    let _ = session::sanitize_file_target as fn(&str, usize) -> String;
    let _ = session::summarize_command_for_presence as fn(&str, usize) -> String;
}
