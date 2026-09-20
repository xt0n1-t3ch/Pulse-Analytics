pub mod access;
pub mod accounts;
pub mod analyzers;
pub mod app_settings;
pub mod commandcode;
pub mod commands;
pub mod db;
/// Debug-only localhost bridge for browser-based UI review. Never compiled
/// into release builds.
#[cfg(debug_assertions)]
pub mod dev_bridge;
pub mod live;
pub mod notifications;
pub mod opencode;
pub mod opencode_go;
pub mod report;
pub mod report_template;
pub mod update_check;

#[cfg(all(test, windows))]
mod windows_manifest_contract {
    // Running this test also proves the harness can load the v6 resource.
    // Keep the assertion tied to the build-script receipt.
    #[test]
    fn pulse_test_harness_uses_tauri_common_controls_resource() {
        assert_eq!(
            option_env!("PULSE_WINDOWS_TEST_MANIFEST"),
            Some("common-controls-v6")
        );
    }
}
