<script lang="ts">
    import { onMount } from "svelte";
    import IconCopy from "@tabler/icons-svelte/icons/copy";
    import IconDownload from "@tabler/icons-svelte/icons/download";
    import {
        getReportsBundle,
        copyFixPrompt,
        generateHtmlReport,
        generateMarkdownReport,
        type CacheHealthReport,
        type Recommendation,
        type InflectionPoint,
        type ModelRoutingReport,
        type ToolFrequencyReport,
        type PromptComplexityReport,
        type SessionHealthReport,
        type TraceOverview,
        type ProviderCapabilities,
        type Severity,
    } from "../lib/api";
    import { addToast, selectedAnalyticsProviderScope } from "../lib/stores";
    import { providerProfile } from "../lib/provider";
    import { fmtCost, monetaryValueLabel } from "../lib/utils";
    import CostTimeline from "../components/CostTimeline.svelte";
    import type { DailyCostPoint } from "../lib/api";
    import SegmentedControl from "../components/SegmentedControl.svelte";

    const windowOptions = [
        { value: "7", label: "7d" },
        { value: "30", label: "30d" },
        { value: "90", label: "90d" },
        { value: "365", label: "1y" },
    ];

    let cache = $state<CacheHealthReport | null>(null);
    let recs = $state<Recommendation[]>([]);
    let inflections = $state<InflectionPoint[]>([]);
    let routing = $state<ModelRoutingReport | null>(null);
    let tools = $state<ToolFrequencyReport | null>(null);
    let prompts = $state<PromptComplexityReport | null>(null);
    let health = $state<SessionHealthReport | null>(null);
    let trace = $state<TraceOverview | null>(null);
    let capabilities = $state<ProviderCapabilities>({
        cache_health: false,
        model_routing: false,
        extra_usage: false,
    });
    let dailyCosts = $state<DailyCostPoint[]>([]);
    let totalSessions = $state(0);
    let loading = $state(true);
    let hasLoaded = $state(false);
    const loadedWindowKeys = new Set<string>();
    let loadError = $state<string | null>(null);
    let days = $state(30);
    let severityFilter = $state<"all" | Severity>("all");
    let copyingReport = $state(false);
    let downloadingHtml = $state(false);
    let reportRequest = 0;
    let windowLabel = $derived(
        days === 7
            ? "7-day"
            : days === 30
              ? "30-day"
              : days === 90
                ? "90-day"
                : "Year",
    );

    function clearReportData(): void {
        cache = null;
        recs = [];
        inflections = [];
        routing = null;
        tools = null;
        prompts = null;
        health = null;
        trace = null;
        dailyCosts = [];
        totalSessions = 0;
    }

    async function loadReports(): Promise<void> {
        const request = ++reportRequest;
        loading = true;
        loadError = null;
        const requestedDays = days;
        const requestedProvider = $selectedAnalyticsProviderScope;
        const requestedWindowKey = `${requestedDays}:${requestedProvider}`;
        if (!hasLoaded) clearReportData();
        try {
            const bundle = await getReportsBundle(
                requestedDays,
                undefined,
                requestedProvider,
            );
            if (
                request !== reportRequest ||
                requestedDays !== days ||
                requestedProvider !== $selectedAnalyticsProviderScope
            )
                return;
            capabilities = bundle.capabilities;
            cache = bundle.cache_health;
            recs = bundle.recommendations;
            inflections = bundle.inflection_points;
            routing = bundle.model_routing;
            tools = bundle.tool_frequency;
            prompts = bundle.prompt_complexity;
            health = bundle.session_health;
            trace = bundle.trace_overview;
            dailyCosts = bundle.daily_costs ?? [];
            totalSessions = bundle.total_sessions;
            loadedWindowKeys.add(requestedWindowKey);
        } catch (err) {
            if (
                request !== reportRequest ||
                requestedDays !== days ||
                requestedProvider !== $selectedAnalyticsProviderScope
            )
                return;
            const requestedWindowLabel =
                requestedDays === 365 ? "1-year" : `${requestedDays}-day`;
            loadError =
                err instanceof Error && err.message
                    ? `${requestedWindowLabel} report unavailable. ${err.message}`
                    : `${requestedWindowLabel} report unavailable. Pulse could not build the report.`;
        } finally {
            if (request === reportRequest) {
                loading = false;
                hasLoaded = true;
            }
        }
    }

    let lastReloadKey = $state("");
    let activeWindowKey = $derived(
        `${days}:${$selectedAnalyticsProviderScope}`,
    );
    let bundleReady = $derived(loadedWindowKeys.has(activeWindowKey));
    $effect(() => {
        const key = activeWindowKey;
        if (key !== lastReloadKey) {
            lastReloadKey = key;
            loadReports();
        }
    });

    let actionableRecs = $derived(
        recs.filter(
            (rec) =>
                !["cache-healthy", "all-good"].includes(rec.id) &&
                rec.fix_prompt.trim().length > 0,
        ),
    );
    let filteredRecs = $derived(
        severityFilter === "all"
            ? actionableRecs
            : actionableRecs.filter((r) => r.severity === severityFilter),
    );

    async function handleFix(rec: Recommendation): Promise<void> {
        if (!rec.fix_prompt) {
            addToast(
                "No prompt available for this recommendation.",
                "info",
                2500,
            );
            return;
        }
        try {
            const prompt = await copyFixPrompt(
                rec.id,
                days,
                $selectedAnalyticsProviderScope,
            );
            await navigator.clipboard.writeText(prompt || rec.fix_prompt);
            addToast(
                `Fix prompt copied — paste into ${$providerProfile.productName}.`,
                "success",
                3500,
            );
        } catch (err) {
            addToast(`Copy failed: ${String(err)}`, "danger", 4000);
        }
    }

    async function handleCopyMarkdown(): Promise<void> {
        if (copyingReport) return;
        copyingReport = true;
        try {
            const md = await generateMarkdownReport(
                days,
                undefined,
                $selectedAnalyticsProviderScope,
            );
            await navigator.clipboard.writeText(md);
            addToast("Report copied to clipboard.", "success", 3000);
        } catch (err) {
            addToast(`Copy failed: ${String(err)}`, "danger", 4000);
        } finally {
            copyingReport = false;
        }
    }

    async function handleDownloadHtml(): Promise<void> {
        if (downloadingHtml) return;
        downloadingHtml = true;
        try {
            const html = await generateHtmlReport(
                days,
                undefined,
                $selectedAnalyticsProviderScope,
            );
            const stamp = new Date().toISOString().slice(0, 10);

            // Blob download (same mechanism as CSV export) instead of the native
            // dialog + fs.writeTextFile: the OS save dialog lets the user pick any
            // path, but writeTextFile is denied outside the fs:scope roots
            // ($HOME/$DOWNLOAD/$DOCUMENT/$DESKTOP) — e.g. any D:\ path — which
            // surfaced as "Download failed". A blob download needs no fs grant and
            // works in both the packaged webview and browser review.
            const blob = new Blob([html], { type: "text/html" });
            const url = URL.createObjectURL(blob);
            const a = document.createElement("a");
            a.href = url;
            a.download = `pulse-report-${stamp}.html`;
            a.click();
            URL.revokeObjectURL(url);
            addToast("Report downloaded.", "success", 3000);
        } catch (err) {
            addToast(`Download failed: ${String(err)}`, "danger", 4000);
        } finally {
            downloadingHtml = false;
        }
    }

    const severityOrder: Record<Severity, number> = {
        critical: 0,
        warning: 1,
        info: 2,
        positive: 3,
    };

    const severityTabs: { id: "all" | Severity; label: string }[] = [
        { id: "all", label: "All" },
        { id: "critical", label: "Critical" },
        { id: "warning", label: "Warning" },
        { id: "info", label: "Info" },
        { id: "positive", label: "Good" },
    ];

    let sortedRecs = $derived(
        [...filteredRecs].sort(
            (a, b) => severityOrder[a.severity] - severityOrder[b.severity],
        ),
    );

    // Derived from the same series the chart plots, so the readouts under the
    // chart can never disagree with the curve above them.
    let totalCost = $derived(dailyCosts.reduce((sum, p) => sum + p.cost, 0));
    /** Averaged over days with activity, not the whole window: an idle day is
     *  not a cheap day, and including it would understate known daily monetary value. */
    let activeDays = $derived(dailyCosts.filter((p) => p.sessions > 0).length);
    let avgDailyCost = $derived(activeDays === 0 ? 0 : totalCost / activeDays);
    let peakDay = $derived(
        dailyCosts.length === 0
            ? null
            : dailyCosts.reduce((a, b) => (b.cost > a.cost ? b : a)),
    );
    let inflectionDayCount = $derived(
        new Set(inflections.map((i) => i.date)).size,
    );
    let inflectionShare = $derived(
        dailyCosts.length === 0
            ? 0
            : (inflectionDayCount / dailyCosts.length) * 100,
    );
    let windowHasSessions = $derived(
        totalSessions > 0 || dailyCosts.some((point) => point.sessions > 0),
    );
    let timelineSessions = $derived(
        dailyCosts.reduce((sum, point) => sum + point.sessions, 0),
    );
    let timelinePricedSessions = $derived(
        dailyCosts.reduce((sum, point) => sum + point.priced_sessions, 0),
    );
    let timelineCostBasis = $derived.by(() => {
        const activePoints = dailyCosts.filter((point) => point.sessions > 0);
        if (activePoints.length === 0 || timelinePricedSessions === 0)
            return "unavailable";
        if (
            timelinePricedSessions < timelineSessions ||
            activePoints.some((point) => point.cost_basis === "partial")
        )
            return "partial";
        if (activePoints.some((point) => point.cost_basis === "estimated"))
            return "estimated";
        return "exact";
    });
    let timelineCostSources = $derived([
        ...new Set(dailyCosts.flatMap((point) => point.cost_sources)),
    ]);
</script>

<div class="reports-view app-view">
    <header class="view-header">
        <div class="view-title-group">
            <h2 class="view-title">Reports</h2>
            <p class="view-sub">{windowLabel} window</p>
        </div>
        <div class="controls">
            <SegmentedControl
                options={windowOptions}
                value={String(days)}
                onchange={(value) => (days = Number(value))}
                ariaLabel="Analysis window"
            />
            <div class="export-group">
                <span class="export-kicker">Exports</span>
                <div class="export-actions">
                    <button
                        class="btn-secondary"
                        onclick={handleCopyMarkdown}
                        disabled={!!loadError || copyingReport}
                    >
                        <IconCopy size={14} stroke={1.8} aria-hidden="true" />
                        {copyingReport ? "Copying..." : "Copy report"}
                    </button>
                    <button
                        class="btn-primary"
                        onclick={handleDownloadHtml}
                        disabled={!!loadError || downloadingHtml}
                    >
                        <IconDownload
                            size={14}
                            stroke={1.8}
                            aria-hidden="true"
                        />
                        {downloadingHtml ? "Downloading..." : "Download HTML"}
                    </button>
                </div>
            </div>
        </div>
    </header>

    {#if loading && !bundleReady && !hasLoaded}
        <section
            class="reports-loading"
            aria-label="Loading reports"
            aria-live="polite"
            data-reports-loading
        >
            <div class="loading-card loading-timeline" aria-hidden="true">
                <span class="skeleton loading-label"></span>
                <span class="skeleton loading-copy"></span>
                <span class="skeleton loading-block timeline-block"></span>
                <div class="loading-metrics">
                    <span class="skeleton"></span><span class="skeleton"
                    ></span><span class="skeleton"></span><span class="skeleton"
                    ></span>
                </div>
            </div>
            <div class="loading-card loading-hero" aria-hidden="true">
                <span class="skeleton loading-grade"></span>
                <div class="loading-content">
                    <span class="skeleton loading-label"></span>
                    <span class="skeleton loading-copy"></span>
                    <div class="loading-metrics">
                        <span class="skeleton"></span><span class="skeleton"
                        ></span><span class="skeleton"></span><span
                            class="skeleton"
                        ></span>
                    </div>
                </div>
            </div>
            <div class="loading-grid" aria-hidden="true">
                <div class="loading-card">
                    <span class="skeleton loading-label"></span><span
                        class="skeleton loading-copy"
                    ></span><span class="skeleton loading-block"></span>
                </div>
                <div class="loading-card">
                    <span class="skeleton loading-label"></span><span
                        class="skeleton loading-copy"
                    ></span><span class="skeleton loading-block"></span>
                </div>
            </div>
            <div class="loading-card" aria-hidden="true">
                <span class="skeleton loading-label"></span><span
                    class="skeleton loading-copy"
                ></span>
                <div class="loading-metrics">
                    <span class="skeleton"></span><span class="skeleton"
                    ></span><span class="skeleton"></span><span class="skeleton"
                    ></span>
                </div>
            </div>
            <div class="loading-grid" aria-hidden="true">
                <div class="loading-card">
                    <span class="skeleton loading-label"></span><span
                        class="skeleton loading-copy"
                    ></span><span class="skeleton loading-block"></span>
                </div>
                <div class="loading-card">
                    <span class="skeleton loading-label"></span><span
                        class="skeleton loading-copy"
                    ></span><span class="skeleton loading-block"></span>
                </div>
            </div>
            <div class="loading-card" aria-hidden="true">
                <span class="skeleton loading-label"></span><span
                    class="skeleton loading-copy"
                ></span><span class="skeleton loading-block"></span>
            </div>
            <div class="loading-card" aria-hidden="true">
                <span class="skeleton loading-label"></span><span
                    class="skeleton loading-copy"
                ></span><span class="skeleton loading-block"></span>
            </div>
        </section>
    {:else if loadError}
        <section class="report-error" role="alert">
            <strong
                >{days === 365 ? "1-year" : `${days}-day`} report unavailable</strong
            >
            <span>{loadError.replace(/^[^.]+\.\s*/, "")}</span>
            <button type="button" onclick={loadReports}>Retry</button>
        </section>
    {:else}
        {#if loading && hasLoaded}
            <div class="reload-banner" role="status">
                <span class="reload-spinner"></span>
                Refreshing for {days === 365 ? "1y" : `${days}d`}…
            </div>
        {/if}
        <div class="report-body" class:reloading={loading && hasLoaded}>
            {#if !windowHasSessions}
                <section class="report-empty">
                    <strong
                        >No sessions in this {days === 365
                            ? "1-year"
                            : `${days}-day`} window.</strong
                    >
                    <span
                        >No saved sessions fall inside the selected dates. Pick
                        a wider window to see more.</span
                    >
                </section>
            {:else}
                <section class="timeline-hero">
                    <div class="th-head">
                        <div class="th-titles">
                            <h3 class="th-title">Monetary value timeline</h3>
                            <p class="th-sub">
                                Daily priced value and inflection points in the {windowLabel}
                                window.
                            </p>
                        </div>
                        <div
                            class="cost-coverage"
                            data-basis={timelineCostBasis}
                        >
                            <strong>
                                {timelineCostBasis === "unavailable"
                                    ? "Monetary value unavailable"
                                    : timelineCostBasis === "partial"
                                      ? "Known monetary-value coverage"
                                      : timelineCostBasis === "estimated"
                                        ? "API-equivalent estimate"
                                        : "Complete coverage"}
                            </strong>
                            <span>
                                {timelinePricedSessions} of {timelineSessions} sessions
                                priced
                                {timelineCostSources.length > 0
                                    ? ` · ${monetaryValueLabel(timelineCostSources)}`
                                    : ""}
                            </span>
                        </div>
                    </div>

                    {#if timelineCostBasis === "unavailable"}
                        <div class="cost-unavailable">
                            <strong
                                >No verified monetary total for this window</strong
                            >
                            <span
                                >Session and workflow analysis remain available
                                below; Pulse will not turn unknown cost into
                                $0.00.</span
                            >
                        </div>
                    {:else}
                        <CostTimeline points={dailyCosts} {inflections} />
                    {/if}

                    <div class="th-stats">
                        <div class="th-stat">
                            <span class="ths-label"
                                >{timelineCostBasis === "partial"
                                    ? "Known subtotal"
                                    : monetaryValueLabel(
                                          timelineCostSources,
                                      )}</span
                            >
                            <span class="ths-value"
                                >{timelineCostBasis === "unavailable"
                                    ? "—"
                                    : fmtCost(totalCost)}</span
                            >
                            <span class="ths-meta"
                                >{dailyCosts.length} days analysed</span
                            >
                        </div>
                        <div class="th-stat">
                            <span class="ths-label">Avg active day</span>
                            <span class="ths-value"
                                >{timelineCostBasis === "unavailable"
                                    ? "—"
                                    : fmtCost(avgDailyCost)}</span
                            >
                            <span class="ths-meta"
                                >{activeDays}
                                {activeDays === 1 ? "day" : "days"} with sessions</span
                            >
                        </div>
                        <div class="th-stat">
                            <span class="ths-label">Peak day</span>
                            <span class="ths-value"
                                >{timelineCostBasis !== "unavailable" && peakDay
                                    ? fmtCost(peakDay.cost)
                                    : "—"}</span
                            >
                            <span class="ths-meta"
                                >{timelineCostBasis !== "unavailable" &&
                                peakDay &&
                                peakDay.cost > 0
                                    ? peakDay.date
                                    : "not reported"}</span
                            >
                        </div>
                        <div class="th-stat">
                            <span class="ths-label">Inflection days</span>
                            <span class="ths-value">{inflectionDayCount}</span>
                            <span class="ths-meta"
                                >{inflectionShare.toFixed(0)}% of window</span
                            >
                        </div>
                    </div>
                </section>

                {#if cache && capabilities.cache_health}
                    <section class="card hero-card">
                        <div class="hero-left">
                            <div
                                class="grade-letter"
                                style="color: {cache.color}; text-shadow: 0 0 24px {cache.color}22;"
                            >
                                {cache.grade}
                            </div>
                            <div class="grade-meta">
                                <h3 class="label">Cache health grade</h3>
                                <div class="ratio">
                                    {cache.trend_weighted_ratio.toFixed(0)}<span
                                        class="pct">%</span
                                    >
                                    <span class="muted">
                                        hit ratio · {cache.grade_label}</span
                                    >
                                </div>
                            </div>
                        </div>
                        <div class="hero-right">
                            <p class="card-sub hero-sub">
                                Trend-weighted cache reuse for {cache.sessions_analyzed}
                                sessions in the {windowLabel} window.
                            </p>
                            <p class="diagnosis">{cache.diagnosis}</p>
                            <div class="hero-stats">
                                <div class="hero-stat">
                                    <span class="hs-label">Cache read</span>
                                    <span class="hs-value"
                                        >{(
                                            cache.total_cache_read / 1e6
                                        ).toFixed(1)}M</span
                                    >
                                </div>
                                <div class="hero-stat">
                                    <span class="hs-label">Cache write</span>
                                    <span class="hs-value"
                                        >{(
                                            cache.total_cache_write / 1e6
                                        ).toFixed(1)}M</span
                                    >
                                </div>
                                <div class="hero-stat">
                                    <span class="hs-label">Pure input</span>
                                    <span class="hs-value"
                                        >{(cache.total_input / 1e6).toFixed(
                                            1,
                                        )}M</span
                                    >
                                </div>
                                <div class="hero-stat">
                                    <span class="hs-label">Sessions</span>
                                    <span class="hs-value"
                                        >{cache.sessions_analyzed}</span
                                    >
                                </div>
                            </div>
                        </div>
                    </section>
                {/if}

                <div
                    class="two-col"
                    class:single={!(capabilities.model_routing && routing)}
                >
                    {#if routing && capabilities.model_routing}
                        <section class="card">
                            <h3 class="card-title">Model routing</h3>
                            <p class="card-sub">
                                Priced session share and rerouting estimates for
                                the {windowLabel} window.
                            </p>
                            <p class="diagnosis section-diagnosis">
                                {routing.diagnosis}
                            </p>
                            <p class="routing-coverage">
                                {routing.priced_sessions} of {routing.total_sessions}
                                sessions priced · {routing.cost_basis.replace(
                                    "_",
                                    " ",
                                )}
                            </p>
                            <div class="routing-bars">
                                {#each [{ label: "Opus", stats: routing.opus, color: "var(--accent)" }, { label: "Sonnet", stats: routing.sonnet, color: "var(--info)" }, { label: "Haiku", stats: routing.haiku, color: "var(--success)" }, { label: "Other", stats: routing.other, color: "var(--token-cache-read)" }] as row}
                                    {#if row.stats.sessions > 0}
                                        <div class="bar-row">
                                            <div class="bar-label">
                                                <span
                                                    class="dot"
                                                    style="background: {row.color}"
                                                ></span>
                                                {row.label}
                                                <span class="bar-count"
                                                    >· {row.stats
                                                        .sessions}</span
                                                >
                                            </div>
                                            <div class="bar-track">
                                                <div
                                                    class="bar-fill"
                                                    style="width: {row.stats
                                                        .cost_share_pct}%; background: {row.color};"
                                                ></div>
                                            </div>
                                            <div class="bar-value">
                                                {row.stats.cost_share_pct.toFixed(
                                                    0,
                                                )}%
                                                <span class="muted"
                                                    >{fmtCost(
                                                        row.stats.cost,
                                                    )}</span
                                                >
                                            </div>
                                        </div>
                                    {/if}
                                {/each}
                            </div>
                            {#if routing.savings_estimate_available && routing.estimated_savings_if_rerouted > 0}
                                <div class="savings-hint">
                                    <span
                                        >Estimated rerouting opportunity from
                                        priced sessions</span
                                    >
                                    <strong
                                        >{fmtCost(
                                            routing.estimated_savings_if_rerouted,
                                        )}</strong
                                    >
                                </div>
                            {:else if !routing.savings_estimate_available}
                                <p class="routing-coverage">
                                    Savings are withheld until cost coverage is
                                    exact.
                                </p>
                            {/if}
                        </section>
                    {/if}

                    <section class="card">
                        <h3 class="card-title">Inflection points</h3>
                        <p class="card-sub">
                            Daily shifts at least 2x the prior baseline in the {windowLabel}
                            window.
                        </p>
                        {#if inflections.length === 0}
                            <div class="empty-inline">
                                No monetary-value shift crossed the inflection
                                threshold.
                            </div>
                        {:else}
                            <ul class="inflection-list">
                                {#each inflections.slice(0, 6) as point}
                                    <li
                                        class="inflection-item"
                                        class:spike={point.direction ===
                                            "spike"}
                                        class:drop={point.direction === "drop"}
                                    >
                                        <div class="inflection-head">
                                            <span class="inflection-date"
                                                >{point.date}</span
                                            >
                                            <span class="inflection-mult"
                                                >{point.multiplier.toFixed(
                                                    1,
                                                )}×</span
                                            >
                                        </div>
                                        <div class="inflection-note">
                                            {point.note}
                                        </div>
                                        <div class="inflection-stats">
                                            {point.sessions_on_day} session{point.sessions_on_day ===
                                            1
                                                ? ""
                                                : "s"}
                                            · {fmtCost(point.cost_on_day)}
                                        </div>
                                    </li>
                                {/each}
                            </ul>
                        {/if}
                    </section>
                </div>

                {#if trace && trace.total_sessions > 0 && trace.traced_sessions > 0}
                    {@const tracedPct =
                        trace.total_sessions > 0
                            ? (trace.traced_sessions / trace.total_sessions) *
                              100
                            : 0}
                    {@const mcpPct =
                        trace.total_tool_calls > 0
                            ? (trace.mcp_tool_calls / trace.total_tool_calls) *
                              100
                            : 0}
                    <section class="card trace-card">
                        <header class="trace-head">
                            <div>
                                <h3 class="card-title">Trace coverage</h3>
                                <p class="card-sub">
                                    Trace, tool, MCP, and compaction coverage
                                    for the {windowLabel} window.
                                </p>
                                <p class="trace-meta">
                                    {trace.total_sessions} session{trace.total_sessions ===
                                    1
                                        ? ""
                                        : "s"} · {trace.provider_display} · {trace.instruction_file}
                                </p>
                            </div>
                            <div class="trace-badge">
                                <span class="trace-badge-num"
                                    >{tracedPct.toFixed(0)}%</span
                                >
                                <span class="trace-badge-lbl">traced</span>
                            </div>
                        </header>

                        <div class="trace-grid">
                            <div class="mini-kv">
                                <span>Traced sessions</span><strong
                                    >{trace.traced_sessions}/{trace.total_sessions}</strong
                                >
                            </div>
                            <div class="mini-kv">
                                <span>Tool calls</span><strong
                                    >{trace.total_tool_calls.toLocaleString()}</strong
                                >
                            </div>
                            <div class="mini-kv">
                                <span>MCP share</span><strong
                                    >{mcpPct.toFixed(0)}%</strong
                                >
                            </div>
                            <div class="mini-kv">
                                <span>Compactions</span><strong
                                    >{trace.total_compactions}</strong
                                >
                            </div>
                        </div>

                        {#if trace.top_tools.length > 0}
                            <div class="trace-top-tools">
                                <div class="trace-subtitle">Top tools</div>
                                <div class="trace-tool-list">
                                    {#each trace.top_tools.slice(0, 6) as t}
                                        <div class="trace-tool-row">
                                            <span class="trace-tool-name"
                                                >{t.name}</span
                                            >
                                            <div class="trace-tool-bar-wrap">
                                                <div
                                                    class="trace-tool-bar"
                                                    style="width:{Math.min(
                                                        100,
                                                        t.share_pct,
                                                    )}%"
                                                ></div>
                                            </div>
                                            <span class="trace-tool-count"
                                                >{t.calls}</span
                                            >
                                        </div>
                                    {/each}
                                </div>
                            </div>
                        {/if}
                    </section>
                {/if}

                {#if health || tools || prompts}
                    {@const healthOn = !!(health && health.available)}
                    {@const toolsOn = !!(tools && tools.available)}
                    <div class="grid-2" class:single={!(healthOn && toolsOn)}>
                        {#if health && health.available}
                            <section class="card">
                                <h3 class="card-title">Session health</h3>
                                <p class="card-sub">
                                    Duration, overlap, and compaction signals
                                    across the {windowLabel} window.
                                </p>
                                <div class="health-hero">
                                    <div
                                        class="health-grade grade-{health.grade.toLowerCase()}"
                                    >
                                        {health.grade}
                                    </div>
                                    <div class="health-score">
                                        {health.health_score}<span
                                            class="health-score-sub">/100</span
                                        >
                                    </div>
                                </div>
                                <p class="diagnosis section-diagnosis">
                                    {health.diagnosis}
                                </p>
                                <div class="mini-grid">
                                    <div class="mini-kv">
                                        <span>Avg duration</span><strong
                                            >{health.avg_duration_minutes.toFixed(
                                                1,
                                            )} min</strong
                                        >
                                    </div>
                                    <div class="mini-kv">
                                        <span>P90 duration</span><strong
                                            >{health.p90_duration_minutes} min</strong
                                        >
                                    </div>
                                    <div class="mini-kv">
                                        <span>Long sessions</span><strong
                                            >{health.long_session_pct.toFixed(
                                                0,
                                            )}%</strong
                                        >
                                    </div>
                                    <div class="mini-kv">
                                        <span>Msgs / session</span><strong
                                            >{health.avg_messages_per_session.toFixed(
                                                1,
                                            )}</strong
                                        >
                                    </div>
                                    <div class="mini-kv">
                                        <span>Peak overlap</span><strong
                                            >{health.peak_overlap_pct}%</strong
                                        >
                                    </div>
                                    <div class="mini-kv">
                                        <span>Compact gaps</span><strong
                                            >{health.compact_gap_pct.toFixed(
                                                0,
                                            )}%</strong
                                        >
                                    </div>
                                </div>
                            </section>
                        {/if}

                        {#if tools && tools.available}
                            <section class="card">
                                <h3 class="card-title">Tool frequency</h3>
                                <p class="card-sub">
                                    Tool-call volume and MCP share across the {windowLabel}
                                    window.
                                </p>
                                <p class="diagnosis section-diagnosis">
                                    {tools.diagnosis}
                                </p>
                                <div class="mini-grid">
                                    <div class="mini-kv">
                                        <span>Total calls</span><strong
                                            >{tools.total_tool_calls.toLocaleString()}</strong
                                        >
                                    </div>
                                    <div class="mini-kv">
                                        <span>Avg / session</span><strong
                                            >{tools.avg_tools_per_session.toFixed(
                                                1,
                                            )}</strong
                                        >
                                    </div>
                                    <div class="mini-kv">
                                        <span>Calls / hour</span><strong
                                            >{tools.avg_tool_calls_per_hour.toFixed(
                                                1,
                                            )}</strong
                                        >
                                    </div>
                                    <div class="mini-kv">
                                        <span>MCP share</span><strong
                                            >{tools.mcp_share_pct.toFixed(
                                                0,
                                            )}%</strong
                                        >
                                    </div>
                                </div>
                                {#if tools.top_tools.length > 0}
                                    <div class="tool-list">
                                        {#each tools.top_tools.slice(0, 8) as t}
                                            <div class="tool-row">
                                                <span class="tool-name"
                                                    >{t.name}</span
                                                >
                                                <div class="tool-bar-wrap">
                                                    <div
                                                        class="tool-bar"
                                                        style="width:{Math.min(
                                                            100,
                                                            t.share_pct,
                                                        )}%"
                                                    ></div>
                                                </div>
                                                <span class="tool-count"
                                                    >{t.count} · {t.share_pct.toFixed(
                                                        1,
                                                    )}%</span
                                                >
                                            </div>
                                        {/each}
                                    </div>
                                {/if}
                            </section>
                        {/if}
                    </div>
                {/if}

                {#if prompts && prompts.available}
                    <section class="card">
                        <h3 class="card-title">Prompt complexity</h3>
                        <p class="card-sub">
                            Complexity and specificity scores for first prompts
                            in the {windowLabel} window.
                        </p>
                        <p class="diagnosis section-diagnosis">
                            {prompts.diagnosis}
                        </p>
                        <div class="mini-grid four">
                            <div class="mini-kv">
                                <span>Prompts analyzed</span><strong
                                    >{prompts.prompts_analyzed.toLocaleString()}</strong
                                >
                            </div>
                            <div class="mini-kv">
                                <span>Avg complexity</span><strong
                                    >{prompts.avg_complexity_score.toFixed(
                                        0,
                                    )}/100</strong
                                >
                            </div>
                            <div class="mini-kv">
                                <span>Avg specificity</span><strong
                                    >{prompts.avg_specificity_score.toFixed(
                                        0,
                                    )}/100</strong
                                >
                            </div>
                            <div class="mini-kv">
                                <span>Low specificity</span><strong
                                    >{prompts.low_specificity_sessions}</strong
                                >
                            </div>
                        </div>
                        {#if prompts.top_sessions.length > 0}
                            <div class="prompt-list">
                                {#each prompts.top_sessions.slice(0, 5) as s}
                                    <div class="prompt-item">
                                        <div class="prompt-head">
                                            <span class="prompt-project"
                                                >{s.project}</span
                                            >
                                            <span class="prompt-label"
                                                >{s.label}</span
                                            >
                                            <span class="prompt-scores"
                                                >C:{s.complexity_score} · S:{s.specificity_score}</span
                                            >
                                        </div>
                                        <div class="prompt-preview">
                                            {s.preview}
                                        </div>
                                    </div>
                                {/each}
                            </div>
                        {/if}
                    </section>
                {/if}

                <section class="card">
                    <header class="recs-header">
                        <div>
                            <h3 class="card-title">Recommendations</h3>
                            <p class="card-sub">
                                Patterns Pulse spotted in your last {days} days.
                            </p>
                        </div>
                        {#if actionableRecs.length > 0}
                            <div class="severity-tabs">
                                {#each severityTabs as t}
                                    <button
                                        class="seg-btn"
                                        class:active={severityFilter === t.id}
                                        onclick={() => (severityFilter = t.id)}
                                    >
                                        {t.label}
                                        {#if t.id !== "all"}
                                            <span class="count-pill">
                                                {actionableRecs.filter(
                                                    (r) => r.severity === t.id,
                                                ).length}
                                            </span>
                                        {/if}
                                    </button>
                                {/each}
                            </div>
                        {/if}
                    </header>

                    {#if !hasLoaded}
                        <div class="empty-inline">Loading…</div>
                    {:else if actionableRecs.length === 0}
                        <div class="empty-good">
                            <span class="eg-check" aria-hidden="true">✓</span>
                            <strong>No actions suggested</strong>
                            <span
                                >{$selectedAnalyticsProviderScope === "all"
                                    ? "Select a provider for its specific recommendations."
                                    : "No thresholds crossed in this window."}</span
                            >
                        </div>
                    {:else if sortedRecs.length === 0}
                        <div class="empty-inline">
                            No items match this filter.
                        </div>
                    {:else}
                        <ul class="rec-list">
                            {#each sortedRecs as rec}
                                <li
                                    class="rec-item"
                                    style="--rec-color: {rec.color}"
                                >
                                    <div class="rec-head">
                                        <span
                                            class="severity-pill"
                                            style="background: {rec.color}22; color: {rec.color}; border-color: {rec.color}55;"
                                        >
                                            {rec.severity}
                                        </span>
                                        <h4 class="rec-title">{rec.title}</h4>
                                    </div>
                                    <p class="rec-desc">{rec.description}</p>
                                    {#if rec.estimated_savings}
                                        <p class="rec-meta">
                                            <span class="meta-key"
                                                >Potential savings</span
                                            >
                                            <span class="meta-val accent"
                                                >{rec.estimated_savings}</span
                                            >
                                        </p>
                                    {/if}
                                    <p class="rec-meta">
                                        <span class="meta-key">Action</span>
                                        <span class="meta-val"
                                            >{rec.action}</span
                                        >
                                    </p>
                                    {#if rec.fix_prompt}
                                        <div class="rec-footer">
                                            <button
                                                class="btn-fix"
                                                onclick={() => handleFix(rec)}
                                            >
                                                <IconCopy
                                                    size={13}
                                                    stroke={2.2}
                                                    aria-hidden="true"
                                                />
                                                Fix with {$providerProfile.productName}
                                            </button>
                                        </div>
                                    {/if}
                                </li>
                            {/each}
                        </ul>
                    {/if}
                </section>
            {/if}
        </div>
    {/if}
</div>

<style>
    .reports-view {
        display: flex;
        flex-direction: column;
        gap: var(--page-gap);
    }

    .view-header {
        display: flex;
        justify-content: space-between;
        align-items: flex-start;
        gap: 16px;
        flex-wrap: wrap;
    }

    .view-title {
        font-size: 22px;
        font-weight: 800;
        letter-spacing: -0.01em;
    }

    .view-sub {
        font-size: 12px;
        color: var(--text-muted);
        max-width: 620px;
        margin-top: 3px;
        line-height: 1.45;
    }

    .controls {
        display: flex;
        gap: 12px;
        align-items: flex-end;
    }
    .export-group {
        display: grid;
        gap: 4px;
    }
    .export-kicker {
        color: var(--text-muted);
        font-size: 9px;
        font-weight: 700;
        letter-spacing: 0.08em;
        line-height: 1;
        text-transform: uppercase;
    }
    .export-actions {
        display: flex;
        gap: 8px;
    }

    .count-pill {
        font-size: 10px;
        font-weight: 700;
        background: var(--bg-elevated);
        padding: 1px 6px;
        border-radius: var(--radius-full);
        corner-shape: round;
    }

    .btn-primary,
    .btn-secondary {
        font-size: 12px;
        font-weight: 600;
        padding: 7px 12px;
        border-radius: var(--radius-md);
        border: 1px solid var(--border);
        cursor: pointer;
        transition: all 0.15s var(--ease);
        display: inline-flex;
        align-items: center;
        gap: 6px;
    }

    .btn-primary {
        background: var(--accent);
        /* Must be the accent's paired foreground, not a fixed dark value: in the
       light theme --accent is near-black, so a hardcoded dark colour renders
       dark-on-dark and the label disappears. */
        color: var(--accent-fg);
        border-color: var(--accent);
    }
    .btn-primary:hover {
        background: var(--accent-hover);
        filter: brightness(1.05);
    }

    .btn-secondary {
        background: var(--bg-elevated);
        color: var(--text-secondary);
    }
    .btn-secondary:hover {
        color: var(--accent);
        border-color: var(--accent);
    }
    .btn-primary:disabled,
    .btn-secondary:disabled {
        opacity: 0.45;
        cursor: not-allowed;
        filter: none;
    }

    .card {
        background: var(--bg-card);
        border: 1px solid var(--border);
        border-radius: var(--radius-lg);
        padding: 20px;
    }

    /* ── Cost timeline hero ──────────────────────────────────────────────
     The timeline is the spine of this screen, so it sits on the page
     surface with generous padding rather than inside a card competing
     with the sections below it. */
    .timeline-hero {
        display: flex;
        flex-direction: column;
        gap: 18px;
        padding: 22px 24px 20px;
        background: var(--bg-card);
        border: 1px solid var(--border);
        border-radius: var(--radius-lg);
    }
    .th-head {
        display: flex;
        align-items: flex-start;
        justify-content: space-between;
        gap: 20px;
    }
    .th-titles {
        display: flex;
        flex-direction: column;
        gap: 4px;
    }
    .th-title {
        font-size: 12px;
        font-weight: 700;
        text-transform: uppercase;
        letter-spacing: 0.07em;
        color: var(--accent);
    }
    .th-sub {
        font-size: var(--fs-sm);
        color: var(--text-muted);
        max-width: 62ch;
    }

    /* Readouts separated by hairlines and spacing only — no nested cards. */
    .th-stats {
        display: grid;
        grid-template-columns: repeat(4, 1fr);
        gap: 0;
        padding-top: 16px;
        border-top: 1px solid var(--border);
    }
    .th-stat {
        display: flex;
        flex-direction: column;
        gap: 3px;
        padding: 0 18px;
        border-left: 1px solid var(--border);
    }
    .th-stat:first-child {
        padding-left: 0;
        border-left: none;
    }
    .ths-label {
        font-size: var(--fs-xs);
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: var(--letter-wider);
        color: var(--text-muted);
    }
    .ths-value {
        font-family: var(--font-mono);
        font-size: var(--fs-xl);
        font-weight: 700;
        letter-spacing: var(--letter-tight);
        color: var(--text-primary);
        font-variant-numeric: tabular-nums;
    }
    .ths-meta {
        font-size: var(--fs-xs);
        color: var(--text-muted);
    }

    @media (max-width: 900px) {
        .th-stats {
            grid-template-columns: repeat(2, 1fr);
            row-gap: 16px;
        }
        .th-stat:nth-child(3) {
            padding-left: 0;
            border-left: none;
        }
    }

    .card-title {
        font-size: 12px;
        font-weight: 700;
        text-transform: uppercase;
        letter-spacing: 0.07em;
        color: var(--accent);
        margin-bottom: 6px;
    }

    .card-sub {
        font-size: 12px;
        color: var(--text-muted);
        line-height: 1.5;
        margin-bottom: 14px;
    }

    .empty-inline {
        color: var(--text-muted);
        font-size: 13px;
        padding: 18px;
        text-align: center;
    }

    .empty-good {
        display: flex;
        flex-direction: column;
        align-items: center;
        gap: 6px;
        padding: 32px 18px;
        text-align: center;
    }
    .empty-good .eg-check {
        display: grid;
        place-items: center;
        width: 34px;
        height: 34px;
        margin-bottom: 4px;
        border-radius: var(--radius-full);
        corner-shape: round;
        color: var(--success);
        background: var(--success-dim);
        font-size: 16px;
        font-weight: 700;
    }
    .empty-good strong {
        color: var(--text-primary);
        font-size: var(--fs-md);
    }
    .empty-good span {
        color: var(--text-muted);
        font-size: var(--fs-sm);
    }

    .two-col {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 16px;
    }

    .two-col.single {
        grid-template-columns: 1fr;
    }

    @media (max-width: 960px) {
        .two-col {
            grid-template-columns: 1fr;
        }
    }

    @media (max-width: 700px) {
        .view-header {
            flex-direction: column;
        }
        .controls {
            width: 100%;
            flex-wrap: wrap;
            align-items: stretch;
        }
        .export-group {
            flex: 1 1 100%;
        }
        .export-actions {
            width: 100%;
        }
        .btn-primary,
        .btn-secondary {
            flex: 1;
            justify-content: center;
        }
    }

    .hero-card {
        display: grid;
        grid-template-columns: 260px 1fr;
        gap: 24px;
        align-items: center;
        padding: 24px;
        /* Matte hero surface with a subtle sheen, not a two-stop gray gradient. */
        position: relative;
        background: var(--panel-sheen-strong), var(--surface-panel);
        border: 1px solid var(--border);
        border-radius: var(--radius-lg);
        box-shadow: var(--elev-2);
    }
    .hero-card::before {
        content: "";
        position: absolute;
        inset: 0 0 auto 0;
        height: 1px;
        background: var(--panel-edge);
        border-radius: var(--radius-lg) var(--radius-lg) 0 0;
        pointer-events: none;
    }

    @media (max-width: 820px) {
        .hero-card {
            grid-template-columns: 1fr;
        }
    }

    .hero-left {
        display: flex;
        align-items: center;
        gap: 20px;
    }

    .grade-letter {
        font-size: 96px;
        font-weight: 900;
        line-height: 0.95;
        letter-spacing: -0.04em;
    }

    .grade-meta .label {
        font-size: 10px;
        font-weight: 700;
        text-transform: uppercase;
        letter-spacing: 0.1em;
        color: var(--text-muted);
    }

    .ratio {
        font-size: 24px;
        font-weight: 800;
        margin-top: 4px;
    }

    .pct {
        font-size: 16px;
        font-weight: 600;
        color: var(--text-muted);
    }

    .muted {
        color: var(--text-muted);
        font-weight: 500;
        font-size: 14px;
    }

    .hero-right {
        display: flex;
        flex-direction: column;
        gap: 14px;
    }

    .diagnosis {
        font-size: 13px;
        line-height: 1.55;
        color: var(--text-secondary);
    }
    .hero-sub {
        margin-bottom: 0;
    }
    .section-diagnosis {
        margin: -4px 0 14px;
    }
    .trace-meta {
        margin-top: -8px;
        color: var(--text-muted);
        font-size: 11px;
    }

    .hero-stats {
        display: grid;
        grid-template-columns: repeat(4, 1fr);
        gap: 10px;
    }

    .hero-stat {
        background: var(--bg-card);
        border: 1px solid var(--border);
        padding: 8px 12px;
        border-radius: var(--radius-md);
        display: flex;
        flex-direction: column;
        gap: 2px;
    }

    .hs-label {
        font-size: 10px;
        font-weight: 700;
        text-transform: uppercase;
        letter-spacing: 0.06em;
        color: var(--text-muted);
    }

    .hs-value {
        font-size: 15px;
        font-weight: 700;
        font-variant-numeric: tabular-nums;
    }

    .routing-bars {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }

    .bar-row {
        display: grid;
        grid-template-columns: 130px 1fr 110px;
        gap: 12px;
        align-items: center;
        font-size: 12px;
    }

    .bar-label {
        display: flex;
        gap: 8px;
        align-items: center;
        font-weight: 600;
    }

    .bar-count {
        color: var(--text-muted);
        font-weight: 400;
    }

    .dot {
        width: 9px;
        height: 9px;
        border-radius: 50%;
        corner-shape: round;
        display: inline-block;
    }

    .bar-track {
        height: 8px;
        background: var(--bg-elevated);
        border-radius: var(--radius-full);
        corner-shape: round;
        overflow: hidden;
    }

    .bar-fill {
        height: 100%;
        border-radius: var(--radius-full);
        corner-shape: round;
        transition: width 0.3s var(--ease);
    }

    .bar-value {
        text-align: right;
        font-weight: 700;
        font-variant-numeric: tabular-nums;
        display: flex;
        justify-content: flex-end;
        gap: 8px;
    }

    .savings-hint {
        margin-top: 14px;
        padding: 10px 14px;
        background: var(--accent-dim);
        border-radius: var(--radius-md);
        font-size: 12px;
        color: var(--text-secondary);
        display: flex;
        justify-content: space-between;
        align-items: center;
    }

    .savings-hint strong {
        color: var(--accent);
        font-size: 14px;
        font-variant-numeric: tabular-nums;
    }

    .inflection-list {
        list-style: none;
        display: flex;
        flex-direction: column;
        gap: 10px;
    }

    .inflection-item {
        padding: 12px 14px;
        background: var(--bg-elevated);
        border-radius: var(--radius-md);
        border-left: 3px solid var(--text-muted);
    }

    .inflection-item.spike {
        border-left-color: var(--warning);
    }

    .inflection-item.drop {
        border-left-color: var(--success);
    }

    .inflection-head {
        display: flex;
        justify-content: space-between;
        align-items: baseline;
    }

    .inflection-date {
        font-weight: 700;
        font-size: 13px;
    }

    .inflection-mult {
        font-size: 15px;
        font-weight: 800;
        font-variant-numeric: tabular-nums;
    }

    .inflection-note {
        font-size: 12px;
        color: var(--text-secondary);
        margin-top: 2px;
        line-height: 1.5;
    }

    .inflection-stats {
        font-size: 11px;
        color: var(--text-muted);
        margin-top: 4px;
    }

    .recs-header {
        display: flex;
        justify-content: space-between;
        align-items: flex-start;
        gap: 16px;
        margin-bottom: 14px;
        flex-wrap: wrap;
    }

    .severity-tabs {
        display: flex;
        gap: 2px;
        flex-wrap: wrap;
        background: var(--bg-elevated);
        padding: 2px;
        border-radius: var(--radius-md);
        border: 1px solid var(--border);
    }

    .rec-list {
        list-style: none;
        display: flex;
        flex-direction: column;
        gap: 10px;
    }

    .rec-item {
        padding: 14px 16px;
        background: var(--bg-elevated);
        border-radius: var(--radius-md);
        border-left: 3px solid var(--rec-color, var(--accent));
    }

    .rec-head {
        display: flex;
        align-items: center;
        gap: 10px;
        margin-bottom: 6px;
    }

    .severity-pill {
        font-size: 10px;
        font-weight: 700;
        text-transform: uppercase;
        letter-spacing: 0.06em;
        padding: 3px 8px;
        border-radius: var(--radius-full);
        corner-shape: round;
        border: 1px solid;
    }

    .rec-title {
        font-size: 14px;
        font-weight: 700;
        line-height: 1.3;
    }

    .rec-desc {
        font-size: 13px;
        color: var(--text-secondary);
        line-height: 1.55;
        margin-top: 4px;
    }

    .rec-meta {
        margin-top: 6px;
        font-size: 12px;
        color: var(--text-secondary);
        line-height: 1.5;
        display: flex;
        gap: 8px;
    }

    .meta-key {
        color: var(--text-muted);
        font-weight: 700;
        text-transform: uppercase;
        letter-spacing: 0.05em;
        font-size: 10px;
        flex-shrink: 0;
        padding-top: 2px;
    }

    .meta-val {
        flex: 1;
    }

    .meta-val.accent {
        color: var(--accent);
        font-weight: 700;
    }

    .rec-footer {
        margin-top: 10px;
    }

    .btn-fix {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        font-size: 12px;
        font-weight: 600;
        padding: 6px 12px;
        border-radius: var(--radius-sm);
        background: var(--accent-dim);
        color: var(--accent);
        border: 1px solid var(--accent);
        cursor: pointer;
        transition: all 0.15s var(--ease);
    }

    .btn-fix:hover {
        background: var(--accent);
        color: var(--accent-fg);
    }

    .report-body {
        display: flex;
        flex-direction: column;
        gap: 16px;
        transition: opacity 0.2s var(--ease);
    }

    .report-body.reloading {
        opacity: 0.5;
        pointer-events: none;
    }

    .reload-banner {
        display: flex;
        align-items: center;
        gap: 8px;
        font-size: 12px;
        font-weight: 600;
        color: var(--accent);
        background: var(--accent-dim);
        border: 1px solid var(--border);
        border-radius: var(--radius-md);
        padding: 8px 14px;
    }

    .reload-spinner {
        width: 12px;
        height: 12px;
        border: 2px solid var(--border);
        border-top-color: var(--accent);
        border-radius: 50%;
        corner-shape: round;
        animation: spin 0.8s linear infinite;
        flex-shrink: 0;
    }

    @keyframes spin {
        to {
            transform: rotate(360deg);
        }
    }

    .reports-loading {
        display: flex;
        flex-direction: column;
        gap: 16px;
    }
    .loading-card {
        display: flex;
        flex-direction: column;
        gap: 12px;
        min-height: 150px;
        padding: 20px;
        background: var(--bg-card);
        border: 1px solid var(--border);
        border-radius: var(--radius-lg);
    }
    .loading-timeline {
        min-height: 330px;
    }
    .loading-hero {
        display: grid;
        grid-template-columns: 180px 1fr;
        align-items: center;
        min-height: 190px;
    }
    .loading-content {
        display: flex;
        flex-direction: column;
        gap: 12px;
    }
    .loading-grid {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 16px;
    }
    .loading-label {
        width: min(150px, 42%);
        height: 12px;
    }
    .loading-copy {
        width: min(430px, 72%);
        height: 10px;
    }
    .loading-block {
        flex: 1;
        min-height: 76px;
    }
    .timeline-block {
        min-height: 180px;
    }
    .loading-grade {
        width: 116px;
        height: 112px;
    }
    .loading-metrics {
        display: grid;
        grid-template-columns: repeat(4, 1fr);
        gap: 10px;
    }
    .loading-metrics .skeleton {
        height: 48px;
    }
    @media (max-width: 820px) {
        .loading-hero,
        .loading-grid {
            grid-template-columns: 1fr;
        }
    }
    @media (max-width: 620px) {
        .loading-metrics {
            grid-template-columns: repeat(2, 1fr);
        }
    }

    .grid-2 {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 14px;
    }
    .grid-2.single {
        grid-template-columns: 1fr;
    }
    @media (max-width: 900px) {
        .grid-2 {
            grid-template-columns: 1fr;
        }
    }

    .health-hero {
        display: flex;
        align-items: baseline;
        gap: 14px;
        margin: 6px 0 10px;
    }
    .health-grade {
        font-size: 34px;
        font-weight: 800;
        letter-spacing: 0.02em;
        line-height: 1;
        padding: 6px 14px;
        border-radius: var(--radius-md);
        background: var(--bg-elevated);
        color: var(--text-primary);
    }
    .health-grade.grade-a {
        color: var(--success);
        background: var(--success-dim);
    }
    .health-grade.grade-b {
        color: var(--token-cache-write);
        background: var(--success-dim);
    }
    .health-grade.grade-c {
        color: var(--warning);
        background: var(--warning-dim);
    }
    .health-grade.grade-d {
        color: var(--warning);
        background: var(--warning-dim);
    }
    .health-grade.grade-f {
        color: var(--danger);
        background: var(--danger-dim);
    }
    .health-score {
        font-size: 28px;
        font-weight: 700;
        color: var(--text-primary);
        font-variant-numeric: tabular-nums;
    }
    .health-score-sub {
        font-size: 14px;
        font-weight: 500;
        color: var(--text-muted);
        margin-left: 2px;
    }

    .mini-grid {
        display: grid;
        grid-template-columns: repeat(3, 1fr);
        gap: 8px 14px;
        margin-top: 12px;
    }
    .mini-grid.four {
        grid-template-columns: repeat(4, 1fr);
    }
    .mini-kv {
        display: flex;
        flex-direction: column;
        padding: 8px 10px;
        background: var(--bg-primary);
        border: 1px solid var(--border);
        border-radius: var(--radius-sm);
    }
    .mini-kv span {
        font-size: 10px;
        font-weight: 600;
        color: var(--text-muted);
        text-transform: uppercase;
        letter-spacing: 0.05em;
    }
    .mini-kv strong {
        font-size: 14px;
        font-weight: 700;
        color: var(--text-primary);
        font-variant-numeric: tabular-nums;
        margin-top: 2px;
    }

    .tool-list {
        display: flex;
        flex-direction: column;
        gap: 6px;
        margin-top: 14px;
    }
    .tool-row {
        display: grid;
        grid-template-columns: 120px 1fr 110px;
        gap: 10px;
        align-items: center;
        font-size: 12px;
    }
    .tool-name {
        color: var(--text-primary);
        font-weight: 600;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .tool-bar-wrap {
        background: var(--bg-elevated);
        height: 6px;
        border-radius: var(--radius-full);
        corner-shape: round;
        overflow: hidden;
    }
    .tool-bar {
        background: var(--accent);
        height: 100%;
        border-radius: var(--radius-full);
        corner-shape: round;
        transition: width 0.4s var(--ease);
    }
    .tool-count {
        text-align: right;
        color: var(--text-muted);
        font-variant-numeric: tabular-nums;
        font-size: 11px;
    }

    .prompt-list {
        display: flex;
        flex-direction: column;
        gap: 8px;
        margin-top: 12px;
    }
    .prompt-item {
        padding: 10px 12px;
        background: var(--bg-primary);
        border: 1px solid var(--border);
        border-radius: var(--radius-sm);
    }
    .prompt-head {
        display: flex;
        gap: 10px;
        align-items: center;
        margin-bottom: 4px;
        font-size: 11px;
    }
    .prompt-project {
        font-weight: 700;
        color: var(--text-primary);
    }
    .prompt-label {
        color: var(--accent);
        background: var(--accent-dim);
        padding: 2px 8px;
        border-radius: var(--radius-full);
        corner-shape: round;
        font-size: 10px;
        font-weight: 600;
        letter-spacing: 0.02em;
    }
    .prompt-scores {
        margin-left: auto;
        font-variant-numeric: tabular-nums;
        color: var(--text-muted);
        font-size: 11px;
    }
    .prompt-preview {
        font-size: 11px;
        color: var(--text-secondary);
        line-height: 1.4;
        overflow: hidden;
        display: -webkit-box;
        -webkit-line-clamp: 2;
        line-clamp: 2;
        -webkit-box-orient: vertical;
    }

    /* Session Topology (trace) */
    .trace-card {
        background: var(--panel-sheen), var(--surface-panel);
        box-shadow: var(--elev-1);
    }
    .trace-head {
        display: flex;
        justify-content: space-between;
        align-items: flex-start;
        gap: 16px;
        margin-bottom: 12px;
        flex-wrap: wrap;
    }
    .trace-badge {
        display: flex;
        flex-direction: column;
        align-items: flex-end;
        gap: 2px;
        padding: 8px 14px;
        background: var(--accent-dim);
        border: 1px solid var(--accent);
        border-radius: var(--radius-md);
    }
    .trace-badge-num {
        font-size: 20px;
        font-weight: 800;
        color: var(--accent);
        font-variant-numeric: tabular-nums;
        line-height: 1;
        letter-spacing: -0.01em;
    }
    .trace-badge-lbl {
        font-size: 9px;
        font-weight: 700;
        text-transform: uppercase;
        letter-spacing: 0.08em;
        color: var(--text-muted);
    }
    .trace-grid {
        display: grid;
        grid-template-columns: repeat(4, 1fr);
        gap: 8px;
        margin-bottom: 14px;
    }
    @media (max-width: 700px) {
        .trace-grid {
            grid-template-columns: repeat(2, 1fr);
        }
    }
    .trace-top-tools {
        margin-top: 6px;
        padding-top: 12px;
        border-top: 1px solid var(--border);
    }
    .trace-subtitle {
        font-size: 10px;
        font-weight: 700;
        text-transform: uppercase;
        letter-spacing: 0.07em;
        color: var(--text-muted);
        margin-bottom: 8px;
    }
    .trace-tool-list {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }
    .trace-tool-row {
        display: grid;
        grid-template-columns: 140px 1fr 56px;
        gap: 12px;
        align-items: center;
        font-size: 12px;
    }
    .trace-tool-name {
        color: var(--text-primary);
        font-weight: 600;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        font-family: "JetBrains Mono", monospace;
        font-size: 11px;
    }
    .trace-tool-bar-wrap {
        background: var(--bg-primary);
        height: 6px;
        border-radius: var(--radius-full);
        corner-shape: round;
        overflow: hidden;
    }
    .trace-tool-bar {
        background: var(--accent);
        height: 100%;
        border-radius: var(--radius-full);
        corner-shape: round;
        opacity: 0.85;
    }
    .trace-tool-count {
        text-align: right;
        color: var(--text-muted);
        font-variant-numeric: tabular-nums;
        font-size: 11px;
        font-weight: 600;
    }

    @media (max-width: 620px) {
        .hero-stats,
        .mini-grid,
        .mini-grid.four {
            grid-template-columns: repeat(2, minmax(0, 1fr));
        }
    }

    .report-error {
        min-height: 220px;
        display: grid;
        place-content: center;
        justify-items: center;
        gap: 9px;
        padding: 32px;
        text-align: center;
        border-block: 1px solid var(--border);
    }
    .report-error strong {
        color: var(--danger);
        font-size: var(--fs-lg);
    }
    .report-error span {
        max-width: 560px;
        color: var(--text-muted);
        font-size: var(--fs-sm);
        line-height: 1.55;
    }
    .report-error button {
        margin-top: 6px;
        padding: 7px 14px;
        color: var(--text-primary);
        background: var(--bg-elevated);
        border: 1px solid var(--border);
        border-radius: var(--radius-sm);
        cursor: pointer;
    }
    .report-empty {
        min-height: min(430px, 48vh);
        display: grid;
        place-content: center;
        justify-items: center;
        gap: 8px;
        padding: 34px;
        color: var(--text-muted);
        text-align: center;
        border-block: 1px solid var(--border);
    }
    .report-empty strong {
        color: var(--text-primary);
        font-size: var(--fs-lg);
    }
    .report-empty span {
        max-width: 580px;
        font-size: var(--fs-sm);
        line-height: 1.55;
    }
    @media (max-width: 620px) {
        .timeline-head,
        .prompt-head {
            flex-wrap: wrap;
        }
        .tool-row,
        .trace-tool-row {
            grid-template-columns: minmax(0, 1fr) auto;
            gap: 6px 10px;
        }
        .tool-bar-wrap,
        .trace-tool-bar-wrap {
            grid-column: 1 / -1;
            grid-row: 2;
        }
        .tool-name,
        .trace-tool-name {
            white-space: normal;
            overflow-wrap: anywhere;
        }
        .card,
        .hero-card {
            padding: 16px;
        }
        .grade-letter {
            font-size: 64px;
        }
    }
    .card-title {
        font-size: 15px;
        text-transform: none;
        letter-spacing: -0.01em;
        color: var(--text-primary);
        font-weight: 650;
    }
    .card-sub {
        line-height: 1.6;
        max-width: 70ch;
    }
    .mini-kv {
        border: 0;
        border-top: 1px solid var(--divider);
        border-radius: 0;
        background: transparent;
        padding: 12px 0;
    }
    .mini-kv strong {
        font-size: 17px;
        font-weight: 650;
    }
    .trace-badge {
        border: 0;
        background: var(--bg-elevated);
    }
    .prompt-item {
        padding: 14px;
        background: var(--bg-elevated);
    }
    .prompt-head {
        flex-wrap: wrap;
        row-gap: 6px;
    }
    .prompt-preview {
        font-size: 12px;
        line-height: 1.65;
    }
    .empty-good {
        display: flex;
        flex-direction: row;
        justify-content: flex-start;
        align-items: center;
        gap: 12px;
        padding: 14px 0;
        min-height: 0;
        text-align: left;
        flex-wrap: wrap;
    }
    .empty-good .eg-check {
        width: 24px;
        height: 24px;
        font-size: 14px;
    }
</style>
