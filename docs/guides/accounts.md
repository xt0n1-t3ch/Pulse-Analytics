[Documentation](../index.md) / Guides / Accounts

# View accounts and provider usage

The unreleased Accounts tab groups Claude, Codex, OpenCode Go and Command Code connections. It does not change your analytics filter, Discord broadcaster or coding-agent login.

## Connect an account

Open **Accounts**, then select **Add account**. Choose the provider and one connection method:

- **Connect for Pulse** creates a private Pulse connection. Claude uses an isolated `CLAUDE_CONFIG_DIR`. Codex uses an isolated `CODEX_HOME` and app-server sign-in. Command Code accepts browser sign-in or an API key. OpenCode Go accepts its API key.
- **Link a local profile** reads an existing profile. Select its absolute folder path. Pulse does not refresh or overwrite that profile's credentials.

The browser development app accepts a folder path. The native Tauri app also has a folder picker. Pending browser connections show a sign-in link and a cancel control. Expired connections show **Connect again**.

## Read the limits

Each connection has its own identity, cached observations, errors and refresh deadline. Pulse renders the windows that the provider reports:

| Provider | Usage and balances |
| --- | --- |
| Claude | Five-hour, weekly and model-scoped limits, including Fable, plus reported extra usage |
| Codex | Reported windows, credits, individual limits and banked resets with expiry; no fabricated five-hour window for Pro |
| OpenCode Go | Reported five-hour, weekly and monthly windows; model allowances and Zen balance only when the response supplies them |
| Command Code | Five-hour, weekly and monthly limits; separate monthly, purchased and free credits |

Codex's native `pro` and `pro_lite` identities resolve through the canonical parser to Pro 20x and Pro 5x. A configured plan override is not account authentication.

Command Code's monthly meter combines the authenticated subscription identity and remaining monthly credits. The API's explicit allocation takes priority. For known individual plans, the fallback uses the dated Command Code plan catalog described in the [provider guide](command-code.md). Unknown plans do not receive an invented allocation. Purchased and free balances do not change monthly usage.

Zero credits is a valid balance. A missing reset stays absent. A cycle end is not a credit expiry. Pulse keeps native units separate and never converts subscription credits into API dollars.

## Refresh or remove a connection

**Refresh** requests a new observation while respecting the provider's cache and retry deadline. Account reads have a shared concurrency bound. Claude normally refreshes after five minutes; other successful account reads after one minute. Errors use backoff and provider retry headers.

**Remove from Pulse** removes only the Pulse link and any Pulse-owned secret files. It does not log out the coding agent, revoke a provider credential or delete session history. A tombstone prevents automatic rediscovery. Explicitly linking the same profile restores its connection.

## Storage and availability

SQLite schema 7 adds account identity records and snapshots. Secret files stay outside SQLite and browser storage, under protected Pulse profiles. Windows profiles use a restricted access control list; Unix profiles use mode `0700`.

Authentication failure does not invalidate local session history. Stale, unavailable, expired and pending states remain distinct. This screen cannot purchase credits, change plans or consume banked resets.

See [storage recovery](storage.md) before rolling back a schema-7 development database. Native Windows IPC and Command Code key reconnection were tested with isolated real data. Fresh OAuth completion on every provider and native macOS/Linux account flows require separate host acceptance.
