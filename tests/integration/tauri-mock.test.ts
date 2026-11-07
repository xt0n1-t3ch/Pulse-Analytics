import { describe, it, expect } from "bun:test";
import {
    getLiveSessions,
    getActiveProvider,
    getContextBreakdown,
    getDiscordPreview,
    discordDisplayPrefsArgs,
} from "@/lib/api";

describe("tauri IPC mock through the api layer", () => {
    it("resolves list commands to empty arrays", async () => {
        await expect(getLiveSessions()).resolves.toEqual([]);
    });

    it("resolves mapped scalar commands to their stub value", async () => {
        expect((await getActiveProvider()) as unknown).toBe("claude");
    });

    it("resolves the centralized Discord preview command", async () => {
        await expect(getDiscordPreview()).resolves.toMatchObject({
            app_name: "Claude Code",
            details: "Claude Code",
            state: "Waiting for session",
        });
    });

    it("maps Discord display prefs to Tauri camelCase argument names", () => {
        expect(
            discordDisplayPrefsArgs({
                show_project: true,
                show_branch: false,
                show_model: true,
                show_activity: true,
                show_tokens: true,
                show_cost: true,
                show_limits: false,
                show_credits: false,
                show_context: false,
                show_systems: true,
            }),
        ).toEqual({
            showProject: true,
            showBranch: false,
            showModel: true,
            showActivity: true,
            showTokens: true,
            showCost: true,
            showLimits: false,
            showCredits: false,
            showContext: false,
            showSystems: true,
        });
    });

    it("resolves unmapped commands to undefined without throwing", async () => {
        await expect(getContextBreakdown()).resolves.toBeUndefined();
    });
});
