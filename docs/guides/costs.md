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
           + cache creation (5m) × 5m write rate
           + cache creation (1h) × 1h write rate
           + cache read × read rate
           + output × output rate) / 1,000,000
```

The [cost owner](../../src/cost.rs) applies supported long-context and Fast modifiers per turn, then accumulates the four category costs. Mixed-speed sessions retain each turn's multiplier. `usage.service_tier` is not the same field as `usage.speed`.

### Cache write lifetime

Claude Code reports the split of each request's cache writes in `message.usage.cache_creation.ephemeral_5m_input_tokens` and `ephemeral_1h_input_tokens`. Pulse bills the 1-hour tokens at 2× the model's input rate and the rest at the 5-minute rate (1.25× input). A request without the split is all 5-minute writes. The 1-hour count is clamped to `cache_creation_input_tokens`, so token totals never change. The split lives only in the per-request arithmetic: the database stores the four category costs and needs no extra column.

| Request on Opus 5.5 | Arithmetic | Cost |
| --- | --- | ---: |
| 100,000 writes, all 1-hour | 100,000 × $8 / 1,000,000 | $0.80 |
| 100,000 writes, no split | 100,000 × $5 / 1,000,000 | $0.50 |
| 40,000 5-minute + 60,000 1-hour writes | (40,000 × $5 + 60,000 × $8) / 1,000,000 | $0.68 |

### Haiku 5.5 prompt tiers

Claude Haiku 5.5 has two rate rows, chosen for each request by its prompt: uncached input + cache creation + cache read. Up to and including 100,000 tokens the request bills at $0.10 input, $0.125 5-minute write, $0.20 1-hour write, $0.01 cache read and $0.50 output. Above 100,000 tokens every category of that request, output included, uses $0.50, $0.625, $1.00, $0.05 and $2.50. There is no other surcharge across its 1M window and no Fast mode.

| Request on Haiku 5.5 | Arithmetic | Cost |
| --- | --- | ---: |
| 50,000 input, 1,000 output (prompt 50,000) | 50,000 × $0.10 + 1,000 × $0.50 | $0.0055 |
| 100,000 input (prompt 100,000, low row) | 100,000 × $0.10 | $0.01 |
| 100,001 input (prompt 100,001, high row) | 100,001 × $0.50 | $0.0500005 |
| 20,000 input, 30,000 writes, 150,000 reads, 2,000 output (prompt 200,000) | 20,000 × $0.50 + 30,000 × $0.625 + 150,000 × $0.05 + 2,000 × $2.50 | $0.04125 |

Tiers apply per request, never to a session total. Two 60,000-token requests are two low-row requests ($0.012 for the input); one 120,000-token request costs $0.06. For the same reason, a history row rebuilt from session totals (when a row has no stored cost) is priced only when its total input is at most 100,000 tokens, where every request must have been in the low row. Above that, the row stays unpriced.

When Claude statusline data supplies `total_cost_usd`, Pulse uses it for the headline. It scales JSONL category proportions to reconcile the breakdown. Without statusline authority, costs remain estimates based on implemented rates and available telemetry. A turn that has no known model, because no earlier line named one, keeps its tokens but adds no cost. A session made only of such turns shows its cost as unavailable, never as a Sonnet 4 price.

Current prices, API limits and the cancelled Sonnet 5 price increase belong in the [Claude model reference](../models/claude.md), not a duplicated rate table here. That reference also covers Sonnet 5.5: it bills at the same rates as Sonnet 5 except cache reads ($0.10 instead of $0.20), with no long-context surcharge and no Fast multiplier, and Pulse classifies the two versions separately. For example, 1,000,000 input tokens plus 1,000,000 output tokens on Sonnet 5.5 cost $2 + $10 = $12, and 50,000 cache-read tokens cost 50,000 × $0.10 / 1,000,000 = $0.005.

## Codex

[Model resolution](../../src/codex/model.rs) normalizes known aliases and reads the [bundled catalog](../../src/codex/model_catalog.json). [Cost arithmetic](../../src/codex/cost.rs) resolves rates, explicit user overrides and completeness.

Codex input totals include cache reads and cache writes; known categories are subtracted once from ordinary input. Cached input is clamped to total input before uncached input is calculated. Missing cache-write telemetry stays missing. Do not reuse Claude's raw-token interpretation for Codex.

The bundled Astra, GPT-6.1 Sol, GPT-6.1 Terra, GPT-6 Sol and GPT-6 Luna entries record Fast at 2× and long-context rates of 2× input/cache and 1.5× output above 272,000 input tokens per request. GPT-5.6, GPT-5.5 and GPT-5.4 entries record the same long-context rates; GPT-5.6 entries still lack an API Fast multiplier, and their base rates match the published table of 2026-11-07. Aggregate input cannot prove each request's long-context price. The resolver preserves partial coverage when a required pricing condition is unresolved, and the history estimate of a model with a long-context threshold is withheld when a stored session total is above the threshold. [GPT-6 rates and gaps](../models/codex.md#gpt-61-sol-gpt-6-sol-and-gpt-6-luna) · [Current API facts and catalog gaps](../models/codex.md#bundled-catalog-gaps).

For example, 100,000 fresh input plus 1,000 output tokens in one GPT-6.1 Sol request cost 0.1 × $2 + 0.001 × $10 = $0.21 Standard and $0.42 Fast. With 300,000 fresh input instead, the request is above the threshold: 0.3 × $2 × 2 + 0.001 × $10 × 1.5 = $1.215.

API dollars and Codex subscription credits are separate units. A new API price does not establish a new credit conversion or account allowance.

Session cost is accumulated per telemetry delta with that sample's model and speed. Duplicate cumulative events do not add value. A large cumulative input total does not trigger request-level long-context pricing. An observed request over 272,000 input tokens on a model with a long-context threshold (the GPT-6 family, GPT-5.6, GPT-5.5 and GPT-5.4) applies the documented input/cache and output multipliers. Missing request boundaries keep the subtotal partial.

## Command Code

Pulse preserves the provider's `costUsd` values and marks incomplete message coverage as partial. SQLite records `commandcode_reported` separately from API-equivalent estimates. A reported amount is not proof of a settled invoice.

## Orion App

Orion records `cost: 0` on every message, which is not a price, so Pulse ignores it and calculates session cost the way it does for Claude Code: exact token counts times published per-model rates. It is a calculation, not an invoice. Orion's `tokens.input` already includes cache read and cache write; Pulse subtracts both once to get uncached input, then prices four categories (input, output, cache write, cache read) and sums them.

Each assistant message is priced as its own request, so long-context and Fast rules apply per request, and subagent messages roll into the parent session. Claude models use [`src/cost.rs`](../../src/cost.rs). Models in the [Codex catalog](../../src/codex/model_catalog.json) use [`src/codex/cost.rs`](../../src/codex/cost.rs), including Orion's `-fast` suffix and its catalog display names such as `5.6-Sol` and `6.1-Sol` (with or without the `GPT-` prefix). A display name that matches more than one catalog entry, such as `5.6-Cyber`, stays unpriced. For example, one million tokens in each category on Opus 5.5 cost $4 + $20 + $5 + $0.20 = $29.20.

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
