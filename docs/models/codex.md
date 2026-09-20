[Documentation](../index.md) / Models and context

# ![](../../assets/icons/brain.svg) Codex models & context

Compare the context Codex exposes with the maximum documented by the OpenAI API. Use this reference to interpret Pulse, not to assume that every account or client has the same limits.

**Verified:** September 5, 2026 · **Local inventory:** Codex 0.153.3 · **Pulse:** 1.8.1

[Context limits](#context-limits) · [Reasoning](#reasoning-and-speed) · [API pricing](#current-api-pricing) · [Catalog gaps](#bundled-catalog-gaps) · [Verification](#verify-your-installation)

> **Astra in this Codex inventory: 272,000 raw → 258,400 usable.**
> Its API documents 1,050,000 total tokens, up to 922,000 input and 128,000 output. The API maximum is not the default Codex budget.

## Context limits

### Codex: locally exposed capacity

These are the visible models in the [sanitized local snapshot](data/codex-context-snapshot.json). The usable column is calculated from the inventory, not from a completed maximum-size request.

| Model ID | Raw Codex tokens | Usable tokens at 95% |
| --- | ---: | ---: |
| `gpt-6-astra` | 272,000 | 258,400 |
| `gpt-5.6-sol` | 272,000 | 258,400 |
| `gpt-5.6-terra` | 272,000 | 258,400 |
| `gpt-5.6-luna` | 272,000 | 258,400 |
| `gpt-5.5` | 272,000 | 258,400 |
| `gpt-5.4-mini` | 272,000 | 258,400 |
| `gpt-5.3-codex-spark` | 128,000 | 121,600 |

The 95% factor belongs to this inventory. Do not apply it to every model, API endpoint or observed session value. A local rollout, selected profile or explicit configuration can change the active limit.

### API: documented maximum capacity

| Official model reference | Total context | Maximum input | Maximum output |
| --- | ---: | ---: | ---: |
| [GPT-6 Astra](https://developers.openai.com/api/docs/models/gpt-6-astra) | 1,050,000 | 922,000 | 128,000 |
| [GPT-5.6 Sol](https://developers.openai.com/api/docs/models/gpt-5.6-sol) | 1,050,000 | 922,000 | 128,000 |
| [GPT-5.6 Terra](https://developers.openai.com/api/docs/models/gpt-5.6-terra) | 1,050,000 | 922,000 | 128,000 |
| [GPT-5.6 Luna](https://developers.openai.com/api/docs/models/gpt-5.6-luna) | 1,050,000 | 922,000 | 128,000 |
| [GPT-5.5](https://developers.openai.com/api/docs/models/gpt-5.5) | 1,050,000 | Not specified on the fetched model card | 128,000 |
| [GPT-5.4 Mini](https://developers.openai.com/api/docs/models/gpt-5.4-mini) | 400,000 | 272,000 | 128,000 |

No public API maximum was verified for Spark in this refresh. Its 128,000-token Codex inventory entry is not proof of a public API endpoint.

Total context, input capacity and output cap are separate constraints. Reasoning and generated output need budget too. Do not label `total - output cap` as an officially documented input limit when the source does not state it.

### What Pulse displays

The [context resolver](../../src/codex/model.rs) uses:

1. Valid observed session JSONL capacity.
2. Matching local `models_cache.json` entry.
3. The bundled catalog entry.
4. Unavailable.

When observed capacity equals the inventory's usable value, Pulse retains the inventory's raw value and percentage with separate provenance. Otherwise, it preserves the observed value without guessing a reserve. It does not subtract another 5% from an already usable value.

Used context is not the same as cumulative session tokens. See [Context tracking](../guides/context.md) for current fill, compaction and provenance.

## Reasoning and speed

| Model family | Local Codex effort values | Public API effort values |
| --- | --- | --- |
| Astra | `low`, `medium`, `high`, `xhigh`, `max`, `ultra` | `low`, `medium`, `high`, `xhigh`, `max` |
| Sol / Terra | `low`, `medium`, `high`, `xhigh`, `max`, `ultra` | `none`, `low`, `medium`, `high`, `xhigh`, `max` |
| Luna | `low`, `medium`, `high`, `xhigh`, `max` | `none`, `low`, `medium`, `high`, `xhigh`, `max` |
| GPT-5.5 / GPT-5.4 Mini | `low`, `medium`, `high`, `xhigh` | `none`, `low`, `medium`, `high`, `xhigh` |
| Spark | `low`, `medium`, `high`, `xhigh` | Not verified |

Local values come from the snapshot; API values come from the linked model cards. `ultra` is a Codex harness value here, not a published Astra API effort. `gpt-5.6` aliases Sol. A subscription named Pro is not itself a model ID.

Speed is a separate field. Selecting Fast does not change the model identity, effort or context capacity. API Fast prices do not establish Codex subscription-credit conversion.

## Current API pricing

USD per million tokens, direct OpenAI API, short-context Standard requests. These are documentation facts, not a claim that the current Pulse binary uses them.

| Model | Input | Cache write | Cache read | Output |
| --- | ---: | ---: | ---: | ---: |
| Astra | $10.00 | $12.50 | $1.00 | $50.00 |
| Sol | $4.00 | $5.00 | $0.40 | $20.00 |
| Terra | $2.00 | $2.50 | $0.20 | $12.00 |
| Luna | $0.20 | $0.25 | $0.02 | $1.20 |

For these four models, the current pricing table lists long-context rates at 2× input/cache and 1.5× output. The model cards place the threshold above 272,000 input tokens. Fast is 2× the applicable Standard rates; Batch/Flex are 50%. Astra Fast is unavailable with EU data residency. Sol's promotional pricing lasts at least through November 21, 2026; that wording is not a guaranteed expiry date. [Official API pricing](https://developers.openai.com/api/docs/pricing).

For GPT-5.6 and later, prompt caching starts at 1,024 visible input tokens. Writes cost 1.25× uncached input; reads cost 0.1×. The default and only supported minimum TTL is `30m`, measured after the latest write or reuse. Earlier families have different eligibility and retention rules. [Prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching).

## Bundled catalog gaps

The [bundled JSON](../../src/codex/model_catalog.json) owns Pulse's fallback metadata and base rates. This documentation refresh does not change it, the shared core pin, configuration or stored analytics.

| Area | Current bundled behavior | Verified difference |
| --- | --- | --- |
| Astra fallback context | 1,050,000 raw at 100% | API-sized fallback; this local Codex inventory is 272,000 at 95% |
| GPT-5.6 fallback context | 372,000 raw / 353,400 usable | Local inventory now reports 272,000 / 258,400 |
| GPT-5.6 API context metadata | Missing | Current API cards publish 1,050,000 total / 922,000 input / 128,000 output |
| Sol base input/output | $5 / $30 | Current published rates: $4 / $20 |
| Terra base input/output | $2.50 / $15 | Current published rates: $2 / $12 |
| Luna base input/output | $1 / $6 | Current published rates: $0.20 / $1.20 |
| GPT-5.6 Fast multiplier | Unavailable | API pricing now publishes 2×; subscription credits remain separate |

The GPT-5.6 cache prices also differ. A complete runtime correction must update all token categories and preserve price dates. A fresh local model inventory corrects context fallback, not pricing. `exact` means complete according to the resolved rates; it does not prove that those rates are still current or that an API estimate equals a bill.

Older and restricted IDs remain recognizable in the bundled catalog. Recognition is not proof that they appear in the current picker or that the account can invoke them. Preserve unknown prices, credit conversions and access states as unavailable.

## Verify your installation

1. Record the Codex client version and the inventory's `fetched_at`.
2. Inspect only the selected model's context and capability fields in `models_cache.json` under `CODEX_HOME` (default `~/.codex`). Do not publish credentials or full transcripts.
3. Compare the active session's observed context with that inventory. Record whether a number is raw, usable or only a bundled fallback.
4. Check relevant profile overrides. `model_context_window` sets context capacity; `model_auto_compact_token_limit` controls compaction, not API entitlement. A larger configured number is not proof that a request is accepted. [Codex configuration reference](https://developers.openai.com/codex/config-reference).
5. Recheck the exact official model card and pricing tier. Preserve the verification date and any time-limited rate.
6. For an authorized runtime catalog change, update the canonical owner first and run `scripts/check-model-catalog-parity.ps1`. Follow the [core integration contract](../maintainers/codex-core.md); do not silently change the pinned core.

The snapshot documents exposed capacity. No maximum-size API request or account-wide availability test was run for this documentation refresh.

## Unreleased session-cost behavior

Per-event model/speed attribution and cache-write accounting now preserve useful session subtotals. Astra's observed request-level long-context multipliers were checked against its official model card on 2026-09-20. This focused correction does not refresh the older GPT-5.6 base-rate rows above. The immutable shared-core release pin stays unchanged; compositor validation uses an explicit local override.
