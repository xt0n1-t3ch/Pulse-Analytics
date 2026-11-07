//! Build script: embeds the Windows icon.
//!
//! Cargo runs this script on the host, so `cfg!(windows)` would describe the
//! build machine. `CARGO_CFG_TARGET_OS` describes the machine the binary is for.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_BIN_NAME").is_ok()
    {
        embed_windows_icon();
    }
}

// `winres` is a build dependency on Windows hosts only (see Cargo.toml).
#[cfg(windows)]
fn embed_windows_icon() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.compile().expect("failed to compile Windows resources");
}

#[cfg(not(windows))]
fn embed_windows_icon() {
    println!(
        "cargo:warning=cross-compiling to Windows from this host: the executable icon is not embedded"
    );
}
