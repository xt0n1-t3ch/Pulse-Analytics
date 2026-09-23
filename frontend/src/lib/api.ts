import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type { AccessSnapshot, AnalyticsProviderScope } from "./access";

/**
 * Tauri command dispatch, with a development fallback.
 *
 * Inside the Pulse window `invoke` talks to the Rust backend over the webview
 * IPC. When the same bundle is opened in a plain browser (Vite dev server, for
 * UI review), that IPC does not exist, so calls are routed through Vite's
 * same-origin proxy to the authenticated Rust dev bridge. No provider fixture
 * or browser-side fallback exists in this path.
 */
const BRIDGE_PATH = "/__pulse_api";

/** True when running inside the Pulse webview, where Tauri IPC and events
 *  exist. False in a plain browser reviewing the UI through the dev bridge. */
export function hasTauriIpc(): boolean {
    return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    if (hasTauriIpc() || !import.meta.env.DEV) {
        return tauriInvoke<T>(command, args);
    }
    const res = await fetch(BRIDGE_PATH, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ command, args: args ?? {} }),
    });
    if (!res.ok) {
        const reason = await res.text();
        throw new Error(`real dev backend rejected ${command}: ${res.status} ${reason}`.trim());
    }
    return (await res.json()) as T;
}

const inFlightReads = new Map<string, Promise<unknown>>();

/** Share an identical read while it is in flight. This avoids serializing the
 * same SQLite snapshot twice when a store update and a mounted view react in
 * the same frame; completed reads are never cached, so freshness still belongs
 * to the backend. */
function invokeRead<T>(
    command: string,
    args?: Record<string, unknown>,
    observationIdentity?: string,
): Promise<T> {
    const key = `${command}:${JSON.stringify(args ?? {})}:${observationIdentity ?? ""}`;
    const existing = inFlightReads.get(key);
    if (existing) return existing as Promise<T>;
    const pending = invoke<T>(command, args).finally(() => {
        if (inFlightReads.get(key) === pending) inFlightReads.delete(key);
    });
    inFlightReads.set(key, pending);
    return pending;
}

export interface HealthResponse {
    version: string;
    uptime_seconds: number;
    discord_status: string;
    discord_enabled: boolean;
}

export interface ModelMetric {
    model: string;
    sessions: number;
    cost: number;
    tokens: number;
}

export interface MetricsResponse {
    total_cost: number;
    cost_available?: boolean;
    cost_basis?: CostBasis;
    input_tokens: number;
    pure_input_tokens: number;
    output_tokens: number;
    cache_write_tokens: number;
    cache_read_tokens: number;
    total_tokens: number;
    session_count: number;
    input_cost: number;
    output_cost: number;
    cache_write_cost: number;
    cache_read_cost: number;
    cache_hit_ratio: number;
    models: ModelMetric[];
}

export interface SubagentDetail {
    agent_type: string;
    model: string;
    tokens: number;
    cost: number;
    activity: string;
}

export interface SessionInfo {
    opencode?: OpenCodeMetadata | null;
    session_id: string;
    session_name: string | null;
    project: string;
    model: string;
    model_id: string;
    provider: string;
    context_window: string;
    cost: number;
    cost_available?: boolean;
    cost_basis?: CostBasis;
    cost_source?: string;
    tokens: number;
    input_tokens: number;
    output_tokens: number;
    cache_write_tokens: number;
    cache_read_tokens: number;
    context_used_tokens?: number;
    context_window_tokens?: number;
    branch: string | null;
    activity: string;
    activity_target: string | null;
    effort: string;
    effort_explicit: boolean;
    is_idle: boolean;
    started_at: string | null;
    duration_secs: number;
    has_thinking: boolean;
    workflow_label: string | null;
    subagent_count: number;
    subagents: SubagentDetail[];
    tokens_per_sec: number;
    input_cost: number;
    output_cost: number;
    cache_write_cost: number;
    cache_read_cost: number;
    speed: string;
    fast: boolean;
    service_tier: string | null;
    app_name?: string | null;
    has_inflated_tokenizer: boolean;
}

export interface RateLimitInfo {
    provider: string;
    usage: UsageSnapshot | null;
    five_hour_pct: number;
    five_hour_resets: string;
    five_hour_label: string;
    five_hour_window_minutes: number | null;
    seven_day_pct: number;
    seven_day_resets: string;
    seven_day_label: string;
    seven_day_window_minutes: number | null;
    sonnet_pct: number | null;
    sonnet_resets: string | null;
    extra_enabled: boolean;
    extra_limit: number | null;
    extra_used: number | null;
    extra_pct: number | null;
    source: string;
}

export interface CreditBalance {
    balance: string | null;
    has_credits: boolean;
    unlimited: boolean;
}

export interface QuotaWindow {
    window_minutes: number;
    used_percent: number;
    remaining_percent: number;
    resets_at: string | null;
}

export interface QuotaScope {
    id: string | null;
    name: string | null;
    kind: "global_account" | "individual_account" | "model" | "other";
    windows: QuotaWindow[];
}

export interface UsageSource {
    lane: "codex_subscription" | "open_ai_api" | "claude_subscription" | "anthropic_api" | "unknown";
    stream_id: string;
    signals: string[];
}

export interface UsageSnapshot {
    /** v1.6 compatibility fields retained by the backend. */
    provider: string;
    source: string;
    /** Structured v1.7 source identity. */
    usage_source: UsageSource;
    scopes: QuotaScope[];
    credits: CreditBalance | null;
    observed_at: string | null;
    provenance_source: string;
}

export interface DiscordUserInfo {
    user_id: string;
    username: string;
    global_name: string | null;
    discriminator: string;
    avatar_hash: string;
    avatar_url: string;
    /** Discord's built-in default avatar. Always resolves; used as the img
     *  onerror fallback when `avatar_url` points at a stale hash (CDN 404). */
    avatar_default_url: string;
    banner_hash: string | null;
    banner_url: string | null;
}

export interface PlanInfo {
    provider: string;
    /** Canonical plan key for the Settings select (e.g. "max_20x"); "" when unknown. */
    plan_key: string;
    plan_name: string;
    detected: boolean;
}

export interface ProviderInfo {
    active_provider: string;
}

export interface ProviderCapabilities {
    cache_health: boolean;
    model_routing: boolean;
    extra_usage: boolean;
}

export interface ProviderCopyInfo {
    provider: string;
    provider_label: string;
    instruction_file: string;
    home_dir: string;
    sessions_store: string;
    fix_label: string;
    global_state_source: string;
}

export interface TraceToolUsage {
    name: string;
    calls: number;
    share_pct: number;
}

export interface TraceOverview {
    provider: string;
    provider_display: string;
    instruction_file: string;
    fix_button_label: string;
    session_store: string;
    global_state_source: string;
    traced_sessions: number;
    total_sessions: number;
    user_messages: number;
    assistant_messages: number;
    total_tool_calls: number;
    total_compactions: number;
    mcp_tool_calls: number;
    cache_hit_ratio: number;
    top_tools: TraceToolUsage[];
    telemetry_mermaid: string;
    cache_mermaid: string;
}

export interface DiscordDisplayPrefs {
    show_project: boolean;
    show_branch: boolean;
    show_model: boolean;
    show_activity: boolean;
    show_tokens: boolean;
    show_cost: boolean;
    show_limits: boolean;
    show_credits: boolean;
    show_context: boolean;
    show_systems: boolean;
}

export interface DiscordPresencePreview {
    provider: string;
    app_name: string;
    details: string;
    state: string;
    large_image_key: string;
    large_text: string;
    small_image_key: string | null;
    small_text: string | null;
    has_session: boolean;
    duration_secs: number;
}

export interface DiscordSettings {
    provider: string;
    enabled: boolean;
    status: string;
    publisher: string;
    display_prefs: DiscordDisplayPrefs;
    desktop_design: "codex_app" | "chatgpt_app" | null;
    supports_desktop_design: boolean;
    supports_field_order: boolean;
    /** False for Claude — "Credits available" reads a Codex account balance that
     *  has no Claude equivalent, so the backend pins the flag off. */
    supports_credits: boolean;
    field_order: string[];
}

export interface AppSnapshot {
    opencode_diagnostics?: string[];
    revision: number;
    sync_state?: "syncing" | "live";
    snapshot_captured_at?: string;
    health: HealthResponse;
    metrics: MetricsResponse;
    sessions: SessionInfo[];
    rate_limits: RateLimitInfo | null;
    discord_preview: DiscordPresencePreview;
    discord_settings: DiscordSettings;
    plan: PlanInfo;
    access: AccessSnapshot;
}

export function getHealth(): Promise<HealthResponse> {
    return invoke("get_health");
}

export function getAppSnapshot(): Promise<AppSnapshot> {
    return invoke("get_app_snapshot");
}

export function getMetrics(): Promise<MetricsResponse> {
    return invoke("get_metrics");
}

export function getLiveSessions(): Promise<SessionInfo[]> {
    return invoke("get_live_sessions");
}

export function getDiscordPreview(): Promise<DiscordPresencePreview> {
    return invoke("get_discord_preview");
}

export function getDiscordSettings(): Promise<DiscordSettings> {
    return invoke("get_discord_settings");
}

export function getRateLimits(): Promise<RateLimitInfo | null> {
    return invoke("get_rate_limits");
}

export function getDiscordUser(): Promise<DiscordUserInfo | null> {
    return invoke("get_discord_user");
}

export function discordDisplayPrefsArgs(prefs: DiscordDisplayPrefs): Record<string, boolean> {
    return {
        showProject: prefs.show_project,
        showBranch: prefs.show_branch,
        showModel: prefs.show_model,
        showActivity: prefs.show_activity,
        showTokens: prefs.show_tokens,
        showCost: prefs.show_cost,
        showLimits: prefs.show_limits,
        showCredits: prefs.show_credits,
        showContext: prefs.show_context,
        showSystems: prefs.show_systems,
    };
}

export function setDiscordFieldOrder(order: string[]): Promise<DiscordSettings> {
    return invoke("set_discord_field_order", { order });
}

export function setDiscordEnabled(enabled: boolean): Promise<DiscordSettings> {
    return invoke("set_discord_enabled", { enabled });
}

export function setDiscordDisplayPrefs(prefs: DiscordDisplayPrefs): Promise<DiscordSettings> {
    return invoke("set_discord_display_prefs", discordDisplayPrefsArgs(prefs));
}

export function setCodexDesktopDesign(
    design: "codex_app" | "chatgpt_app",
): Promise<DiscordSettings> {
    return invoke("set_codex_desktop_design", { design });
}

export function getPlanInfo(): Promise<PlanInfo> {
    return invoke("get_plan_info");
}

export function getActiveProvider(): Promise<ProviderInfo> {
    return invoke("get_active_provider");
}

export function setActiveProvider(provider: string): Promise<void> {
    return invoke("set_active_provider", { provider });
}

export interface AppSettings {
    close_to_tray: boolean;
}

export function getAppSettings(): Promise<AppSettings> {
    return invoke("get_app_settings");
}

export function setCloseToTray(enabled: boolean): Promise<AppSettings> {
    return invoke("set_close_to_tray", { enabled });
}

export function getProviderCopy(): Promise<ProviderCopyInfo> {
    return invoke("get_provider_copy");
}

export function getTraceOverview(
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<TraceOverview> {
    return invoke("get_trace_overview", { days: days ?? null, provider: provider ?? null });
}

export function setPlanOverride(plan: string, provider?: "codex" | "claude"): Promise<void> {
    return invoke("set_plan_override", { plan, provider: provider ?? null });
}

export type CostBasis = "exact" | "partial" | "estimated" | "unavailable";

export interface HistoricalSession {
    opencode?: OpenCodeMetadata | null;
    id: string;
    provider: string;
    session_name: string | null;
    project: string;
    model: string;
    model_id: string;
    context_window: string;
    branch: string | null;
    effort: string;
    started_at: string | null;
    ended_at: string | null;
    duration_secs: number;
    total_cost: number;
    cost_basis: CostBasis;
    cost_source: string;
    known_cost: number | null;
    input_tokens: number;
    output_tokens: number;
    cache_write_tokens: number;
    cache_read_tokens: number;
    total_tokens: number;
    input_cost: number;
    output_cost: number;
    cache_write_cost: number;
    cache_read_cost: number;
    has_thinking: boolean;
    subagent_count: number;
    is_active: boolean;
}

export interface DailyStat {
    date: string;
    project: string;
    model: string;
    session_count: number;
    priced_sessions: number;
    total_cost: number;
    cost_basis: CostBasis;
    cost_sources: string[];
    total_tokens: number;
    input_tokens: number;
    output_tokens: number;
    cache_write_tokens: number;
    cache_read_tokens: number;
}

export interface AnalyticsSummary {
    total_sessions: number;
    priced_sessions: number;
    total_cost: number;
    cost_basis: CostBasis;
    cost_sources: string[];
    total_tokens: number;
    total_cache_read: number;
    total_cache_write: number;
    avg_duration_secs: number;
    avg_tokens_per_session: number;
    avg_cost_per_session: number;
    top_project: string;
    top_model: string;
    days_tracked: number;
}

export interface ProjectStat {
    project: string;
    session_count: number;
    priced_sessions: number;
    total_cost: number;
    cost_basis: CostBasis;
    cost_sources: string[];
    total_tokens: number;
    avg_session_cost: number;
    avg_duration_secs: number;
    cache_read_tokens: number;
    cache_write_tokens: number;
    top_model: string;
}

export interface HourlyActivity {
    hour: number;
    session_count: number;
    priced_sessions: number;
    total_cost: number;
    cost_basis: CostBasis;
    cost_sources: string[];
}

export interface CostForecast {
    billed_spend_usd: number | null;
    daily_billed_spend_usd: number | null;
    projected_billed_spend_usd: number | null;
    api_equivalent_usd: number | null;
    daily_api_equivalent_usd: number | null;
    projected_api_equivalent_usd: number | null;
    days_elapsed: number;
    days_in_month: number;
    cost_basis: CostBasis;
    cost_sources: string[];
    sessions: number;
    priced_sessions: number;
    billed_sessions: number;
    api_equivalent_sessions: number;
    refreshed_at: string;
}

export interface BudgetStatus {
    monthly_budget: number;
    alert_threshold_pct: number;
    billed_spend_usd: number | null;
    projected_billed_spend_usd: number | null;
    api_equivalent_usd: number | null;
    projected_api_equivalent_usd: number | null;
    pct_used: number | null;
    over_budget: boolean;
    cost_basis: CostBasis;
    cost_sources: string[];
    sessions: number;
    priced_sessions: number;
    billed_sessions: number;
    api_equivalent_sessions: number;
    refreshed_at: string;
}

export interface DashboardBundle {
    summary: AnalyticsSummary;
    sessions: HistoricalSession[];
    forecast: CostForecast;
    hourly_activity: HourlyActivity[];
}

export function getDashboardBundle(
    provider?: AnalyticsProviderScope,
): Promise<DashboardBundle> {
    return invokeRead("get_dashboard_bundle", { provider: provider ?? null });
}

export interface ModelStat {
    model: string;
    session_count: number;
    priced_sessions: number;
    cost_basis: CostBasis;
    cost_sources: string[];
    total_cost: number;
}

export function getSessionHistory(
    days?: number,
    project?: string,
    limit?: number,
    provider?: AnalyticsProviderScope,
): Promise<HistoricalSession[]> {
    return invokeRead("get_session_history", {
        days: days ?? null,
        project: project ?? null,
        limit: limit ?? null,
        provider: provider ?? null,
    });
}

export function searchSessions(
    query: string,
    limit?: number,
    provider?: AnalyticsProviderScope,
): Promise<HistoricalSession[]> {
    return invoke("search_sessions", { query, limit: limit ?? null, provider: provider ?? null });
}

export function getDailyStats(
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<DailyStat[]> {
    return invokeRead("get_daily_stats", { days: days ?? null, provider: provider ?? null });
}

export function getAnalyticsSummary(provider?: AnalyticsProviderScope): Promise<AnalyticsSummary> {
    return invokeRead("get_analytics_summary", { provider: provider ?? null });
}

export interface ContextFileEntry {
    name: string;
    tokens: number;
}

export interface ContextBreakdown {
    model: string;
    context_window: number;
    used_tokens: number;
    free_space: number;
    autocompact_buffer: number;
    system_prompt: number;
    system_tools: number;
    memory_files: ContextFileEntry[];
    memory_total: number;
    skills: ContextFileEntry[];
    skills_total: number;
    messages: number;
    mcp_tools: ContextFileEntry[];
    mcp_total: number;
}

export function getContextBreakdown(
    sessionId?: string,
    provider?: AnalyticsProviderScope,
): Promise<ContextBreakdown> {
    return invokeRead("get_context_breakdown", {
        sessionId: sessionId ?? null,
        provider: provider ?? null,
    });
}

export interface SessionContextBreakdown {
    session_id: string;
    project: string;
    model_id: string;
    is_idle: boolean;
    activity: string;
    breakdown: ContextBreakdown;
}

export function getContextBreakdowns(
    sessionIds?: string[],
    provider?: AnalyticsProviderScope,
): Promise<SessionContextBreakdown[]> {
    return invokeRead("get_context_breakdowns", {
        sessionIds: sessionIds ?? null,
        provider: provider ?? null,
    });
}

export interface SessionContextUsage {
    session_id: string;
    project: string;
    model: string;
    model_display: string;
    used_tokens: number;
    window_tokens: number;
    utilization_pct: number;
    recommendation: string;
}

export function getSessionsContextUsage(
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<SessionContextUsage[]> {
    return invoke("get_sessions_context_usage", { days: days ?? null, provider: provider ?? null });
}

export function getProjectStats(
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<ProjectStat[]> {
    return invoke("get_project_stats", { days: days ?? null, provider: provider ?? null });
}

export function getHourlyActivity(
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<HourlyActivity[]> {
    return invokeRead("get_hourly_activity", { days: days ?? null, provider: provider ?? null });
}

export function getTopSessions(
    limit?: number,
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<HistoricalSession[]> {
    return invoke("get_top_sessions", {
        limit: limit ?? null,
        days: days ?? null,
        provider: provider ?? null,
    });
}

export function getCostForecast(provider?: AnalyticsProviderScope): Promise<CostForecast> {
    return invokeRead("get_cost_forecast", { provider: provider ?? null });
}

/**
 * Window-wide cost aggregates.
 *
 * Distinct from `getSessionHistory`, which returns a capped page for the table.
 * KPIs must use these totals so a window with more sessions than the page size
 * still reports its complete provenance-aware monetary value.
 */
export interface CostTotals {
    days: number;
    sessions: number;
    total_cost: number;
    input_cost: number;
    output_cost: number;
    cache_write_cost: number;
    cache_read_cost: number;
    total_tokens: number;
    input_tokens: number;
    output_tokens: number;
    cache_write_tokens: number;
    cache_read_tokens: number;
    pure_input_tokens: number;
    cost_basis: CostBasis;
    cost_sources: string[];
    priced_sessions: number;
    billed_spend_usd: number | null;
    api_equivalent_usd: number | null;
    billed_sessions: number;
    api_equivalent_sessions: number;
    by_model: CostSlice[];
    by_project: CostSlice[];
}

export interface CostSlice {
    label: string;
    cost: number;
    sessions: number;
}

export interface CostsBundle {
    history: HistoricalSession[];
    forecast: CostForecast;
    budget: BudgetStatus;
    totals: CostTotals;
    daily_usage: DailyStat[];
}

export function getCostsBundle(
    project?: string,
    provider?: AnalyticsProviderScope,
    observationIdentity?: string,
): Promise<CostsBundle> {
    return invokeRead("get_costs_bundle", {
        project: project ?? null,
        provider: provider ?? null,
    }, observationIdentity);
}

export function getCostTotals(
    days?: number,
    project?: string,
    provider?: AnalyticsProviderScope,
    observationIdentity?: string,
): Promise<CostTotals> {
    return invokeRead("get_cost_totals", {
        days: days ?? null,
        project: project ?? null,
        provider: provider ?? null,
    }, observationIdentity);
}

export function getBudgetStatus(provider?: AnalyticsProviderScope): Promise<BudgetStatus> {
    return invokeRead("get_budget_status", { provider: provider ?? null });
}

export function setBudget(monthlyBudget: number, alertThresholdPct?: number): Promise<void> {
    return invoke("set_budget", { monthlyBudget, alertThresholdPct: alertThresholdPct ?? null });
}

export function getModelDistribution(
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<ModelStat[]> {
    return invoke("get_model_distribution_v2", { days: days ?? null, provider: provider ?? null });
}

export function exportAllData(provider?: AnalyticsProviderScope): Promise<Record<string, unknown>> {
    return invoke("export_all_data", { provider: provider ?? null });
}

export function clearHistory(provider?: AnalyticsProviderScope): Promise<number> {
    return invoke("clear_history", { provider: provider ?? null });
}

export function getDbSize(): Promise<number> {
    return invoke("get_db_size");
}

export function generateHtmlReport(
    days?: number,
    project?: string,
    provider?: AnalyticsProviderScope,
): Promise<string> {
    return invoke("generate_html_report", {
        days: days ?? null,
        project: project ?? null,
        provider: provider ?? null,
    });
}

export function generateMarkdownReport(
    days?: number,
    project?: string,
    provider?: AnalyticsProviderScope,
): Promise<string> {
    return invoke("generate_markdown_report", {
        days: days ?? null,
        project: project ?? null,
        provider: provider ?? null,
    });
}


export type Severity = "critical" | "warning" | "info" | "positive";

export interface CacheHealthReport {
    grade: string;
    grade_label: string;
    color: string;
    hit_ratio: number;
    trend_weighted_ratio: number;
    total_cache_read: number;
    total_cache_write: number;
    total_input: number;
    sessions_analyzed: number;
    diagnosis: string;
}

export interface InflectionPoint {
    date: string;
    multiplier: number;
    direction: "spike" | "drop" | "";
    sessions_on_day: number;
    observed_sessions_on_day: number;
    cost_on_day: number;
    baseline_cost: number;
    cost_basis: CostBasis;
    cost_sources: string[];
    note: string;
}

export interface FamilyStats {
    sessions: number;
    priced_sessions: number;
    cost: number;
    cost_share_pct: number;
    avg_cost_per_session: number;
}

export interface ModelRoutingReport {
    total_sessions: number;
    priced_sessions: number;
    total_cost: number;
    cost_basis: CostBasis;
    cost_sources: string[];
    opus: FamilyStats;
    sonnet: FamilyStats;
    haiku: FamilyStats;
    other: FamilyStats;
    savings_estimate_available: boolean;
    estimated_savings_if_rerouted: number;
    diagnosis: string;
}

export interface Recommendation {
    id: string;
    severity: Severity;
    title: string;
    description: string;
    estimated_savings: string | null;
    action: string;
    fix_prompt: string;
    color: string;
}

export function getCacheHealth(
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<CacheHealthReport> {
    return invoke("get_cache_health", { days: days ?? null, provider: provider ?? null });
}

export function getInflectionPoints(
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<InflectionPoint[]> {
    return invoke("get_inflection_points", { days: days ?? null, provider: provider ?? null });
}

export function getModelRouting(
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<ModelRoutingReport | null> {
    return invoke("get_model_routing", { days: days ?? null, provider: provider ?? null });
}

export function getRecommendations(
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<Recommendation[]> {
    return invoke("get_recommendations", { days: days ?? null, provider: provider ?? null });
}

export function copyFixPrompt(
    recId: string,
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<string> {
    return invoke("copy_fix_prompt", {
        recId,
        days: days ?? null,
        provider: provider ?? null,
    });
}


export interface ToolUsageEntry {
    name: string;
    count: number;
    share_pct: number;
}

export interface ToolFrequencyReport {
    available: boolean;
    sessions_analyzed: number;
    traced_sessions: number;
    total_tool_calls: number;
    avg_tools_per_session: number;
    avg_tool_calls_per_hour: number;
    mcp_tool_calls: number;
    mcp_share_pct: number;
    compact_gap_sessions: number;
    diagnosis: string;
    top_tools: ToolUsageEntry[];
}

export interface PromptComplexitySession {
    session_id: string;
    project: string;
    complexity_score: number;
    specificity_score: number;
    label: string;
    preview: string;
}

export interface PromptComplexityReport {
    available: boolean;
    sessions_analyzed: number;
    prompts_analyzed: number;
    avg_complexity_score: number;
    avg_specificity_score: number;
    high_complexity_sessions: number;
    low_specificity_sessions: number;
    diagnosis: string;
    top_sessions: PromptComplexitySession[];
}

export interface SessionHealthReport {
    available: boolean;
    sessions_analyzed: number;
    health_score: number;
    grade: string;
    avg_duration_minutes: number;
    p90_duration_minutes: number;
    long_session_pct: number;
    avg_messages_per_session: number;
    peak_overlap_pct: number;
    compact_gap_pct: number;
    diagnosis: string;
}

export function getToolFrequency(
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<ToolFrequencyReport> {
    return invoke("get_tool_frequency", { days: days ?? null, provider: provider ?? null });
}

export function getPromptComplexity(
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<PromptComplexityReport> {
    return invoke("get_prompt_complexity", { days: days ?? null, provider: provider ?? null });
}

export function getSessionHealth(
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<SessionHealthReport> {
    return invoke("get_session_health", { days: days ?? null, provider: provider ?? null });
}


export interface ReportsBundle {
    provider: string;
    capabilities: ProviderCapabilities;
    days: number;
    total_sessions: number;
    priced_sessions: number;
    cost_basis: CostBasis;
    cost_sources: string[];
    recommendations: Recommendation[];
    trace_overview: TraceOverview;
    tool_frequency: ToolFrequencyReport;
    prompt_complexity: PromptComplexityReport;
    session_health: SessionHealthReport;
    cache_health: CacheHealthReport;
    model_routing: ModelRoutingReport | null;
    inflection_points: InflectionPoint[];
    daily_costs: DailyCostPoint[];
}

/** One day on the Reports monetary-value timeline. Zero-filled across the window. */
export interface DailyCostPoint {
    date: string;
    cost: number;
    sessions: number;
    priced_sessions: number;
    cost_basis: CostBasis;
    cost_sources: string[];
}

export function getReportsBundle(
    days?: number,
    project?: string,
    provider?: AnalyticsProviderScope,
): Promise<ReportsBundle> {
    return invoke("get_reports_bundle", {
        days: days ?? null,
        project: project ?? null,
        provider: provider ?? null,
    });
}

export interface SessionHistoryFilter {
    from_iso?: string | null;
    to_iso?: string | null;
    project?: string | null;
    model?: string | null;
    min_cost?: number | null;
    max_cost?: number | null;
    limit?: number | null;
    provider?: AnalyticsProviderScope | null;
}

export function getSessionHistoryFiltered(
    filter: SessionHistoryFilter,
): Promise<HistoricalSession[]> {
    return invoke("get_session_history_filtered", {
        fromIso: filter.from_iso ?? null,
        toIso: filter.to_iso ?? null,
        project: filter.project ?? null,
        model: filter.model ?? null,
        minCost: filter.min_cost ?? null,
        maxCost: filter.max_cost ?? null,
        limit: filter.limit ?? null,
        provider: filter.provider ?? null,
    });
}

export function getSessionsByHourRange(
    startHour: number,
    endHour: number,
    days?: number,
    provider?: AnalyticsProviderScope,
): Promise<HistoricalSession[]> {
    return invoke("get_sessions_by_hour_range", {
        startHour,
        endHour,
        days: days ?? null,
        provider: provider ?? null,
    });
}

/// Ask the background poller to drop its usage cache and hit the API on the
/// next tick (~5s cycle). Returns immediately; stores re-poll picks up fresh data.
export function refreshUsage(): Promise<void> {
    return invoke("refresh_usage");
}

export type NotificationKind =
    | "provider_health"
    | "quota_threshold"
    | "quota_reset"
    | "discord_connectivity";

/** Durable backend notification as stored in Pulse's local SQLite database. */
export interface PulseNotification {
    id: number;
    kind: NotificationKind;
    provider: string;
    key: string;
    title: string;
    body: string;
    action: string | null;
    created_at: string;
    read_at: string | null;
    dismissed_at: string | null;
}

export function getNotifications(limit = 30): Promise<PulseNotification[]> {
    return invoke("get_notifications", { limit });
}

export function getUnreadNotificationCount(): Promise<number> {
    return invoke("get_unread_notification_count");
}

export function markNotificationRead(id: number): Promise<boolean> {
    return invoke("mark_notification_read", { id });
}

export function markAllNotificationsRead(): Promise<number> {
    return invoke("mark_all_notifications_read");
}

export function dismissNotification(id: number): Promise<boolean> {
    return invoke("dismiss_notification", { id });
}

export interface AppUpdateAsset {
    name: string;
    download_url: string;
    size: number;
    content_type: string;
    /** "windows" | "macos" | "linux", or null for non-installer assets. */
    platform: string | null;
}

/** Size of the semver jump a release represents. */
export type UpdateSeverity = "none" | "patch" | "minor" | "major";

export interface AppUpdateInfo {
    current_version: string;
    latest_version: string | null;
    update_available: boolean;
    release_name: string | null;
    release_notes: string | null;
    release_url: string;
    published_at: string | null;
    checked_at: string;
    assets: AppUpdateAsset[];
    severity: UpdateSeverity;
}

export function checkAppUpdate(): Promise<AppUpdateInfo> {
    return invoke("check_app_update");
}

export function openAppReleasePage(url?: string | null): Promise<void> {
    return invoke("open_app_release_page", { url: url ?? null });
}

export interface OpenCodeMetadata {
    model_provider: string;
    model_id: string;
    model_name: string;
    variant: string | null;
    surface: string;
    context_source: string | null;
    source_database: string;
    models: { provider_id: string; model_id: string; input: number; output: number; reasoning: number; cache_read: number; cache_write: number; cost: number | null }[];
}

export function markNotificationUnread(id: number): Promise<boolean> {
    return invoke("mark_notification_unread", { id });
}
export function markAllNotificationsUnread(): Promise<number> {
    return invoke("mark_all_notifications_unread");
}
export function dismissAllNotifications(): Promise<{ count: number; undo_token: string }> {
    return invoke("dismiss_all_notifications");
}
export function restoreNotifications(token: string): Promise<number> {
    return invoke("restore_notifications", { token });
}
