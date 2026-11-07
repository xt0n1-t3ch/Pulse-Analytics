# Adaptive access fixture contract

## Introduction

This reference is for maintainers validating provider access presentation without
calling a provider or Discord. It records the versioned fixture envelope, the
consumer DTO shape, and the exact boundary between mocked frontend tests and
live desktop probes. The fixtures are synthetic and contain no credentials or
personal data.

## Fixture envelope

`tests/fixtures/providers/manifest.json` is the inventory owner. Every listed
fixture is a `fixture.json` with these required fields:

| Field | Contract |
| --- | --- |
| `fixture_version` | Integer matching the manifest version (`1` today). |
| `raw_payload` | Sanitized provider/IPC-shaped JSON; never a live response. |
| `status` | Scenario outcome (`ok`, `unauthorized`, `upstream_error`, `no_data`, `stale`, `window_removed`, or `proof`). |
| `source_contract` | Versioned source identity such as `codex_subscription.v1` or `discord_ipc.v1`; it is a fixture contract, not an endpoint claim. |
| `captured_at` | Fixed ISO-8601 timestamp identifying the synthetic capture. |
| `expected_dto` | Expected consumer DTO or `{ "routes": [...] }` aggregate. |

The inventory covers Codex and Claude subscriptions, negative OpenAI and
Anthropic API outcomes, hybrid/no-data/stale/window-removal states, and Discord
IPC proof. API negative fixtures describe an error response only; they do not
contain a placeholder API key.

## Access route DTO

An authenticated route is represented by the following fields. `source.kind`
keeps subscription access separate from API access (`codex_subscription`,
`open_ai_api`, `claude_subscription`, `anthropic_api`). `availability` and
`freshness` are independent: stale data may remain visible as provenance while
its numeric window percentage is not displayable. A missing route is represented
by an empty `routes` array, not a fabricated zero.

```json
{
  "source": {
    "id": "codex-subscription:fixture",
    "kind": "codex_subscription",
    "provider": "codex",
    "auth_method": "app_server",
    "proof": "quota_response",
    "plan": "Pro 20x"
  },
  "availability": "available",
  "freshness": "fresh",
  "provenance": "app_server",
  "observed_at": "2026-08-01T00:00:00Z",
  "fetched_at": "2026-08-01T00:00:00Z",
  "expires_at": "2026-08-01T00:00:30Z",
  "windows": [],
  "credits": null,
  "extra_usage": null,
  "error": null
}
```

This is an expected consumer shape only. The fixture runner does not invent or
call a backend endpoint, and it does not replace the canonical Rust/Tauri
command contract.

## Source identity and analytics scope

Pulse keeps two keys deliberately separate:

- `source.id` is the stable account/access-lane identity used for quotas,
  freshness, provenance, reset credits, and source diagnostics.
- `source.provider` is the intentional analytics aggregation key passed to
  history, summary, context, cost, forecast, hourly, and report commands.

The supported provider scopes are `codex`, `claude`, `openai`, `anthropic`, and
the explicit aggregate `all`. API lanes are never aliases for subscription
session stores: selecting `openai` cannot show Codex subscription sessions, and
selecting `anthropic` cannot show Claude subscription sessions. If an API lane
has no provider-owned session telemetry, its analytics result is empty or
unavailable rather than borrowed from another lane. `all` is the only scope
that combines providers.

## Runner boundaries

`bun run test` owns mocked component and IPC contracts. `cargo nextest run --workspace` owns backend behavior. Fixtures remain synthetic inputs, never provider authentication proof.

`bun run dev` starts the authenticated loopback Rust bridge and Vite. Browser review proves rendering against real backend state, not native WebView behavior. Use `bun run dev:tauri` for a native development window and `bun run build:portable` for an embedded UI. Installed-runtime acceptance requires the actual packaged window with development ports stopped.

The Windows-only browser E2E harness has been removed. Native account checks and Discord acknowledgement require separate explicit live proof. Never stop Discord or unrelated agents for an automated fixture run. Cleanup must target only exact owned executable/PID identities.

## Cost exactness boundary

Cost is not a field on an access route. The two additive metrics vectors live
under `tests/fixtures/metrics/` so a quota fixture cannot accidentally invent a
price. `true_zero.json` carries `cost: 0`, `cost_available: true`, and
`cost_basis: "exact"`; `unavailable.json` carries a transport zero with
`cost_available: false` and `cost_basis: "unavailable"`. Consumers must keep
those states distinct and must not emit a Discord cost field for the unavailable
case. Tests must validate both vectors without promoting a transport zero to known cost.
