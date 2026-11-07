import { describe, it, expect, beforeEach } from "bun:test";
import { vi } from "../bun-setup";
import { render, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
import { get } from "svelte/store";
import { listen } from "@tauri-apps/api/event";
import Dashboard from "@/views/Dashboard.svelte";
import * as stores from "@/lib/stores";
import type {
    HealthResponse,
    MetricsResponse,
    SessionInfo,
    RateLimitInfo,
    PlanInfo,
    AnalyticsSummary,
    HistoricalSession,
    CostForecast,
    HourlyActivity,
    DailyStat,
    ProjectStat,
} from "@/lib/api";
import type { AccessSnapshot } from "@/lib/access";

const health: HealthResponse = {
    version: "0.1.0",
    uptime_seconds: 300,
    discord_status: "Connected",
    discord_enabled: true,
};

const metrics: MetricsResponse = {
    total_cost: 8,
    input_tokens: 200_000,
    pure_input_tokens: 150_000,
    output_tokens: 60_000,
    cache_write_tokens: 40_000,
    cache_read_tokens: 300_000,
    total_tokens: 600_000,
    session_count: 2,
    input_cost: 2,
    output_cost: 3,
    cache_write_cost: 2,
    cache_read_cost: 1,
    cache_hit_ratio: 66,
    models: [
        { model: "Claude Opus 4.8", sessions: 2, cost: 8, tokens: 600_000 },
    ],
};

function makeSession(id: string, project: string): SessionInfo {
    return {
        has_inflated_tokenizer: false,
        workflow_label: null,
        session_id: id,
        session_name: null,
        project,
        model: "Claude Opus 4.8",
        model_id: "claude-opus-4-8",
        provider: "claude",
        context_window: "200K",
        cost: 4,
        tokens: 300_000,
        input_tokens: 100_000,
        output_tokens: 30_000,
        cache_write_tokens: 20_000,
        cache_read_tokens: 150_000,
        branch: "main",
        activity: "Editing",
        activity_target: "stores.ts",
        effort: "High",
        effort_explicit: true,
        is_idle: false,
        started_at: "2026-05-28T10:00:00Z",
        duration_secs: 600,
        has_thinking: true,
        subagent_count: 0,
        subagents: [],
        tokens_per_sec: 42,
        input_cost: 1,
        output_cost: 1.5,
        cache_write_cost: 1,
        cache_read_cost: 0.5,
        speed: "standard",
        fast: false,
        service_tier: null,
        app_name: null,
    };
}

const liveSessions = [makeSession("s1", "pulse"), makeSession("s2", "other")];

const rateLimitInfo: RateLimitInfo = {
    provider: "claude",
    usage: {
        provider: "claude",
        source: "provider_api",
        usage_source: {
            lane: "claude_subscription",
            stream_id: "claude-subscription:usage",
            signals: ["claude_subscription_usage"],
        },
        scopes: [
            {
                id: "global",
                name: "Claude account",
                kind: "other",
                windows: [
                    {
                        window_minutes: 300,
                        used_percent: 40,
                        remaining_percent: 60,
                        resets_at: "2026-05-28T18:00:00Z",
                    },
                    {
                        window_minutes: 10080,
                        used_percent: 55,
                        remaining_percent: 45,
                        resets_at: "2026-06-01T00:00:00Z",
                    },
                ],
            },
        ],
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
    extra_enabled: false,
    extra_limit: null,
    extra_used: null,
    extra_pct: null,
    source: "Anthropic usage API",
};

const planInfoFixture: PlanInfo = {
    provider: "claude",
    plan_key: "max_20x",
    plan_name: "Max 20x ($200/mo)",
    detected: true,
};

const accessFixture: AccessSnapshot = {
    routes: [
        {
            local_history: { available: false, sessions: 0 },
            source: {
                id: "claude-subscription:default",
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
            fetched_at: "2026-05-28T12:00:01Z",
            expires_at: "2026-05-28T12:00:31Z",
            windows: [
                {
                    key: "weekly",
                    label: "Weekly",
                    window_minutes: 10080,
                    used_percent: 55,
                    remaining_percent: 45,
                    resets_at: "2026-06-01T00:00:00Z",
                },
            ],
            credits: null,
            extra_usage: null,
            error: null,
        },
    ],
};

const summary: AnalyticsSummary = {
    total_sessions: 2,
    priced_sessions: 2,
    total_cost: 8,
    cost_basis: "exact",
    cost_sources: ["anthropic_api_equivalent"],
    total_tokens: 600_000,
    total_cache_read: 300_000,
    total_cache_write: 40_000,
    avg_duration_secs: 600,
    avg_tokens_per_session: 300_000,
    avg_cost_per_session: 4,
    top_project: "pulse",
    top_model: "Claude Opus 4.8",
    days_tracked: 14,
};

function hist(id: string, project: string): HistoricalSession {
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
        total_cost: 6,
        cost_basis: "exact",
        cost_source: "anthropic_api_equivalent",
        known_cost: 6,
        input_tokens: 50_000,
        output_tokens: 20_000,
        cache_write_tokens: 10_000,
        cache_read_tokens: 100_000,
        total_tokens: 180_000,
        input_cost: 1.8,
        output_cost: 2.4,
        cache_write_cost: 1.2,
        cache_read_cost: 0.6,
        has_thinking: false,
        subagent_count: 0,
        is_active: false,
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
    sessions: 1,
    priced_sessions: 1,
    billed_sessions: 1,
    api_equivalent_sessions: 0,
    refreshed_at: "2026-08-12T12:00:00Z",
};

const hourly: HourlyActivity[] = [
    {
        hour: 9,
        session_count: 2,
        priced_sessions: 2,
        total_cost: 5,
        cost_basis: "exact",
        cost_sources: ["anthropic_api_equivalent"],
    },
];
const daily: DailyStat[] = [
    {
        date: "2026-05-20",
        project: "pulse",
        model: "Claude Opus 4.8",
        session_count: 2,
        priced_sessions: 2,
        total_cost: 6,
        cost_basis: "exact",
        cost_sources: ["anthropic_api_equivalent"],
        total_tokens: 500_000,
        input_tokens: 100_000,
        output_tokens: 50_000,
        cache_write_tokens: 40_000,
        cache_read_tokens: 310_000,
    },
];
const projects: ProjectStat[] = [
    {
        project: "pulse",
        session_count: 2,
        priced_sessions: 2,
        total_cost: 8,
        cost_basis: "exact",
        cost_sources: ["anthropic_api_equivalent"],
        total_tokens: 600_000,
        avg_session_cost: 4,
        avg_duration_secs: 600,
        cache_read_tokens: 300_000,
        cache_write_tokens: 40_000,
        top_model: "Claude Opus 4.8",
    },
];

const getHealth = vi.fn(async () => health);
const getMetrics = vi.fn(async () => metrics);
const getLiveSessions = vi.fn(async () => liveSessions);
const getDiscordPreview = vi.fn(async () => ({
    provider: "claude",
    app_name: "Claude Code",
    details: "Editing · pulse",
    state: "Claude Opus 4.8",
    has_session: true,
    duration_secs: 600,
}));
const getRateLimits = vi.fn(async () => rateLimitInfo);
const getPlanInfo = vi.fn(async () => planInfoFixture);
const getAppSnapshot = vi.fn(async () => ({
    revision: 1,
    health,
    metrics,
    sessions: liveSessions,
    rate_limits: rateLimitInfo,
    discord_preview: await getDiscordPreview(),
    discord_settings: {
        provider: "claude",
        enabled: true,
        status: "Connected",
        publisher: "pulse",
        display_prefs: {
            show_project: true,
            show_branch: true,
            show_model: true,
            show_activity: true,
            show_tokens: false,
            show_cost: false,
            show_limits: true,
            show_credits: false,
            show_context: true,
            show_systems: true,
        },
        desktop_design: null,
        supports_desktop_design: false,
        supports_field_order: false,
        field_order: [],
    },
    plan: planInfoFixture,
    access: accessFixture,
}));

vi.mock("@/lib/api", async (importOriginal) => {
    const actual = await importOriginal<typeof import("@/lib/api")>();
    return {
        ...actual,
        getAppSnapshot: () => getAppSnapshot(),
        getHealth: () => getHealth(),
        getMetrics: () => getMetrics(),
        getLiveSessions: () => getLiveSessions(),
        getDiscordPreview: () => getDiscordPreview(),
        getRateLimits: () => getRateLimits(),
        getPlanInfo: () => getPlanInfo(),
        getAnalyticsSummary: async () => summary,
        getSessionHistory: async () => [hist("h1", "pulse")],
        getCostForecast: async () => forecast,
        getHourlyActivity: async () => hourly,
        getDailyStats: async () => daily,
        getProjectStats: async () => projects,
    };
});

describe("poll() to stores to Dashboard full flow", () => {
    beforeEach(async () => {
        getHealth.mockClear();
        getMetrics.mockClear();
        getLiveSessions.mockClear();
        getDiscordPreview.mockClear();
        getRateLimits.mockClear();
        getPlanInfo.mockClear();
        getAppSnapshot.mockClear();
        const {
            health: h,
            metrics: m,
            sessions: s,
            discordPresencePreview: dp,
            rateLimits: r,
            planInfo: p,
            accessSnapshot: a,
        } = stores;
        stores.stopSnapshotSync();
        h.set(null);
        m.set(null);
        s.set([]);
        dp.set(null);
        r.set(null);
        p.set(null);
        a.set(null);
        await Promise.resolve();
        await Promise.resolve();
        getDiscordPreview.mockClear();
    });

    it("hydrates every global store from a single poll() pass", async () => {
        await stores.poll();

        expect(getAppSnapshot).toHaveBeenCalledTimes(1);
        expect(getHealth).not.toHaveBeenCalled();
        expect(getMetrics).not.toHaveBeenCalled();
        expect(getLiveSessions).not.toHaveBeenCalled();
        expect(getDiscordPreview).toHaveBeenCalledTimes(1);
        expect(getRateLimits).not.toHaveBeenCalled();
        expect(getPlanInfo).not.toHaveBeenCalled();

        expect(get(stores.health)).toEqual(health);
        expect(get(stores.metrics)).toEqual(metrics);
        expect(get(stores.sessions)).toHaveLength(2);
        expect(get(stores.discordPresencePreview)?.details).toBe(
            "Editing · pulse",
        );
        expect(get(stores.rateLimits)).toEqual(rateLimitInfo);
        expect(get(stores.planInfo)).toEqual(planInfoFixture);
        expect(get(stores.accessSnapshot)).toEqual(accessFixture);
        expect(get(stores.activeSessions)).toHaveLength(2);
    });

    it("marks the last snapshot disconnected without blanking it during revalidation", async () => {
        await stores.poll();
        expect(get(stores.backendConnection)).toBe("live");

        getAppSnapshot.mockRejectedValueOnce(new Error("bridge offline"));
        await stores.poll();

        expect(get(stores.backendConnection)).toBe("disconnected");
        expect(get(stores.health)).toEqual(health);
        expect(get(stores.metrics)).toEqual(metrics);
        expect(get(stores.sessions)).toEqual(liveSessions);
        expect(get(stores.rateLimits)).toEqual(rateLimitInfo);
        expect(get(stores.planInfo)).toEqual(planInfoFixture);
        expect(get(stores.accessSnapshot)).toEqual(accessFixture);
    });

    it("ignores an older poll response that resolves after a newer snapshot", async () => {
        let resolveOlder!: (
            snapshot: Awaited<ReturnType<typeof getAppSnapshot>>,
        ) => void;
        const older = new Promise<Awaited<ReturnType<typeof getAppSnapshot>>>(
            (resolve) => {
                resolveOlder = resolve;
            },
        );
        const newer = {
            revision: 2,
            health: { ...health, version: "newer" },
            metrics,
            sessions: liveSessions,
            rate_limits: rateLimitInfo,
            discord_preview: await getDiscordPreview(),
            discord_settings: {
                provider: "claude" as const,
                enabled: true,
                status: "Connected",
                publisher: "pulse",
                display_prefs: {
                    show_project: true,
                    show_branch: true,
                    show_model: true,
                    show_activity: true,
                    show_tokens: false,
                    show_cost: false,
                    show_limits: true,
                    show_credits: false,
                    show_context: true,
                    show_systems: true,
                },
                desktop_design: null,
                supports_desktop_design: false,
                supports_field_order: false,
                field_order: [],
            },
            plan: planInfoFixture,
            access: accessFixture,
        };

        getAppSnapshot
            .mockImplementationOnce(() => older)
            .mockResolvedValueOnce(newer);

        const olderPoll = stores.poll();
        const queuedPoll = stores.poll();
        expect(queuedPoll).toBe(olderPoll);
        resolveOlder({
            ...newer,
            revision: 1,
            health: { ...health, version: "older" },
        });
        await olderPoll;

        await waitFor(() => expect(getAppSnapshot).toHaveBeenCalledTimes(2));
        await waitFor(() => expect(get(stores.health)?.version).toBe("newer"));
        expect(get(stores.backendConnection)).toBe("live");
    });

    it("invalidates provider-bound state and rejects an in-flight snapshot from the prior provider", async () => {
        let resolvePrior!: (
            snapshot: Awaited<ReturnType<typeof getAppSnapshot>>,
        ) => void;
        const prior = new Promise<Awaited<ReturnType<typeof getAppSnapshot>>>(
            (resolve) => {
                resolvePrior = resolve;
            },
        );
        const current = {
            ...(await getAppSnapshot()),
            revision: 3,
            health: { ...health, version: "current-provider" },
        };
        getAppSnapshot
            .mockImplementationOnce(() => prior)
            .mockResolvedValueOnce(current);

        const priorPoll = stores.poll();
        stores.invalidateLiveSnapshotForProviderChange();
        const currentPoll = stores.poll();

        expect(currentPoll).toBe(priorPoll);
        expect(get(stores.backendConnection)).toBe("connecting");
        expect(get(stores.accessSnapshot)).toBeNull();
        expect(get(stores.rateLimits)).toBeNull();

        resolvePrior({
            ...current,
            revision: 2,
            health: { ...health, version: "prior-provider" },
        });
        await priorPoll;

        await waitFor(() => expect(getAppSnapshot).toHaveBeenCalledTimes(3));
        await waitFor(() =>
            expect(get(stores.health)?.version).toBe("current-provider"),
        );
        expect(get(stores.backendConnection)).toBe("live");
    });

    it("attaches the snapshot listener before initial hydration", async () => {
        const order: string[] = [];
        vi.mocked(listen).mockImplementationOnce(async () => {
            order.push("listen");
            return () => undefined;
        });
        getAppSnapshot.mockImplementationOnce(async () => {
            order.push("snapshot");
            return {
                revision: 1,
                health,
                metrics,
                sessions: liveSessions,
                rate_limits: rateLimitInfo,
                discord_preview: await getDiscordPreview(),
                discord_settings: {
                    provider: "claude",
                    enabled: true,
                    status: "Connected",
                    publisher: "pulse",
                    display_prefs: {
                        show_project: true,
                        show_branch: true,
                        show_model: true,
                        show_activity: true,
                        show_tokens: false,
                        show_cost: false,
                        show_limits: true,
                        show_credits: false,
                        show_context: true,
                        show_systems: true,
                    },
                    desktop_design: null,
                    supports_desktop_design: false,
                    supports_field_order: false,
                    field_order: [],
                },
                plan: planInfoFixture,
                access: accessFixture,
            };
        });
        stores.startSnapshotSync();
        await waitFor(() => expect(getAppSnapshot).toHaveBeenCalledTimes(1));

        expect(order).toEqual(["listen", "snapshot"]);
        stores.stopSnapshotSync();
    });

    it("renders the Dashboard against the polled store state end to end", async () => {
        await stores.poll();
        await tick();

        const { container, getByText } = render(Dashboard);
        await tick();

        await waitFor(() => expect(getByText("Provider limits")).toBeTruthy());
        expect(getByText("Working now")).toBeTruthy();
        expect(container.querySelector(".stats-row")).toBeNull();
        await waitFor(() => {
            expect(
                container.querySelectorAll("[data-session-instance]").length,
            ).toBe(2);
        });
    });

    it("does not fabricate frontend threshold notifications from ordinary usage changes", async () => {
        stores.toasts.set([]);
        const before = {
            ...rateLimitInfo,
            five_hour_pct: 79,
            seven_day_pct: 94,
        };
        const after = {
            ...rateLimitInfo,
            five_hour_pct: 81,
            seven_day_pct: 96,
        };
        const baseSnapshot = await getAppSnapshot();
        getAppSnapshot
            .mockResolvedValueOnce({ ...baseSnapshot, rate_limits: before })
            .mockResolvedValueOnce({ ...baseSnapshot, rate_limits: after });

        await stores.poll();
        await stores.poll();

        expect(get(stores.toasts)).toEqual([]);
    });
});
