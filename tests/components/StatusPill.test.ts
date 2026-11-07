import { describe, it, expect } from "bun:test";
import { render } from "@testing-library/svelte";
import StatusPill from "@/components/StatusPill.svelte";

describe("StatusPill", () => {
    it("renders the label and exposes the semantic state", () => {
        const { container, getByText } = render(StatusPill, {
            props: { state: "live", label: "Live" },
        });
        expect(getByText("Live")).toBeTruthy();
        expect(
            container.querySelector(".status-pill")?.getAttribute("data-state"),
        ).toBe("live");
    });

    it("maps each state to its own data-state so color is not reinvented per view", () => {
        for (const state of [
            "live",
            "stale",
            "waiting",
            "expired",
            "paused",
            "neutral",
        ] as const) {
            const { container, unmount } = render(StatusPill, {
                props: { state, label: state },
            });
            expect(
                container
                    .querySelector(".status-pill")
                    ?.getAttribute("data-state"),
            ).toBe(state);
            unmount();
        }
    });

    it("only animates the beacon for a live pulse", () => {
        const { container: live, unmount } = render(StatusPill, {
            props: { state: "live", label: "Live", pulse: true },
        });
        expect(live.querySelector(".sp-beacon.pulse")).not.toBeNull();
        unmount();

        const { container: waiting } = render(StatusPill, {
            props: { state: "waiting", label: "Waiting", pulse: true },
        });
        expect(waiting.querySelector(".sp-beacon.pulse")).toBeNull();
    });
});
