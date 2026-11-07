import { beforeEach, describe, expect, it } from "bun:test";
import { render } from "@testing-library/svelte";
import { tick } from "svelte";
import AllowanceRail from "@/components/AllowanceRail.svelte";
import type { AccessRouteSnapshot } from "@/lib/access";

function claudeExpired(): AccessRouteSnapshot {
    return {
        source: {
            id: "claude-subscription:default",
            kind: "claude_subscription",
            provider: "claude",
            auth_method: "oauth",
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
        local_history: { available: true, sessions: 300 },
        error: "token expired",
        unavailable_reason: "expired",
    };
}

describe("AllowanceRail", () => {
    beforeEach(async () => {
        const { accessSnapshot, selectedAccessSourceId } =
            await import("@/lib/stores");
        accessSnapshot.set(null);
        selectedAccessSourceId.set("all");
    });

    it("explains an expired Claude session instead of a generic waiting state", async () => {
        const { accessSnapshot, selectedAccessSourceId } =
            await import("@/lib/stores");
        accessSnapshot.set({ routes: [claudeExpired()] });
        selectedAccessSourceId.set("claude-subscription:default");

        const { container, getByText } = render(AllowanceRail);
        await tick();

        expect(getByText("Session expired")).toBeTruthy();
        // The historical analytics stay available and are named.
        expect(container.textContent).toContain("300");
        expect(container.textContent).toContain("Claude Code");
        // It must not fall back to the generic proof-waiting empty state.
        expect(container.querySelector(".allowance-empty")).toBeNull();
    });

    it("keeps the generic empty state when a selected source has no history either", async () => {
        const { accessSnapshot, selectedAccessSourceId } =
            await import("@/lib/stores");
        const noHistory = claudeExpired();
        noHistory.local_history = { available: false, sessions: 0 };
        accessSnapshot.set({ routes: [noHistory] });
        selectedAccessSourceId.set("claude-subscription:default");

        const { container } = render(AllowanceRail);
        await tick();

        expect(container.querySelector(".allowance-local")).toBeNull();
        expect(container.querySelector(".allowance-empty")).not.toBeNull();
    });

    it("renders authenticated individual spend limits as spend data, not quota windows", async () => {
        const { accessSnapshot } = await import("@/lib/stores");
        const route: AccessRouteSnapshot = {
            source: {
                id: "codex-subscription:default",
                kind: "codex_subscription",
                provider: "codex",
                auth_method: "app_server",
                proof: "quota_response",
                plan: "plus",
            },
            availability: "available",
            freshness: "fresh",
            provenance: "app_server",
            observed_at: "2026-08-04T09:00:00Z",
            fetched_at: "2026-08-04T09:00:00Z",
            expires_at: "2026-08-04T09:00:30Z",
            windows: [],
            credits: null,
            individualSpendLimits: [
                {
                    limitId: "workspace",
                    limit: "100.00",
                    used: "25.00",
                    remainingPercent: 75,
                    resetsAt: "2026-08-05T09:00:00Z",
                },
            ],
            extra_usage: null,
            local_history: { available: true, sessions: 1 },
            error: null,
        };
        accessSnapshot.set({ routes: [route] });

        const { container, getByText } = render(AllowanceRail);
        await tick();

        expect(getByText("25.00 of 100.00")).toBeTruthy();
        expect(container.querySelector(".window-row")).toBeNull();
    });

    it("distinguishes Codex model quota windows by canonical duration", async () => {
        const { accessSnapshot } = await import("@/lib/stores");
        const route: AccessRouteSnapshot = {
            source: {
                id: "codex-subscription:default",
                kind: "codex_subscription",
                provider: "codex",
                auth_method: "app_server",
                proof: "quota_response",
                plan: "pro_20x",
            },
            availability: "available",
            freshness: "fresh",
            provenance: "app_server",
            observed_at: "2026-08-27T09:00:00Z",
            fetched_at: "2026-08-27T09:00:00Z",
            expires_at: "2026-08-27T09:00:30Z",
            windows: [
                {
                    key: "model_five_hour",
                    label: "GPT-5.3-Codex-Spark",
                    window_minutes: 300,
                    used_percent: 0,
                    remaining_percent: 100,
                    resets_at: "2026-08-27T14:00:00Z",
                },
                {
                    key: "model_weekly",
                    label: "GPT-5.3-Codex-Spark",
                    window_minutes: 10_080,
                    used_percent: 0,
                    remaining_percent: 100,
                    resets_at: "2026-09-03T13:31:00Z",
                },
            ],
            credits: null,
            extra_usage: null,
            local_history: { available: true, sessions: 1 },
            error: null,
        };
        accessSnapshot.set({ routes: [route] });

        const { container, getByText, getAllByText } = render(AllowanceRail);
        await tick();

        expect(getByText("5-hour limit")).toBeTruthy();
        expect(getByText("Weekly limit")).toBeTruthy();
        expect(getAllByText("GPT-5.3-Codex-Spark")).toHaveLength(2);
        expect(container.textContent).not.toContain("1w");
        expect(
            container.querySelector('[title="Weekly usage limit"]'),
        ).not.toBeNull();
    });

    it("renders extra usage alongside authenticated quota windows", async () => {
        const { accessSnapshot } = await import("@/lib/stores");
        const route: AccessRouteSnapshot = {
            ...claudeExpired(),
            source: {
                ...claudeExpired().source,
                proof: "quota_response",
            },
            availability: "available",
            freshness: "fresh",
            windows: [
                {
                    key: "five_hour",
                    label: "5-hour",
                    window_minutes: 300,
                    used_percent: 25,
                    remaining_percent: 75,
                    resets_at: null,
                },
            ],
            extra_usage: {
                enabled: true,
                limit: 100,
                used: 12.5,
                utilization: 12.5,
            },
        };
        accessSnapshot.set({ routes: [route] });

        const { container, getByText } = render(AllowanceRail);
        await tick();

        expect(container.querySelector(".window-row")).not.toBeNull();
        expect(getByText("Month-to-date usage")).toBeTruthy();
        expect(getByText("$12.50")).toBeTruthy();
    });
});
