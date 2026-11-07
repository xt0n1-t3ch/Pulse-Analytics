import { describe, expect, it } from "bun:test";
import { render } from "@testing-library/svelte";

describe("Heatmap.svelte", () => {
    it("keeps the 24-hour strip and adds volume, coverage, and peak-hour context", async () => {
        const Heatmap = (await import("@/components/Heatmap.svelte")).default;
        const { container, getByText } = render(Heatmap, {
            props: {
                data: [
                    { hour: 9, session_count: 3, total_cost: 5 },
                    { hour: 14, session_count: 2, total_cost: 4 },
                ],
            },
        });

        expect(container.querySelectorAll(".heatmap-cell")).toHaveLength(24);
        expect(container.querySelectorAll(".heatmap-label")).toHaveLength(24);
        expect(container.querySelector(".heatmap-label")?.textContent).toBe(
            "12 AM",
        );
        expect(
            [...container.querySelectorAll(".heatmap-label")].at(-1)
                ?.textContent,
        ).toBe("11 PM");
        expect(getByText("5 sessions")).toBeTruthy();
        expect(getByText("2 active hours")).toBeTruthy();
        expect(getByText("Peak 9 AM")).toBeTruthy();
        expect(
            container.querySelector('.heatmap-cell[title^="9 AM:"]'),
        ).toBeTruthy();
        expect(
            container
                .querySelector(".heatmap-grid")
                ?.getAttribute("aria-label"),
        ).toContain("5 sessions");
    });
});
