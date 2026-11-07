import { describe, it, expect, beforeEach } from "bun:test";
import { vi } from "../bun-setup";
import { render, waitFor, fireEvent } from "@testing-library/svelte";
import type { ReportsBundle } from "@/lib/api";
import { provider } from "@/lib/provider";

/** Ten days ending today, so date-derived assertions stay stable. */
function makeDailyCosts(): ReportsBundle["daily_costs"] {
    const today = new Date();
    return Array.from({ length: 10 }, (_, i) => {
        const d = new Date(today);
        d.setDate(d.getDate() - (9 - i));
        const date = d.toISOString().slice(0, 10);
        // One clear peak on the 4th day, two idle days, rest modest.
        const cost = i === 3 ? 42 : i === 5 || i === 6 ? 0 : 6;
        const sessions = cost > 0 ? 2 : 0;
        return {
            date,
            cost,
            sessions,
            priced_sessions: sessions,
            cost_basis:
                sessions > 0 ? ("exact" as const) : ("unavailable" as const),
            cost_sources: sessions > 0 ? ["anthropic_api_equivalent"] : [],
        };
    });
}

function makeBundle(): ReportsBundle {
    return {
        provider: "claude",
        capabilities: {
            cache_health: true,
            model_routing: true,
            extra_usage: true,
        },
        days: 30,
        total_sessions: 3,
        priced_sessions: 3,
        cost_basis: "exact",
        cost_sources: ["anthropic_api_equivalent"],
        recommendations: [
            {
                id: "rec-1",
                severity: "warning",
                title: "Trim memory files",
                description: "Your memory footprint is heavy.",
                estimated_savings: "$1.20",
                action: "Edit CLAUDE.md",
                fix_prompt: "Help me trim my memory files.",
                color: "#fbbf24",
            },
        ],
        trace_overview: {
            provider: "claude",
            provider_display: "Claude Code",
            instruction_file: "CLAUDE.md",
            fix_button_label: "Fix with Claude",
            session_store: "",
            global_state_source: "",
            traced_sessions: 2,
            total_sessions: 3,
            user_messages: 10,
            assistant_messages: 12,
            total_tool_calls: 40,
            total_compactions: 1,
            mcp_tool_calls: 4,
            cache_hit_ratio: 80,
            top_tools: [],
            telemetry_mermaid: "",
            cache_mermaid: "",
        },
        tool_frequency: {
            available: true,
            sessions_analyzed: 3,
            traced_sessions: 2,
            total_tool_calls: 40,
            avg_tools_per_session: 13,
            avg_tool_calls_per_hour: 5,
            mcp_tool_calls: 4,
            mcp_share_pct: 10,
            compact_gap_sessions: 0,
            diagnosis: "Healthy tool mix.",
            top_tools: [],
        },
        prompt_complexity: {
            available: true,
            sessions_analyzed: 3,
            prompts_analyzed: 9,
            avg_complexity_score: 50,
            avg_specificity_score: 60,
            high_complexity_sessions: 1,
            low_specificity_sessions: 0,
            diagnosis: "Prompts are specific.",
            top_sessions: [],
        },
        session_health: {
            available: true,
            sessions_analyzed: 3,
            health_score: 88,
            grade: "A",
            avg_duration_minutes: 12,
            p90_duration_minutes: 30,
            long_session_pct: 10,
            avg_messages_per_session: 8,
            peak_overlap_pct: 5,
            compact_gap_pct: 0,
            diagnosis: "Sessions look healthy.",
        },
        cache_health: {
            grade: "A",
            grade_label: "Excellent",
            color: "#62b462",
            hit_ratio: 80,
            trend_weighted_ratio: 82,
            total_cache_read: 5_000_000,
            total_cache_write: 1_000_000,
            total_input: 2_000_000,
            sessions_analyzed: 3,
            diagnosis: "Cache is doing its job.",
        },
        model_routing: {
            total_sessions: 3,
            priced_sessions: 3,
            total_cost: 10,
            cost_basis: "exact",
            cost_sources: ["anthropic_api_equivalent"],
            opus: {
                sessions: 2,
                priced_sessions: 2,
                cost: 8,
                cost_share_pct: 80,
                avg_cost_per_session: 4,
            },
            sonnet: {
                sessions: 1,
                priced_sessions: 1,
                cost: 2,
                cost_share_pct: 20,
                avg_cost_per_session: 2,
            },
            haiku: {
                sessions: 0,
                priced_sessions: 0,
                cost: 0,
                cost_share_pct: 0,
                avg_cost_per_session: 0,
            },
            other: {
                sessions: 0,
                priced_sessions: 0,
                cost: 0,
                cost_share_pct: 0,
                avg_cost_per_session: 0,
            },
            savings_estimate_available: true,
            estimated_savings_if_rerouted: 1.5,
            diagnosis: "Mostly Opus.",
        },
        inflection_points: [],
        daily_costs: makeDailyCosts(),
    };
}

let resolvers: Array<() => void> = [];
let activeBundle = makeBundle();
const reportExports = vi.hoisted(() => ({
    markdown: vi.fn(async () => "# Pulse report"),
    html: vi.fn(async () => "<html>Pulse report</html>"),
}));
const getReportsBundle = vi.fn(
    (_days?: number) =>
        new Promise<ReportsBundle>((resolve) => {
            resolvers.push(() => resolve(activeBundle));
        }),
);

vi.mock("@/lib/api", async (importOriginal) => {
    const actual = await importOriginal<typeof import("@/lib/api")>();
    return {
        ...actual,
        getReportsBundle: (days?: number) => getReportsBundle(days),
        generateMarkdownReport: reportExports.markdown,
        generateHtmlReport: reportExports.html,
    };
});

function flushAll(): void {
    resolvers.forEach((r) => r());
    resolvers = [];
}

describe("Reports.svelte", () => {
    beforeEach(() => {
        getReportsBundle.mockClear();
        reportExports.markdown.mockClear();
        reportExports.html.mockClear();
        resolvers = [];
        activeBundle = makeBundle();
        provider.set("claude");
    });

    it("shows Codex cache health without exposing Claude-only model routing", async () => {
        provider.set("codex");
        activeBundle = {
            ...makeBundle(),
            provider: "codex",
            capabilities: {
                cache_health: true,
                model_routing: false,
                extra_usage: false,
            },
            trace_overview: {
                ...makeBundle().trace_overview,
                provider: "codex",
                provider_display: "Codex",
                instruction_file: "AGENTS.md",
                fix_button_label: "Fix with Codex",
            },
            model_routing: null,
        };
        const Reports = (await import("@/views/Reports.svelte")).default;
        const { findByText, queryByText } = render(Reports);

        await waitFor(() => expect(resolvers.length).toBeGreaterThan(0));
        flushAll();

        expect(await findByText("Cache is doing its job.")).toBeTruthy();
        expect(queryByText("Model routing")).toBeNull();
    });

    it("populates sections from a single bundle call", async () => {
        const Reports = (await import("@/views/Reports.svelte")).default;
        const { findByText } = render(Reports);

        await waitFor(() => expect(resolvers.length).toBeGreaterThan(0));
        flushAll();

        expect(await findByText("Trim memory files")).toBeTruthy();
        expect(await findByText("Cache is doing its job.")).toBeTruthy();
        expect(getReportsBundle).toHaveBeenCalledTimes(1);
    });

    it("shows report skeletons without blocking header controls until the first bundle arrives", async () => {
        const Reports = (await import("@/views/Reports.svelte")).default;
        const { container, getByRole, findByText } = render(Reports);

        await waitFor(() => expect(resolvers.length).toBeGreaterThan(0));

        expect(
            container.querySelector("[data-reports-loading]"),
        ).not.toBeNull();
        expect(
            container.querySelectorAll("[data-reports-loading] .skeleton")
                .length,
        ).toBeGreaterThan(0);
        expect(
            (getByRole("button", { name: "7d" }) as HTMLButtonElement).disabled,
        ).toBe(false);
        expect(
            (getByRole("button", { name: "Copy report" }) as HTMLButtonElement)
                .disabled,
        ).toBe(false);
        expect(
            (
                getByRole("button", {
                    name: "Download HTML",
                }) as HTMLButtonElement
            ).disabled,
        ).toBe(false);

        flushAll();

        expect(await findByText("Monetary value timeline")).toBeTruthy();
        await waitFor(() => {
            expect(
                container.querySelector("[data-reports-loading]"),
            ).toBeNull();
        });
    });

    it("binds the view subtitle to the selected analysis window", async () => {
        const Reports = (await import("@/views/Reports.svelte")).default;
        const { getByText, findByText } = render(Reports);

        await waitFor(() => expect(resolvers.length).toBeGreaterThan(0));
        flushAll();
        expect(await findByText("30-day window")).toBeTruthy();

        await fireEvent.click(getByText("1y"));
        expect(await findByText("Year window")).toBeTruthy();
    });

    it("disables report exports while each request is pending", async () => {
        let resolveMarkdown!: (value: string) => void;
        reportExports.markdown.mockImplementationOnce(
            () =>
                new Promise((resolve) => {
                    resolveMarkdown = resolve;
                }),
        );
        const Reports = (await import("@/views/Reports.svelte")).default;
        const { getByRole } = render(Reports);

        await waitFor(() => expect(resolvers.length).toBeGreaterThan(0));
        flushAll();
        const copy = getByRole("button", { name: "Copy report" });
        await waitFor(() =>
            expect((copy as HTMLButtonElement).disabled).toBe(false),
        );
        void fireEvent.click(copy);

        await waitFor(() =>
            expect(
                (
                    getByRole("button", {
                        name: "Copying...",
                    }) as HTMLButtonElement
                ).disabled,
            ).toBe(true),
        );
        resolveMarkdown("# Pulse report");
        await waitFor(() =>
            expect(
                (
                    getByRole("button", {
                        name: "Copy report",
                    }) as HTMLButtonElement
                ).disabled,
            ).toBe(false),
        );
    });

    it("shows loading feedback on a re-fetch triggered by a filter change", async () => {
        const Reports = (await import("@/views/Reports.svelte")).default;
        const { container, getByText } = render(Reports);

        await waitFor(() => expect(resolvers.length).toBeGreaterThan(0));
        flushAll();
        await waitFor(() => {
            expect(container.querySelector(".report-body")).not.toBeNull();
        });

        await fireEvent.click(getByText("7d"));

        await waitFor(() => {
            expect(container.querySelector(".reload-banner")).not.toBeNull();
        });

        flushAll();
        await waitFor(() => {
            expect(container.querySelector(".reload-banner")).toBeNull();
        });
    });

    it("distinguishes a failed 7-day request from a real empty window and offers retry", async () => {
        const Reports = (await import("@/views/Reports.svelte")).default;
        const { getByText, findByRole, queryByText } = render(Reports);

        await waitFor(() => expect(resolvers.length).toBeGreaterThan(0));
        flushAll();
        await waitFor(() =>
            expect(queryByText("Monetary value timeline")).toBeTruthy(),
        );

        getReportsBundle.mockRejectedValueOnce(new Error("query failed"));
        await fireEvent.click(getByText("7d"));

        const alert = await findByRole("alert");
        expect(alert.textContent).toContain("7-day report unavailable");
        expect(alert.textContent).toContain("Retry");
        expect(alert.textContent).not.toContain("No actionable findings");
    });

    it("requests the selected 7-day backend window and explains a genuinely empty result", async () => {
        const Reports = (await import("@/views/Reports.svelte")).default;
        const { getByText, findByText } = render(Reports);

        await waitFor(() => expect(resolvers.length).toBeGreaterThan(0));
        flushAll();

        activeBundle = {
            ...makeBundle(),
            days: 7,
            total_sessions: 0,
            daily_costs: Array.from({ length: 7 }, (_, index) => ({
                date: `2026-08-0${index + 1}`,
                cost: 0,
                sessions: 0,
                priced_sessions: 0,
                cost_basis: "unavailable",
                cost_sources: [],
            })),
            recommendations: [],
            inflection_points: [],
            model_routing: null,
            capabilities: {
                cache_health: false,
                model_routing: false,
                extra_usage: false,
            },
        };
        await fireEvent.click(getByText("7d"));
        await waitFor(() =>
            expect(getReportsBundle).toHaveBeenLastCalledWith(7),
        );
        flushAll();

        expect(
            await findByText("No sessions in this 7-day window."),
        ).toBeTruthy();
    });

    describe("cost timeline", () => {
        it("renders the timeline chart from the bundle series", async () => {
            const Reports = (await import("@/views/Reports.svelte")).default;
            const { container } = render(Reports);

            await waitFor(() => expect(resolvers.length).toBeGreaterThan(0));
            flushAll();

            await waitFor(() => {
                expect(
                    container.querySelector(".timeline-hero"),
                ).not.toBeNull();
            });
            // The area and line paths are the chart; both must be drawn.
            expect(container.querySelector("path.tl-line")).not.toBeNull();
            expect(container.querySelector("path.tl-area")).not.toBeNull();
        });

        it("summarises spend from the same series the chart plots", async () => {
            const Reports = (await import("@/views/Reports.svelte")).default;
            const { container, findByText } = render(Reports);

            await waitFor(() => expect(resolvers.length).toBeGreaterThan(0));
            flushAll();

            await waitFor(() => {
                expect(container.querySelector(".th-stats")).not.toBeNull();
            });
            // 7 active days at $6 plus one $42 peak = $84 total.
            expect(await findByText("$84.00")).toBeTruthy();
            // Peak day readout comes from the max of the series.
            expect(await findByText("$42.00")).toBeTruthy();
            // Average is over active days only (84 / 8 = 10.50), not all 10 days.
            expect(await findByText("$10.50")).toBeTruthy();
        });
    });
});
