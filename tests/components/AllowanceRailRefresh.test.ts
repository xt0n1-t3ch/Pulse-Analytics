import { describe, expect, it } from "bun:test";
import { vi } from "../bun-setup";
import { fireEvent, render, waitFor } from "@testing-library/svelte";

const refreshUsage = vi.fn(async () => undefined);

vi.mock("@/lib/api", async (importOriginal) => {
    const actual = await importOriginal<typeof import("@/lib/api")>();
    return { ...actual, refreshUsage: () => refreshUsage() };
});

describe("AllowanceRail refresh", () => {
    it("reads provider limits only when the user asks, once per click", async () => {
        const AllowanceRail = (
            await import("@/components/AllowanceRail.svelte")
        ).default;
        const { getByRole } = render(AllowanceRail);
        expect(refreshUsage).not.toHaveBeenCalled();

        const button = getByRole("button", { name: "Refresh provider limits" });
        await fireEvent.click(button);
        await fireEvent.click(button);

        await waitFor(() => expect(refreshUsage).toHaveBeenCalledTimes(1));
        expect(button.hasAttribute("disabled")).toBe(true);
    });
});
