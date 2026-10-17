use std::path::Path;

use serde_json::Value;

fn tauri_config() -> Value {
    serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json")
}

fn capability_permissions() -> Vec<Value> {
    let capability: Value =
        serde_json::from_str(include_str!("../capabilities/default.json")).expect("capability");
    capability["permissions"]
        .as_array()
        .expect("permissions array")
        .clone()
}

fn csp_directive<'a>(csp: &'a str, name: &str) -> &'a str {
    csp.split(';')
        .map(str::trim)
        .find_map(|directive| {
            directive
                .strip_prefix(name)
                .filter(|rest| rest.starts_with(' '))
        })
        .map(str::trim)
        .unwrap_or_else(|| panic!("CSP is missing {name}"))
}

#[test]
fn webview_does_not_expose_the_global_tauri_object() {
    assert_eq!(tauri_config()["app"]["withGlobalTauri"], Value::Bool(false));
}

#[test]
fn webview_csp_allows_only_bundled_code_ipc_and_discord_images() {
    let config = tauri_config();
    let security = &config["app"]["security"];
    let csp = security["csp"].as_str().expect("security.csp string");

    assert_eq!(csp_directive(csp, "default-src"), "'self'");
    assert_eq!(csp_directive(csp, "script-src"), "'self'");
    assert_eq!(csp_directive(csp, "style-src"), "'self' 'unsafe-inline'");
    assert_eq!(
        csp_directive(csp, "img-src"),
        "'self' data: blob: https://cdn.discordapp.com"
    );
    assert_eq!(
        csp_directive(csp, "connect-src"),
        "ipc: http://ipc.localhost"
    );
    assert_eq!(csp_directive(csp, "object-src"), "'none'");
    assert_eq!(csp_directive(csp, "frame-ancestors"), "'none'");
    assert!(!csp.contains("unsafe-eval"));
    assert_eq!(
        security["dangerousDisableAssetCspModification"],
        serde_json::json!(["style-src"]),
        "Tauri nonces would otherwise disable 'unsafe-inline' for Svelte style attributes"
    );
}

#[test]
fn shell_open_accepts_only_https_urls() {
    assert_eq!(tauri_config()["plugins"]["shell"]["open"], "https://\\w.+");
}

#[test]
fn capability_grants_no_filesystem_access_and_only_used_plugin_commands() {
    let permissions = capability_permissions();
    let identifiers: Vec<&str> = permissions
        .iter()
        .map(|permission| {
            permission
                .as_str()
                .expect("plain permission identifiers only; scoped grants need review")
        })
        .collect();

    assert_eq!(
        identifiers,
        [
            "core:window:allow-minimize",
            "core:window:allow-start-dragging",
            "core:window:allow-maximize",
            "core:window:allow-unmaximize",
            "core:window:allow-close",
            "core:window:allow-is-maximized",
            "core:event:allow-listen",
            "core:event:allow-unlisten",
            "core:event:allow-emit",
            "core:app:allow-set-app-theme",
            "dialog:allow-open",
            "shell:allow-open",
            "updater:default",
            "process:allow-restart",
        ]
    );
}

fn collect_frontend_sources(dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).expect("frontend source directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            collect_frontend_sources(&path, out);
        } else if matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("ts" | "svelte" | "js")
        ) {
            let text = std::fs::read_to_string(&path).expect("frontend source");
            out.push((path.display().to_string(), text));
        }
    }
}

#[test]
fn frontend_uses_tauri_api_imports_instead_of_the_global_object() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../frontend/src");
    let mut sources = Vec::new();
    collect_frontend_sources(&root, &mut sources);
    assert!(!sources.is_empty());

    for (path, text) in sources {
        assert!(
            !text.contains("__TAURI__"),
            "{path} reads window.__TAURI__, which withGlobalTauri=false removes"
        );
    }
}
