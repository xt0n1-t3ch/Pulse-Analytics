import { describe, it, expect, beforeEach } from "bun:test";
import { vi } from "../bun-setup";
import { fireEvent, render, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
import type {
    HistoricalSession,
    CostForecast,
    BudgetStatus,
    CostTotals,
} from "@/lib/api";

vi.mock("@/components/Chart.svelte", async () => ({
    default: (await import("../fixtures/ChartStub.svelte")).default,
}));

function hist(
    id: string,
    project: string,
    parts: { input: number; output: number; cacheW: number; cacheR: number },
): HistoricalSession {
    const total = parts.input + parts.output + parts.cacheW + parts.cacheR;
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
        total_cost: total,
        cost_basis: "exact",
        cost_source: "anthropic_api_equivalent",
        known_cost: total,
        input_tokens: 50_000,
        output_tokens: 20_000,
        cache_write_tokens: 10_000,
        cache_read_tokens: 100_000,
        total_tokens: 180_000,
        input_cost: parts.input,
        output_cost: parts.output,
        cache_write_cost: parts.cacheW,
        cache_read_cost: parts.cacheR,
        has_thinking: false,
        subagent_count: 0,
        is_active: false,
    };
}

const histList = [
    hist("h1", "pulse", { input: 3, output: 4, cacheW: 2, cacheR: 1 }),
    hist("h2", "other", { input: 1.5, output: 2, cacheW: 1, cacheR: 0.5 }),
];

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

const budget: BudgetStatus = {
    monthly_budget: 100,
    alert_threshold_pct: 80,
    billed_spend_usd: 30,
    projected_billed_spend_usd: 93,
    api_equivalent_usd: null,
    projected_api_equivalent_usd: null,
    pct_used: 30,
    over_budget: false,
    cost_basis: "exact",
    cost_sources: ["provider_billed"],
    sessions: 2,
    priced_sessions: 2,
    billed_sessions: 2,
    api_equivalent_sessions: 0,
    refreshed_at: "2026-08-12T12:00:00Z",
};

const getSessionHistory = vi.fn(async () => histList);
const getCostForecast = vi.fn(async () => forecast);
const getBudgetStatus = vi.fn(async () => budget);
const setBudget = vi.fn(async () => undefined);

/** Window-wide aggregate matching `histList`, since the KPIs read from here
 *  rather than summing the visible page. */
const totals: CostTotals = {
    days: 30,
    sessions: histList.length,
    total_cost: histList.reduce((s, h) => s + h.total_cost, 0),
    input_cost: histList.reduce((s, h) => s + h.input_cost, 0),
    output_cost: histList.reduce((s, h) => s + h.output_cost, 0),
    cache_write_cost: histList.reduce((s, h) => s + h.cache_write_cost, 0),
    cache_read_cost: histList.reduce((s, h) => s + h.cache_read_cost, 0),
    total_tokens: histList.reduce((s, h) => s + h.total_tokens, 0),
    input_tokens: histList.reduce((s, h) => s + h.input_tokens, 0),
    output_tokens: histList.reduce((s, h) => s + h.output_tokens, 0),
    cache_write_tokens: histList.reduce((s, h) => s + h.cache_write_tokens, 0),
    cache_read_tokens: histList.reduce((s, h) => s + h.cache_read_tokens, 0),
    pure_input_tokens: 40_000,
    cost_basis: "exact",
    cost_sources: ["anthropic_api_equivalent"],
    priced_sessions: histList.length,
    billed_spend_usd: null,
    api_equivalent_usd: histList.reduce((s, h) => s + h.total_cost, 0),
    billed_sessions: 0,
    api_equivalent_sessions: histList.length,
    by_model: [
        {
            label: "Claude Opus 4.8",
            cost: histList.reduce((s, h) => s + h.total_cost, 0),
            sessions: 2,
        },
    ],
    by_project: [
        { label: "pulse", cost: 10, sessions: 1 },
        { label: "other", cost: 5, sessions: 1 },
    ],
};
/** Aggregate for a single project, so a filtered view can be told apart from
 *  the unfiltered one. */
const projectTotals: CostTotals = {
    ...totals,
    sessions: 1,
    total_cost: 10,
    api_equivalent_usd: 10,
    api_equivalent_sessions: 1,
    by_project: [{ label: "pulse", cost: 10, sessions: 1 }],
};
const getCostTotals = vi.fn(async (_days?: number, project?: string) =>
    project ? projectTotals : totals,
);
async function buildCostsBundle(project?: string) {
    const [history, bundleForecast, bundleBudget, bundleTotals] =
        await Promise.all([
            getSessionHistory(),
            getCostForecast(),
            getBudgetStatus(),
            getCostTotals(30, project),
        ]);
    return {
        history,
        forecast: bundleForecast,
        budget: bundleBudget,
        totals: bundleTotals,
        daily_usage: [],
    };
}
const getCostsBundle = vi.fn(buildCostsBundle);

vi.mock("@/lib/api", async (importOriginal) => {
    const actual = await importOriginal<typeof import("@/lib/api")>();
    return {
        ...actual,
        getCostsBundle: (project?: string) => getCostsBundle(project),
        getSessionHistory: () => getSessionHistory(),
        getCostForecast: () => getCostForecast(),
        getCostTotals: (days?: number, project?: string) =>
            getCostTotals(days, project),
        getBudgetStatus: () => getBudgetStatus(),
        setBudget: () => setBudget(),
    };
});

describe("Costs.svelte", () => {
    beforeEach(async () => {
        getCostsBundle.mockReset();
        getCostsBundle.mockImplementation(buildCostsBundle);
        getSessionHistory.mockReset();
        getSessionHistory.mockResolvedValue(histList);
        getCostForecast.mockReset();
        getCostForecast.mockResolvedValue(forecast);
        getCostTotals.mockReset();
        getCostTotals.mockImplementation(
            async (_days?: number, project?: string) =>
                project ? projectTotals : totals,
        );
        getBudgetStatus.mockReset();
        getBudgetStatus.mockResolvedValue(budget);
        setBudget.mockReset();
        setBudget.mockResolvedValue(undefined);
        const { sessions, selectedAnalyticsProviderScope, planInfo } =
            await import("@/lib/stores");
        sessions.set([]);
        selectedAnalyticsProviderScope.set("all");
        planInfo.set(null);
    });

    it("leads with the value ledger and keeps the budget cockpit for known spend", async () => {
        const Costs = (await import("@/views/Costs.svelte")).default;
        const { container } = render(Costs);
        await tick();

        await waitFor(() => expect(getSessionHistory).toHaveBeenCalled());
        // Usage value is the hero; the spend gauge remains available below it.
        await waitFor(() =>
            expect(container.querySelector(".value-ledger")).not.toBeNull(),
        );
        await waitFor(() =>
            expect(container.querySelector(".cockpit")).not.toBeNull(),
        );
        const ledger = container.querySelector(".value-ledger") as HTMLElement;
        const cockpit = container.querySelector(".cockpit") as HTMLElement;
        expect(
            ledger.compareDocumentPosition(cockpit) &
                Node.DOCUMENT_POSITION_FOLLOWING,
        ).toBeTruthy();

        const labels = [...container.querySelectorAll(".is-label")].map((e) =>
            e.textContent?.trim(),
        );
        expect(labels).toEqual([
            "Cost per session",
            "Cost per 1M tokens",
            "Cache savings",
            "30-day total",
        ]);
    });

    it("reads KPI totals from the window aggregate, not the visible page", async () => {
        const Costs = (await import("@/views/Costs.svelte")).default;
        const { container } = render(Costs);
        await tick();

        await waitFor(() => expect(getCostTotals).toHaveBeenCalled());
        await waitFor(() =>
            expect(container.querySelector(".is-value")).not.toBeNull(),
        );

        const values = [...container.querySelectorAll(".is-value")].map((e) =>
            e.textContent?.trim(),
        );
        // The provenance-labeled 30-day value mirrors the aggregate exactly.
        expect(values[3]).toBe("$" + totals.total_cost.toFixed(2));
        // Average divides by the aggregate session count.
        expect(values[0]).toBe(
            "$" + (totals.total_cost / totals.sessions).toFixed(2),
        );
    });

    it("renders a monetary-value breakdown whose legend reconciles to the per-component total", async () => {
        const Costs = (await import("@/views/Costs.svelte")).default;
        const { container, getByText } = render(Costs);
        await tick();

        await waitFor(() =>
            expect(container.querySelector(".cost-type-bar")).not.toBeNull(),
        );

        const inputCost = histList.reduce((s, h) => s + h.input_cost, 0);
        const outputCost = histList.reduce((s, h) => s + h.output_cost, 0);
        const cacheWCost = histList.reduce((s, h) => s + h.cache_write_cost, 0);
        const cacheRCost = histList.reduce((s, h) => s + h.cache_read_cost, 0);

        const vals = [
            ...container.querySelectorAll(".cost-type-legend .ct-val"),
        ].map((e) => e.textContent?.trim());
        expect(vals).toEqual([
            "$" + inputCost.toFixed(2),
            "$" + outputCost.toFixed(2),
            "$" + cacheWCost.toFixed(2),
            "$" + cacheRCost.toFixed(2),
        ]);

        const legendTotal = inputCost + outputCost + cacheWCost + cacheRCost;
        const rowsTotal = histList.reduce((s, h) => s + h.total_cost, 0);
        expect(legendTotal).toBeCloseTo(rowsTotal, 5);
        expect(getByText("Session details")).toBeTruthy();
    });

    it("bounds the initial session-detail DOM while preserving the complete export dataset", async () => {
        const manySessions = Array.from({ length: 75 }, (_, index) => ({
            ...hist(`history-${index}`, `project-${index}`, {
                input: 1,
                output: 1,
                cacheW: 0,
                cacheR: 0,
            }),
        }));
        getSessionHistory.mockResolvedValueOnce(manySessions);

        const Costs = (await import("@/views/Costs.svelte")).default;
        const { container, getByRole } = render(Costs);

        await waitFor(() =>
            expect(
                container.querySelectorAll(".detail-table tbody tr"),
            ).toHaveLength(50),
        );
        await fireEvent.click(getByRole("button", { name: "Show 25 more" }));
        await waitFor(() =>
            expect(
                container.querySelectorAll(".detail-table tbody tr"),
            ).toHaveLength(75),
        );
    });

    it("plots spend, projection and cap on the cockpit gauge", async () => {
        const Costs = (await import("@/views/Costs.svelte")).default;
        const { container, findByText } = render(Costs);
        await tick();

        await waitFor(() =>
            expect(container.querySelector(".ck-track")).not.toBeNull(),
        );
        // $30 spent, $93 projected, $100 cap: healthy, and the cap tick is drawn.
        expect(container.querySelector(".ck-figure")?.textContent).toContain(
            "30.00",
        );
        expect(container.querySelector(".ck-cap")).not.toBeNull();
        expect(await findByText(/under the .* cap/)).toBeTruthy();
    });

    /** A filtered project may have more sessions than the capped table page, so
     *  its KPIs must come from an aggregate fetched for that project. */
    it("reloads the window aggregate for the selected project", async () => {
        const Costs = (await import("@/views/Costs.svelte")).default;
        const { container } = render(Costs);
        await tick();
        await waitFor(() => expect(getCostTotals).toHaveBeenCalled());

        const select = container.querySelector("select") as HTMLSelectElement;
        // The option list is derived from loaded sessions, so wait for it before
        // selecting: assigning an absent value silently no-ops.
        await waitFor(() =>
            expect([...select.options].some((o) => o.value === "pulse")).toBe(
                true,
            ),
        );
        await fireEvent.change(select, { target: { value: "pulse" } });
        await tick();

        await waitFor(() =>
            expect(getCostTotals).toHaveBeenCalledWith(30, "pulse"),
        );
        await waitFor(() => {
            const values = [...container.querySelectorAll(".is-value")].map(
                (e) => e.textContent?.trim(),
            );
            expect(values[3]).toBe("$" + projectTotals.total_cost.toFixed(2));
        });
    });

    /** With a session still running, a KPI fetched once on mount silently goes
     *  stale while the table keeps moving. Identity-only snapshot replacements,
     *  however, must not refetch the monetary aggregate. */
    it("refreshes the aggregate only when the live monetary fingerprint changes", async () => {
        const { sessions } = await import("@/lib/stores");
        const live = {
            session_id: "live-cost",
            provider: "claude",
            project: "pulse",
            model: "Claude Opus 4.8",
            branch: null,
            cost: 1,
            cost_available: true,
            cost_basis: "exact",
            input_tokens: 10,
            output_tokens: 5,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
        } as any;
        sessions.set([live]);
        const Costs = (await import("@/views/Costs.svelte")).default;
        render(Costs);
        await tick();
        await waitFor(() => expect(getCostTotals).toHaveBeenCalled());
        const initialCalls = getCostTotals.mock.calls.length;

        sessions.set([{ ...live }]);
        await tick();
        expect(getCostTotals.mock.calls.length).toBe(initialCalls);

        sessions.set([{ ...live, cost: 2, output_tokens: 6 }]);
        await tick();

        await waitFor(() =>
            expect(getCostTotals.mock.calls.length).toBeGreaterThan(
                initialCalls,
            ),
        );
    });

    it("does not render the active database row twice beside its live snapshot", async () => {
        const { sessions } = await import("@/lib/stores");
        const duplicate = { ...histList[0], is_active: true };
        getSessionHistory.mockResolvedValueOnce([duplicate, histList[1]]);
        sessions.set([
            {
                session_id: "runtime-pulse",
                project: "pulse",
                model: "Claude Opus 4.8",
                branch: null,
                cost: 10,
                tokens: 180_000,
                input_tokens: 50_000,
                output_tokens: 20_000,
                cache_write_tokens: 10_000,
                cache_read_tokens: 100_000,
                input_cost: 3,
                output_cost: 4,
                cache_write_cost: 2,
                cache_read_cost: 1,
                is_idle: false,
            } as any,
        ]);

        const Costs = (await import("@/views/Costs.svelte")).default;
        const { container } = render(Costs);
        await waitFor(() =>
            expect(
                container.querySelectorAll(".detail-table tbody tr").length,
            ).toBeGreaterThan(0),
        );

        const pulseRows = [
            ...container.querySelectorAll(".detail-table tbody tr"),
        ].filter((row) => row.textContent?.includes("pulse"));
        expect(pulseRows).toHaveLength(1);
    });

    it("keeps ambiguous structurally matching active history rows visible", async () => {
        const { sessions } = await import("@/lib/stores");
        const first = { ...histList[0], id: "db-active-1", is_active: true };
        const second = { ...histList[0], id: "db-active-2", is_active: true };
        getSessionHistory.mockResolvedValueOnce([first, second]);
        sessions.set([
            {
                session_id: "runtime-pulse",
                project: "pulse",
                model: "Claude Opus 4.8",
                branch: null,
                cost: 10,
                tokens: 180_000,
                input_tokens: 50_000,
                output_tokens: 20_000,
                cache_write_tokens: 10_000,
                cache_read_tokens: 100_000,
                input_cost: 3,
                output_cost: 4,
                cache_write_cost: 2,
                cache_read_cost: 1,
                is_idle: false,
            } as any,
        ]);

        const Costs = (await import("@/views/Costs.svelte")).default;
        const { container } = render(Costs);
        await waitFor(() => {
            const pulseRows = [
                ...container.querySelectorAll(".detail-table tbody tr"),
            ].filter((row) => row.textContent?.includes("pulse"));
            expect(pulseRows).toHaveLength(3);
        });
    });

    it("fails closed when the cost backend is unavailable instead of rendering fake zeroes", async () => {
        getCostsBundle.mockRejectedValue(new Error("database unavailable"));
        const Costs = (await import("@/views/Costs.svelte")).default;
        const { findByRole, queryByText } = render(Costs);

        const alert = await findByRole("alert");
        expect(alert.textContent).toContain("Cost data unavailable");
        expect(alert.textContent).toContain("Retry");
        expect(queryByText("Provider-billed this month")).toBeNull();
    });

    it("keeps the current-month zero state concise", async () => {
        getCostForecast.mockResolvedValueOnce({
            ...forecast,
            billed_spend_usd: 0,
            projected_billed_spend_usd: 0,
            daily_billed_spend_usd: 0,
            days_elapsed: 3,
        });
        const Costs = (await import("@/views/Costs.svelte")).default;
        const { findByText, queryByText } = render(Costs);

        expect(
            await findByText("No spend recorded this month yet."),
        ).toBeTruthy();
        expect(queryByText(/previous 30 days include/i)).toBeNull();
    });

    it("keeps partial coverage inside the ledger footer", async () => {
        getCostTotals.mockResolvedValue({
            ...totals,
            cost_basis: "partial",
            cost_sources: ["codex_api_equivalent"],
            priced_sessions: 1,
            sessions: 2,
        } as CostTotals);
        const Costs = (await import("@/views/Costs.svelte")).default;
        const { container, findByText } = render(Costs);

        expect(await findByText("Partial cost coverage")).toBeTruthy();
        const ledger = container.querySelector(".value-ledger") as HTMLElement;
        expect(ledger.textContent?.replace(/\s+/g, " ")).toContain(
            "Known cost for 1 of 2 sessions. This is a lower bound.",
        );
        expect(ledger.textContent?.replace(/\s+/g, " ")).toContain(
            "API-equivalent · session-calculated",
        );
        expect(container.querySelector(".coverage-strip")).toBeNull();
    });

    it("passes the parsed plan price to the subscription cockpit", async () => {
        const { planInfo } = await import("@/lib/stores");
        planInfo.set({
            provider: "codex",
            plan_key: "pro_20x",
            plan_name: "Pro 20x ($200/mo)",
            detected: true,
        });
        getCostForecast.mockResolvedValueOnce({
            ...forecast,
            billed_spend_usd: null,
            projected_billed_spend_usd: null,
            daily_billed_spend_usd: null,
            billed_sessions: 0,
            api_equivalent_usd: 84.25,
            projected_api_equivalent_usd: 112.5,
            api_equivalent_sessions: 2,
            cost_basis: "estimated",
            cost_sources: ["codex_api_equivalent"],
        });

        const Costs = (await import("@/views/Costs.svelte")).default;
        const { findByText } = render(Costs);

        expect(await findByText("$200.00")).toBeTruthy();
        expect(await findByText("Pro 20x subscription")).toBeTruthy();
    });

    it("renders the full model label without ellipsis", async () => {
        const fullModelName = "GPT-5.6 Sol · Medium Reasoning";
        getCostTotals.mockResolvedValue({
            ...totals,
            by_model: [
                { label: fullModelName, cost: totals.total_cost, sessions: 2 },
            ],
        });

        const Costs = (await import("@/views/Costs.svelte")).default;
        const { container, findByText } = render(Costs);

        expect(await findByText(fullModelName)).toBeTruthy();
        const label = container.querySelector(".mc-name") as HTMLElement;
        expect(label.title).toBe(fullModelName);
        expect(getComputedStyle(label).textOverflow).not.toBe("ellipsis");
        expect(getComputedStyle(label).whiteSpace).not.toBe("nowrap");
    });

    it("turns unavailable billing into a provenance-aware value ledger", async () => {
        getSessionHistory.mockResolvedValueOnce(
            histList.map((session) => ({
                ...session,
                total_cost: 0,
                known_cost: null,
                cost_basis: "unavailable" as const,
                cost_source: "unknown",
                input_cost: 0,
                output_cost: 0,
                cache_write_cost: 0,
                cache_read_cost: 0,
            })),
        );
        getCostForecast.mockResolvedValueOnce({
            ...forecast,
            billed_spend_usd: null,
            projected_billed_spend_usd: null,
            daily_billed_spend_usd: null,
            api_equivalent_usd: null,
            projected_api_equivalent_usd: null,
            daily_api_equivalent_usd: null,
            cost_basis: "unavailable",
            cost_sources: [],
            priced_sessions: 0,
            billed_sessions: 0,
            api_equivalent_sessions: 0,
        });
        getBudgetStatus.mockResolvedValueOnce({
            ...budget,
            billed_spend_usd: null,
            projected_billed_spend_usd: null,
            api_equivalent_usd: null,
            projected_api_equivalent_usd: null,
            pct_used: null,
            cost_basis: "unavailable",
            cost_sources: [],
            priced_sessions: 0,
            billed_sessions: 0,
            api_equivalent_sessions: 0,
        });
        getCostTotals.mockResolvedValue({
            ...totals,
            total_cost: 0,
            input_cost: 0,
            output_cost: 0,
            cache_write_cost: 0,
            cache_read_cost: 0,
            cost_basis: "unavailable",
            cost_sources: [],
            priced_sessions: 0,
            by_model: [],
            by_project: [],
        });

        const Costs = (await import("@/views/Costs.svelte")).default;
        const {
            container,
            findByText,
            findAllByText,
            getAllByText,
            queryByText,
        } = render(Costs);

        expect(await findByText("Spend and coverage")).toBeTruthy();
        expect(
            (await findAllByText("Cost not reported by provider")).length,
        ).toBeGreaterThan(0);
        await waitFor(() => {
            const values = [...container.querySelectorAll(".ledger-value")].map(
                (element) => element.textContent?.trim(),
            );
            expect(values).toContain("360.0K");
            expect(values).toContain("2");
            expect(values).toContain("0/2");
        });
        expect(getAllByText("Not reported").length).toBeGreaterThan(0);
        expect(container.querySelector(".cockpit")).toBeNull();
        expect(container.querySelector(".usage-token-mix")).not.toBeNull();
        expect(queryByText("No spend recorded this month yet.")).toBeNull();
    });
});
