//! Pulse repository automation, run as `cargo xtask <command>`.
//!
//! Every command is a library function that takes explicit paths and returns the
//! summary line it prints, so the tests call them directly and never need a shell.

pub mod audit;
pub mod catalog;
pub mod changelog;
pub mod check;
pub mod ci;
pub mod clock;
pub mod core_pin;
pub mod efficiency;
pub mod fsx;
pub mod git;
pub mod llms;
pub mod local_assets;
pub mod meta;
pub mod platform_assets;
pub mod release_assets;
pub mod release_contract;
pub mod repo_config;
pub mod sbom;
pub mod semver;
pub mod upstream;

use std::path::PathBuf;

/// Root of the Pulse checkout this crate was built from.
pub fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map_or_else(|| manifest_dir.clone(), PathBuf::from)
}
