import { afterEach, describe, expect, it } from "bun:test";
import { vi } from "../bun-setup";

vi.mock("@tauri-apps/api/core", () => ({
    invoke: vi.fn(),
}));

describe("browser development transport", () => {
    afterEach(() => {
        vi.unstubAllGlobals();
    });

    it("posts commands to the same-origin real backend proxy without fixture data", async () => {
        const globalRecord = globalThis as Record<string, unknown>;
        const windowRecord = window as unknown as Record<string, unknown>;
        const savedGlobalInternals = globalRecord.__TAURI_INTERNALS__;
        const savedWindowInternals = windowRecord.__TAURI_INTERNALS__;
        delete globalRecord.__TAURI_INTERNALS__;
        delete windowRecord.__TAURI_INTERNALS__;

        const fetchMock = vi.fn(
            async () =>
                new Response(
                    JSON.stringify({
                        version: "1.7.0",
                        uptime_seconds: 1,
                        discord_status: "Connected",
                        discord_enabled: true,
                    }),
                    {
                        status: 200,
                        headers: { "Content-Type": "application/json" },
                    },
                ),
        );
        vi.stubGlobal("fetch", fetchMock);

        const { getHealth } = await import("../../frontend/src/lib/api");
        await getHealth();

        expect(fetchMock).toHaveBeenCalledWith("/__pulse_api", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ command: "get_health", args: {} }),
        });

        globalRecord.__TAURI_INTERNALS__ = savedGlobalInternals;
        windowRecord.__TAURI_INTERNALS__ = savedWindowInternals;
    });

    it("shares an identical backend read while the first request is still in flight", async () => {
        const globalRecord = globalThis as Record<string, unknown>;
        const windowRecord = window as unknown as Record<string, unknown>;
        const savedGlobalInternals = globalRecord.__TAURI_INTERNALS__;
        const savedWindowInternals = windowRecord.__TAURI_INTERNALS__;
        delete globalRecord.__TAURI_INTERNALS__;
        delete windowRecord.__TAURI_INTERNALS__;

        let resolveFetch!: (response: Response) => void;
        const fetchMock = vi.fn(
            () =>
                new Promise<Response>((resolve) => {
                    resolveFetch = resolve;
                }),
        );
        vi.stubGlobal("fetch", fetchMock);

        const { getCostForecast } = await import("../../frontend/src/lib/api");
        const first = getCostForecast("all");
        const duplicate = getCostForecast("all");
        expect(fetchMock).toHaveBeenCalledTimes(1);

        resolveFetch(
            new Response(
                JSON.stringify({ refreshed_at: "2026-08-12T00:00:00Z" }),
                {
                    status: 200,
                    headers: { "Content-Type": "application/json" },
                },
            ),
        );
        expect((await Promise.all([first, duplicate])) as unknown).toEqual([
            { refreshed_at: "2026-08-12T00:00:00Z" },
            { refreshed_at: "2026-08-12T00:00:00Z" },
        ]);

        globalRecord.__TAURI_INTERNALS__ = savedGlobalInternals;
        windowRecord.__TAURI_INTERNALS__ = savedWindowInternals;
    });

    it("does not coalesce cost reads from different observed session fingerprints", async () => {
        const globalRecord = globalThis as Record<string, unknown>;
        const windowRecord = window as unknown as Record<string, unknown>;
        const savedGlobalInternals = globalRecord.__TAURI_INTERNALS__;
        const savedWindowInternals = windowRecord.__TAURI_INTERNALS__;
        delete globalRecord.__TAURI_INTERNALS__;
        delete windowRecord.__TAURI_INTERNALS__;

        const pending: Array<(response: Response) => void> = [];
        const fetchMock = vi.fn(
            () => new Promise<Response>((resolve) => pending.push(resolve)),
        );
        vi.stubGlobal("fetch", fetchMock);

        const { getCostTotals } = await import("../../frontend/src/lib/api");
        const first = getCostTotals(30, undefined, "all", "snapshot-a");
        const changed = getCostTotals(30, undefined, "all", "snapshot-b");
        expect(fetchMock).toHaveBeenCalledTimes(2);

        for (const resolve of pending) {
            resolve(
                new Response(JSON.stringify({ days: 30 }), {
                    status: 200,
                    headers: { "Content-Type": "application/json" },
                }),
            );
        }
        expect((await Promise.all([first, changed])) as unknown).toEqual([
            { days: 30 },
            { days: 30 },
        ]);

        globalRecord.__TAURI_INTERNALS__ = savedGlobalInternals;
        windowRecord.__TAURI_INTERNALS__ = savedWindowInternals;
    });
});
