# Canonical Codex presence core

Pulse consumes Codex telemetry and Rich Presence composition from the
standalone [Codex Discord Rich Presence](https://github.com/xt0n1-t3ch/Codex-Discord-Rich-Presence)
repository. `codex-presence-core` is the UI-free owner. Pulse owns Tauri
integration, analytics persistence, and presentation; it must not recreate
parsing or Discord line composition in TypeScript or a second Rust module.

## Promoted contract

Pulse **v1.9.0** consumes `codex-presence-core` **2.0.1** from the immutable
upstream **v1.11.2** release at commit
`0caece60a71eb657d7b829e3b6c7c4896a0a536c`. Pulse v1.8.2 consumed
`codex-presence-core` 2.0.0 from upstream v1.10.3 at commit
`9d20ffdb1c4ec6fa37edc00952badd041ec5bc02`.

| Surface | Promoted value | Release proof |
| --- | --- | --- |
| Core package | `codex-presence-core` 2.0.1 | Published upstream v1.11.2 release |
| Git dependency | Canonical repository plus full `rev` | Cargo manifests and lockfile resolve to the same commit |
| Canonical manifest | v1.11.2 + exact commit | `src/codex/UPSTREAM.json` matches the Cargo Git pin |
| Presence config | Schema 13 | Migration fixtures pass |
| Pulse database | Schema 6 in 1.8.2; schema 7 in 1.9.0 | Migration and query-plan fixtures pass |

Branches, shortened SHAs, path dependencies, and unpeeled tag objects are not
release identities. The release contract requires the exact commit reached by
the annotated upstream tag.

## Source and compatibility boundary

The core exports model identity, reasoning effort, semantic usage snapshots,
quota scopes/windows, Credits, service tier, configuration layout, and
deterministic Rich Presence composition. Pulse may translate those DTOs into
Tauri responses but may not reinterpret positional limits, guess unsupported
pricing, or infer unavailable provider capabilities.

Files under `src/codex/` are Pulse-owned adapters or compatibility surfaces
listed in `src/codex/UPSTREAM.json`. The manifest records schema compatibility
and the immutable upstream identity. `scripts/check-codex-rich-presence-upstream.ps1`
checks the manifest against the Cargo dependency before a release is accepted.

## Validation

Run the complete local gate before promotion:

```powershell
npm run verify
npm --prefix frontend run build
pwsh -File scripts/e2e/run-pulse.ps1 -Mode Tauri -RunPlaywright
```

Browser QA uses the authenticated loopback bridge and real local history; the
Tauri runner uses an isolated profile so it cannot collide with installed
Pulse. Packaging must then produce exact-version NSIS/MSI installers, a
validated Windows SPDX SBOM, and a checksum manifest.

## Future upstream updates

1. Publish and verify a new immutable upstream release.
2. Record its peeled commit in both Cargo manifests and
   `src/codex/UPSTREAM.json`.
3. Refresh `Cargo.lock` and bump the Pulse compatibility version.
4. Run upstream, model, parser, analytics, localhost, Tauri, SBOM, and checksum
   gates again.
5. Publish a new Pulse patch or minor release; never move an existing tag or
   replace an immutable asset.

## Promoted cost and composition work

Upstream v1.11.2 publishes per-event cost accumulation, observed-request long-context pricing, currency-only known subtotals and the reserved compositor cost space. Pulse 1.9.0 pins that release, so the ordinary pinned build contains those behaviors and no source override is required. Canonical parser, model and cost changes are mirrored into Pulse's Rust adapters. `scripts/check-model-catalog-parity.ps1` compares the catalog with its canonical owner.

Set `PULSE_CODEX_CORE_PATH` only for the development launcher. For Cargo checks against unpublished core work, pass an explicit temporary configuration that patches the canonical Git source to the local core directory, and run the ordinary pinned build as well.

The override can change Cargo.lock's resolved source. Restore the immutable Git resolution after the overridden checks. Never describe a path patch as a released shared-core version.
