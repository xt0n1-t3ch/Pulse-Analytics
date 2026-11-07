import { describe, it, expect } from "bun:test";
import { render } from "@testing-library/svelte";
import CostTimeline from "@/components/CostTimeline.svelte";
import type { DailyCostPoint, InflectionPoint } from "@/lib/api";

function series(costs: number[]): DailyCostPoint[] {
    return costs.map((cost, i) => ({
        date: `2026-07-${String(i + 1).padStart(2, "0")}`,
        cost,
        sessions: cost > 0 ? 1 : 0,
        priced_sessions: cost > 0 ? 1 : 0,
        cost_basis: "exact",
        cost_sources: ["anthropic_api_equivalent"],
    }));
}

function inflection(date: string, multiplier = 2.5): InflectionPoint {
    return {
        date,
        multiplier,
        direction: "spike",
        sessions_on_day: 2,
        observed_sessions_on_day: 2,
        cost_basis: "exact",
        cost_sources: ["anthropic_api_equivalent"],
        cost_on_day: 40,
        baseline_cost: 16,
        note: "Cost per session spiked.",
    };
}

describe("CostTimeline.svelte", () => {
    it("draws the area and line for a populated series", () => {
        const { container } = render(CostTimeline, {
            props: { points: series([4, 9, 2, 12]), inflections: [] },
        });

        const line = container.querySelector("path.tl-line");
        expect(line).not.toBeNull();
        expect(line?.getAttribute("d")).toContain("M");
        // The area path closes back to the baseline so the gradient can fill it.
        expect(
            container.querySelector("path.tl-area")?.getAttribute("d"),
        ).toContain("Z");
    });

    it("shows an empty state instead of an axis when there is no series", () => {
        const { container, getByText } = render(CostTimeline, {
            props: { points: [], inflections: [] },
        });

        expect(
            getByText("No monetary value recorded in this window."),
        ).toBeTruthy();
        expect(container.querySelector("svg.tl-svg")).toBeNull();
    });

    it("places one marker per inflection day that exists in the window", () => {
        const points = series([4, 9, 2, 12]);
        const { container } = render(CostTimeline, {
            props: {
                points,
                inflections: [
                    inflection(points[1].date),
                    inflection(points[3].date),
                ],
            },
        });

        expect(container.querySelectorAll("rect.tl-marker").length).toBe(2);
    });

    /** A marker on a day outside the plotted range would sit at a coordinate
     *  that means nothing, so those inflections are dropped, not clamped. */
    it("drops inflections whose date is outside the plotted window", () => {
        const { container } = render(CostTimeline, {
            props: {
                points: series([4, 9, 2, 12]),
                inflections: [inflection("2025-01-01")],
            },
        });

        expect(container.querySelectorAll("rect.tl-marker").length).toBe(0);
    });

    it("calls out the most significant inflection under the chart", () => {
        const points = series([4, 9, 2, 12]);
        const { getByText } = render(CostTimeline, {
            props: {
                points,
                inflections: [
                    inflection(points[1].date, 2.1),
                    inflection(points[3].date, 6.8),
                ],
            },
        });

        // The 6.8x day outranks the 2.1x day.
        expect(getByText(/6\.8× the rolling baseline/)).toBeTruthy();
        expect(getByText(new RegExp(points[3].date))).toBeTruthy();
    });

    /** The detector's multiplier is already today/baseline, so a 0.2 day is a
     *  fifth of the baseline: an 80% drop. Reporting its reciprocal would claim
     *  a "5x fall", which is not what the number means. */
    it("states a drop as a percentage decrease and the real ratio", () => {
        const points = series([20, 18, 4]);
        const { getByText, queryByText } = render(CostTimeline, {
            props: { points, inflections: [inflection(points[2].date, 0.2)] },
        });

        expect(getByText(/fell 80% below the rolling baseline/)).toBeTruthy();
        expect(getByText(/0\.20× baseline/)).toBeTruthy();
        expect(queryByText(/5\.0× below/)).toBeNull();
        expect(queryByText(/ran 0\.2×/)).toBeNull();
    });

    /** Ranking on the raw multiplier would always pick the spike and bury a
     *  larger drop, so significance is distance from the baseline either way. */
    it("ranks a large drop above a smaller spike", () => {
        const points = series([20, 18, 4, 30]);
        const { getByText } = render(CostTimeline, {
            props: {
                points,
                inflections: [
                    inflection(points[3].date, 1.6),
                    inflection(points[2].date, 0.1),
                ],
            },
        });

        // 0.1x is a 10x fall, which outranks the 1.6x spike.
        expect(getByText(/fell 90% below the rolling baseline/)).toBeTruthy();
    });

    it("states plainly when monetary value held to its baseline", () => {
        const { getByText } = render(CostTimeline, {
            props: { points: series([4, 5, 4]), inflections: [] },
        });

        expect(
            getByText(
                "No monetary-value inflections — the observed value held to its baseline.",
            ),
        ).toBeTruthy();
    });

    /** Axis labels that overprint each other at the right edge are unreadable,
     *  so the final tick is dropped when it crowds its neighbour. */
    it("never renders two axis labels on top of each other", () => {
        const { container } = render(CostTimeline, {
            props: {
                points: series(Array.from({ length: 30 }, (_, i) => i)),
                inflections: [],
            },
        });

        const xs = Array.from(container.querySelectorAll("text.tl-axis"))
            .map((t) => Number(t.getAttribute("x")))
            // Axis value labels share the class; keep only the date row.
            .filter((v, _, all) => all.filter((o) => o === v).length === 1)
            .sort((a, b) => a - b);

        for (let i = 1; i < xs.length; i++) {
            expect(xs[i] - xs[i - 1]).toBeGreaterThan(30);
        }
    });

    /** Every colour must come from a theme token so the chart follows the
     *  light/dark switch like the rest of the app. */
    it("uses no hardcoded colours", async () => {
        const { readFileSync } = await import("node:fs");
        const { resolve } = await import("node:path");
        const source = readFileSync(
            resolve(
                import.meta.dir,
                "../../frontend/src/components/CostTimeline.svelte",
            ),
            "utf8",
        );
        const css = source.match(/<style>([\s\S]*)<\/style>/)?.[1] ?? "";
        expect(css.length).toBeGreaterThan(0);
        expect(css.match(/#[0-9a-fA-F]{3,8}\b/g)).toBeNull();
        expect(css.match(/rgba?\(/g)).toBeNull();
    });
});
