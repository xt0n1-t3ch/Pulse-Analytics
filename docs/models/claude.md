[Documentation](../index.md) / Models / Claude

# ![](../../assets/icons/brain.svg) Claude models & context

Compare Claude API limits, Claude Code behavior and Pulse's implemented estimates. This is the Claude counterpart to the [Codex model guide](codex.md); neither provider inherits the other's capacity or prices.

**Verified:** September 5, 2026 · **Installed Claude Code:** 2.1.251 · **Pulse:** 1.8.1 · **Opus 5.5 rows verified:** September 22, 2026 against the official models overview and pricing pages

[Models](#current-models) · [Claude Code capacity](#claude-code-capacity) · [Prices](#current-api-prices) · [Implementation gaps](#pulse-implementation-gaps) · [Verification](#verify-an-installation)

> **Three different facts:** the API's maximum, the session's available window and its auto-compaction threshold. Claude does not use the 95% Codex inventory rule.

## Current models

| Model | Claude API ID | API context | Maximum output |
| --- | --- | ---: | ---: |
| Fable 5.1 | `claude-fable-5-1` | 1,000,000 | 128,000 |
| Mythos 5.1, restricted access | `claude-mythos-5-1` | 1,000,000 | 128,000 |
| Opus 5.5 | `claude-opus-5-5` | 1,000,000 | 128,000 |
| Opus 5, legacy | `claude-opus-5` | 1,000,000 | 128,000 |
| Sonnet 5 | `claude-sonnet-5` | 1,000,000 | 128,000 |
| Haiku 4.5 | `claude-haiku-4-5-20251001` | 200,000 | 64,000 |

The current general lineup is Fable 5.1, Opus 5.5, Sonnet 5 and Haiku 4.5. Opus 5 remains available as a legacy model. Mythos 5.1 shares Fable 5.1's specifications but requires invitation through Project Glasswing. Model recognition in Pulse does not establish account access. [Models overview](https://platform.claude.com/docs/en/models/overview), [Fable/Mythos 5.1 reference](https://platform.claude.com/docs/en/models/fable-5-1/overview), [Opus 5.5 reference](https://platform.claude.com/docs/en/models/opus-5-5/overview).

Fable 5, Mythos 5, Opus 4.6/4.7/4.8 and Sonnet 4.6 also have a documented 1M API window and 128K output cap. API context includes the conversation and generated output; 128K is an output ceiling, not extra capacity. Do not invent an independently guaranteed input maximum by subtracting two headline values. Use the Models API's `max_input_tokens` and `max_tokens` for the selected deployment when available. [API context limits](https://platform.claude.com/docs/en/build-with-claude/context-windows#context-window-sizes-by-model), [Models API](https://platform.claude.com/docs/en/api/models/list).

## Claude Code capacity

| Situation | Available window / compaction behavior |
| --- | --- |
| Fable 5.1 / Fable 5, direct Anthropic route | Native 1M window; account/model access still applies |
| Sonnet 5, direct Anthropic route | 1M window; default compaction around 967K |
| Opus on Max, Team or Enterprise | 1M included under the documented plan rules |
| Opus on Pro | 1M requires usage credits |
| Sonnet 4.6 on a subscription | 1M requires usage credits, including Max |
| API/pay-as-you-go with a supported model | Full documented API access; request limits still apply |
| `CLAUDE_CODE_DISABLE_1M_CONTEXT=1` | Native-1M sessions are held to 200K |
| Gateway or partner deployment | Confirm model routing and window; do not assume direct-route defaults |

An explicit auto-compaction setting can lower the working threshold. `/autocompact`, `--autocompact` and `CLAUDE_CODE_AUTO_COMPACT_WINDOW` have distinct scope and precedence. A larger setting cannot create provider capacity. [Claude Code model configuration](https://code.claude.com/docs/en/model-config#context-window-and-auto-compaction).

The approximately 967K Sonnet threshold is not a universal 96.7% rule. No active Claude statusline capacity snapshot was available during this refresh, and no maximum-size request was run. These Claude Code limits are sourced configuration behavior, not a measurement of every local model.

## Effort, speed and caching

Fable 5.1/5, Opus 5/4.8/4.7 and Sonnet 5 expose `low`, `medium`, `high`, `xhigh` and `max` in the current Claude Code guide. The API also accepts all five levels for Opus 5.5, whose API default is `medium` instead of `high`; its thinking is always on. Pulse reads effort from the transcript or `settings.json` and does not infer a per-model default. Older Opus/Sonnet 4.6 omit `xhigh`. Haiku does not use that effort selector. `ultracode` is a Claude Code orchestration setting, not another model effort value. [Effort configuration](https://code.claude.com/docs/en/model-config#adjust-effort-level).

Fast processing is separate from reasoning effort and subscription plan. The current API offers Fast for Opus 5.5, Opus 5 and Opus 4.8, not Fable, Sonnet or Haiku. Cache lifetime and cache-read price are also separate capabilities. [API Fast and cache pricing](https://platform.claude.com/docs/en/about-claude/pricing#feature-specific-pricing).

## Current API prices

USD per million tokens, direct Claude API, Standard processing. Subscription allowances and partner invoices are separate.

| Model | Input | Write 5m | Write 1h | Cache read | Output |
| --- | ---: | ---: | ---: | ---: | ---: |
| Fable 5.1 / Mythos 5.1 | $10.00 | $12.50 | $20.00 | $0.25 | $50.00 |
| Opus 5.5 | $4.00 | $5.00 | $8.00 | $0.20 | $20.00 |
| Opus 5 | $5.00 | $6.25 | $10.00 | $0.50 | $25.00 |
| Sonnet 5 | $2.00 | $2.50 | $4.00 | $0.20 | $10.00 |
| Haiku 4.5 | $1.00 | $1.25 | $2.00 | $0.10 | $5.00 |

Sonnet 5's launch price is now its standard price. Anthropic cancelled the planned September 1 increase to $3/$15. Fable/Mythos 5.1 cache reads cost $0.25, compared with $1 for version 5. Opus 5.5 cache reads cost 0.05x input ($0.20), not the standard 0.10x. Opus 5.5 Fast input/output cost $8/$40; Opus 5/4.8 Fast input/output cost $10/$50. Cache modifiers apply to Fast rates. [Official pricing](https://platform.claude.com/docs/en/about-claude/pricing#model-pricing).

## Pulse implementation gaps

The [Claude cost owner](../../src/cost.rs) and [session parser](../../src/session.rs) are separate from Codex's JSON catalog. This guide does not change runtime arithmetic or reprice history.

| Area | Current Pulse behavior | Verified limitation |
| --- | --- | --- |
| Sonnet 5 | Permanent $2/$10, $2.50 write and $0.20 read | Corrected in 1.9.5; earlier builds switched to $3/$15 after September 1, 2026 |
| Fable/Mythos 5.1 cache | Version 5.1 and later read at $0.25; version 5 keeps $1 | Corrected in 1.9.5; earlier builds used $1 for every version |
| Opus 5.5 | Version-specific $4/$20 rates, $5 write and $0.20 read; Fast at 2x | Implemented September 22, 2026; `claude-opus-5-5` no longer inherits Opus 5 rates |
| Context | Model/suffix/peak heuristic selects 1M or 200K | Does not prove the active plan, gateway or compaction setting |
| Cache writes | JSONL estimates use the 5-minute write rate | A missing TTL cannot establish the 1-hour rate |
| Unknown Claude model | Existing pricing fallback is Sonnet-like | Do not mistake fallback recognition for an authoritative rate |
| Tokenizer warning | Implemented family-specific display flag | Not a token multiplier and not proof of every newer model's tokenizer |

A valid Claude statusline `total_cost_usd` remains the authoritative headline when present. JSONL categories are reconciled to that value. Without it, price freshness and supported telemetry constrain the estimate. See [cost arithmetic](../guides/costs.md) and [current fill](../guides/context.md).

### Maintainer notes retained from the older guides

- `model_pricing(model_id)` has no date logic. The cancelled Sonnet 5 increase and its "Intro Pricing" badge were removed in 1.9.5.
- `is_ga_1m_context()` and `supports_1m_context()` handle recognized Fable/Mythos and Sonnet 5 families. A `[1m]` suffix is normalized; it does not grant account access.
- `ReasoningEffort::from_api` accepts the documented aliases for Extra High and Max. Detection and display are independent of API model availability.
- Per-turn `usage.speed` drives the Fast multiplier only for supported Opus versions. `service_tier` is separate. Mixed Standard/Fast turns accumulate independently.
- Tests cover version-specific rates, family parsing, context classification, speed and cost breakdown. Passing those tests proves implemented behavior, not current provider pricing. See the [test map](../../tests/index.md).

## Verify an installation

1. Record Claude Code and Pulse versions and the exact model ID.
2. Check `/model`, `/status`, route, plan and context controls without publishing credentials.
3. Read the statusline's capacity when available. Compare current fill and compaction evidence with that capacity, not with accumulated session tokens.
4. Recheck the official model and pricing pages for the selected version, route and speed.
5. Label unobserved access and capacity as unverified. Correct runtime rates through focused tests before calling estimates current.
