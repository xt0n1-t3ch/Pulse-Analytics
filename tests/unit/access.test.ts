import { describe, expect, it } from "bun:test";
import {
    accessKindLabel,
    accessSourceName,
    analyticsProviderScopeForSelection,
    allowancePresentation,
    authenticatedAccessRoutes,
    displayableAccessRoutes,
    providerMatchesAnalyticsScope,
    windowLabel,
    type AccessRouteSnapshot,
} from "@/lib/access";

function route(
    overrides: Partial<AccessRouteSnapshot> = {},
): AccessRouteSnapshot {
    return {
        source: {
            id: "codex-subscription:default",
            kind: "codex_subscription",
            provider: "codex",
            auth_method: "app_server",
            proof: "quota_response",
            plan: "Pro 20x",
        },
        availability: "available",
        freshness: "fresh",
        provenance: "app_server",
        observed_at: "2026-08-01T14:00:00Z",
        fetched_at: "2026-08-01T14:00:04Z",
        expires_at: "2026-08-01T14:00:34Z",
        windows: [],
        credits: null,
        extra_usage: null,
        local_history: { available: false, sessions: 0 },
        error: null,
        ...overrides,
    };
}

describe("adaptive access presentation", () => {
    it("labels subscription and API routes as separate access products", () => {
        expect(accessKindLabel("codex_subscription")).toEqual({
            product: "Codex",
            access: "Subscription",
        });
        expect(accessKindLabel("open_ai_api")).toEqual({
            product: "OpenAI",
            access: "API",
        });
        expect(accessKindLabel("claude_subscription")).toEqual({
            product: "Claude",
            access: "Subscription",
        });
        expect(accessKindLabel("anthropic_api")).toEqual({
            product: "Anthropic",
            access: "API",
        });
    });

    it("does not repeat the provider when the plan already includes it", () => {
        expect(accessSourceName(route().source)).toBe("Codex Pro 20x");
        expect(
            accessSourceName({
                ...route().source,
                kind: "claude_subscription",
                provider: "claude",
                plan: "Claude Max 20x",
            }),
        ).toBe("Claude Max 20x");
    });

    it("keeps authenticated quota routes separate from local analytics routes", () => {
        const codex = route();
        const openAi = route({
            source: {
                ...codex.source,
                id: "openai-api:configured",
                kind: "open_ai_api",
                auth_method: "api_key",
                proof: "authenticated_probe",
                plan: null,
            },
            availability: "unavailable",
            windows: [],
        });

        const claudeHistory = route({
            source: {
                ...codex.source,
                id: "claude-subscription:default",
                kind: "claude_subscription",
                provider: "claude",
                auth_method: "oauth",
                proof: "none",
                plan: null,
            },
            availability: "unavailable",
            freshness: "unknown",
            local_history: { available: true, sessions: 300 },
            error: "token expired",
        });
        const unprovedOpenAi = route({
            source: {
                ...openAi.source,
                id: "openai-api:unproved",
                proof: "none",
            },
        });

        expect(
            authenticatedAccessRoutes([
                codex,
                openAi,
                claudeHistory,
                unprovedOpenAi,
            ]),
        ).toEqual([codex, openAi]);
        expect(
            displayableAccessRoutes([
                codex,
                openAi,
                claudeHistory,
                unprovedOpenAi,
            ]),
        ).toEqual([codex, openAi, claudeHistory]);
    });

    it("keeps provider-native window labels dynamic", () => {
        expect(
            windowLabel({
                key: "weekly",
                label: "Weekly",
                window_minutes: null,
            }),
        ).toBe("Weekly");
        expect(
            windowLabel({ key: "rolling", label: null, window_minutes: 17 }),
        ).toBe("17m");
        expect(
            windowLabel({ key: "fable", label: null, window_minutes: null }),
        ).toBe("Fable");
    });

    it("never turns unavailable or stale usage into a numeric percentage", () => {
        const window = {
            key: "weekly",
            label: "Weekly",
            window_minutes: null,
            used_percent: 63,
            remaining_percent: 37,
            resets_at: null,
        };

        expect(
            allowancePresentation(route({ windows: [window] }), window),
        ).toEqual({ percent: 37, direction: "remaining" });
        expect(
            allowancePresentation(
                route({
                    source: {
                        ...route().source,
                        id: "claude-subscription:default",
                        kind: "claude_subscription",
                        provider: "claude",
                        auth_method: "oauth",
                    },
                    windows: [window],
                }),
                window,
            ),
        ).toEqual({ percent: 63, direction: "used" });
        expect(
            allowancePresentation(
                route({ freshness: "stale", windows: [window] }),
                window,
            ),
        ).toBeNull();
        expect(
            allowancePresentation(
                route({ availability: "unavailable", windows: [window] }),
                window,
            ),
        ).toBeNull();
    });

    it("does not guess a missing provider-specific percentage", () => {
        const onlyUsed = {
            key: "weekly",
            label: "Weekly",
            window_minutes: 10_080,
            used_percent: 4,
        };
        expect(
            allowancePresentation(route({ windows: [onlyUsed] }), onlyUsed),
        ).toBeNull();

        const onlyRemaining = {
            key: "weekly",
            label: "Weekly",
            window_minutes: 10_080,
            remaining_percent: 96,
        };
        expect(
            allowancePresentation(
                route({
                    source: {
                        ...route().source,
                        id: "claude-subscription:default",
                        kind: "claude_subscription",
                        provider: "claude",
                        auth_method: "oauth",
                    },
                    windows: [onlyRemaining],
                }),
                onlyRemaining,
            ),
        ).toBeNull();
    });

    it("keeps source identity and analytics provider scope separate", () => {
        const codex = route();
        const claude = route({
            source: {
                ...route().source,
                id: "claude-subscription:work",
                kind: "claude_subscription",
                provider: "claude",
                auth_method: "oauth",
            },
        });
        const openAi = route({
            source: {
                ...route().source,
                id: "openai-api:team-a",
                kind: "open_ai_api",
                provider: "openai",
                auth_method: "api_key",
                plan: null,
            },
        });

        expect(
            analyticsProviderScopeForSelection("all", [codex, claude, openAi]),
        ).toBe("all");
        expect(
            analyticsProviderScopeForSelection("claude-subscription:work", [
                codex,
                claude,
                openAi,
            ]),
        ).toBe("claude");
        expect(
            analyticsProviderScopeForSelection("openai-api:team-a", [
                codex,
                claude,
                openAi,
            ]),
        ).toBe("openai");
        expect(
            analyticsProviderScopeForSelection("missing-source", [
                codex,
                claude,
                openAi,
            ]),
        ).toBeNull();
    });

    it("never aliases API lanes to subscription session providers", () => {
        expect(providerMatchesAnalyticsScope("codex", "codex")).toBe(true);
        expect(providerMatchesAnalyticsScope("claude", "claude")).toBe(true);
        expect(providerMatchesAnalyticsScope("codex", "openai")).toBe(false);
        expect(providerMatchesAnalyticsScope("claude", "anthropic")).toBe(
            false,
        );
        expect(providerMatchesAnalyticsScope("codex", "claude")).toBe(false);
        expect(providerMatchesAnalyticsScope("claude", "codex")).toBe(false);
        expect(providerMatchesAnalyticsScope("codex", "all")).toBe(true);
        expect(providerMatchesAnalyticsScope("claude", "all")).toBe(true);
    });
});
