<script lang="ts">
  import type { SessionInfo } from "../lib/api";
  import { fmtTokens, fmtCost, fmtExactCost, fmtDuration, fmtTps, classifyActivity, fmtPromoEndDate } from "../lib/utils";
  import { slide } from "svelte/transition";

  let { session }: { session: SessionInfo } = $props();
  let expanded = $state(false);
  let modelLabel = $derived(session.model.replace(/ · (?:Minimal|Light|Low|Medium|High|Extra High|Max|Ultra)(?: · ⚡ Fast)?$/, "").replace(/ · ⚡ Fast$/, ""));
  let activityClass = $derived(classifyActivity(session.activity));

  let pureInput = $derived(Math.max(0, session.input_tokens - session.cache_write_tokens - session.cache_read_tokens));
  let tokenTotal = $derived(pureInput + session.output_tokens + session.cache_write_tokens + session.cache_read_tokens);
  let inputPct = $derived(tokenTotal > 0 ? (pureInput / tokenTotal) * 100 : 0);
  let outputPct = $derived(tokenTotal > 0 ? (session.output_tokens / tokenTotal) * 100 : 0);
  let cacheWPct = $derived(tokenTotal > 0 ? (session.cache_write_tokens / tokenTotal) * 100 : 0);
  let cacheRPct = $derived(tokenTotal > 0 ? (session.cache_read_tokens / tokenTotal) * 100 : 0);
  let cacheHitRatio = $derived(
    session.cache_read_tokens + pureInput > 0
      ? (session.cache_read_tokens / (session.cache_read_tokens + pureInput)) * 100
      : 0,
  );

  let isMythosClass = $derived(/(?:fable|mythos)/.test(session.model_id.toLowerCase()));
  let introPricingTitle = $derived(
    session.intro_pricing
      ? `Introductory pricing — $${session.intro_pricing.intro.input_per_million.toFixed(2)} / $${session.intro_pricing.intro.output_per_million.toFixed(2)} per MTok in/out through ${fmtPromoEndDate(session.intro_pricing.ends_at)}, then $${session.intro_pricing.regular.input_per_million.toFixed(2)} / $${session.intro_pricing.regular.output_per_million.toFixed(2)} automatically.`
      : "",
  );
</script>

<div
  class="session-card"
  class:idle={session.is_idle}
  class:expanded
  role="button"
  tabindex="0"
  onclick={() => (expanded = !expanded)}
  onkeydown={(e) => e.key === "Enter" && (expanded = !expanded)}
>
  <div class="session-header">
    <span class="session-project">{session.project}{#if session.session_name} <span class="session-name-hint">— {session.session_name}</span>{/if}</span>
    {#if session.branch}
      <span class="badge branch">{session.branch}</span>
    {/if}
    <span class="badge model" class:mythos-class={isMythosClass} title={session.model}>{modelLabel}{#if session.has_inflated_tokenizer}<span
          class="inflated-marker"
          title="Inflated tokenizer — this model can produce more tokens than its predecessor for the same text, raising cost at unchanged per-token rates."
        >⚠</span>{/if}</span>
    {#if session.intro_pricing}
      <span class="badge promo" title={introPricingTitle}>Intro Pricing</span>
    {/if}
    {#if session.fast}
      <span class="badge fast" title="Fast mode — priority speed billing on this turn">⚡ Fast</span>
    {/if}
    <span class="badge ctx" class:ctx-1m={session.context_window === "1M"}>{session.context_window}</span>
    <span
      class="badge effort"
      class:effort-implicit={!session.effort_explicit}
      title={session.effort_explicit ? "Reasoning effort reported by the session" : "Effort is not reported by the session"}
    >{session.effort_explicit ? "" : "~"}{session.effort}</span>
    {#if session.has_thinking}
      <span class="badge thinking">Thinking</span>
    {/if}
    {#if session.subagent_count > 0}
      <span class="badge subagent">{session.subagent_count} {session.subagent_count === 1 ? "agent" : "agents"}</span>
    {/if}
  </div>

  <div class="session-body">
    <div class="session-activity {activityClass}">
      <span class="activity-dot"></span>
      <span>{session.activity}</span>
      {#if session.activity_target}
        <span class="activity-target">{session.activity_target}</span>
      {/if}
    </div>

    <div class="session-stats">
      {#if session.duration_secs}
        <span class="stat">{fmtDuration(session.duration_secs)}</span>
      {/if}
      <span class="stat">{fmtTokens(session.tokens)}</span>
      {#if session.tokens_per_sec > 0}
        <span class="stat tps">{fmtTps(session.tokens_per_sec)}</span>
      {/if}
      <span class="stat cost">{fmtExactCost(session.cost, session.cost_available === true)}</span>
    </div>
  </div>

  {#if expanded}
    <div class="detail" transition:slide={{ duration: 200 }}>
      <div class="detail-section">
        <h4 class="detail-title">Token Breakdown</h4>
        <div class="token-bar">
          <div class="token-seg input" style="width:{inputPct}%" title="Input {fmtTokens(pureInput)}"></div>
          <div class="token-seg output" style="width:{outputPct}%" title="Output {fmtTokens(session.output_tokens)}"></div>
          <div class="token-seg cache-w" style="width:{cacheWPct}%" title="Cache Write {fmtTokens(session.cache_write_tokens)}"></div>
          <div class="token-seg cache-r" style="width:{cacheRPct}%" title="Cache Read {fmtTokens(session.cache_read_tokens)}"></div>
        </div>
        <div class="token-legend">
          <span class="legend-item"><span class="dot input"></span>Input {fmtTokens(pureInput)}</span>
          <span class="legend-item"><span class="dot output"></span>Output {fmtTokens(session.output_tokens)}</span>
          <span class="legend-item"><span class="dot cache-w"></span>Cache Write {fmtTokens(session.cache_write_tokens)}</span>
          <span class="legend-item"><span class="dot cache-r"></span>Cache Read {fmtTokens(session.cache_read_tokens)}</span>
        </div>
      </div>

      {#if session.cost_available === true}
      <div class="detail-section">
        <h4 class="detail-title">{session.cost_basis === "partial" ? "Known subtotal" : session.provider === "codex" ? "API-equivalent value" : "Reported value"}</h4>
        {#if session.cost_basis === "partial"}
          <p class="cost-unavailable">Partial estimate. Some usage components are not priced.</p>
        {/if}
        {#if session.opencode && session.cost_available === true}
          <span class="cost-unavailable">OpenCode-reported total: {fmtCost(session.cost)}. Token-category charges are not reported.</span>
        {:else if session.provider === "commandcode"}
          <span class="cost-unavailable">Command Code estimate: {fmtCost(session.cost)}. Token-category charges are not reported.</span>
        {:else if session.cost_available === true}
          <div class="cost-grid">
            <span class="cost-label">Input</span><span class="cost-val">{fmtCost(session.input_cost)}</span>
            <span class="cost-label">Output</span><span class="cost-val">{fmtCost(session.output_cost)}</span>
            <span class="cost-label">Cache Write</span><span class="cost-val">{fmtCost(session.cache_write_cost)}</span>
            <span class="cost-label">Cache Read</span><span class="cost-val">{fmtCost(session.cache_read_cost)}</span>
          </div>
        {:else}
          <span class="cost-unavailable">Monetary value is unavailable for this session.</span>
        {/if}
      </div>

      {/if}
      <div class="detail-section">
        <h4 class="detail-title">Session details</h4>
        <div class="perf-grid">
          <span class="perf-label">Output Speed</span><span class="perf-val">{session.tokens_per_sec > 0 ? fmtTps(session.tokens_per_sec) : "—"}</span>
          <span class="perf-label">Cache Hit Ratio</span><span class="perf-val">{cacheHitRatio.toFixed(1)}%</span>
          <span class="perf-label">Total Tokens</span><span class="perf-val">{fmtTokens(session.tokens)}</span>
          <span class="perf-label">Context Window</span><span class="perf-val">{session.context_window}</span>
          <span class="perf-label">Model ID</span><span class="perf-val mono-sm">{session.model_id || "—"}</span>
        </div>
      </div>
    </div>

    {#if session.subagents.length > 0}
      <div class="subagents-section">
        <h4 class="detail-title">Subagents ({session.subagents.length})</h4>
        <div class="subagent-list">
          {#each session.subagents as sa}
            <div class="subagent-row">
              <span class="sa-type">{sa.agent_type}</span>
              <span class="badge model sa-model">{sa.model}</span>
              <span class="sa-activity">{sa.activity}</span>
              <span class="sa-tokens">{fmtTokens(sa.tokens)}</span>
              <span class="sa-cost" title="Per-subagent cost provenance is unavailable">—</span>
            </div>
          {/each}
        </div>
      </div>
    {/if}
  {/if}
</div>

<style>
  .session-card {
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: 14px 18px;
    transition: all 0.2s var(--ease);
    cursor: pointer;
    user-select: none;
  }

  .session-card:hover {
    border-color: var(--border-hover);
    background: var(--bg-card-hover);
  }

  .session-card.idle { opacity: 0.55; }

  .session-header {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 10px;
    flex-wrap: wrap;
  }

  .session-project {
    font-weight: 700;
    font-size: 14px;
    color: var(--text-primary);
  }
  .session-name-hint {
    font-weight: 400;
    font-size: 11px;
    color: var(--text-muted);
  }

  .badge {
    font-size: 10px;
    padding: 2px 7px;
    border-radius: 99px;
    font-weight: 600;
    letter-spacing: 0.02em;
  }

  .badge.branch { color: var(--accent); background: var(--accent-dim); }
  .badge.model { color: var(--success); background: var(--success-dim); }
  .badge.model.mythos-class { color: var(--info); background: var(--info-dim); }
  .badge.effort { color: var(--text-muted); background: var(--bg-elevated); }
  .badge.effort.effort-implicit { opacity: 0.65; font-style: italic; cursor: help; }
  .badge.thinking { color: var(--token-cache-read); background: var(--token-cache-read-dim); }
  .badge.subagent { color: var(--info); background: var(--info-dim); }
  .badge.fast { color: var(--warning); background: var(--warning-dim); }
  .badge.promo { color: var(--success); background: var(--success-dim); cursor: help; }

  .inflated-marker {
    margin-left: 4px;
    color: var(--warning);
    cursor: help;
    font-weight: 700;
  }

  .session-body {
    min-width: 0;
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
  }

  .session-activity {
    min-width: 0;
    flex: 1;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    font-weight: 500;
  }

  .activity-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--text-muted);
    flex-shrink: 0;
  }

  .session-activity.thinking .activity-dot {
    background: var(--success);
    box-shadow: 0 0 6px var(--success-glow);
    animation: pulse-glow 2s ease-in-out infinite;
  }

  .session-activity.editing .activity-dot { background: var(--accent); }
  .session-activity.reading .activity-dot { background: var(--info); }
  .session-activity.running .activity-dot { background: var(--warning); }

  .activity-target {
    min-width: 0;
    overflow: hidden;
    color: var(--text-muted);
    font-size: 12px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .session-stats {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 14px;
  }

  .stat {
    font-size: 12px;
    color: var(--text-secondary);
    font-variant-numeric: tabular-nums;
  }

  .stat.tps {
    color: var(--info);
    font-weight: 600;
  }

  .stat.cost {
    font-weight: 700;
    font-size: 16px;
    color: var(--text-primary);
  }

  /* Expanded detail */
  .detail {
    margin-top: 14px;
    padding-top: 14px;
    border-top: 1px solid var(--border);
    display: flex;
    gap: 24px;
  }

  .detail-section {
    flex: 1;
    min-width: 0;
  }

  .detail-title {
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-muted);
    margin-bottom: 8px;
  }

  .token-bar {
    display: flex;
    height: 8px;
    border-radius: 99px;
    overflow: hidden;
    background: var(--bg-elevated);
    margin-bottom: 8px;
  }

  .token-seg { height: 100%; transition: width 0.3s var(--ease); }
  .token-seg.input { background: var(--info); }
  .token-seg.output { background: var(--token-output); }
  .token-seg.cache-w { background: var(--token-cache-write); }
  .token-seg.cache-r { background: var(--token-cache-read); }

  .token-legend {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    font-size: 11px;
    color: var(--text-secondary);
  }

  .legend-item { display: flex; align-items: center; gap: 4px; }
  .dot { width: 6px; height: 6px; border-radius: 50%; flex-shrink: 0; }
  .dot.input { background: var(--info); }
  .dot.output { background: var(--token-output); }
  .dot.cache-w { background: var(--token-cache-write); }
  .dot.cache-r { background: var(--token-cache-read); }

  .cost-grid, .perf-grid {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 12px;
    font-size: 12px;
  }

  .cost-label, .perf-label { color: var(--text-muted); }
  .cost-val, .perf-val { color: var(--text-primary); font-weight: 600; font-variant-numeric: tabular-nums; text-align: right; }

  .badge.ctx { color: var(--text-muted); background: var(--bg-elevated); }
  .badge.ctx-1m { color: var(--warning); background: var(--warning-dim); }

  .mono-sm { font-family: 'JetBrains Mono', 'Fira Code', monospace; font-size: 10px; }

  .subagents-section { margin-top: 12px; padding-top: 12px; border-top: 1px solid var(--border); }
  .subagent-list { display: flex; flex-direction: column; gap: 4px; }
  .subagent-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    border-radius: var(--radius-sm);
    background: var(--bg-elevated);
    font-size: 12px;
  }
  .sa-type { font-weight: 600; color: var(--text-primary); min-width: 80px; }
  .sa-model { font-size: 9px !important; }
  .sa-activity { flex: 1; color: var(--text-secondary); }
  .sa-tokens { color: var(--text-muted); font-variant-numeric: tabular-nums; min-width: 50px; text-align: right; }
  .sa-cost { font-weight: 700; color: var(--accent); font-variant-numeric: tabular-nums; min-width: 50px; text-align: right; }

  @media (max-width: 620px) {
    .session-body {
      align-items: flex-start;
      flex-direction: column;
      gap: 10px;
    }

    .session-activity {
      width: 100%;
    }

    .session-stats {
      width: 100%;
      flex-wrap: wrap;
      gap: 8px 14px;
    }

    .detail {
      flex-direction: column;
      gap: 16px;
    }

    .perf-val,
    .mono-sm {
      min-width: 0;
      overflow-wrap: anywhere;
    }

    .subagent-row {
      display: grid;
      grid-template-columns: minmax(0, 1fr) auto;
    }

    .sa-type,
    .sa-activity {
      min-width: 0;
      overflow-wrap: anywhere;
    }

    .sa-cost {
      grid-column: 1 / -1;
      min-width: 0;
      text-align: left;
    }
  }
  .detail { display:grid; grid-template-columns:repeat(auto-fit,minmax(220px,1fr)); gap:20px; padding:18px 0 4px; }
  .detail-section { padding:0 4px; }
  .detail-title { font-size:12px; text-transform:none; letter-spacing:0; color:var(--text-primary); }
  .token-legend { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:10px; }
  .cost-grid, .perf-grid { gap:9px 14px; line-height:1.5; }
  .perf-val { overflow-wrap:anywhere; }
</style>
