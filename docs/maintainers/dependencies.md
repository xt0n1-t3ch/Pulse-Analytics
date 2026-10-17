# Dependency audit scope

The 2026-10-17 workspace scan (cargo-audit 0.22.2, advisory database of 2026-10-16, 624 crates) reports three vulnerability-class entries, all in `zip 4.6.1`, and 9 warning-class advisories. `scripts/audit-rust.ps1` runs `cargo audit --deny warnings` with the reviewed exceptions in `scripts/rustsec-accepted-warnings.json`, which expire on `review_after` (2026-11-17) and then fail the release preflight until renewed. The ten GTK3 notices (RUSTSEC-2024-0411 to 0420) no longer appear in the graph and were removed from the exceptions on 2026-09-27.

The warnings and the `zip` entries are inherited from the existing Tauri dependency graph.

## Updater zip advisories

[RUSTSEC-2026-0367](https://rustsec.org/advisories/RUSTSEC-2026-0367) (SUID/SGID bits preserved on extraction), [RUSTSEC-2026-0368](https://rustsec.org/advisories/RUSTSEC-2026-0368) (symlink targets outside the destination) and [RUSTSEC-2026-0369](https://rustsec.org/advisories/RUSTSEC-2026-0369) (Unix permissions applied through symlinks) are fixed in `zip` 9.0.0. Pulse cannot take that fix yet: `tauri-plugin-updater` 2.10.1 declares `zip = "4"` as a Windows-only dependency, and no updater release accepts `zip` 9.

These entries are accepted, not resolved, for these reasons:

- `zip` is compiled only for Windows. The updater's only use is `Update::extract_zip`.
- `Update::download` verifies the minisign signature of the whole download against the `pubkey` in `src-tauri/tauri.conf.json` before it returns the bytes. `install` accepts only bytes that `download` returned, so no archive is parsed before the signature passes. A malicious archive needs the Pulse updater signing key.
- Pulse's signed Windows updater payloads are the NSIS `.exe` and the `.msi`, not zip archives, so the release path does not call `extract_zip`.
- In `zip` 4.6.1, extraction applies Unix mode bits only under `cfg(unix)`. RUSTSEC-2026-0367 and the permission change in RUSTSEC-2026-0369 do not apply to the Windows build.

Remove these exceptions when an updater release accepts `zip` 9.0.0 or later.

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
