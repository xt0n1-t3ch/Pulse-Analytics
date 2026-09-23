import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, waitFor, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import Dashboard from "@/views/Dashboard.svelte";
import {
  accessSnapshot,
  metrics,
  planInfo,
  rateLimits,
  selectedAccessSourceId,
  selectedAnalyticsProviderScope,
  sessions,
} from "@/lib/stores";
import { provider } from "@/lib/provider";
import { formatResetDateTime } from "@/lib/utils";
import type {
  MetricsResponse,
  AnalyticsSummary,
  HistoricalSession,
  CostForecast,
  HourlyActivity,
  DailyStat,
  ProjectStat,
  SessionInfo,
} from "@/lib/api";

const metricsFixture: MetricsResponse = {
  total_cost: 12.5,
  cost_available: true,
  cost_basis: "exact",
  input_tokens: 400_000,
  pure_input_tokens: 300_000,
  output_tokens: 120_000,
  cache_write_tokens: 80_000,
  cache_read_tokens: 600_000,
  total_tokens: 1_100_000,
  session_count: 4,
  input_cost: 4,
  output_cost: 5,
  cache_write_cost: 2,
  cache_read_cost: 1.5,
  cache_hit_ratio: 66,
  models: [
    { model: "Claude Opus 4.8", sessions: 3, cost: 10, tokens: 900_000 },
    { model: "Claude Sonnet 4.6", sessions: 1, cost: 2.5, tokens: 200_000 },
  ],
};

const summary: AnalyticsSummary = {
  total_sessions: 4,
  priced_sessions: 4,
  total_cost: 12.5,
  cost_basis: "exact",
  cost_sources: ["anthropic_api_equivalent"],
  total_tokens: 1_100_000,
  total_cache_read: 600_000,
  total_cache_write: 80_000,
  avg_duration_secs: 900,
  avg_tokens_per_session: 275_000,
  avg_cost_per_session: 3.125,
  top_project: "pulse",
  top_model: "Claude Opus 4.8",
  days_tracked: 14,
};

function hist(id: string, project: string, cost: number): HistoricalSession {
  return {
    id,
    provider: "claude",
    session_name: null,
    project,
    model: "Claude Opus 4.8",
    model_id: "claude-opus-4-8",
    context_window: "200K",
    branch: null,
    effort: "High",
    started_at: "2026-05-20T10:00:00Z",
    ended_at: "2026-05-20T10:30:00Z",
    duration_secs: 1800,
    total_cost: cost,
    cost_basis: "exact",
    cost_source: "anthropic_api_equivalent",
    known_cost: cost,
    input_tokens: 50_000,
    output_tokens: 20_000,
    cache_write_tokens: 10_000,
    cache_read_tokens: 100_000,
    total_tokens: 180_000,
    input_cost: cost * 0.3,
    output_cost: cost * 0.4,
    cache_write_cost: cost * 0.2,
    cache_read_cost: cost * 0.1,
    has_thinking: false,
    workflow_label: null,
    subagent_count: 0,
    is_active: false,
  };
}

function liveSession(
  id: string,
  project: string,
  used: number,
  window: number,
  activity: string,
): SessionInfo {
  return {
    session_id: id,
    session_name: project,
    project,
    model: "GPT-5.6 Sol",
    model_id: "gpt-5.6-sol",
    provider: "codex",
    context_window: "353.4K",
    cost: id === "live-1" ? 4.25 : 2.5,
    cost_available: true,
    cost_basis: "exact",
    tokens: 320_000,
    input_tokens: 80_000,
    output_tokens: 20_000,
    cache_write_tokens: 20_000,
    cache_read_tokens: 200_000,
    context_used_tokens: used,
    context_window_tokens: window,
    branch: "main",
    activity,
    activity_target: null,
    effort: "High",
    effort_explicit: true,
    is_idle: false,
    started_at: "2026-07-25T13:00:00Z",
    duration_secs: 900,
    has_thinking: true,
    workflow_label: null,
    subagent_count: 0,
    subagents: [],
    tokens_per_sec: 48,
    input_cost: 1,
    output_cost: 1,
    cache_write_cost: 1,
    cache_read_cost: 1.25,
    speed: "standard",
    fast: false,
    service_tier: null,
      has_inflated_tokenizer: false,
  };
}

const forecast: CostForecast = {
  billed_spend_usd: 30,
  daily_billed_spend_usd: 3,
  projected_billed_spend_usd: 93,
  api_equivalent_usd: null,
  daily_api_equivalent_usd: null,
  projected_api_equivalent_usd: null,
  days_elapsed: 10,
  days_in_month: 31,
  cost_basis: "exact",
  cost_sources: ["provider_billed"],
  sessions: 2,
  priced_sessions: 2,
  billed_sessions: 2,
  api_equivalent_sessions: 0,
  refreshed_at: "2026-08-12T12:00:00Z",
};

const hourly: HourlyActivity[] = [
  { hour: 9, session_count: 3, priced_sessions: 3, total_cost: 5, cost_basis: "exact", cost_sources: ["anthropic_api_equivalent"] },
  { hour: 14, session_count: 2, priced_sessions: 2, total_cost: 4, cost_basis: "exact", cost_sources: ["anthropic_api_equivalent"] },
];

const daily: DailyStat[] = [
  { date: "2026-05-19", project: "pulse", model: "Claude Opus 4.8", session_count: 2, priced_sessions: 2, total_cost: 6, cost_basis: "exact", cost_sources: ["anthropic_api_equivalent"], total_tokens: 500_000, input_tokens: 100_000, output_tokens: 50_000, cache_write_tokens: 40_000, cache_read_tokens: 310_000 },
  { date: "2026-05-20", project: "pulse", model: "Claude Opus 4.8", session_count: 2, priced_sessions: 2, total_cost: 6.5, cost_basis: "exact", cost_sources: ["anthropic_api_equivalent"], total_tokens: 600_000, input_tokens: 200_000, output_tokens: 70_000, cache_write_tokens: 40_000, cache_read_tokens: 290_000 },
];

const projects: ProjectStat[] = [
  { project: "pulse", session_count: 3, priced_sessions: 3, total_cost: 10, cost_basis: "exact", cost_sources: ["anthropic_api_equivalent"], total_tokens: 900_000, avg_session_cost: 3.33, avg_duration_secs: 1200, cache_read_tokens: 500_000, cache_write_tokens: 60_000, top_model: "Claude Opus 4.8" },
  { project: "other", session_count: 1, priced_sessions: 1, total_cost: 2.5, cost_basis: "exact", cost_sources: ["anthropic_api_equivalent"], total_tokens: 200_000, avg_session_cost: 2.5, avg_duration_secs: 600, cache_read_tokens: 100_000, cache_write_tokens: 20_000, top_model: "Claude Sonnet 4.6" },
];

const emptySummary: AnalyticsSummary = {
  ...summary,
  total_sessions: 0,
  priced_sessions: 0,
  total_cost: 0,
  cost_basis: "unavailable",
  cost_sources: [],
  total_tokens: 0,
  top_project: "—",
  top_model: "—",
  days_tracked: 0,
};
const getAnalyticsSummary = vi.fn(async (scope?: string) =>
  scope === "openai" || scope === "anthropic" ? emptySummary : summary,
);
const getSessionHistory = vi.fn(
  async (_days?: number, _project?: string, _limit?: number, scope?: string) =>
    scope === "openai" || scope === "anthropic"
      ? []
      : [hist("h1", "pulse", 6), hist("h2", "other", 4)],
);
const getCostForecast = vi.fn(async (scope?: string) =>
  scope === "openai" || scope === "anthropic"
    ? { ...forecast, billed_spend_usd: null, projected_billed_spend_usd: null, daily_billed_spend_usd: null, sessions: 0, priced_sessions: 0, billed_sessions: 0, cost_basis: "unavailable" as const, cost_sources: [] }
    : forecast,
);
const getHourlyActivity = vi.fn(async (_days?: number, scope?: string) =>
  scope === "openai" || scope === "anthropic" ? [] : hourly,
);
const getDashboardBundle = vi.fn(async (scope?: string) => {
  const [bundleSummary, bundleSessions, bundleForecast, bundleHourly] = await Promise.all([
    getAnalyticsSummary(scope),
    getSessionHistory(30, undefined, 50, scope),
    getCostForecast(scope),
    getHourlyActivity(30, scope),
  ]);
  return {
    summary: bundleSummary,
    sessions: bundleSessions,
    forecast: bundleForecast,
    hourly_activity: bundleHourly,
  };
});
const getDailyStats = vi.fn(async () => daily);
const getProjectStats = vi.fn(async () => projects);

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...actual,
    getDashboardBundle: (scope?: string) => getDashboardBundle(scope),
    getAnalyticsSummary: (scope?: string) => getAnalyticsSummary(scope),
    getSessionHistory: (days?: number, project?: string, limit?: number, scope?: string) =>
      getSessionHistory(days, project, limit, scope),
    getCostForecast: (scope?: string) => getCostForecast(scope),
    getHourlyActivity: (days?: number, scope?: string) => getHourlyActivity(days, scope),
    getDailyStats: () => getDailyStats(),
    getProjectStats: () => getProjectStats(),
  };
});

describe("Dashboard.svelte", () => {
  beforeEach(() => {
    getDashboardBundle.mockClear();
    getAnalyticsSummary.mockClear();
    getSessionHistory.mockClear();
    getCostForecast.mockClear();
    getHourlyActivity.mockClear();
    provider.set("claude");
    metrics.set(metricsFixture);
    sessions.set([]);
    accessSnapshot.set(null);
    selectedAccessSourceId.set("all");
    selectedAnalyticsProviderScope.set("all");
    planInfo.set({ provider: "claude", plan_key: "max_20x", plan_name: "Max 20x ($200/mo)", detected: true });
    rateLimits.set({
      provider: "claude",
      usage: {
        source: {
          lane: "claude_subscription",
          stream_id: "claude-subscription:usage",
          signals: ["claude_subscription_usage"],
        },
        scopes: [{
          id: "global",
          name: "Claude account",
          kind: "other",
          windows: [
            { window_minutes: 300, used_percent: 40, remaining_percent: 60, resets_at: "2026-05-28T18:00:00Z" },
            { window_minutes: 10080, used_percent: 55, remaining_percent: 45, resets_at: "2026-06-01T00:00:00Z" },
          ],
        }],
        credits: null,
        observed_at: "2026-05-28T12:00:00Z",
        provenance_source: "Anthropic usage API",
      },
      five_hour_pct: 40,
      five_hour_resets: "2026-05-28T18:00:00Z",
      five_hour_label: "5-hour window",
      five_hour_window_minutes: 300,
      seven_day_pct: 55,
      seven_day_resets: "2026-06-01T00:00:00Z",
      seven_day_label: "Weekly",
      seven_day_window_minutes: null,
      sonnet_pct: 12,
      sonnet_resets: "2026-06-01T00:00:00Z",
      extra_enabled: true,
      extra_limit: 50,
      extra_used: 7.5,
      extra_pct: 15,
      source: "Anthropic usage API",
    });
  });

  it("mounts the Direction 2 Home workspace without duplicate chrome or diagnostics", async () => {
    const { container } = render(Dashboard);
    await waitFor(() => {
      expect(container.querySelector("[data-session-focus]")?.textContent).toContain("$6.00");
    });

    expect(container.querySelector("[data-dashboard-layout='direction-two']")).not.toBeNull();
    expect(container.querySelector(".home-intro")).toBeNull();
    expect(container.querySelector(".home-grid")).not.toBeNull();
    expect(container.querySelector(".work-now")).not.toBeNull();
    expect(container.querySelector(".provider-workspace")).toBeNull();
    expect(container.querySelector(".focus-panel.surface-panel")).toBeNull();
    expect(container.querySelector(".insight-card.card")).toBeNull();
    expect(container.querySelector("[data-session-focus] .metric-strip")).not.toBeNull();
    expect(container.querySelector("[data-source-inspector]")).toBeNull();
    expect(container.textContent).not.toContain("Work now");
    expect(container.textContent).not.toContain("Source diagnostics");
    expect(container.querySelector("[data-session-focus]")?.textContent).toContain("180.0K");
  });

  it("surfaces analytics backend failure instead of rendering historical zeroes", async () => {
    metrics.set(null);
    getAnalyticsSummary.mockRejectedValueOnce(new Error("database unavailable"));
    const { findByRole } = render(Dashboard);

    const alert = await findByRole("alert");
    expect(alert.textContent).toContain("Historical analytics unavailable");
    expect(alert.textContent).toContain("Retry");
  });

  it("never relabels a previous provider bundle after a scope switch fails", async () => {
    const { container, queryByText, findByRole } = render(Dashboard);
    await waitFor(() => expect(queryByText("Provider-billed projection")).toBeTruthy());

    getDashboardBundle.mockRejectedValueOnce(new Error("openai ledger unavailable"));
    selectedAnalyticsProviderScope.set("openai");

    const alert = await findByRole("alert");
    expect(alert.textContent).toContain("openai ledger unavailable");
    expect(container.textContent).not.toContain("Provider-billed projection");
    expect(container.querySelector("[data-session-focus]")?.textContent).not.toContain("$6.00");
  });

  it("does not reserve an allowance rail when every source lacks provider proof", async () => {
    accessSnapshot.set({
      routes: [{
        source: {
          id: "openai-api:unproved",
          kind: "open_ai_api",
          provider: "openai",
          auth_method: "api_key",
          proof: "none",
          plan: null,
        },
        availability: "unavailable",
        freshness: "unknown",
        provenance: "none",
        observed_at: null,
        fetched_at: null,
        expires_at: null,
        windows: [],
        credits: null,
        extra_usage: null,
        error: "Provider proof was not observed.",
      }],
    });
    sessions.set([liveSession("live-1", "Planner regression", 120_000, 200_000, "Running tests")]);

    const { container, getAllByText } = render(Dashboard);
    await tick();

    expect(getAllByText("Planner regression").length).toBeGreaterThan(0);
    expect(container.querySelector("[aria-label='Provider allowances']")).toBeNull();
    expect(container.querySelector(".work-now")).not.toBeNull();
  });

  it("composes backend truth as a live-session focus and telemetry ledger", async () => {
    sessions.set([liveSession("live-1", "Planner regression", 120_000, 200_000, "Running tests")]);

    const { container } = render(Dashboard);
    await tick();

    expect(container.querySelector("[data-dashboard-layout='direction-two']")).not.toBeNull();
    expect(container.querySelector("[data-session-focus]")).not.toBeNull();
    expect(container.querySelector("[data-telemetry-ledger]")).not.toBeNull();
    expect(container.querySelector("[data-session-focus] .metric-strip")).not.toBeNull();
    expect(container.querySelector("[data-telemetry-ledger]")?.textContent).not.toContain("Plan limits");
    expect(container.querySelector("[data-session-focus]")?.textContent).toContain("Planner regression");
    expect(container.querySelector(".focus-chart-head")?.textContent).toContain("240.0K tokens this session");
    const mixValues = [...container.querySelectorAll(".mix-legend strong")].map((node) => node.textContent?.trim());
    expect(mixValues).toEqual(["0", "20.0K", "20.0K", "200.0K"]);
  });

  it("shows every active session and lets the exact context-window fraction follow selection", async () => {
    sessions.set([
      liveSession("live-1", "Marcos Reyes Website", 100_000, 353_400, "Thinking"),
      liveSession("live-2", "cc-discord-presence", 212_100, 353_400, "Running command"),
    ]);

    const { container, getByRole, getByText } = render(Dashboard);
    await tick();

    expect(getByText("2 active sessions")).toBeTruthy();
    expect(container.textContent).not.toContain("live instances");
    expect(container.querySelectorAll("[data-session-instance]")).toHaveLength(2);
    expect(getByText("Context Window")).toBeTruthy();
    expect(container.querySelector("[data-telemetry-ledger]")?.textContent).toContain("100.0K / 353.4K");

    await fireEvent.click(getByRole("tab", { name: /cc-discord-presence/ }));
    expect(container.querySelector("[data-telemetry-ledger]")?.textContent).toContain("212.1K / 353.4K");
    expect(container.querySelector("[data-session-focus]")?.textContent).toContain("cc-discord-presence");
  });

  it("labels retained idle snapshots as recent rather than live", async () => {
    sessions.set([{ ...liveSession("idle-1", "Retained session", 10_000, 353_400, "Waiting"), is_idle: true }]);

    const { container } = render(Dashboard);
    await tick();

    expect(container.querySelector("[data-session-focus]")?.textContent).toContain("Recent session");
    expect(container.querySelector("[data-session-focus]")?.textContent).toContain("Idle");
    expect(container.querySelector("[data-session-focus]")?.textContent).not.toContain("Live session");
  });

  it("does not mix an older history branch into a live session header", async () => {
    const current = { ...liveSession("live-no-branch", "Current work", 10_000, 353_400, "Thinking"), branch: null };
    const older = { ...hist("old", "Older work", 4), branch: "legacy/history-branch" };
    getSessionHistory.mockResolvedValueOnce([older]);
    sessions.set([current]);

    const { container } = render(Dashboard);
    await waitFor(() => expect(getSessionHistory).toHaveBeenCalled());

    const focus = container.querySelector("[data-session-focus]")?.textContent ?? "";
    expect(focus).toContain("Current work");
    expect(focus).not.toContain("legacy/history-branch");
  });

  it("does not repeat the detailed cost breakdown already owned by Costs", async () => {
    const { container } = render(Dashboard);
    await tick();

    expect(container.textContent).not.toContain("Cost Breakdown");
    expect(container.querySelector(".bd-row")).toBeNull();
  });

  it("shows provider allowances only from authenticated access routes", async () => {
    accessSnapshot.set({
      routes: [{
        source: {
          id: "claude-subscription:test",
          kind: "claude_subscription",
          provider: "claude",
          auth_method: "oauth",
          proof: "quota_response",
          plan: "Max 20x",
        },
        availability: "available",
        freshness: "fresh",
        provenance: "provider_api",
        observed_at: "2026-05-28T12:00:00Z",
        fetched_at: "2026-05-28T12:00:00Z",
        expires_at: "2026-05-28T12:05:00Z",
        windows: [{
          key: "five_hour",
          label: "5h",
          window_minutes: 300,
          used_percent: 40,
          remaining_percent: 60,
          resets_at: "2026-05-28T18:00:00Z",
        }],
        credits: null,
        extra_usage: null,
        error: null,
      }],
    });
    const { container, getByText } = render(Dashboard);
    await tick();

    expect(getByText("Claude Max 20x")).toBeTruthy();
    expect(getByText("40% used")).toBeTruthy();
    expect(container.textContent).not.toContain("Model Distribution");
    expect(container.textContent).toContain(formatResetDateTime("2026-05-28T18:00:00Z"));
  });

  it("omits unreported monetary tiles instead of displaying empty metrics", async () => {
    sessions.set([{ ...liveSession("unpriced", "Unpriced work", 80_000, 100_000, "Thinking"), cost_available:false, cost_basis:"unavailable" }]);
    const {container}=render(Dashboard); await tick();
    expect(container.querySelector("[data-session-focus]")?.textContent).not.toContain("Exact total not reported");
    expect([...container.querySelectorAll(".stat-label")].some((label)=>label.textContent==="Reported value")).toBe(false);
    expect(container.textContent).toContain("Session tokens");
  });

  it("keeps at-a-glance metadata to one line and moves coverage to a tooltip", async () => {
    getCostForecast.mockResolvedValueOnce({
      ...forecast,
      billed_spend_usd: null,
      daily_billed_spend_usd: null,
      projected_billed_spend_usd: null,
      api_equivalent_usd: 10_444,
      daily_api_equivalent_usd: 387,
      projected_api_equivalent_usd: 11_992,
      days_elapsed: 27,
      api_equivalent_sessions: 2,
      billed_sessions: 0,
      cost_basis: "estimated",
      cost_sources: ["anthropic_api_equivalent"],
    });

    const { container } = render(Dashboard);
    await waitFor(() => expect(container.querySelector(".forecast-meta")).not.toBeNull());

    const card = container.querySelector(".forecast-info")?.closest<HTMLElement>(".insight-card");
    const meta = card?.querySelectorAll(".forecast-meta") ?? [];
    expect(meta).toHaveLength(1);
    expect(meta[0]?.textContent?.replace(/\s+/g, " ").trim()).toBe(
      "This month · $10,444 API-equivalent",
    );
    expect(card?.title).toBe("Coverage: 27/31 days");
    expect(container.querySelector(".insight-row .heatmap-card")).toBeNull();
    expect(card?.querySelector(".insight-kicker")).toBeNull();
  });

  it("does not leak global Codex cost or tokens into an empty Claude selection", async () => {
    getAnalyticsSummary.mockResolvedValueOnce(emptySummary);
    getSessionHistory.mockResolvedValueOnce([]);
    getCostForecast.mockResolvedValueOnce({
      ...forecast,
      billed_spend_usd: null,
      projected_billed_spend_usd: null,
      daily_billed_spend_usd: null,
      sessions: 0,
      priced_sessions: 0,
      billed_sessions: 0,
      cost_basis: "unavailable",
      cost_sources: [],
    });
    getHourlyActivity.mockResolvedValueOnce([]);
    sessions.set([liveSession("live-1", "codex-project", 80_000, 100_000, "Thinking")]);
    metrics.set(metricsFixture);
    accessSnapshot.set({
      routes: [{
        source: {
          id: "claude-subscription:test",
          kind: "claude_subscription",
          provider: "claude",
          auth_method: "oauth",
          proof: "quota_response",
          plan: "Max 20x",
        },
        availability: "available",
        freshness: "fresh",
        provenance: "provider_api",
        observed_at: "2026-08-01T12:00:00Z",
        fetched_at: "2026-08-01T12:00:00Z",
        expires_at: "2026-08-01T12:05:00Z",
        windows: [],
        credits: null,
        extra_usage: null,
        error: null,
      }],
    });
    selectedAnalyticsProviderScope.set("claude");
    selectedAccessSourceId.set("claude-subscription:test");

    const { container } = render(Dashboard);
    await waitFor(() => {
      expect(getSessionHistory).toHaveBeenCalledWith(30, undefined, 50, "claude");
      expect(getAnalyticsSummary).toHaveBeenCalledWith("claude");
    });

    // The initial analytics window may render skeletons first; settle on the
    // loaded empty-selection content before asserting its absence guarantees.
    await waitFor(() => {
      const settled = container.querySelector("[data-session-focus]")?.textContent ?? "";
      expect(settled).toContain("No active session");
      expect(settled).toContain("No active session");
    });

    const focus = container.querySelector("[data-session-focus]")?.textContent ?? "";
    expect(focus).toContain("No active session");
    expect(focus).toContain("No active session");
    expect(focus).not.toContain("Unavailable");
    expect(focus).not.toContain("$4.25");
    expect(focus).not.toContain("1.1M");
  });

  it("renders weekly-only Codex quota and credits without inventing a five-hour window", async () => {
    provider.set("codex");
    planInfo.set({ provider: "codex", plan_key: "pro_20x", plan_name: "Pro 20x", detected: true });
    accessSnapshot.set({
      routes: [{
        source: {
          id: "codex-subscription:test",
          kind: "codex_subscription",
          provider: "codex",
          auth_method: "app_server",
          proof: "quota_response",
          plan: "Pro 20x",
        },
        availability: "available",
        freshness: "fresh",
        provenance: "app_server",
        observed_at: "2026-07-16T00:00:00Z",
        fetched_at: "2026-07-16T00:00:00Z",
        expires_at: "2026-07-16T00:05:00Z",
        windows: [{
          key: "weekly",
          label: "Weekly",
          window_minutes: 10_080,
          used_percent: 4,
          remaining_percent: 96,
          resets_at: null,
        }],
        credits: { balance: "2500", has_credits: true, unlimited: false },
        extra_usage: null,
        error: null,
      }],
    });

    const { container, getByText } = render(Dashboard);
    await tick();

    expect(getByText("Weekly limit")).toBeTruthy();
    expect(getByText("96% remaining")).toBeTruthy();
    expect(container.textContent).not.toContain("5h");
    expect(container.textContent).not.toContain("Spark");
  });
  it("keeps completed OpenCode work out of the live focus and shows idle", async () => {
    selectedAnalyticsProviderScope.set("opencode");
    sessions.set([{...liveSession("old-open", "Finished smoke", 90000, 200000, "Waiting for input"), provider:"opencode", model:"mimo-v2.5-free", is_idle:true}]);
    getSessionHistory.mockResolvedValueOnce([{...hist("old-open","Finished smoke",0),provider:"opencode",model:"mimo-v2.5-free"}]);
    const {container}=render(Dashboard);
    await tick();
    const focus=container.querySelector("[data-session-focus]");
    expect(focus?.textContent).toContain("No active session");
    expect(focus?.textContent).toContain("Idle");
    expect(focus?.textContent).not.toContain("mimo-v2.5-free");
    expect(focus?.querySelector(".metric-strip")).toBeNull();
  });

});
