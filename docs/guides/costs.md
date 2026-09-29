[Documentation](../index.md) / Guides / Costs

# ![](../../assets/icons/scale.svg) Cost calculation

Understand what Pulse's monetary values measure. Provider-billed spend, API-equivalent estimates and OpenCode-reported value are not interchangeable.

[Claude](#claude-code) · [Codex](#codex) · [Orion App](#orion-app) · [OpenCode](#opencode) · [Completeness](#completeness-and-freshness)

> **Rates need current evidence.** The [Claude](../models/claude.md#pulse-implementation-gaps) and [Codex](../models/codex.md#bundled-catalog-gaps) references list confirmed differences between current provider prices and this runtime. Documentation alone does not fix the estimates.

## Claude Code

Raw JSONL usage has four independent token categories:

| Field | Meaning |
| --- | --- |
| `usage.input_tokens` | Uncached input only |
| `usage.cache_creation_input_tokens` | Input written to cache |
| `usage.cache_read_input_tokens` | Input read from cache |
| `usage.output_tokens` | Generated output |

The [parser](../../src/session.rs) adds all three input categories to Pulse's aggregate `input_tokens`. Only that aggregate includes cached input. Do not subtract cache tokens from the raw uncached-input field.

Claude Code writes one JSONL line per content block of a response (thinking, text, tool_use), and each line repeats that response's `usage`. While a response streams, `output_tokens` can grow from one line to the next. Pulse counts each `message.id` once, at its latest usage, even when lines from other responses come in between. Lines without a `message.id` still count one by one. Before 1.9.4, Pulse counted every line, so tokens and costs were overstated.

```text
turn input = uncached input + cache creation + cache read
turn cost = (uncached input × input rate
           + cache creation × write rate
           + cache read × read rate
           + output × output rate) / 1,000,000
```

The [cost owner](../../src/cost.rs) applies supported long-context and Fast modifiers per turn, then accumulates the four category costs. Mixed-speed sessions retain each turn's multiplier. `usage.service_tier` is not the same field as `usage.speed`.

When Claude statusline data supplies `total_cost_usd`, Pulse uses it for the headline. It scales JSONL category proportions to reconcile the breakdown. Without statusline authority, costs remain estimates based on implemented rates and available telemetry. Missing cache TTL is priced using the 5-minute write rate, not a demonstrated 1-hour rate.

Current prices, API limits and the cancelled Sonnet 5 price increase belong in the [Claude model reference](../models/claude.md), not a duplicated rate table here. That reference also covers Sonnet 5.5: it bills at the same per-token rates as Sonnet 5, with no long-context surcharge and no Fast multiplier, and Pulse classifies the two versions separately. For example, 1,000,000 input tokens plus 1,000,000 output tokens on Sonnet 5.5 cost $2 + $10 = $12.

## Codex

[Model resolution](../../src/codex/model.rs) normalizes known aliases and reads the [bundled catalog](../../src/codex/model_catalog.json). [Cost arithmetic](../../src/codex/cost.rs) resolves rates, explicit user overrides and completeness.

Codex input totals include cache reads and cache writes; known categories are subtracted once from ordinary input. Cached input is clamped to total input before uncached input is calculated. Missing cache-write telemetry stays missing. Do not reuse Claude's raw-token interpretation for Codex.

The bundled Astra entry records Fast at 2×. GPT-5.6 entries still lack a Fast multiplier and use older base rates. Aggregate input cannot prove each request's long-context price. The resolver preserves partial coverage when a required pricing condition is unresolved. [Current API facts and catalog gaps](../models/codex.md#bundled-catalog-gaps).

API dollars and Codex subscription credits are separate units. A new API price does not establish a new credit conversion or account allowance.

Session cost is accumulated per telemetry delta with that sample's model and speed. Duplicate cumulative events do not add value. A large cumulative input total does not trigger request-level long-context pricing. An observed Astra request over 272,000 input tokens applies the documented input/cache and output multipliers. Missing request boundaries keep the subtotal partial.

## Command Code

Pulse preserves the provider's `costUsd` values and marks incomplete message coverage as partial. SQLite records `commandcode_reported` separately from API-equivalent estimates. A reported amount is not proof of a settled invoice.

## Orion App

Orion records `cost: 0` on every message, which is not a price, so Pulse ignores it and calculates session cost the way it does for Claude Code: exact token counts times published per-model rates. It is a calculation, not an invoice. Orion's `tokens.input` already includes cache read and cache write; Pulse subtracts both once to get uncached input, then prices four categories (input, output, cache write, cache read) and sums them.

Each assistant message is priced as its own request, so long-context and Fast rules apply per request, and subagent messages roll into the parent session. Claude models use [`src/cost.rs`](../../src/cost.rs). Models in the [Codex catalog](../../src/codex/model_catalog.json) use [`src/codex/cost.rs`](../../src/codex/cost.rs), including Orion's `-fast` suffix and its catalog display names such as `5.6-Sol`. A display name that matches more than one catalog entry, such as `5.6-Cyber`, stays unpriced. For example, one million tokens in each category on Opus 5.5 cost $4 + $20 + $5 + $0.20 = $29.20.

| Basis | When |
| --- | --- |
| `estimated` | Every model in the session has a known rate. Shown as the session cost, calculated from token counts. |
| `partial` | At least one model has no known rate, or a rate has an unresolved condition such as an unpublished Fast multiplier. The known part is a lower bound. |
| `unavailable` | No model in the session has a known rate. Never shown as zero. |

A session with no model requests is a true zero. SQLite stores Orion rows with the `api_equivalent` source and the four category costs, so history, the Costs view and the forecast add them without a second calculation. Reading history never reprices an Orion row from its totals, because that would send an unknown model id through a default tier. Reopening an older Orion row re-imports it by session ID and replaces its values.

Output speed is generated tokens divided by the time between the first token and completion, over the session's completed, streamed requests recorded in Orion's `model_usage` table. An older store without that table reports no speed.

## OpenCode

Pulse preserves OpenCode's reported value and per-model contributions. A genuine reported zero remains zero. An absent cost remains unavailable; Pulse does not reconstruct it with the Claude or Codex catalog. OpenCode-reported value is not proof of a settled provider invoice. [OpenCode integration](opencode.md#interpretar-los-datos).

## Completeness and freshness

| Codex status | Meaning |
| --- | --- |
| `exact` | Required components are covered by selected rates and supported telemetry |
| `partial` | A known subtotal exists, but a component or pricing condition is unresolved |
| `unavailable` | The model or required rates cannot be resolved |

`exact` does not prove that a rate is still current, or that an estimate equals a bill. Unknown Codex models do not borrow another model's rate. Claude's existing family fallback has a different limitation, documented in its model guide.

Discord shows the best known amount as currency only, including partial subtotals, when the monetary field is enabled. Coverage and API-equivalent versus reported provenance remain available in Pulse; unknown amounts stay absent. A long model label cannot silently remove an enabled cost. Historical rows without monetary provenance retain `legacy/unknown`.

## Comparing totals

Check provider scope, time range, live/history membership and monetary provenance first. Home's live value and Costs' selected history window answer different questions. Use the priced-session denominator for monetary averages; unknown value must not become a zero-cost session.

Cache savings are estimates against the applicable uncached rate, not a provider credit. Cache hit ratio is read tokens divided by read plus uncached input. Output speed uses observed API duration when available; wall-clock time includes idle work and is not interchangeable.

## Implementation and tests

- [Claude parser and per-turn accumulation](../../src/session.rs)
- [Claude rate and modifier functions](../../src/cost.rs)
- [Codex catalog and provenance](../../src/codex/model.rs)
- [Codex arithmetic](../../src/codex/cost.rs)
- [GUI aggregation](../../src-tauri/src/commands.rs)
- [Regression map](../../tests/index.md)
