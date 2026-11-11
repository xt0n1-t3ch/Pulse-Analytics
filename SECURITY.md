# Security policy

## Supported versions

Report vulnerabilities against the latest stable Pulse release. The checkout version is 2.1.0; consult [Releases](https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/latest) for published packages. Older installations should upgrade before checking whether a problem remains. Do not assume every historical 1.x package receives backported fixes.

## Report a vulnerability

Do not open a public issue containing exploit details or private data. Use [GitHub private vulnerability reporting](https://github.com/xt0n1-t3ch/Pulse-Analytics/security/advisories/new).

Include the affected version, operating system and architecture, provider, reproduction steps, expected security boundary, impact and any suggested remediation. Redact tokens, credentials, account identifiers, prompts and transcripts. Do not attach a provider home directory, analytics database or diagnostic archive without reviewing it.

The project aims to acknowledge reports within 72 hours and resolve confirmed issues within 14 days where practical. These are response goals, not guaranteed service levels. Coordinated disclosure timing depends on impact and upstream dependencies.

## Scope

- Pulse's Tauri desktop application (`pulse`) and frontend.
- The core library and Claude daemon (`cc-discord-presence`).
- Read-only provider ingestion, protected account profiles, analytics storage, exports and the debug-only development bridge.
- Official release packages, updater signatures and repository release tooling.

For dependency vulnerabilities, include the Pulse impact and report upstream where appropriate. Third-party forks and social engineering against contributors are outside this project's control. See the [dependency audit](docs/maintainers/dependencies.md) for reviewed exceptions and the [release procedure](docs/maintainers/releases.md) for artifact integrity.

## Data and verification boundaries

Pulse keeps analytics local. Enabled provider quota checks, release/update checks and selected Discord fields use the network. Configured credentials are not proof of successful authentication. Provider folders and logins remain independent of Pulse-owned data.

Verify release checksums and use only packages for your OS and architecture. GitHub macOS packages are not Apple Developer ID signed or notarized; updater signatures are a separate integrity mechanism. Keep system security controls enabled. Local tests and fixture screenshots do not establish installed-runtime security or native acceptance.
