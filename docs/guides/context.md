[Documentation](../index.md) / Models and analytics / Context

# ![](../../assets/icons/scale.svg) Context tracking

Read the current context meter without confusing model capacity, accumulated tokens or a pre-compaction peak. Pulse keeps those measurements separate for each provider.

[The four numbers](#the-four-numbers) · [Codex](#codex) · [Claude Code](#claude-code) · [OpenCode](#opencode) · [Troubleshooting](#when-the-numbers-differ)

## The four numbers

| Number | Meaning | Not the same as |
| --- | --- | --- |
| API context maximum | Provider-documented capacity for that model and endpoint | The budget exposed by Codex |
| Usable session window | Capacity reported or resolved for the active session | Remaining free space |
| Current fill | Tokens in the most recent context measurement | All tokens used during the session |
| Session total / peak | Accumulated usage, or the largest single input seen | Current fill after compaction |

> **Example:** a 272,000-token Codex inventory with a 95% factor yields 258,400 usable tokens. If current fill is 100,000, the remaining budget is 158,400 tokens. Do not subtract another 5% from the usable value.

## Codex

The [model guide](../models/codex.md#context-limits) owns the dated model tables. Astra's API maximum and its locally exposed Codex window are deliberately separate there.

Pulse resolves valid observed JSONL capacity before the local model cache and bundled catalog. If the observed capacity matches the inventory's effective value, it keeps the raw capacity and percentage from that inventory. Otherwise, it retains the observed capacity without inventing a raw/usable split. [Resolver implementation](../../src/codex/model.rs).

`ContextWindowSnapshot` supplies the active capacity and usage to the GUI. `context_window_tokens` is the denominator; current context usage is the numerator. Cumulative session usage is not that numerator. [GUI mapping](../../src-tauri/src/commands.rs).

Codex has distinct configuration keys for context capacity and automatic compaction. `model_context_window` does not prove provider access to a larger window, and `model_auto_compact_token_limit` is a trigger rather than a model specification. [Configuration reference](https://developers.openai.com/codex/config-reference).

## Claude Code

The [Claude model guide](../models/claude.md#claude-code-capacity) separates native API capacity, Claude Code availability and compaction thresholds. The Sonnet 5.5 and Sonnet 5 default of about 967K is not a reserve rule for every Claude model. Both Sonnet versions always run with a 1M window on the direct Anthropic route, so Pulse selects 1,000,000 for them without a `[1m]` suffix.

The [JSONL parser](../../src/session.rs) maintains two fields:

| Field | Purpose | Across compaction |
| --- | --- | --- |
| `current_context_tokens` | Latest total input: uncached input + cache creation + cache read | Resets to valid `postTokens`, then follows later turns |
| `max_turn_api_input` | Largest total input in any observed turn | Keeps the historical peak |

The current meter uses `current_context_tokens`. Pulse's capacity heuristic uses the model's GA context flag, a `[1m]` suffix or a historical input above 200,000 to select 1,000,000 instead of 200,000. This is the current implementation, not a universal claim about every Claude account. [Capacity selection](../../src-tauri/src/commands.rs).

### Compaction example

```json
{
  "type": "system",
  "subtype": "compact_boundary",
  "compactMetadata": {
    "trigger": "manual",
    "preTokens": 959012,
    "postTokens": 25500
  }
}
```

This illustrative event sets current fill to 25,500. It does not reduce the lifetime peak. Missing or malformed compaction metadata leaves the previous current value intact. Before the v1.4.1 correction, current-state surfaces incorrectly used the peak and could remain full after compaction.

## OpenCode

Pulse reads context from the message or provider/model metadata. It does not assign a Codex or Claude window to an OpenCode model, and cumulative session tokens do not become occupied context. Missing capacity remains unavailable. A completed session stays in history but is not a live presence candidate. [OpenCode integration](opencode.md#interpretar-los-datos).

## When the numbers differ

| Symptom | Check first |
| --- | --- |
| API card says 1.05M; Codex shows 258.4K | Different capacity owners; compare local inventory and session evidence |
| Old docs say 353.4K for GPT-5.6 | Inventory version and verification date; see the model guide's catalog gaps |
| Fill drops but total usage grows | Compaction changes current fill, not accumulated work |
| Historical peak stays high | Expected; the peak is not the live meter |
| Home and Costs show different money totals | Provider, time range, live/history scope and monetary provenance |
| Model capacity is absent | Keep unavailable; do not borrow another model's window |

When reporting a discrepancy, include the Pulse/Codex versions, model ID, source, timestamp, raw/usable capacity and current fill. Do not attach private prompts, credentials or an entire transcript.
