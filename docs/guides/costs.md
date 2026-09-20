[Documentation](../index.md) / Guides / Costs

# ![](../../assets/icons/scale.svg) Cost calculation

Understand what Pulse's monetary values measure. Provider-billed spend, API-equivalent estimates and OpenCode-reported value are not interchangeable.

[Claude](#claude-code) · [Codex](#codex) · [OpenCode](#opencode) · [Completeness](#completeness-and-freshness)

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

```text
turn input = uncached input + cache creation + cache read
turn cost = (uncached input × input rate
           + cache creation × write rate
           + cache read × read rate
           + output × output rate) / 1,000,000
```

The [cost owner](../../src/cost.rs) applies supported long-context and Fast modifiers per turn, then accumulates the four category costs. Mixed-speed sessions retain each turn's multiplier. `usage.service_tier` is not the same field as `usage.speed`.

When Claude statusline data supplies `total_cost_usd`, Pulse uses it for the headline. It scales JSONL category proportions to reconcile the breakdown. Without statusline authority, costs remain estimates based on implemented rates and available telemetry. Missing cache TTL is priced using the 5-minute write rate, not a demonstrated 1-hour rate.

Current prices, API limits and the cancelled Sonnet 5 price increase belong in the [Claude model reference](../models/claude.md), not a duplicated rate table here.

## Codex

[Model resolution](../../src/codex/model.rs) normalizes known aliases and reads the [bundled catalog](../../src/codex/model_catalog.json). [Cost arithmetic](../../src/codex/cost.rs) resolves rates, explicit user overrides and completeness.

Codex input totals include cache reads and cache writes; known categories are subtracted once from ordinary input. Cached input is clamped to total input before uncached input is calculated. Missing cache-write telemetry stays missing. Do not reuse Claude's raw-token interpretation for Codex.

The bundled Astra entry records Fast at 2×. GPT-5.6 entries still lack a Fast multiplier and use older base rates. Aggregate input cannot prove each request's long-context price. The resolver preserves partial coverage when a required pricing condition is unresolved. [Current API facts and catalog gaps](../models/codex.md#bundled-catalog-gaps).

API dollars and Codex subscription credits are separate units. A new API price does not establish a new credit conversion or account allowance.

Session cost is accumulated per telemetry delta with that sample's model and speed. Duplicate cumulative events do not add value. A large cumulative input total does not trigger request-level long-context pricing. An observed Astra request over 272,000 input tokens applies the documented input/cache and output multipliers. Missing request boundaries keep the subtotal partial.

## Command Code

Pulse preserves the provider's `costUsd` values and marks incomplete message coverage as partial. SQLite records `commandcode_reported` separately from API-equivalent estimates. A reported amount is not proof of a settled invoice.

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
