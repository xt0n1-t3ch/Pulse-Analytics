let moduleId = 0;
import { get } from "svelte/store";
import { beforeEach, describe, expect, it } from "bun:test";
import { vi } from "../bun-setup";

interface Deferred<T> {
    promise: Promise<T>;
    resolve: (value: T) => void;
    reject: (reason?: unknown) => void;
}

function deferred<T>(): Deferred<T> {
    let resolve!: (value: T) => void;
    let reject!: (reason?: unknown) => void;
    const promise = new Promise<T>((resolvePromise, rejectPromise) => {
        resolve = resolvePromise;
        reject = rejectPromise;
    });
    return { promise, resolve, reject };
}

const api = vi.hoisted(() => ({
    getActiveProvider: vi.fn(),
    getProviderCopy: vi.fn(),
    setActiveProvider: vi.fn(),
}));

vi.mock("@/lib/api", () => api);

describe("provider selection ordering", () => {
    beforeEach(() => {
        localStorage.clear();
        document.documentElement.removeAttribute("data-provider");
        api.getActiveProvider.mockReset();
        api.getProviderCopy.mockReset();
        api.setActiveProvider.mockReset();
    });

    it("shows Orion's current session index rather than its retired conversation database", async () => {
        const { PROVIDERS } = await import("@/lib/provider");
        expect(PROVIDERS.orion.sessionsPath).toBe(
            "~/.orion/projects/index.sqlite",
        );
    });

    it("publishes only persisted provider selections while persistence is serialized", async () => {
        const bootstrap = deferred<{ active_provider: string }>();
        const firstPersist = deferred<void>();
        const secondPersist = deferred<void>();
        api.getActiveProvider.mockReturnValue(bootstrap.promise);
        api.getProviderCopy.mockResolvedValue({
            provider: "claude",
            provider_label: "Claude Code",
            instruction_file: "CLAUDE.md",
            home_dir: "~/.claude",
            sessions_store: "~/.claude/projects",
            fix_label: "Fix with Claude Code",
            global_state_source: "~/.claude",
        });
        api.setActiveProvider
            .mockReturnValueOnce(firstPersist.promise)
            .mockReturnValueOnce(secondPersist.promise);

        const providerModule = (await import(
            `../../frontend/src/lib/provider.ts?test=${++moduleId}`
        )) as typeof import("@/lib/provider");
        const first = providerModule.setProvider("codex");
        expect(get(providerModule.provider)).toBe("claude");

        const second = providerModule.setProvider("claude");
        expect(get(providerModule.provider)).toBe("claude");
        await Promise.resolve();
        expect(api.setActiveProvider).toHaveBeenCalledTimes(1);
        expect(api.setActiveProvider).toHaveBeenNthCalledWith(1, "codex");

        firstPersist.resolve();
        await first;
        await Promise.resolve();
        expect(get(providerModule.provider)).toBe("claude");
        expect(api.setActiveProvider).toHaveBeenNthCalledWith(2, "claude");

        secondPersist.resolve();
        await second;
        expect(get(providerModule.provider)).toBe("claude");
        expect(get(providerModule.providerRevision)).toBe(1);
    });

    it("ignores a stale bootstrap provider after a user mutation starts", async () => {
        const bootstrap = deferred<{ active_provider: string }>();
        api.getActiveProvider.mockReturnValue(bootstrap.promise);
        api.getProviderCopy.mockResolvedValue({
            provider: "codex",
            provider_label: "Codex",
            instruction_file: "AGENTS.md",
            home_dir: "~/.codex",
            sessions_store: "~/.codex/sessions",
            fix_label: "Fix with Codex",
            global_state_source: "~/.codex",
        });
        api.setActiveProvider.mockResolvedValue(undefined);

        const providerModule = (await import(
            `../../frontend/src/lib/provider.ts?test=${++moduleId}`
        )) as typeof import("@/lib/provider");
        await providerModule.setProvider("codex");
        bootstrap.resolve({ active_provider: "claude" });
        await Promise.resolve();
        await Promise.resolve();

        expect(get(providerModule.provider)).toBe("codex");
        expect(document.documentElement.getAttribute("data-provider")).toBe(
            "codex",
        );
    });
});
