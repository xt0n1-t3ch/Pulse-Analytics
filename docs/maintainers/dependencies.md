# Dependency audit scope

The 2026-09-27 workspace scan (cargo-audit 0.22.2, 624 crates) reports zero vulnerability-class entries and 9 warning-class advisories. `scripts/audit-rust.ps1` runs `cargo audit --deny warnings` with the reviewed exceptions in `scripts/rustsec-accepted-warnings.json`, which expire on `review_after` (2026-10-27) and then fail the release preflight until renewed. The ten GTK3 notices (RUSTSEC-2024-0411 to 0420) no longer appear in the graph and were removed from the exceptions.

The warnings are inherited from the existing Tauri dependency graph. The dependency versions were not changed by the OpenCode/Astra feature work.

## Windows release

The Windows normal/runtime dependency tree does not contain `glib 0.18.5` or `rand 0.7.3`.

`rand 0.7.3` is a build dependency through `phf_generator`, `phf_codegen` and `selectors`, used by Tauri's HTML tooling. [RUSTSEC-2026-0097](https://rustsec.org/advisories/RUSTSEC-2026-0097) describes re-entrant custom logging through the thread RNG. This project does not define that logger pattern. The advisory remains recorded rather than suppressed.

GTK/GLib warnings concern the non-Windows Tauri platform graph. Other maintenance notices include `fxhash`, the `unic` family and `proc-macro-error`. These require upstream dependency work; this release does not claim to resolve them.

## Reproduce

```powershell
cargo audit --json
cargo tree -p pulse --target x86_64-pc-windows-msvc --edges normal
cargo tree --workspace --target all -i rand@0.7.3
```

The standalone Codex Discord Rich Presence runtime has a separate dependency graph and passes its warning-denying RustSec gate.
