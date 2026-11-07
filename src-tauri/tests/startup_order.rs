//! Source-level checks of the startup order in `main.rs`.

#[test]
fn single_instance_plugin_is_registered_before_the_background_poller() {
    let source = include_str!("../src/main.rs");
    let single_instance = source
        .find(".plugin(tauri_plugin_single_instance::init")
        .expect("single-instance plugin registration");
    let setup = source.find(".setup(|app|").expect("Tauri setup");
    let poller = source
        .find("commands::start_background_poller")
        .expect("background poller startup");

    assert!(
        single_instance < setup && setup < poller,
        "a rejected second launch must not reach poller startup"
    );
}

#[test]
fn installed_pulse_is_bypassed_only_by_the_isolated_debug_probe() {
    let source = include_str!("../src/main.rs");

    assert!(source.contains("#[cfg(debug_assertions)]\nfn native_e2e_probe_requested()"));
    assert!(source.contains("std::env::var_os(\"PULSE_E2E_RUN_ID\").is_some()"));
    assert!(source.contains("#[cfg(not(debug_assertions))]\nfn native_e2e_probe_requested()"));
    assert!(source.contains("fn native_e2e_probe_requested() -> bool {\n    false\n}"));
}
