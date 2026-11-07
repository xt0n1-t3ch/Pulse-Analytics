import { describe, it, expect } from "bun:test";
import { render } from "@testing-library/svelte";
import BudgetCockpit from "@/components/BudgetCockpit.svelte";
import type { BudgetStatus, CostForecast } from "@/lib/api";

function forecast(overrides: Partial<CostForecast> = {}): CostForecast {
    return {
        billed_spend_usd: 7371.35,
        daily_billed_spend_usd: 307.14,
        projected_billed_spend_usd: 9521.32,
        api_equivalent_usd: 8100,
        daily_api_equivalent_usd: 337.5,
        projected_api_equivalent_usd: 10462.5,
        days_elapsed: 24,
        days_in_month: 31,
        cost_basis: "exact",
        cost_sources: ["provider_billed", "api_equivalent"],
        sessions: 4,
        priced_sessions: 4,
        billed_sessions: 2,
        api_equivalent_sessions: 2,
        refreshed_at: "2026-08-12T12:00:00Z",
        ...overrides,
    };
}

function budget(monthly: number): BudgetStatus {
    return {
        monthly_budget: monthly,
        alert_threshold_pct: 80,
        billed_spend_usd: 7371.35,
        projected_billed_spend_usd: 9521.32,
        api_equivalent_usd: 8100,
        projected_api_equivalent_usd: 10462.5,
        pct_used: (7371.35 / monthly) * 100,
        over_budget: 9521.32 > monthly,
        cost_basis: "exact",
        cost_sources: ["provider_billed", "api_equivalent"],
        sessions: 4,
        priced_sessions: 4,
        billed_sessions: 2,
        api_equivalent_sessions: 2,
        refreshed_at: "2026-08-12T12:00:00Z",
    };
}

const noop = (): void => undefined;

describe("BudgetCockpit.svelte", () => {
    it("leads with month-to-date spend as the headline figure", () => {
        const { container } = render(BudgetCockpit, {
            props: { forecast: forecast(), budget: null, onSetBudget: noop },
        });

        expect(container.querySelector(".ck-figure")?.textContent).toContain(
            "7371.35",
        );
    });

    it("offers to set a cap when none exists, and hides the budget tick", () => {
        const { container, getByText } = render(BudgetCockpit, {
            props: { forecast: forecast(), budget: null, onSetBudget: noop },
        });

        expect(getByText("Set a cap")).toBeTruthy();
        expect(container.querySelector(".ck-cap")).toBeNull();
    });

    it("reads healthy when the projection lands under the cap", () => {
        const { container, getByText } = render(BudgetCockpit, {
            props: {
                forecast: forecast(),
                budget: budget(12000),
                onSetBudget: noop,
            },
        });

        expect(container.querySelector(".cockpit.warn")).toBeNull();
        expect(container.querySelector(".cockpit.over")).toBeNull();
        expect(getByText(/under the .* cap/)).toBeTruthy();
    });

    /** Spend still inside the cap but heading past it is the case the whole
     *  screen exists to surface, so it must be visually distinct. */
    it("warns when spend is under the cap but projected to overshoot", () => {
        const { container, getByText } = render(BudgetCockpit, {
            props: {
                forecast: forecast(),
                budget: budget(8000),
                onSetBudget: noop,
            },
        });

        expect(container.querySelector(".cockpit.warn")).not.toBeNull();
        expect(container.querySelector(".cockpit.over")).toBeNull();
        expect(getByText(/On course to overshoot/)).toBeTruthy();
    });

    it("escalates to over-budget once spend already exceeds the cap", () => {
        const { container, getByText } = render(BudgetCockpit, {
            props: {
                forecast: forecast(),
                budget: budget(5000),
                onSetBudget: noop,
            },
        });

        expect(container.querySelector(".cockpit.over")).not.toBeNull();
        expect(getByText(/Already .* over the/)).toBeTruthy();
    });

    /** The configured threshold is a heads-up in its own right. Spending $85 of
     *  a $100 cap at an 80% threshold has to warn even when the projection still
     *  lands under the cap. */
    it("warns once spend crosses the configured alert threshold", () => {
        const { container, getByText } = render(BudgetCockpit, {
            props: {
                forecast: forecast({
                    billed_spend_usd: 85,
                    projected_billed_spend_usd: 95,
                    daily_billed_spend_usd: 3.5,
                }),
                budget: {
                    ...budget(100),
                    billed_spend_usd: 85,
                    pct_used: 85,
                    projected_billed_spend_usd: 95,
                    over_budget: false,
                },
                onSetBudget: noop,
            },
        });

        expect(container.querySelector(".cockpit.warn")).not.toBeNull();
        expect(container.querySelector(".cockpit.over")).toBeNull();
        expect(getByText(/past your 80% alert threshold/)).toBeTruthy();
    });

    /** A zero threshold means the user disabled it, so only the projection
     *  should be able to raise a warning. */
    it("stays healthy under the cap when no alert threshold is configured", () => {
        const { container } = render(BudgetCockpit, {
            props: {
                forecast: forecast({
                    billed_spend_usd: 85,
                    projected_billed_spend_usd: 95,
                    daily_billed_spend_usd: 3.5,
                }),
                budget: {
                    ...budget(100),
                    alert_threshold_pct: 0,
                    billed_spend_usd: 85,
                    pct_used: 85,
                    projected_billed_spend_usd: 95,
                    over_budget: false,
                },
                onSetBudget: noop,
            },
        });

        expect(container.querySelector(".cockpit.warn")).toBeNull();
    });

    /** The gauge is only readable if spend, projection and cap share a scale
     *  and stay in order along the track. */
    it("places spend, projection and cap in ascending order on one track", () => {
        const { container } = render(BudgetCockpit, {
            props: {
                forecast: forecast(),
                budget: budget(12000),
                onSetBudget: noop,
            },
        });

        const width = (sel: string) =>
            parseFloat(
                (container.querySelector(sel) as HTMLElement).style.width,
            );
        const spent = width(".ck-spent");
        const projected = width(".ck-projected");
        const capLeft = parseFloat(
            (container.querySelector(".ck-cap") as HTMLElement).style.left,
        );

        expect(spent).toBeLessThan(projected);
        expect(projected).toBeLessThan(capLeft);
        expect(capLeft).toBeLessThanOrEqual(100);
    });

    it("states plainly when nothing has been spent this month, even after the first day", () => {
        const { getByText } = render(BudgetCockpit, {
            props: {
                forecast: forecast({
                    billed_spend_usd: 0,
                    days_elapsed: 3,
                    projected_billed_spend_usd: 0,
                    daily_billed_spend_usd: 0,
                }),
                budget: null,
                onSetBudget: noop,
            },
        });

        expect(getByText("No spend recorded this month yet.")).toBeTruthy();
    });

    it("does not render unknown spend and projection as zero dollars", () => {
        const { container, getByText } = render(BudgetCockpit, {
            props: {
                forecast: forecast({
                    billed_spend_usd: null,
                    projected_billed_spend_usd: null,
                    daily_billed_spend_usd: null,
                    api_equivalent_usd: null,
                    projected_api_equivalent_usd: null,
                    daily_api_equivalent_usd: null,
                    cost_basis: "unavailable",
                    cost_sources: [],
                    sessions: 4,
                    priced_sessions: 0,
                    billed_sessions: 0,
                    api_equivalent_sessions: 0,
                }),
                budget: null,
                onSetBudget: noop,
            },
        });

        expect(container.querySelector(".ck-figure")?.textContent?.trim()).toBe(
            "—",
        );
        expect(getByText("Provider billed · unavailable")).toBeTruthy();
        expect(
            getByText("Provider-billed spend is unavailable for this month."),
        ).toBeTruthy();
        expect(container.querySelector(".ck-track")).toBeNull();
    });

    it("shows the real subscription price when provider billing is unavailable", () => {
        const { container, getByText } = render(BudgetCockpit, {
            props: {
                forecast: forecast({
                    billed_spend_usd: null,
                    projected_billed_spend_usd: null,
                    daily_billed_spend_usd: null,
                    billed_sessions: 0,
                    api_equivalent_usd: 84.25,
                    projected_api_equivalent_usd: 112.5,
                    api_equivalent_sessions: 4,
                    cost_basis: "estimated",
                    cost_sources: ["api_equivalent"],
                }),
                budget: null,
                subscriptionPrice: 200,
                subscriptionLabel: "Pro 20x",
                onSetBudget: noop,
            },
        });

        expect(container.querySelector(".ck-figure")?.textContent?.trim()).toBe(
            "$200.00",
        );
        expect(
            getByText("Pro 20x subscription").getAttribute("data-state"),
        ).toBe("available");
        expect(getByText("API-equivalent projection")).toBeTruthy();
        expect(getByText("$112.50")).toBeTruthy();
        expect(
            getByText(
                "Your Pro 20x subscription covers this month — measured usage is worth $84.25 at published API rates.",
            ),
        ).toBeTruthy();
        expect(container.querySelector(".ck-equivalent")).toBeNull();
    });

    it("keeps the honest unavailable state when no subscription price is known", () => {
        const { container, getByText } = render(BudgetCockpit, {
            props: {
                forecast: forecast({
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
                }),
                budget: null,
                subscriptionPrice: null,
                subscriptionLabel: "",
                onSetBudget: noop,
            },
        });

        expect(container.querySelector(".ck-figure")?.textContent?.trim()).toBe(
            "—",
        );
        expect(getByText("Provider billed · unavailable")).toBeTruthy();
        expect(
            getByText("Provider-billed spend is unavailable for this month."),
        ).toBeTruthy();
    });

    it("shows known lower-bound spend when forecast coverage is partial", () => {
        const { container, getByText } = render(BudgetCockpit, {
            props: {
                forecast: forecast({
                    billed_spend_usd: 85,
                    projected_billed_spend_usd: 95,
                    daily_billed_spend_usd: 3.5,
                    cost_basis: "partial",
                    sessions: 4,
                    priced_sessions: 3,
                }),
                budget: null,
                onSetBudget: noop,
            },
        });

        expect(container.querySelector(".ck-figure")?.textContent).toContain(
            "$85",
        );
        expect(getByText(/Known provider-billed lower bound:/)).toBeTruthy();
        expect(container.querySelector(".ck-track")).not.toBeNull();
    });

    it("never applies API-equivalent value to a billing budget", () => {
        const { container, getByText } = render(BudgetCockpit, {
            props: {
                forecast: forecast({
                    billed_spend_usd: null,
                    projected_billed_spend_usd: null,
                    daily_billed_spend_usd: null,
                    billed_sessions: 0,
                    api_equivalent_usd: 9000,
                    projected_api_equivalent_usd: 12000,
                    api_equivalent_sessions: 4,
                    cost_basis: "estimated",
                    cost_sources: ["api_equivalent"],
                }),
                budget: budget(100),
                onSetBudget: noop,
            },
        });

        expect(container.querySelector(".ck-track")).toBeNull();
        expect(container.querySelector(".cockpit.warn")).toBeNull();
        expect(container.querySelector(".cockpit.over")).toBeNull();
        expect(
            getByText(
                "Usage is priced at published API rates; none of it is provider-billed this month.",
            ),
        ).toBeTruthy();
    });

    it("uses no hardcoded colours", async () => {
        const { readFileSync } = await import("node:fs");
        const { resolve } = await import("node:path");
        const source = readFileSync(
            resolve(
                import.meta.dir,
                "../../frontend/src/components/BudgetCockpit.svelte",
            ),
            "utf8",
        );
        const css = source.match(/<style>([\s\S]*)<\/style>/)?.[1] ?? "";
        expect(css.length).toBeGreaterThan(0);
        expect(css.match(/#[0-9a-fA-F]{3,8}\b/g)).toBeNull();
        expect(css.match(/rgba?\(/g)).toBeNull();
    });
});
