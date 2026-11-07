import { describe, it, expect, beforeEach } from "bun:test";
import { vi } from "../bun-setup";
import { render } from "@testing-library/svelte";
import type { ReportsBundle } from "@/lib/api";

const minimalBundle: ReportsBundle = {
    provider: "claude",
    capabilities: {
        cache_health: true,
        model_routing: true,
        extra_usage: true,
    },
    days: 30,
    total_sessions: 1,
    priced_sessions: 1,
    cost_basis: "exact",
    cost_sources: ["anthropic_api_equivalent"],
    daily_costs: [],
    recommendations: [
        {
            id: "rec-x",
            severity: "info",
            title: "Flow recommendation",
            description: "Generated in flow test.",
            estimated_savings: null,
            action: "Do the thing",
            fix_prompt: "Apply the flow recommendation.",
            color: "#7cb9e8",
        },
    ],
    trace_overview: {
        provider: "claude",
        provider_display: "Claude Code",
        instruction_file: "CLAUDE.md",
        fix_button_label: "Fix with Claude",
        session_store: "",
        global_state_source: "",
        traced_sessions: 1,
        total_sessions: 1,
        user_messages: 1,
        assistant_messages: 1,
        total_tool_calls: 1,
        total_compactions: 0,
        mcp_tool_calls: 0,
        cache_hit_ratio: 0,
        top_tools: [],
        telemetry_mermaid: "",
        cache_mermaid: "",
    },
    tool_frequency: {
        available: false,
        sessions_analyzed: 1,
        traced_sessions: 1,
        total_tool_calls: 0,
        avg_tools_per_session: 0,
        avg_tool_calls_per_hour: 0,
        mcp_tool_calls: 0,
        mcp_share_pct: 0,
        compact_gap_sessions: 0,
        diagnosis: "",
        top_tools: [],
    },
    prompt_complexity: {
        available: false,
        sessions_analyzed: 1,
        prompts_analyzed: 0,
        avg_complexity_score: 0,
        avg_specificity_score: 0,
        high_complexity_sessions: 0,
        low_specificity_sessions: 0,
        diagnosis: "",
        top_sessions: [],
    },
    session_health: {
        available: false,
        sessions_analyzed: 1,
        health_score: 0,
        grade: "A",
        avg_duration_minutes: 0,
        p90_duration_minutes: 0,
        long_session_pct: 0,
        avg_messages_per_session: 0,
        peak_overlap_pct: 0,
        compact_gap_pct: 0,
        diagnosis: "",
    },
    cache_health: {
        grade: "A",
        grade_label: "Excellent",
        color: "#62b462",
        hit_ratio: 0,
        trend_weighted_ratio: 0,
        total_cache_read: 0,
        total_cache_write: 0,
        total_input: 0,
        sessions_analyzed: 1,
        diagnosis: "Flow cache diagnosis.",
    },
    model_routing: {
        total_sessions: 1,
        priced_sessions: 0,
        total_cost: 0,
        cost_basis: "unavailable",
        cost_sources: [],
        opus: {
            sessions: 0,
            priced_sessions: 0,
            cost: 0,
            cost_share_pct: 0,
            avg_cost_per_session: 0,
        },
        sonnet: {
            sessions: 0,
            priced_sessions: 0,
            cost: 0,
            cost_share_pct: 0,
            avg_cost_per_session: 0,
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
        savings_estimate_available: false,
        estimated_savings_if_rerouted: 0,
        diagnosis: "",
    },
    inflection_points: [],
};

const getReportsBundle = vi.fn(async () => minimalBundle);

vi.mock("@/lib/api", async (importOriginal) => {
    const actual = await importOriginal<typeof import("@/lib/api")>();
    return {
        ...actual,
        getReportsBundle: () => getReportsBundle(),
    };
});

describe("Phase 5 flow", () => {
    beforeEach(() => {
        getReportsBundle.mockClear();
    });

    it("renders the reports bundle through a single bundle call", async () => {
        const Reports = (await import("@/views/Reports.svelte")).default;
        const { findByText } = render(Reports);

        expect(await findByText("Flow recommendation")).toBeTruthy();
        expect(getReportsBundle).toHaveBeenCalledTimes(1);
    });
});
