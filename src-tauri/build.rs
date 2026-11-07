//! Build script: embeds the Windows test manifest and runs `tauri-build`.

fn configure_test_manifest() {
    // Cargo's Windows lib-test harness does not receive `rustc-link-arg-tests`
    // from a build script, so a literal test-only link flag cannot protect
    // `cargo test --lib`. Keep one explicit manifest for both targets until
    // Cargo exposes that scope; it preserves Tauri's non-elevated trustInfo as
    // well as Common-Controls v6 instead of dropping production policy.
    let manifest =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    println!("cargo:rustc-env=PULSE_WINDOWS_TEST_MANIFEST=common-controls-v6");
}

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        configure_test_manifest();
    }

    let attributes = tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new_without_app_manifest()
            .window_icon_path("icons/icon.ico"),
    );
    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
