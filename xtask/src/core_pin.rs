//! Finds the `codex-presence-core` dependency in the root manifest, the one
//! place every release check reads the shared-core pin from.
use anyhow::{Context, Result};

const CORE: &str = "codex-presence-core";

/// The `codex-presence-core` table of `[dependencies]`, or of
/// `[workspace.dependencies]` when `[dependencies]` inherits it with
/// `workspace = true`.
pub fn core_dependency(manifest: &toml::Table) -> Result<&toml::Table> {
    let declared = dependency_in(manifest.get("dependencies"))
        .context("Cargo.toml does not declare codex-presence-core")?;
    if declared.get("workspace").and_then(toml::Value::as_bool) != Some(true) {
        return Ok(declared);
    }
    dependency_in(
        manifest
            .get("workspace")
            .and_then(|workspace| workspace.get("dependencies")),
    )
    .context(
        "Cargo.toml inherits codex-presence-core but [workspace.dependencies] does not declare it",
    )
}

fn dependency_in(dependencies: Option<&toml::Value>) -> Option<&toml::Table> {
    dependencies?.get(CORE)?.as_table()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(text: &str) -> toml::Table {
        toml::from_str(text).expect("valid manifest")
    }

    #[test]
    fn reads_a_direct_pin() {
        let manifest =
            manifest("[dependencies]\ncodex-presence-core = { git = \"g\", rev = \"r\" }\n");
        assert_eq!(
            core_dependency(&manifest).unwrap()["rev"].as_str(),
            Some("r")
        );
    }

    #[test]
    fn follows_a_pin_inherited_from_the_workspace() {
        let manifest = manifest(
            "[workspace.dependencies]\ncodex-presence-core = { git = \"g\", rev = \"r\", version = \"2.0.1\" }\n\n[dependencies]\ncodex-presence-core.workspace = true\n",
        );
        let pin = core_dependency(&manifest).unwrap();
        assert_eq!(pin["git"].as_str(), Some("g"));
        assert_eq!(pin["version"].as_str(), Some("2.0.1"));
    }

    #[test]
    fn names_a_missing_workspace_pin() {
        let manifest = manifest("[dependencies]\ncodex-presence-core.workspace = true\n");
        let error = core_dependency(&manifest).unwrap_err().to_string();
        assert!(error.contains("[workspace.dependencies] does not declare it"));
    }

    #[test]
    fn names_a_missing_dependency() {
        let error = core_dependency(&manifest("[dependencies]\n"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not declare codex-presence-core"));
    }
}
