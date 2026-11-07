import { describe, it, expect, beforeEach } from "bun:test";
import { vi } from "../bun-setup";
import { fireEvent, render, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
import type {
    SessionInfo,
    HistoricalSession,
    AnalyticsSummary,
} from "@/lib/api";

function makeSession(id: string, project: string, cost: number): SessionInfo {
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
        cost,
        cost_available: true,
        cost_basis: "exact",
        tokens: 120_000,
        input_tokens: 40_000,
        output_tokens: 20_000,
        cache_write_tokens: 10_000,
        cache_read_tokens: 50_000,
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
        input_cost: cost * 0.3,
        output_cost: cost * 0.4,
        cache_write_cost: cost * 0.2,
        cache_read_cost: cost * 0.1,
        speed: "standard",
        fast: false,
        service_tier: null,
        app_name: null,
    };
}

function hist(id: string, project: string, cost: number): HistoricalSession {
    return {
        id,
        provider: "claude",
        session_name: null,
        project,
        model: "Claude Opus 4.8",
        model_id: "claude-opus-4-8",
        context_window: "200K",
        branch: "main",
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
        subagent_count: 1,
        is_active: false,
    };
}

const summary: AnalyticsSummary = {
    total_sessions: 5,
    priced_sessions: 5,
    total_cost: 25,
    cost_basis: "exact",
    cost_sources: ["anthropic_api_equivalent"],
    total_tokens: 2_000_000,
    total_cache_read: 1_000_000,
    total_cache_write: 100_000,
    avg_duration_secs: 1200,
    avg_tokens_per_session: 400_000,
    avg_cost_per_session: 5,
    top_project: "pulse",
    top_model: "Claude Opus 4.8",
    days_tracked: 30,
};

const getAnalyticsSummary = vi.fn(async () => summary);
const getSessionHistory = vi.fn(async () => [
    hist("h1", "pulse", 6),
    hist("h2", "other", 4),
]);
const getSessionHistoryFiltered = vi.fn(async () => [] as HistoricalSession[]);
const searchSessions = vi.fn(async () => [] as HistoricalSession[]);

vi.mock("@/lib/api", async (importOriginal) => {
    const actual = await importOriginal<typeof import("@/lib/api")>();
    return {
        ...actual,
        getAnalyticsSummary: () => getAnalyticsSummary(),
        getSessionHistory: () => getSessionHistory(),
        getSessionHistoryFiltered: () => getSessionHistoryFiltered(),
        searchSessions: () => searchSessions(),
    };
});

describe("Sessions.svelte", () => {
    beforeEach(() => {
        getAnalyticsSummary.mockClear();
        getSessionHistory.mockClear();
    });

    it("mounts, shows the KPI tiles, and lists live session rows", async () => {
        const { sessions } = await import("@/lib/stores");
        sessions.set([
            makeSession("s1", "pulse", 3),
            makeSession("s2", "other", 2),
        ]);

        const Sessions = (await import("@/views/Sessions.svelte")).default;
        const { container } = render(Sessions);
        await tick();

        const labels = [...container.querySelectorAll(".stat-label")].map((e) =>
            e.textContent?.trim(),
        );
        expect(labels).toEqual([
            "Active sessions",
            "Live tokens",
            "Reported value",
            "Output rate",
        ]);

        await waitFor(() => {
            expect(
                container.querySelectorAll(".session-list .session-card")
                    .length,
            ).toBe(2);
        });
        expect(container.querySelector(".stat-value")?.textContent).toBe("2");
    });

    it("loads history as a labelled data table", async () => {
        const { sessions } = await import("@/lib/stores");
        sessions.set([]);

        const Sessions = (await import("@/views/Sessions.svelte")).default;
        const { getByRole, getByText } = render(Sessions);
        await tick();

        await waitFor(() => expect(getSessionHistory).toHaveBeenCalled());
        const table = await waitFor(() =>
            getByRole("table", { name: "Session history ledger" }),
        );
        expect(table.querySelectorAll("tbody .ht-row")).toHaveLength(2);
        expect(table.querySelectorAll('thead th[scope="col"]')).toHaveLength(7);
        expect(getByText("Session history")).toBeTruthy();
    });

    it("keeps historical cost ranking in the single sortable history ledger", async () => {
        const { sessions } = await import("@/lib/stores");
        sessions.set([]);

        const Sessions = (await import("@/views/Sessions.svelte")).default;
        const { container, queryByText } = render(Sessions);
        await tick();

        await waitFor(() =>
            expect(container.querySelectorAll(".ht-row").length).toBe(2),
        );
        expect(queryByText("Most Costly Sessions (30 days)")).toBeNull();
    });

    it("uses custom project and sort controls plus segmented history ranges", async () => {
        const { sessions } = await import("@/lib/stores");
        sessions.set([makeSession("s1", "pulse", 3)]);

        const Sessions = (await import("@/views/Sessions.svelte")).default;
        const { container, getByRole } = render(Sessions);
        await tick();

        expect(container.querySelector("select")).toBeNull();
        expect(getByRole("button", { name: "Filter by project" })).toBeTruthy();
        expect(getByRole("group", { name: "Sort sessions" })).toBeTruthy();
        expect(getByRole("group", { name: "History range" })).toBeTruthy();
    });

    it("excludes retained idle snapshots from live KPIs and rows", async () => {
        const { sessions } = await import("@/lib/stores");
        sessions.set([
            makeSession("live", "active-project", 3),
            { ...makeSession("idle", "idle-project", 99), is_idle: true },
        ]);

        const Sessions = (await import("@/views/Sessions.svelte")).default;
        const { container } = render(Sessions);
        await tick();

        expect(container.querySelector(".stat-value")?.textContent).toBe("1");
        expect(
            container.querySelectorAll(".session-list .session-card"),
        ).toHaveLength(1);
        expect(container.textContent).not.toContain("idle-project");
    });

    it("marks an unpriced historical session unavailable instead of rendering its raw estimate", async () => {
        const { sessions } = await import("@/lib/stores");
        sessions.set([]);
        getSessionHistory.mockResolvedValueOnce([
            {
                ...hist("unpriced", "unpriced-project", 99),
                cost_basis: "unavailable",
                cost_source: "unknown",
                known_cost: null,
            },
        ]);

        const Sessions = (await import("@/views/Sessions.svelte")).default;
        const { container } = render(Sessions);

        await waitFor(() =>
            expect(container.querySelectorAll(".ht-row").length).toBe(1),
        );
        const row = container.querySelector(".ht-row");
        expect(row?.textContent).toContain("—");
        expect(row?.textContent).not.toContain("Unavailable");
        expect(row?.textContent).not.toContain("$99.00");
    });

    it("labels API-equivalent historical cost as estimated", async () => {
        const { sessions } = await import("@/lib/stores");
        sessions.set([]);
        getSessionHistory.mockResolvedValueOnce([
            {
                ...hist("estimated", "estimated-project", 2.5),
                cost_basis: "estimated",
                cost_source: "api_equivalent",
            },
        ]);

        const Sessions = (await import("@/views/Sessions.svelte")).default;
        const { container } = render(Sessions);

        await waitFor(() =>
            expect(container.querySelectorAll(".ht-row")).toHaveLength(1),
        );
        const row = container.querySelector(".ht-row");
        expect(row?.textContent).toContain("$2.50");
        expect(row?.textContent).toContain("estimate");
    });

    it("keeps search and all-time facts in one divided toolbar", async () => {
        const { sessions } = await import("@/lib/stores");
        sessions.set([]);
        getAnalyticsSummary.mockResolvedValueOnce({
            ...summary,
            cost_basis: "partial",
            total_cost: 8303.05,
        });

        const Sessions = (await import("@/views/Sessions.svelte")).default;
        const { container, getByRole } = render(Sessions);

        await waitFor(() =>
            expect(container.querySelector(".history-summary")).not.toBeNull(),
        );
        const input = getByRole("textbox", { name: "Search session history" });
        const toolbar = input.closest(".history-search-toolbar");
        const facts = toolbar?.querySelector(".history-summary");

        expect(toolbar).not.toBeNull();
        expect(facts).not.toBeNull();
        expect(facts?.textContent?.replace(/\s+/g, " ")).toContain(
            "$8303.05 API-equivalent value · known subtotal",
        );
        expect(
            toolbar?.querySelectorAll(".search-box, .history-summary"),
        ).toHaveLength(2);
        expect(container.querySelector(".history-summary.card")).toBeNull();
    });

    it("marks an unavailable all-time cost as unavailable instead of zero dollars", async () => {
        const { sessions } = await import("@/lib/stores");
        sessions.set([]);
        getAnalyticsSummary.mockResolvedValueOnce({
            ...summary,
            priced_sessions: 0,
            total_cost: 0,
            avg_cost_per_session: 0,
            cost_basis: "unavailable",
            cost_sources: [],
        });

        const Sessions = (await import("@/views/Sessions.svelte")).default;
        const { container } = render(Sessions);
        await tick();

        await waitFor(() => {
            expect(
                container.querySelector(".history-summary")?.textContent,
            ).toContain(
                "5 sessions · — Known monetary value · 2.0M tokens · top contributor pulse · 30 days",
            );
        });
        expect(
            container.querySelector(".history-summary")?.textContent,
        ).not.toContain("$0.00");
    });

    it("does not turn an initial history read failure into a false empty result", async () => {
        const { sessions } = await import("@/lib/stores");
        sessions.set([]);
        getSessionHistory.mockRejectedValueOnce(
            new Error("database unavailable"),
        );

        const Sessions = (await import("@/views/Sessions.svelte")).default;
        const { findByRole, queryByText } = render(Sessions);

        expect((await findByRole("alert")).textContent).toContain(
            "Session history unavailable",
        );
        expect(
            queryByText(/No sessions match the selected history filters/i),
        ).toBeNull();
    });

    it("bounds the initial history DOM without truncating the loaded dataset", async () => {
        const manySessions = Array.from({ length: 75 }, (_, index) => ({
            ...hist(`history-${index}`, `project-${index}`, index + 1),
        }));
        getSessionHistory.mockResolvedValueOnce(manySessions);

        const Sessions = (await import("@/views/Sessions.svelte")).default;
        const { container, getByRole } = render(Sessions);

        await waitFor(() =>
            expect(container.querySelectorAll(".ht-row")).toHaveLength(50),
        );
        await fireEvent.click(getByRole("button", { name: "Show 25 more" }));
        await waitFor(() =>
            expect(container.querySelectorAll(".ht-row")).toHaveLength(75),
        );
    });
});
