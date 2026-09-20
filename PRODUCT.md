# Pulse product context

## Platform and stack

Pulse is a local-first desktop analytics application. It uses Rust, Tauri 2, Svelte 5, TypeScript, and SQLite. Its development browser uses the real Rust command layer through an authenticated loopback bridge.

## Audience and purpose

Developers use Pulse while working in Claude Code, Codex, OpenCode, and Command Code. They need to understand current sessions, historical usage, account allowances, and the information they publish to Discord.

## Accounts

Accounts is an independent overview of connected provider accounts. Users can discover local profiles or establish separate Pulse connections. Removing a connection must not sign a coding client out or revoke its credentials. Accounts displays provider-reported windows, balances, reset entitlements, identity, and freshness. It does not change plans, buy credits, or consume resets.

## Product constraints

- Provider evidence owns identity, allowance, and monetary claims. Unknown values are not zero.
- Session analytics and account quotas remain distinct.
- CLI and Desktop share one Command Code identity and adapter.
- Discord monetary fields contain only the currency amount. Coverage and provenance stay visible in Pulse.
- Preserve the established monochrome visual system, keyboard access, themes, and responsive behavior.
- Secrets stay outside analytics records, logs, and persistent frontend storage.

## Success criteria

A user can identify each connected account, inspect its real allowances, and add or remove a Pulse connection without changing any coding-agent login. The same backend supports the native window and development browser. Live runtime proof remains separate from fixtures and static checks.
