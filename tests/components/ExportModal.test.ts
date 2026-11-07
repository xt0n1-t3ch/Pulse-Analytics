import { describe, expect, it } from "bun:test";
import { vi } from "../bun-setup";
import { fireEvent, render, waitFor } from "@testing-library/svelte";
import ExportModal from "@/components/ExportModal.svelte";

const props = {
    open: true,
    title: "Export sessions",
    defaultFilename: "sessions",
    columns: [{ key: "project", label: "Project", enabled: true }],
    rows: [{ project: "pulse" }],
    onclose: vi.fn(),
};

describe("ExportModal accessibility", () => {
    it("traps keyboard focus, handles Escape, and restores the opener", async () => {
        const opener = document.createElement("button");
        opener.textContent = "Open export";
        document.body.appendChild(opener);
        opener.focus();

        const { getByRole, rerender } = render(ExportModal, { props });
        const dialog = getByRole("dialog");
        const close = getByRole("button", { name: "Close export dialog" });
        const exportButton = getByRole("button", { name: "Export 1 rows" });
        await waitFor(() => expect(document.activeElement).toBe(close));

        await fireEvent.keyDown(dialog, { key: "Tab", shiftKey: true });
        expect(document.activeElement).toBe(exportButton);
        await fireEvent.keyDown(dialog, { key: "Tab" });
        expect(document.activeElement).toBe(close);

        await fireEvent.keyDown(dialog, { key: "Escape" });
        expect(props.onclose).toHaveBeenCalledTimes(1);
        await rerender({ ...props, open: false });
        await waitFor(() => expect(document.activeElement).toBe(opener));
        opener.remove();
    });
});
