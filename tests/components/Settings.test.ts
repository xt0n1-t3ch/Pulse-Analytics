import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, waitFor, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import { get } from "svelte/store";
import type { AnalyticsSummary, HealthResponse } from "@/lib/api";

const summary: AnalyticsSummary = {
  total_sessions: 42,
  priced_sessions: 42,
  total_cost: 100,
  cost_basis: "exact",
  cost_sources: ["anthropic_api_equivalent"],
  total_tokens: 5_000_000,
  total_cache_read: 2_000_000,
  total_cache_write: 200_000,
  avg_duration_secs: 1200,
  avg_tokens_per_session: 120_000,
  avg_cost_per_session: 2.38,
  top_project: "pulse",
  top_model: "Claude Opus 4.8",
  days_tracked: 30,
};

const getDbSize = vi.fn(async () => 5 * 1024 * 1024);
const getAnalyticsSummary = vi.fn(async () => summary);
const getPlanInfo = vi.fn(async () => ({ provider: "claude", plan_key: "max_20x", plan_name: "Max 20x", detected: true }));
const setPlanOverride = vi.fn(async () => undefined);
const setActiveProvider = vi.fn(async () => undefined);
const getProviderCopy = vi.fn(async () => null);
const clearHistory = vi.fn(async () => 7);
const exportAllData = vi.fn(async () => ({ ok: true }));
const repairPreview = {
  rows_checked: 40,
  rows_to_update: 12,
  rows_without_transcript: 3,
  tokens_before: 90_000_000,
  tokens_after: 35_000_000,
  cost_before: 180.5,
  cost_after: 64.25,
  backup_path: null,
  applied: false,
};
const previewClaudeHistoryRepair = vi.fn(async () => repairPreview);
const applyClaudeHistoryRepair = vi.fn(async () => ({
  ...repairPreview,
  applied: true,
  backup_path: "C:/pulse/pulse-analytics.db.pre-history-repair-20260922T120000Z.bak",
}));

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...actual,
    getDbSize: () => getDbSize(),
    getAnalyticsSummary: () => getAnalyticsSummary(),
    getPlanInfo: () => getPlanInfo(),
    setPlanOverride: (plan: string) => setPlanOverride(plan),
    setActiveProvider: (provider: string) => setActiveProvider(provider),
    getProviderCopy: () => getProviderCopy(),
    clearHistory: () => clearHistory(),
    exportAllData: (providerScope?: string) => exportAllData(providerScope),
    previewClaudeHistoryRepair: () => previewClaudeHistoryRepair(),
    applyClaudeHistoryRepair: () => applyClaudeHistoryRepair(),
  };
});

const healthFixture: HealthResponse = {
  version: "0.1.0",
  uptime_seconds: 120,
  discord_status: "Connected",
  discord_enabled: true,
};

describe("Settings.svelte", () => {
  beforeEach(async () => {
    getDbSize.mockClear();
    getAnalyticsSummary.mockClear();
    getPlanInfo.mockReset();
    setPlanOverride.mockReset();
    setActiveProvider.mockReset();
    getProviderCopy.mockReset();
    getPlanInfo.mockResolvedValue({ provider: "claude", plan_key: "max_20x", plan_name: "Max 20x", detected: true });
    setPlanOverride.mockResolvedValue(undefined);
    setActiveProvider.mockResolvedValue(undefined);
    getProviderCopy.mockResolvedValue(null);
    clearHistory.mockClear();
    exportAllData.mockClear();
    const { health, rateLimits, planInfo, accessSnapshot, selectedAccessSourceId } = await import("@/lib/stores");
    const { provider } = await import("@/lib/provider");
    provider.set("claude");
    health.set(healthFixture);
    rateLimits.set(null);
    planInfo.set({ provider: "claude", plan_key: "max_20x", plan_name: "Max 20x", detected: true });
    accessSnapshot.set({ routes: [
      {
        source: { id: "claude-subscription", kind: "claude_subscription", provider: "claude", auth_method: "oauth", proof: "quota_response", plan: "max_20x" },
        availability: "available", freshness: "fresh", provenance: "provider_api",
        observed_at: null, fetched_at: null, expires_at: null, windows: [], credits: null,
        extra_usage: null, local_history: { available: true, sessions: 1 }, error: null,
      },
      {
        source: { id: "codex-subscription", kind: "codex_subscription", provider: "codex", auth_method: "app_server", proof: "quota_response", plan: "pro_20x" },
        availability: "available", freshness: "fresh", provenance: "app_server",
        observed_at: null, fetched_at: null, expires_at: null, windows: [], credits: null,
        extra_usage: null, local_history: { available: true, sessions: 1 }, error: null,
      },
    ] });
    selectedAccessSourceId.set("claude-subscription");
  });

  it("uses the exact OpenCode logo and theme-aware identity color", async () => {
    const { provider, providerProfile } = await import("@/lib/provider");
    provider.set("opencode");
    const { accessSnapshot } = await import("@/lib/stores");
    const route = get(accessSnapshot)!.routes[0];
    accessSnapshot.set({ routes: [{ ...route, source: { ...route.source, kind: "open_code_go", provider: "opencode", proof: "quota_response" } }] });
    const Settings = (await import("@/views/Settings.svelte")).default;
    const { container } = render(Settings, { onToggleTheme: vi.fn(), currentTheme: "light" });
    await tick();
    expect(container.querySelector(".it-mark img")?.getAttribute("src")).toContain("opencode-v2");
    expect(get(providerProfile).accent).toBe("var(--text-primary)");
    expect(container.textContent).toContain("OpenCode Go usage API");
  });

  it("mounts and shows the identity masthead plus configuration controls", async () => {
    const Settings = (await import("@/views/Settings.svelte")).default;
    const { container, getByText } = render(Settings, {
      props: { onToggleTheme: () => {}, currentTheme: "dark" },
    });
    await tick();

    expect(getByText("Settings")).toBeTruthy();
    expect(getByText("Data sources")).toBeTruthy();
    expect(getByText("Data management")).toBeTruthy();
    expect(container.querySelectorAll(".rail-ctrl").length).toBe(4);
  });

  it("reflects a manual plan override on the select instead of reverting to auto", async () => {
    const { planInfo } = await import("@/lib/stores");
    planInfo.set({ provider: "claude", plan_key: "max_20x", plan_name: "Max 20x", detected: false });
    const Settings = (await import("@/views/Settings.svelte")).default;
    const { container } = render(Settings, {
      props: { onToggleTheme: () => {}, currentTheme: "dark" },
    });
    await tick();

    const planSelect = container.querySelector('[aria-label="Plan override"]');
    expect(planSelect).toBeTruthy();
    expect(planSelect?.textContent).toContain("Max 20x");
    expect(planSelect?.textContent).not.toContain("Auto-detect");
  });

  it("exports the selected analytics scope", async () => {
    const { selectedAnalyticsProviderScope } = await import("@/lib/stores");
    selectedAnalyticsProviderScope.set("all");
    const { getByRole } = render((await import("@/views/Settings.svelte")).default);
    const exportButton = getByRole("button", { name: "Export JSON" });
    await waitFor(() => expect((exportButton as HTMLButtonElement).disabled).toBe(false));

    await fireEvent.click(exportButton);

    await waitFor(() => expect(exportAllData).toHaveBeenCalledWith("all"));
  });

  it("never combines the active provider with a stale plan from another provider", async () => {
    const { planInfo } = await import("@/lib/stores");
    const { provider } = await import("@/lib/provider");
    provider.set("claude");
    planInfo.set({ provider: "codex", plan_key: "pro_20x", plan_name: "Pro 20x", detected: true });

    const Settings = (await import("@/views/Settings.svelte")).default;
    const { container, getByText } = render(Settings, {
      props: { onToggleTheme: () => {}, currentTheme: "dark" },
    });
    await tick();

    expect(getByText("Detecting plan…")).toBeTruthy();
    expect(container.querySelector(".it-line")?.textContent).not.toContain("Pro 20x");
  });

  it("keeps analytics and account selection independent when the broadcaster changes", async () => {
    const { selectedAccessSourceId, selectedAnalyticsProviderScope } = await import("@/lib/stores");
    getPlanInfo.mockResolvedValueOnce({
      provider: "codex",
      plan_key: "pro_20x",
      plan_name: "Pro 20x",
      detected: true,
    });
    const Settings = (await import("@/views/Settings.svelte")).default;
    const { getByRole } = render(Settings, {
      props: { onToggleTheme: () => {}, currentTheme: "dark" },
    });

    await fireEvent.click(getByRole("button", { name: "Active provider" }));
    await fireEvent.click(getByRole("option", { name: "Codex" }));

    await waitFor(() => expect(setActiveProvider).toHaveBeenCalledWith("codex"));
    expect(get(selectedAnalyticsProviderScope)).toBe("all");
  });

  it("never renders an invalid same-provider plan claim", async () => {
    const { planInfo } = await import("@/lib/stores");
    const { provider } = await import("@/lib/provider");
    provider.set("claude");
    planInfo.set({ provider: "claude", plan_key: "pro_20x", plan_name: "Pro 20x", detected: true });

    const Settings = (await import("@/views/Settings.svelte")).default;
    const { container, getByText } = render(Settings, {
      props: { onToggleTheme: () => {}, currentTheme: "dark" },
    });
    await tick();

    expect(getByText("Not reported")).toBeTruthy();
    expect(container.querySelector(".it-line")?.textContent).not.toContain("Pro 20x");
  });

  it("loads the database size and session total from the api layer", async () => {
    const Settings = (await import("@/views/Settings.svelte")).default;
    const { getByText } = render(Settings, {
      props: { onToggleTheme: () => {}, currentTheme: "dark" },
    });
    await tick();

    await waitFor(() => expect(getDbSize).toHaveBeenCalled());
    await waitFor(() => expect(getByText("5.0 MB")).toBeTruthy());
    expect(getByText("42")).toBeTruthy();
  });

  it("shows backend failure explicitly instead of fabricating an empty database", async () => {
    getDbSize.mockRejectedValueOnce(new Error("database locked"));
    getAnalyticsSummary.mockRejectedValueOnce(new Error("database locked"));
    const Settings = (await import("@/views/Settings.svelte")).default;
    const { container, findByRole } = render(Settings, {
      props: { onToggleTheme: () => {}, currentTheme: "dark" },
    });

    expect((await findByRole("alert")).textContent).toContain("Local analytics unavailable");
    const values = [...container.querySelectorAll(".dm-val")].map((node) => node.textContent?.trim());
    expect(values).toEqual(["Unavailable", "Unavailable"]);
    expect(values).not.toContain("0 B");
  });

  it("requires a confirm step before clearing history", async () => {
    const Settings = (await import("@/views/Settings.svelte")).default;
    const { getByText } = render(Settings, {
      props: { onToggleTheme: () => {}, currentTheme: "dark" },
    });
    await tick();
    await waitFor(() => expect(getDbSize).toHaveBeenCalled());
    await waitFor(() => expect(getByText("Clear history").closest("button")?.hasAttribute("disabled")).toBe(false));

    await fireEvent.click(getByText("Clear history"));
    expect(clearHistory).not.toHaveBeenCalled();

    await fireEvent.click(getByText("Confirm clear"));
    await waitFor(() => expect(clearHistory).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(getByText("Cleared 7 sessions")).toBeTruthy());
  });

  it("previews the Claude history correction and applies it only after confirmation", async () => {
    previewClaudeHistoryRepair.mockClear();
    applyClaudeHistoryRepair.mockClear();
    const Settings = (await import("@/views/Settings.svelte")).default;
    const { getByText, container } = render(Settings, {
      props: { onToggleTheme: () => {}, currentTheme: "dark" },
    });
    await tick();

    await fireEvent.click(getByText("Check history"));
    await waitFor(() => expect(previewClaudeHistoryRepair).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(getByText("of 40 saved")).toBeTruthy());
    expect(container.textContent).toContain("3 saved sessions no longer have a transcript");
    expect(applyClaudeHistoryRepair).not.toHaveBeenCalled();

    await fireEvent.click(getByText("Correct sessions…"));
    expect(applyClaudeHistoryRepair).not.toHaveBeenCalled();
    await fireEvent.click(getByText("Back up and correct 12 sessions"));
    await waitFor(() => expect(applyClaudeHistoryRepair).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(container.textContent).toContain("Corrected 12 sessions"));
    expect(container.textContent).toContain("pre-history-repair-20260922T120000Z.bak");
  });

  it("toggles the theme through the appearance control", async () => {
    const onToggleTheme = vi.fn();
    const Settings = (await import("@/views/Settings.svelte")).default;
    const { getByText } = render(Settings, {
      props: { onToggleTheme, currentTheme: "dark" },
    });
    await tick();

    await fireEvent.click(getByText("Light"));
    expect(onToggleTheme).toHaveBeenCalledTimes(1);
  });

  it("rolls back a failed plan override instead of leaving an optimistic claim", async () => {
    const { planInfo } = await import("@/lib/stores");
    planInfo.set({ provider: "claude", plan_key: "max_20x", plan_name: "Max 20x", detected: false });
    setPlanOverride.mockRejectedValueOnce(new Error("write failed"));

    const Settings = (await import("@/views/Settings.svelte")).default;
    const { getByRole } = render(Settings, {
      props: { onToggleTheme: () => {}, currentTheme: "dark" },
    });
    await tick();

    await fireEvent.click(getByRole("button", { name: "Plan override" }));
    await fireEvent.click(getByRole("option", { name: "Auto-detect" }));

    await waitFor(() => expect(getByRole("alert").textContent).toContain("could not be saved"));
    expect(getByRole("button", { name: "Plan override" }).textContent).toContain("Max 20x");
  });

  it("keeps the newest provider plan when an older request resolves last", async () => {
    let resolveCodex!: (value: {
      provider: "codex";
      plan_key: string;
      plan_name: string;
      detected: boolean;
    }) => void;
    let resolveClaude!: (value: {
      provider: "claude";
      plan_key: string;
      plan_name: string;
      detected: boolean;
    }) => void;
    getPlanInfo
      .mockImplementationOnce(() => new Promise((resolve) => { resolveCodex = resolve; }))
      .mockImplementationOnce(() => new Promise((resolve) => { resolveClaude = resolve; }));

    const Settings = (await import("@/views/Settings.svelte")).default;
    const { getByRole, getByText, queryByText } = render(Settings, {
      props: { onToggleTheme: () => {}, currentTheme: "dark" },
    });
    await tick();

    await fireEvent.click(getByRole("button", { name: "Active provider" }));
    await fireEvent.click(getByRole("option", { name: "Codex" }));
    await waitFor(() => expect(getByRole("button", { name: "Active provider" }).textContent).toContain("Codex"));

    await fireEvent.click(getByRole("button", { name: "Active provider" }));
    await fireEvent.click(getByRole("option", { name: "Claude Code" }));

    await waitFor(() => expect(getPlanInfo).toHaveBeenCalledTimes(2));
    resolveClaude({ provider: "claude", plan_key: "max_20x", plan_name: "Max 20x", detected: true });
    await waitFor(() => expect(getByText("Max 20x")).toBeTruthy());
    resolveCodex({ provider: "codex", plan_key: "pro_20x", plan_name: "Pro 20x", detected: true });
    await Promise.resolve();

    expect(queryByText("Pro 20x")).toBeNull();
    expect(getByRole("button", { name: "Active provider" }).textContent).toContain("Claude Code");
  });
});
