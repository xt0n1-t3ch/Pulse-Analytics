import { describe, it, expect } from "bun:test";
import { vi } from "../bun-setup";
import { render, fireEvent } from "@testing-library/svelte";
import SegmentedControl from "@/components/SegmentedControl.svelte";

const options = [
    { value: "7", label: "7d" },
    { value: "30", label: "30d" },
    { value: "90", label: "90d" },
];

describe("SegmentedControl", () => {
    it("renders one button per option and marks the active one", () => {
        const { getByRole } = render(SegmentedControl, {
            props: {
                options,
                value: "30",
                onchange: () => {},
                ariaLabel: "Window",
            },
        });
        const active = getByRole("button", { name: "30d" });
        expect(active.getAttribute("aria-pressed")).toBe("true");
        expect(
            getByRole("button", { name: "7d" }).getAttribute("aria-pressed"),
        ).toBe("false");
    });

    it("emits the option value on click", async () => {
        const onchange = vi.fn();
        const { getByRole } = render(SegmentedControl, {
            props: { options, value: "30", onchange, ariaLabel: "Window" },
        });
        await fireEvent.click(getByRole("button", { name: "90d" }));
        expect(onchange).toHaveBeenCalledWith("90");
    });

    it("uses tab semantics when role is tablist", () => {
        const { getByRole } = render(SegmentedControl, {
            props: {
                options,
                value: "7",
                onchange: () => {},
                ariaLabel: "Preset",
                role: "tablist",
            },
        });
        const tab = getByRole("tab", { name: "7d" });
        expect(tab.getAttribute("aria-selected")).toBe("true");
    });

    it("disables every option when disabled", () => {
        const { getAllByRole } = render(SegmentedControl, {
            props: {
                options,
                value: "7",
                onchange: () => {},
                ariaLabel: "Window",
                disabled: true,
            },
        });
        for (const button of getAllByRole("button")) {
            expect((button as HTMLButtonElement).disabled).toBe(true);
        }
    });
});
