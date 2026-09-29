<script lang="ts">
  import { onMount } from "svelte";
  import StatCard from "../components/StatCard.svelte";
  import SessionCard from "../components/SessionCard.svelte";
  import { sessions, selectedAnalyticsProviderScope } from "../lib/stores";
  import { providerMatchesAnalyticsScope } from "../lib/access";
  import { fmtTokens, fmtCost, fmtExactCost, fmtDuration, fmtTps, classifyActivity, fmtClock, monetaryValueLabel, CALCULATED_COST_NOTE, isCalculatedCost } from "../lib/utils";
  import { getSessionHistory, getAnalyticsSummary, searchSessions, getSessionHistoryFiltered } from "../lib/api";
  import type { HistoricalSession, AnalyticsSummary } from "../lib/api";
  import { fly } from "svelte/transition";
  import ExportModal from "../components/ExportModal.svelte";
  import Select from "../components/Select.svelte";
  import SegmentedControl from "../components/SegmentedControl.svelte";
  import type { ExportColumn } from "../lib/export";

  let showExport = $state(false);
  let expandedId = $state<string | null>(null);
  let compareMode = $state(false);
  let compareIds = $state<Set<string>>(new Set());

  const historyColumns: ExportColumn[] = [
    { key: "project", label: "Project", enabled: true },
    { key: "model", label: "Model", enabled: true },
    { key: "context_window", label: "Context", enabled: true },
    { key: "branch", label: "Branch", enabled: true },
    { key: "total_tokens", label: "Tokens", enabled: true },
    { key: "input_tokens", label: "Input Tokens", enabled: true },
    { key: "output_tokens", label: "Output Tokens", enabled: true },
    { key: "cache_write_tokens", label: "Cache Write", enabled: true },
    { key: "cache_read_tokens", label: "Cache Read", enabled: true },
    { key: "duration_secs", label: "Duration (s)", enabled: true },
    { key: "total_cost", label: "Known Monetary Value (USD)", enabled: true },
    { key: "cost_basis", label: "Monetary Value Basis", enabled: true },
    { key: "cost_source", label: "Monetary Value Source", enabled: true },
    { key: "started_at", label: "Started", enabled: true },
    { key: "ended_at", label: "Ended", enabled: false },
    { key: "effort", label: "Effort", enabled: false },
    { key: "is_active", label: "Active", enabled: false },
  ];

  let sortBy = $state("recent");
  let projectFilter = $state("");
  const sortOptions = [
    { value: "recent", label: "Recent" },
    { value: "cost", label: "Monetary value" },
    { value: "tokens", label: "Tokens" },
    { value: "duration", label: "Duration" },
    { value: "tps", label: "Throughput" },
    { value: "project", label: "Project" },
  ];
  const historyRangeOptions = [
    { value: "1", label: "Today" },
    { value: "7", label: "Last 7 days" },
    { value: "30", label: "Last 30 days" },
    { value: "90", label: "Last 90 days" },
    { value: "365", label: "Last year" },
  ];

  let liveSessions = $derived(
    $sessions.filter(
      (session) =>
        !session.is_idle
        && providerMatchesAnalyticsScope(session.provider, $selectedAnalyticsProviderScope),
    ),
  );

  let filtered = $derived.by(() => {
    let list = projectFilter
      ? liveSessions.filter((s) => s.project === projectFilter)
      : liveSessions;
    return [...list].sort((a, b) => {
      if (sortBy === "recent") return Date.parse(b.started_at ?? "") - Date.parse(a.started_at ?? "");
      if (sortBy === "cost") {
        const left = a.cost_available === true ? a.cost : -1;
        const right = b.cost_available === true ? b.cost : -1;
        return right - left;
      }
      if (sortBy === "tokens") return b.tokens - a.tokens;
      if (sortBy === "duration") return b.duration_secs - a.duration_secs;
      if (sortBy === "tps") return b.tokens_per_sec - a.tokens_per_sec;
      return a.project.localeCompare(b.project);
    });
  });

  let totalTokens = $derived(filtered.reduce((s, x) => s + x.tokens, 0));
  let totalCost = $derived(filtered.reduce((s, x) => s + (x.cost_available === true ? x.cost : 0), 0));
  let totalCostAvailable = $derived(filtered.length === 0 || filtered.every((session) => session.cost_available === true));
  let totalCostLabel = $derived(filtered.length > 0 && filtered.every((session) => isCalculatedCost(session)) ? "Session cost" : "Reported value");
  let avgTps = $derived(filtered.length ? filtered.reduce((s, x) => s + x.tokens_per_sec, 0) / filtered.length : 0);
  let totalInput = $derived(filtered.reduce((s, x) => s + Math.max(0, x.input_tokens - x.cache_write_tokens - x.cache_read_tokens), 0));
  let totalOutput = $derived(filtered.reduce((s, x) => s + x.output_tokens, 0));
  let totalCacheW = $derived(filtered.reduce((s, x) => s + x.cache_write_tokens, 0));
  let totalCacheR = $derived(filtered.reduce((s, x) => s + x.cache_read_tokens, 0));
  let totalAll = $derived(totalInput + totalOutput + totalCacheW + totalCacheR);

  let activityCounts = $derived.by(() => {
    const counts: Record<string, number> = {};
    filtered.forEach((s) => {
      const a = classifyActivity(s.activity);
      counts[a] = (counts[a] || 0) + 1;
    });
    return Object.entries(counts).sort((a, b) => b[1] - a[1]);
  });

  let modelCounts = $derived.by(() => {
    const counts: Record<string, number> = {};
    filtered.forEach((s) => { counts[s.model] = (counts[s.model] || 0) + 1; });
    return Object.entries(counts).sort((a, b) => b[1] - a[1]);
  });

  let history = $state<HistoricalSession[]>([]);
  let knownProjects = $state<string[]>([]);
  let summary = $state<AnalyticsSummary | null>(null);
  let historyLoading = $state(true);
  let historyError = $state<string | null>(null);
  let historyRequest = 0;
  const HISTORY_PAGE_SIZE = 50;
  let visibleHistoryLimit = $state(HISTORY_PAGE_SIZE);
  let historyDays = $state(7);
  let searchQuery = $state("");
  // granular filters
  let fromDate = $state("");
  let toDate = $state("");
  let minCost = $state<number | null>(null);
  let modelFilter = $state("");

  let exportRows = $derived(
    history.map((h) => ({
      ...h,
      total_cost: h.known_cost,
    } as Record<string, unknown>)),
  );
  let projects = $derived(
    [...new Set([...liveSessions.map((s) => s.project), ...knownProjects])].sort(),
  );
  let projectOptions = $derived([
    { value: "", label: "All projects" },
    ...projects.map((project) => ({ value: project, label: project })),
  ]);

  let compareList = $derived(history.filter((h) => compareIds.has(h.id)));
  let visibleHistory = $derived([...history].sort((a,b) => {
    if (sortBy === "cost") return (b.known_cost ?? -1) - (a.known_cost ?? -1);
    if (sortBy === "tokens") return b.total_tokens - a.total_tokens;
    if (sortBy === "duration") return b.duration_secs - a.duration_secs;
    if (sortBy === "tps") return b.output_tokens / Math.max(b.duration_secs,1) - a.output_tokens / Math.max(a.duration_secs,1);
    if (sortBy === "project") return a.project.localeCompare(b.project);
    return Date.parse(b.started_at ?? "") - Date.parse(a.started_at ?? "");
  }).slice(0, visibleHistoryLimit));

  let backgroundRefreshing = false;
  async function loadHistory(background = false): Promise<void> {
    if (background && (historyLoading || backgroundRefreshing)) return;
    backgroundRefreshing = true;
    const request = ++historyRequest;
    const provider = $selectedAnalyticsProviderScope;
    if (!background) { historyLoading = true; showExport = false; compareIds = new Set(); }
    historyError = null;
    try {
      const useAdvanced = fromDate || toDate || minCost !== null || modelFilter || historyDays === 1;
      const [nextSummary, nextHistory] = await Promise.all([
        getAnalyticsSummary(provider),
        searchQuery.trim() ? searchSessions(searchQuery, 500, provider) : useAdvanced
          ? getSessionHistoryFiltered({
              // Compose the selected date-range window with the advanced
              // filters: without an explicit From, fall back to the window
              // dropdown (Today/7/30/90/365) so e.g. "Last 7 days" + a Model
              // filter still honors the 7-day window instead of searching all
              // time. An explicit From always wins.
              from_iso: fromDate
                ? new Date(fromDate).toISOString()
                : historyDays === 1 ? new Date(new Date().setHours(0,0,0,0)).toISOString() : historyDays > 0
                  ? new Date(Date.now() - historyDays * 86_400_000).toISOString()
                  : null,
              to_iso: toDate ? new Date(toDate + "T23:59:59").toISOString() : null,
              project: projectFilter || null,
              model: modelFilter || null,
              min_cost: minCost,
              limit: 500,
              provider,
            })
          : getSessionHistory(historyDays, projectFilter || undefined, 200, provider),
      ]);
      if (request !== historyRequest) return;
      summary = nextSummary;
      const lower = fromDate ? Date.parse(fromDate + "T00:00:00") : historyDays === 1 ? new Date().setHours(0,0,0,0) : Date.now() - historyDays * 86_400_000;
      const upper = toDate ? Date.parse(toDate + "T23:59:59") : Infinity;
      history = searchQuery.trim() ? nextHistory.filter((row) => {
        const time = Date.parse(row.started_at ?? "");
        return time >= lower && time <= upper && (!projectFilter || row.project === projectFilter)
          && (!modelFilter || `${row.model} ${row.model_id}`.toLowerCase().includes(modelFilter.toLowerCase()))
          && (minCost === null || (row.known_cost !== null && row.known_cost >= minCost));
      }) : nextHistory;
      if (!background) visibleHistoryLimit = HISTORY_PAGE_SIZE;
      knownProjects = [...new Set([...knownProjects,...nextHistory.map((session) => session.project)])].sort();
    } catch (error) {
      if (request !== historyRequest) return;
      historyError = error instanceof Error && error.message
        ? `Session history unavailable. ${error.message}`
        : "Session history unavailable. Pulse could not load your saved sessions.";
    } finally {
      if (request === historyRequest) { historyLoading = false; backgroundRefreshing = false; }
    }
  }

  function resetFilters(): void {
    fromDate = "";
    toDate = "";
    minCost = null;
    modelFilter = "";
    loadHistory();
  }

  async function doSearch(): Promise<void> { await loadHistory(); }

  function toggleExpand(id: string): void {
    expandedId = expandedId === id ? null : id;
  }

  function toggleCompare(id: string): void {
    const next = new Set(compareIds);
    if (next.has(id)) next.delete(id);
    else if (next.size < 3) next.add(id);
    compareIds = next;
  }

  onMount(() => {
    void loadHistory();
    const timer = setInterval(() => { if (document.visibilityState !== "hidden") void loadHistory(true); }, 5000);
    return () => clearInterval(timer);
  });
  let previousProviderScope: string | undefined;
  $effect(() => {
    const provider = $selectedAnalyticsProviderScope;
    if (previousProviderScope !== undefined && provider !== previousProviderScope) {
      expandedId = null;
      compareIds = new Set();
      knownProjects = [];
      void (searchQuery.trim() ? doSearch() : loadHistory());
    }
    previousProviderScope = provider;
  });
</script>

<div class="sessions-view app-view">
  <div class="view-header">
    <div class="title-line">
      <h2 class="view-title">Sessions</h2>
    </div>
    <div class="filters">
      <div class="filter-control project-control">
        <span class="control-label">Project</span>
        <Select
          value={projectFilter}
          options={projectOptions}
          variant="inline"
          ariaLabel="Filter by project"
          onchange={(value) => { projectFilter = value; void loadHistory(); }}
        />
      </div>
      <div class="filter-control sort-control">
        <span class="control-label">Sort</span>
        <SegmentedControl options={sortOptions} value={sortBy} ariaLabel="Sort sessions" size="sm" onchange={(value) => (sortBy = value)} />
      </div>
    </div>
  </div>

  <div class="stats-row metric-strip">
    <StatCard label="Active sessions" value={String(filtered.length)} />
    <StatCard label="Live tokens" value={filtered.length > 0 ? fmtTokens(totalTokens) : "—"} />
    {#if filtered.length > 0 && totalCostAvailable}<StatCard label={totalCostLabel} value={fmtCost(totalCost)} />{/if}
    {#if filtered.some((session) => session.tokens_per_sec > 0)}<StatCard label="Output rate" value={fmtTps(avgTps)} />{:else}<StatCard label="Output tokens" value={fmtTokens(totalOutput)} />{/if}
  </div>

  <div class="session-list">
    {#if filtered.length === 0}
      <div class="empty-state state-panel">
        <div class="empty-text">No sessions live right now</div>
        <span class="empty-sub">Telemetry fills in while you work. Full history stays below.</span>
      </div>
    {:else}
      {#each filtered as session (session.session_id)}
        <div in:fly={{ y: 12, duration: 200 }}>
          <SessionCard {session} />
        </div>
      {/each}
    {/if}
  </div>

  <div class="card surface-matte">
    <div class="card-title-row">
      <h3 class="card-title">Session history</h3>
      <div class="title-actions">
        <button class="action-btn" class:active={compareMode} onclick={() => { compareMode = !compareMode; compareIds = new Set(); }}>
          {compareMode ? "Exit Compare" : "Compare"}
        </button>
        {#if history.length > 0}
          <button class="action-btn" onclick={() => showExport = true}>Export CSV</button>
        {/if}
      </div>
    </div>
    <div class="history-controls">
      <div class="history-toolbar">
        <SegmentedControl
          options={historyRangeOptions}
          value={String(historyDays)}
          ariaLabel="History range"
          size="sm"
          onchange={(value) => { historyDays = Number(value); void loadHistory(); }}
        />
        <label class="flt">
          <span class="flt-lbl">From</span>
          <input type="date" bind:value={fromDate} onchange={() => loadHistory()} />
        </label>
        <label class="flt">
          <span class="flt-lbl">To</span>
          <input type="date" bind:value={toDate} onchange={() => loadHistory()} />
        </label>
        <label class="flt compact">
          <span class="flt-lbl">Min cost</span>
          <input type="number" min="0" step="0.01" placeholder="0.00" bind:value={minCost} onchange={() => loadHistory()} />
        </label>
        <label class="flt model-filter">
          <span class="flt-lbl">Model</span>
          <input type="text" placeholder="opus / sonnet" bind:value={modelFilter} onchange={() => loadHistory()} />
        </label>
        {#if fromDate || toDate || minCost !== null || modelFilter}
          <button class="btn btn-ghost reset-btn" onclick={resetFilters}>Reset</button>
        {/if}
      </div>
      <div class="history-search-toolbar">
        <label class="search-box">
          <span class="sr-only">Search session history</span>
          <input type="text" placeholder="Search sessions" bind:value={searchQuery} onkeydown={(e) => e.key === "Enter" && doSearch()} />
        </label>
        {#if summary}
          <div class="history-summary">
            All-time: <strong>{summary.total_sessions}</strong> sessions ·
            <strong>{summary.cost_basis === "unavailable" ? "—" : fmtCost(summary.total_cost)}</strong>
            {monetaryValueLabel(summary.cost_sources)}{summary.cost_basis === "partial" ? " · known subtotal" : ""} ·
            <strong>{fmtTokens(summary.total_tokens)}</strong> tokens · top contributor
            <strong>{summary.top_project}</strong> · <strong>{summary.days_tracked}</strong> days
          </div>
        {/if}
      </div>
    </div>

    {#if historyError}
      <section class="history-state error" role="alert">
        <strong>{history.length > 0 ? "Showing the last verified history" : historyError.split(".")[0]}</strong>
        <span>{historyError.split(".").slice(1).join(".").trim()}</span>
        <button type="button" onclick={() => void loadHistory()}>Retry</button>
      </section>
    {:else if historyLoading}
      <section class="history-state" role="status">
        <strong>{history.length > 0 ? "Refreshing session history" : "Loading session history"}</strong>
        <span>Reading the selected provider and time window.</span>
      </section>
    {/if}

    {#if compareMode && compareList.length >= 2}
      <div class="compare-panel">
        <h4 class="compare-title">Comparison · {compareList.length} sessions</h4>
        <div class="compare-grid" style="--compare-cols:{compareList.length}">
          <div class="compare-label"></div>
          {#each compareList as c}<div class="compare-head">{c.project}</div>{/each}
          <div class="compare-label">Model</div>
          {#each compareList as c}<div class="compare-cell">{c.model}</div>{/each}
          <div class="compare-label">Tokens</div>
          {#each compareList as c}<div class="compare-cell">{fmtTokens(c.total_tokens)}</div>{/each}
          <div class="compare-label">Monetary value</div>
          {#each compareList as c}
            <div class="compare-cell accent">
              {c.known_cost === null
                ? "Unavailable"
                : c.cost_basis === "partial"
                  ? `${fmtCost(c.known_cost)} known`
                  : c.cost_basis === "estimated"
                    ? `${fmtCost(c.known_cost)} estimated`
                  : fmtCost(c.known_cost)}
            </div>
          {/each}
          <div class="compare-label">Duration</div>
          {#each compareList as c}<div class="compare-cell">{c.duration_secs > 0 ? fmtDuration(c.duration_secs) : "—"}</div>{/each}
          <div class="compare-label">Cache Hit</div>
          {#each compareList as c}
            {@const total = c.cache_read_tokens + Math.max(0, c.input_tokens - c.cache_write_tokens - c.cache_read_tokens)}
            <div class="compare-cell">{total > 0 ? ((c.cache_read_tokens / total) * 100).toFixed(0) + "%" : "—"}</div>
          {/each}
          <div class="compare-label">Effort</div>
          {#each compareList as c}<div class="compare-cell">{c.effort}</div>{/each}
        </div>
      </div>
    {/if}

    {#if history.length > 0 || (!historyLoading && !historyError)}
    <div class="history-table" class:refreshing={historyLoading && history.length > 0}>
      <table>
        <caption class="sr-only">Session history ledger</caption>
        <thead>
          <tr class="ht-header">
            {#if compareMode}<th scope="col" class="ht-col check"><span class="sr-only">Compare</span></th>{/if}
            <th scope="col" class="ht-col status"><span class="sr-only">Status</span></th>
            <th scope="col" class="ht-col project">Project</th>
            <th scope="col" class="ht-col model">Model</th>
            <th scope="col" class="ht-col numeric">Tokens</th>
            <th scope="col" class="ht-col numeric">Duration</th>
            <th scope="col" class="ht-col cost numeric">Monetary value</th>
            <th scope="col" class="ht-col date">Date</th>
          </tr>
        </thead>
        <tbody>
      {#each visibleHistory as h (h.id)}
        <tr
          class="ht-row"
          class:active={h.is_active}
          class:expanded={expandedId === h.id}
          onclick={() => toggleExpand(h.id)}
        >
            {#if compareMode}
              <td class="ht-col check">
                <input aria-label={`Compare ${h.project} session`} type="checkbox" checked={compareIds.has(h.id)} onclick={(e) => { e.stopPropagation(); toggleCompare(h.id); }} disabled={!compareIds.has(h.id) && compareIds.size >= 3} />
              </td>
            {/if}
            <td class="ht-col status"><span class="status-dot" class:active={h.is_active}></span><span class="sr-only">{h.is_active ? "Active" : "Ended"}</span></td>
            <th scope="row" class="ht-col project">
              <button type="button" aria-expanded={expandedId === h.id} onclick={(e) => { e.stopPropagation(); toggleExpand(h.id); }}>
                <span>{h.project}</span>
                <small>{h.branch || h.session_name || "No branch"}</small>
              </button>
            </th>
            <td class="ht-col model" title={h.model}>{h.model}</td>
            <td class="ht-col numeric">{fmtTokens(h.total_tokens)}</td>
            <td class="ht-col numeric">{h.duration_secs > 0 ? fmtDuration(h.duration_secs) : "—"}</td>
            <td class="ht-col cost numeric" class:unavailable={h.known_cost === null}>
              {#if h.known_cost === null}
                —
              {:else}
                <span class="cost-value">{fmtCost(h.known_cost)}</span>
                <span class="cost-tag" class:exact={h.cost_basis === "exact"} class:estimate={h.cost_basis !== "exact"}>
                  {h.cost_basis === "exact" ? "exact" : h.cost_basis === "partial" ? "lower bound" : "estimate"}
                </span>
              {/if}
            </td>
            <td class="ht-col date">{h.started_at?.slice(0, 10) ?? "—"}</td>
          </tr>
          {#if expandedId === h.id}
            <tr class="ht-detail-row">
              <td colspan={compareMode ? 8 : 7}>
                <div class="ht-detail" transition:fly={{ y: -8, duration: 150 }}>
                  {#if h.opencode}
                    <div class="model-provenance">
                      <strong>{h.opencode.model_name || h.opencode.model_id}</strong>
                      <span>{h.opencode.model_provider} · {h.opencode.surface} · {h.opencode.variant || "No variant reported"}</span>
                      {#if h.opencode.models.length > 1}<ul>{#each h.opencode.models as model}<li>{model.provider_id}/{model.model_id}: {model.cost === null ? "Cost not reported" : fmtCost(model.cost)}</li>{/each}</ul>{/if}
                    </div>
                  {/if}
              <div class="detail-grid">
                <div class="detail-section">
                  <span class="detail-label">Token Breakdown</span>
                  <div class="detail-row"><span>Input</span><span>{fmtTokens(Math.max(0, h.input_tokens - h.cache_write_tokens - h.cache_read_tokens))}</span></div>
                  <div class="detail-row"><span>Output</span><span>{fmtTokens(h.output_tokens)}</span></div>
                  <div class="detail-row"><span>Cache Write</span><span>{fmtTokens(h.cache_write_tokens)}</span></div>
                  <div class="detail-row"><span>Cache Read</span><span>{fmtTokens(h.cache_read_tokens)}</span></div>
                </div>
                <div class="detail-section">
                  <span class="detail-label">Monetary Value Breakdown</span>
                  {#if h.known_cost === null}
                    <p class="detail-unavailable">The provider did not return enough billing inputs for this session.</p>
                  {:else if h.opencode}
                    <p class="detail-unavailable">OpenCode reports a total, not separate token-category charges.</p>
                    <div class="detail-row"><span>Reported total</span><span>{fmtCost(h.total_cost)}</span></div>
                  {:else}
                    {#if h.cost_basis === "partial"}
                      <p class="detail-unavailable">Known subtotal; this session has incomplete cost coverage.</p>
                    {:else if h.cost_basis === "estimated" || monetaryValueLabel([h.cost_source]) === "API-equivalent value"}
                      <p class="detail-unavailable">{CALCULATED_COST_NOTE}</p>
                    {:else if monetaryValueLabel([h.cost_source]) === "Provider-billed spend"}
                      <p class="detail-unavailable">Provider-reported billing for this session.</p>
                    {/if}
                    <div class="detail-row"><span>Input</span><span>{fmtCost(h.input_cost)}</span></div>
                    <div class="detail-row"><span>Output</span><span>{fmtCost(h.output_cost)}</span></div>
                    <div class="detail-row"><span>Cache Write</span><span>{fmtCost(h.cache_write_cost)}</span></div>
                    <div class="detail-row"><span>Cache Read</span><span>{fmtCost(h.cache_read_cost)}</span></div>
                  {/if}
                </div>
                <div class="detail-section">
                  <span class="detail-label">Details</span>
                  <div class="detail-row"><span>Effort</span><span>{h.effort}</span></div>
                  <div class="detail-row"><span>Thinking</span><span>{h.has_thinking ? "Yes" : "No"}</span></div>
                  <div class="detail-row"><span>Subagents</span><span>{h.subagent_count}</span></div>
                  <div class="detail-row"><span>Context</span><span>{h.context_window}</span></div>
                </div>
              </div>
                </div>
              </td>
            </tr>
          {/if}
      {:else}
        <tr><td class="ht-empty" colspan={compareMode ? 8 : 7}>No sessions match the selected history filters. All-time totals above remain unchanged.</td></tr>
      {/each}
        </tbody>
      </table>
    </div>
    {#if history.length > visibleHistory.length}
      <button
        class="show-more"
        type="button"
        onclick={() => (visibleHistoryLimit += HISTORY_PAGE_SIZE)}
      >
        Show {Math.min(HISTORY_PAGE_SIZE, history.length - visibleHistory.length)} more
      </button>
    {/if}
    {/if}
  </div>
</div>

<ExportModal
  open={showExport}
  title="Export session history"
  defaultFilename="pulse-sessions"
  columns={historyColumns}
  rows={exportRows}
  onclose={() => showExport = false}
/>

<style>
  .model-provenance { display: grid; gap: 6px; grid-column: 1 / -1; font-size: 12px; line-height: 1.5; color: var(--text-secondary); }
  .model-provenance strong { color: var(--text-primary); }
  .model-provenance ul { padding-left: 18px; }

  .sessions-view { display: flex; flex-direction: column; gap: var(--page-gap); }
  .view-header { display: flex; align-items: flex-end; gap: 20px; flex-wrap: wrap; }
  .title-line { display: flex; align-items: center; gap: 10px; }
  .view-title { font-size: 20px; font-weight: 700; }
  .filters { margin-left: auto; display: flex; align-items: end; gap: 10px; }
  .filter-control { display: grid; gap: 4px; min-width: 170px; }
  .project-control { min-width: 210px; }
  .control-label, .flt-lbl { font-size: 9px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.08em; color: var(--text-muted); }
  .stats-row { grid-template-columns: repeat(4, 1fr); }

  .card { position: relative; background: var(--surface-panel); border: 1px solid var(--border); border-radius: var(--radius-lg); padding: 20px; overflow: hidden; }
  .card-title-row { display: flex; justify-content: space-between; align-items: center; margin-bottom: 14px; }
  .card-title-row .card-title { margin-bottom: 0; }
  .card-title { font-size: 12px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.06em; color: var(--accent); margin-bottom: 14px; }
  .title-actions { display: flex; gap: 6px; }
  .action-btn { font-size: 11px; font-weight: 600; color: var(--text-secondary); background: var(--bg-elevated); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 4px 12px; cursor: pointer; transition: all 0.15s ease; }
  .action-btn:hover { color: var(--accent); border-color: var(--accent); background: var(--accent-dim); }
  .action-btn.active { color: var(--accent); border-color: var(--accent); background: var(--accent-dim); }

  .session-list { display: flex; flex-direction: column; gap: 8px; }
  .empty-state { text-align: center; padding: 34px 24px; }
  .empty-text { font-size: 14px; font-weight: 600; color: var(--text-primary); }
  .empty-sub { margin-top: 6px; font-size: 12px; color: var(--text-muted); }

  .compare-panel { margin-bottom: 16px; padding: 16px; background: var(--bg-primary); border: 1px solid var(--accent-dim); border-radius: var(--radius-md); }
  .compare-title { font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.05em; color: var(--accent); margin-bottom: 12px; }
  .compare-grid { display: grid; grid-template-columns: 80px repeat(var(--compare-cols), 1fr); gap: 6px 12px; font-size: 12px; }
  .compare-label { font-weight: 600; color: var(--text-muted); padding: 4px 0; }
  .compare-head { font-weight: 700; color: var(--text-primary); padding: 4px 0; border-bottom: 1px solid var(--border); }
  .compare-cell { color: var(--text-secondary); padding: 4px 0; font-variant-numeric: tabular-nums; }
  .compare-cell.accent { color: var(--accent); font-weight: 700; }

  .history-controls { margin-bottom: 16px; display: flex; flex-direction: column; gap: 10px; }
  .history-state {
    min-height: 96px;
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 6px;
    margin-bottom: 14px;
    color: var(--text-muted);
    text-align: center;
    border-block: 1px solid var(--divider);
  }
  .history-state strong { color: var(--text-primary); font-size: 12px; }
  .history-state span { font-size: 10px; }
  .history-state.error strong { color: var(--danger); }
  .history-state button {
    margin-top: 4px;
    padding: 5px 11px;
    color: var(--text-primary);
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .history-toolbar { display: flex; align-items: end; gap: 10px; flex-wrap: wrap; padding: 10px 12px; background: var(--bg-elevated); border: 1px solid var(--border); border-radius: var(--radius-md); }
  .flt { display: flex; flex-direction: column; gap: 4px; min-width: 112px; font-size: 11px; }
  .flt input { width: 100%; min-height: var(--control-height); padding: 6px 9px; font-size: 11px; color: var(--text-primary); background: var(--bg-input); border: 1px solid var(--border); border-radius: var(--radius-sm); }
  .flt.compact { width: 90px; min-width: 90px; }
  .flt.model-filter { flex: 1 1 150px; }
  .reset-btn { min-height: var(--control-height); }
  .history-search-toolbar {
    overflow: hidden;
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
  }
  .search-box { position: relative; display: block; min-width: 0; }
  .search-box input { width: 100%; padding: 9px 14px 9px 32px; font: inherit; font-size: 12px; background: transparent; border: 0; border-radius: 0; color: var(--text-primary); outline: none; transition: box-shadow 0.15s ease; }
  .search-box input:focus { box-shadow: inset 0 0 0 2px var(--accent); }
  .search-box::before { content: ""; position: absolute; left: 12px; top: 50%; transform: translateY(-50%); width: 12px; height: 12px; border: 1.5px solid var(--text-muted); border-radius: 50%; pointer-events: none; }
  .search-box::after { content: ""; position: absolute; left: 22px; top: 60%; width: 4px; height: 1.5px; background: var(--text-muted); transform: rotate(45deg); pointer-events: none; }
  .history-summary { padding: 8px 14px; overflow: hidden; color: var(--text-muted); border-top: 1px solid var(--divider); font-size: 11px; line-height: 1.5; text-overflow: ellipsis; white-space: nowrap; }
  .history-summary strong { color: var(--text-primary); font-variant-numeric: tabular-nums; }

  .history-table { font-size: 12px; max-height: 500px; overflow: auto; }
  .history-table table { width: 100%; min-width: 760px; border-collapse: collapse; table-layout: fixed; }
  .history-table.refreshing { opacity: 0.72; pointer-events: none; }
  .ht-header { position: sticky; top: 0; z-index: 1; background: var(--bg-card); }
  .ht-header th { padding: 10px 12px; border-bottom: 1px solid var(--border); color: var(--text-muted); font-weight: 700; text-transform: uppercase; font-size: 9px; letter-spacing: 0.08em; }
  .ht-header .status { width: 30px; }
  .ht-header .project { width: 25%; }
  .ht-header .model { width: 22%; }
  .ht-header .numeric { width: 12%; }
  .ht-header .date { width: 12%; }
  .ht-row { transition: background 0.15s var(--ease); cursor: pointer; }
  .ht-row > * { padding: 11px 12px; border-bottom: 1px solid var(--border); }
  .ht-row:hover { background: var(--bg-card-hover); }
  .ht-row.active { background: var(--success-dim); }
  .ht-row.active > :first-child { box-shadow: inset 2px 0 var(--success); }
  .ht-row.expanded { background: var(--bg-elevated); }
  .ht-col { color: var(--text-secondary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .ht-col.numeric { text-align: right; font-variant-numeric: tabular-nums; }
  .ht-col.project { text-align: left; color: var(--text-primary); }
  .ht-col.project button { width: 100%; display: grid; gap: 2px; text-align: left; }
  .ht-col.project button > span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 700; }
  .ht-col.project small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-muted); font-size: 10px; font-weight: 500; }
  .ht-col.model { text-align: left; font-family: var(--font-mono); }
  .ht-col.cost { color: var(--text-primary); }
  .ht-col.cost.unavailable { color: var(--text-muted); font-weight: 500; }
  .cost-value { font-weight: 750; }
  .cost-tag { display: inline-flex; margin-left: 6px; padding: 2px 6px; border: 1px solid; border-radius: var(--radius-full); font-size: 9px; font-weight: 700; line-height: 1.2; text-transform: uppercase; letter-spacing: 0.04em; }
  .cost-tag.exact { color: var(--success); background: var(--success-dim); border-color: color-mix(in srgb, var(--success) 34%, transparent); }
  .cost-tag.estimate { color: var(--warning); background: var(--warning-dim); border-color: color-mix(in srgb, var(--warning) 34%, transparent); }
  .ht-col.date { color: var(--text-muted); font-size: 11px; text-align: right; font-variant-numeric: tabular-nums; }
  .ht-col.status, .ht-col.check { width: 30px; text-align: center; }
  .status-dot { display: inline-block; width: 7px; height: 7px; border-radius: 50%; background: var(--text-muted); }
  .status-dot.active { background: var(--success); box-shadow: 0 0 6px var(--success-glow); }
  .ht-empty { text-align: center; padding: 40px; color: var(--text-muted); font-size: 12px; }

  .ht-detail-row > td { padding: 0; border-bottom: 1px solid var(--border); }
  .ht-detail { padding: 12px 14px 16px; background: var(--bg-elevated); border-top: 1px solid var(--border); }
  .detail-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 16px; }
  .detail-section { display: flex; flex-direction: column; gap: 4px; }
  .detail-label { font-size: 10px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.05em; color: var(--accent); margin-bottom: 4px; }
  .detail-row { display: flex; justify-content: space-between; font-size: 11px; color: var(--text-secondary); padding: 2px 0; }
  .detail-row span:last-child { font-weight: 600; color: var(--text-primary); font-variant-numeric: tabular-nums; }
  .detail-unavailable { max-width: 34ch; color: var(--text-muted); font-size: 11px; line-height: 1.5; }
  .show-more { align-self: center; margin: 12px auto 0; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-elevated); color: var(--text-secondary); padding: 7px 16px; font-size: 11px; font-weight: 600; cursor: pointer; }
  .show-more:hover { color: var(--accent); border-color: var(--accent); }

  .card { min-width: 0; }
  .history-table, .compare-panel { overscroll-behavior-inline: contain; }
  .sr-only { position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; border: 0; }

  @media (max-width: 1050px) {
    .stats-row { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .detail-grid { grid-template-columns: 1fr 1fr; }
  }

  @media (max-width: 800px) {
    .view-header { align-items: stretch; flex-direction: column; gap: 10px; }
    .filters { width: 100%; margin-left: 0; }
    .filter-control { flex: 1; min-width: 0; }
  }

  @media (max-width: 620px) {
    .stats-row, .detail-grid { grid-template-columns: 1fr; }
    .history-toolbar { align-items: stretch; }
    .flt, .flt.compact, .flt.model-filter { flex: 1 1 calc(50% - 8px); width: auto; min-width: 120px; }
    .filters { flex-direction: column; align-items: stretch; }
  }
  @media (max-width: 620px) {
    .stats-row { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .stats-row :global(.stat-card) { padding: 12px; min-width: 0; }
    .filters :global(.segmented) { flex-wrap: wrap; }
    .filters :global(.seg-opt) { min-height: 36px; }
    .ht-col.model { white-space: normal; overflow-wrap: anywhere; }
    .history-summary { white-space: normal; line-height: 1.6; }
  }
  .stats-row { grid-template-columns:repeat(auto-fit,minmax(160px,1fr)); }
  .filters { max-width:100%; }
  .sort-control :global(.segmented) { flex-wrap:wrap; height:auto; min-height:32px; }
  .sort-control :global(.seg-opt) { min-height:28px; }
</style>
