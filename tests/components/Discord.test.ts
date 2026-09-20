import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, waitFor, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import type {
  DiscordDisplayPrefs,
  DiscordSettings,
  SessionInfo,
  DiscordUserInfo,
  HealthResponse,
  DiscordPresencePreview,
} from "@/lib/api";

const defaultDisplayPrefs: DiscordDisplayPrefs = {
  show_project: true,
  show_branch: true,
  show_model: true,
  show_activity: true,
  show_tokens: false,
  show_cost: false,
  show_limits: true,
  show_credits: true,
  show_context: true,
  show_systems: true,
};
let discordSettings: DiscordSettings;
const setDiscordEnabled = vi.fn(async (enabled: boolean) => {
  discordSettings = { ...discordSettings, enabled };
  return discordSettings;
});
const getDiscordSettings = vi.fn(async () => discordSettings);
const setDiscordDisplayPrefs = vi.fn(async (prefs: DiscordDisplayPrefs) => {
  discordSettings = { ...discordSettings, display_prefs: prefs };
  return discordSettings;
});
const setDiscordFieldOrder = vi.fn(async (order: string[]) => {
  discordSettings = { ...discordSettings, field_order: order };
  return discordSettings;
});
const setCodexDesktopDesign = vi.fn(
  async (design: "codex_app" | "chatgpt_app") => {
    discordSettings = { ...discordSettings, desktop_design: design };
    return discordSettings;
  },
);
let discordPreviewPayload: unknown = null;
const getDiscordPreview = vi.fn(async () => discordPreviewPayload);

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...actual,
    setDiscordEnabled: (enabled: boolean) => setDiscordEnabled(enabled),
    getDiscordSettings: () => getDiscordSettings(),
    setDiscordDisplayPrefs: (prefs: DiscordDisplayPrefs) => setDiscordDisplayPrefs(prefs),
    setDiscordFieldOrder: (order: string[]) => setDiscordFieldOrder(order),
    setCodexDesktopDesign: (design: "codex_app" | "chatgpt_app") =>
      setCodexDesktopDesign(design),
    getDiscordPreview: () => getDiscordPreview(),
  };
});

function makeSession(id: string, project: string): SessionInfo {
  return {
    session_id: id,
    session_name: null,
    project,
    model: "Claude Opus 4.8",
    model_id: "claude-opus-4-8",
    provider: "claude",
    context_window: "200K",
    cost: 2.5,
    cost_available: true,
    cost_basis: "exact",
    tokens: 120_000,
    input_tokens: 40_000,
    output_tokens: 20_000,
    cache_write_tokens: 10_000,
    cache_read_tokens: 50_000,
    branch: "main",
    activity: "Editing stores.ts",
    activity_target: "stores.ts",
    effort: "High",
    effort_explicit: true,
    is_idle: false,
    started_at: "2026-05-28T10:00:00Z",
    duration_secs: 600,
    has_thinking: true,
    workflow_label: null,
    subagent_count: 0,
    subagents: [],
    tokens_per_sec: 42,
    input_cost: 0.8,
    output_cost: 1,
    cache_write_cost: 0.5,
    cache_read_cost: 0.2,
    speed: "standard",
    fast: false,
    service_tier: null,
    app_name: null,
  };
}

const discordUserFixture: DiscordUserInfo = {
  user_id: "123",
  username: "xt0n1",
  global_name: "Tony",
  discriminator: "0",
  avatar_hash: "abc",
  avatar_url: "https://cdn.discordapp.com/avatars/123/abc.png",
  avatar_default_url: "https://cdn.discordapp.com/embed/avatars/5.png",
  banner_hash: null,
  banner_url: null,
};

const healthFixture: HealthResponse = {
  version: "0.1.0",
  uptime_seconds: 120,
  discord_status: "Connected",
  discord_enabled: true,
};

describe("Discord.svelte", () => {
  beforeEach(async () => {
    setDiscordEnabled.mockClear();
    getDiscordSettings.mockClear();
    setDiscordDisplayPrefs.mockClear();
    setDiscordFieldOrder.mockClear();
    setCodexDesktopDesign.mockClear();
    discordSettings = {
      provider: "claude",
      enabled: true,
      status: "Connected",
      publisher: "pulse",
      display_prefs: { ...defaultDisplayPrefs },
      desktop_design: null,
      supports_desktop_design: false,
      supports_field_order: false,
      supports_credits: false,
      field_order: ["project", "branch", "model", "activity", "tokens", "cost", "quotas", "credits", "context", "systems"],
    };
    getDiscordPreview.mockClear();
    discordPreviewPayload = null;
    const { provider } = await import("@/lib/provider");
    provider.set("claude");
    const {
      accessSnapshot,
      sessions,
      discordUser,
      health,
      discordPreview,
      discordPresencePreview,
      selectedAccessSourceId,
    } = await import("@/lib/stores");
    accessSnapshot.set(null);
    selectedAccessSourceId.set("all");
    sessions.set([makeSession("s1", "pulse")]);
    discordUser.set(discordUserFixture);
    health.set(healthFixture);
    discordPresencePreview.set(null);
    discordPreview.set({
      showProject: true,
      showBranch: true,
      showModel: true,
      showActivity: true,
      showTokens: false,
      showCost: false,
      showLimits: true,
      showCredits: true,
      showContext: true,
      showSystems: true,
    });
  });

  it("mounts and shows the live-preview profile with the active session details", async () => {
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container, getByText } = render(Discord);
    await tick();

    expect(getByText("Discord")).toBeTruthy();
    expect(container.querySelector(".dp-profile")).not.toBeNull();
    await waitFor(() => {
      expect(container.querySelector(".dp-activity-details")?.textContent).toContain("pulse");
    });
    expect(getByText("Tony")).toBeTruthy();
    expect(container.querySelector(".dp-handle")?.textContent?.trim()).toBe("@xt0n1");
    expect(container.querySelector(".dp-art-large")).not.toBeNull();
  });

  it("offers native OpenCode without borrowed quotas or a substitute logo", async () => {
    const { provider } = await import("@/lib/provider");
    provider.set("opencode");
    discordSettings = { ...discordSettings, provider: "opencode", supports_credits: false, supports_desktop_design: false };
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container, getByText } = render(Discord);
    await tick();
    expect(container.querySelector(".dp-art-large")?.getAttribute("src")).toContain("opencode-v2");
    const quota = getByText("Usage quotas").closest(".field-cell")?.querySelector("input");
    expect((quota as HTMLInputElement).disabled).toBe(false);
    for (const label of ["Credits available", "Systems"]) {
      expect((getByText(label).closest(".field-cell")?.querySelector("input") as HTMLInputElement).disabled).toBe(true);
    }
    expect(container.querySelector(".fc-den")?.textContent).toBe("/8");
  });

  it("renders the Discord global name instead of the username", async () => {
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container, getByText } = render(Discord);
    await tick();

    expect(getByText("Tony")).toBeTruthy();
    expect(container.querySelector(".dp-username")?.textContent?.trim()).toBe("Tony");
  });

  it.each(["commandcode", "opencode"] as const)("names the selected %s provider when idle", async (id) => {
    const { provider } = await import("@/lib/provider");
    const { sessions, discordPresencePreview } = await import("@/lib/stores");
    const name = id === "commandcode" ? "Command Code" : "OpenCode";
    provider.set(id);
    sessions.set([]);
    discordSettings = { ...discordSettings, provider: id };
    discordPreviewPayload = {
      provider: id, app_name: name, details: "Idle", state: `Waiting for ${name}`,
      large_image_key: id, large_text: name, small_image_key: null,
      small_text: null, has_session: false, duration_secs: 0,
    };
    discordPresencePreview.set(discordPreviewPayload as DiscordPresencePreview);
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await waitFor(() => expect(container.querySelector(".hm-state")?.textContent?.trim()).toBe(`${name} is idle`));
    if (id === "commandcode") expect(container.textContent).toContain("Publishing idle Command Code status to Discord");
  });

  it("does not claim Command Code idle publication when Discord is unavailable", async () => {
    const { provider } = await import("@/lib/provider");
    const { sessions, discordPresencePreview, health } = await import("@/lib/stores");
    provider.set("commandcode");
    sessions.set([]);
    discordSettings = { ...discordSettings, provider: "commandcode", status: "Discord unavailable" };
    health.set({ ...healthFixture, discord_status: "Discord unavailable" });
    discordPreviewPayload = {
      provider: "commandcode", app_name: "Command Code", details: "Idle", state: "Waiting for Command Code",
      large_image_key: "commandcode", large_text: "Command Code", small_image_key: null,
      small_text: null, has_session: false, duration_secs: 0,
    };
    discordPresencePreview.set(discordPreviewPayload as DiscordPresencePreview);
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await waitFor(() => expect(container.querySelector(".hm-state")?.textContent?.trim()).toBe("Waiting for Discord"));
  });

  it("renders the Discord username as a handle below the display name", async () => {
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { getByText } = render(Discord);
    await tick();

    expect(getByText("@xt0n1")).toBeTruthy();
  });

  /** The headline answers "is Discord showing me right now?", so it has to
   *  agree with the IPC diagnostic beside it. */
  it("reports Broadcasting only when Rich Presence is enabled and IPC is connected", async () => {
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await tick();

    const state = container.querySelector(".hm-state");
    expect(state?.textContent?.trim()).toBe("Broadcasting");
    expect(state?.classList.contains("live")).toBe(true);
  });

  it("waits rather than claiming a broadcast while Discord IPC is disconnected", async () => {
    const { health } = await import("@/lib/stores");
    health.set({ ...healthFixture, discord_status: "Disconnected" });

    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await tick();

    const state = container.querySelector(".hm-state");
    expect(state?.textContent?.trim()).toBe("Waiting for Discord");
    expect(state?.classList.contains("live")).toBe(false);
    expect(state?.classList.contains("waiting")).toBe(true);
  });

  it("reports Paused when Rich Presence is switched off", async () => {
    const { health } = await import("@/lib/stores");
    discordSettings = { ...discordSettings, enabled: false };
    health.set({ ...healthFixture, discord_enabled: false });

    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await tick();
    await tick();

    const state = container.querySelector(".hm-state");
    expect(state?.textContent?.trim()).toBe("Paused");
    expect(state?.classList.contains("waiting")).toBe(false);
  });

  it("renders the backend Discord payload instead of recomputing branch visibility locally", async () => {
    const { sessions, discordPresencePreview } = await import("@/lib/stores");
    const session = makeSession("active1", "PropertyAlpha-Agent");
    session.branch = "feat/marketplace-addtochat-liveview-management";
    sessions.set([session]);
    discordPresencePreview.set({
      provider: "claude",
      app_name: "Claude Code",
      details: "Thinking · PropertyAlpha-Agent",
      state: "Claude Opus 4.8 · ULTRACODE · 1 agent · 161.5M tokens · $195.35",
      large_image_key: "large",
      large_text: "Claude Code",
      small_image_key: null,
      small_text: null,
      has_session: true,
      duration_secs: 19_200,
    });

    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await tick();

    const details = container.querySelector(".dp-activity-details")?.textContent ?? "";
    const state = container.querySelector(".dp-activity-state")?.textContent ?? "";
    expect(details).toBe("Thinking · PropertyAlpha-Agent");
    expect(details).not.toContain("feat/marketplace");
    expect(state).toContain("ULTRACODE");
    expect(state).toContain("1 agent");
  });

  it("renders the backend small activity asset as a decorative chip", async () => {
    const { discordPresencePreview } = await import("@/lib/stores");
    discordPresencePreview.set({
      provider: "claude",
      app_name: "Claude Code",
      details: "Thinking · pulse",
      state: "Claude Opus 4.8",
      large_image_key: "large",
      large_text: "Claude Code",
      small_image_key: "codex-app",
      small_text: "Codex App",
      has_session: true,
      duration_secs: 30,
    });

    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await tick();

    const chip = container.querySelector(".dp-art-small") as HTMLImageElement;
    expect(chip).not.toBeNull();
    expect(chip.getAttribute("src")).toContain("codex-app");
    expect(chip.getAttribute("aria-hidden")).toBe("true");
    expect(chip.getAttribute("alt")).toBe("");
  });

  it("does not render a small activity chip when the backend omits it", async () => {
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await tick();

    expect(container.querySelector(".dp-art-small")).toBeNull();
  });

  it("renders ChatGPT App identity and art from the canonical backend asset key", async () => {
    const { sessions, discordPresencePreview } = await import("@/lib/stores");
    const { provider } = await import("@/lib/provider");
    const session = makeSession("codex1", "pulse");
    session.provider = "codex";
    session.app_name = "ChatGPT App";
    sessions.set([session]);
    provider.set("codex");
    discordPresencePreview.set({
      provider: "codex",
      app_name: "ChatGPT App",
      details: "Running command · pulse",
      state: "GPT-5.6 Sol · Max | Pro 20x ($200/month)",
      large_image_key: "codex-logo",
      large_text: "ChatGPT App",
      small_image_key: null,
      small_text: null,
      has_session: true,
      duration_secs: 42,
    });

    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container, getAllByText } = render(Discord);
    await tick();

    expect(getAllByText("ChatGPT App").length).toBeGreaterThan(0);
    const art = container.querySelector(".dp-art-large") as HTMLImageElement;
    expect(art.getAttribute("src")).toContain("chatgpt-app");
    expect(art.getAttribute("alt")).toBe("ChatGPT App");
  });

  it("renders the field toggles and the master Rich Presence toggle", async () => {
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container, getByText } = render(Discord);
    await tick();

    expect(getByText("Rich Presence")).toBeTruthy();
    expect(container.querySelectorAll(".field-cell").length).toBe(10);
    expect(getByText("Usage quotas")).toBeTruthy();
    expect(getByText("Credits available")).toBeTruthy();
    expect(getByText("Context usage")).toBeTruthy();
    expect(container.querySelectorAll(".preset-opt").length).toBe(3);
  });

  it("does not broadcast stale allowance windows as current quota", async () => {
    const { accessSnapshot } = await import("@/lib/stores");
    accessSnapshot.set({
      routes: [{
        source: {
          id: "claude-subscription:default",
          kind: "claude_subscription",
          provider: "claude",
          auth_method: "oauth",
          proof: "quota_response",
          plan: "max_20x",
        },
        availability: "available",
        freshness: "stale",
        provenance: "provider_api",
        observed_at: "2026-08-03T10:00:00Z",
        fetched_at: "2026-08-03T10:00:00Z",
        expires_at: null,
        windows: [{
          key: "weekly",
          label: "Weekly",
          window_minutes: 10080,
          used_percent: 88,
          remaining_percent: null,
          resets_at: "2026-08-07T00:00:00Z",
        }],
        credits: null,
        extra_usage: null,
        error: null,
      }],
    });

    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await tick();

    expect(container.querySelector(".dp-activity-state")?.textContent).not.toContain("Weekly 88% used");
  });

  it("keeps the broadcast preview independent from the selected account", async () => {
    const { sessions, selectedAccessSourceId } = await import("@/lib/stores");
    const claude = makeSession("claude-1", "claude-project");
    const codex = makeSession("codex-1", "codex-project");
    codex.provider = "codex";
    codex.model = "GPT-5.6 Sol";
    sessions.set([claude, codex]);
    selectedAccessSourceId.set("codex-subscription:default");
    const { accessSnapshot } = await import("@/lib/stores");
    accessSnapshot.set({
      routes: [{
        source: {
          id: "codex-subscription:default",
          kind: "codex_subscription",
          provider: "codex",
          auth_method: "app_server",
          proof: "quota_response",
          plan: "pro_20x",
        },
        availability: "available",
        freshness: "fresh",
        provenance: "app_server",
        observed_at: "2026-08-03T10:00:00Z",
        fetched_at: "2026-08-03T10:00:00Z",
        expires_at: null,
        windows: [],
        credits: null,
        extra_usage: null,
        error: null,
      }],
    });

    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await tick();

    expect(container.querySelector(".dp-activity-details")?.textContent).toContain("claude-project");
  });

  it("marks provider-unsupported fields unavailable instead of offering a switch that reverts", async () => {
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container, getByText } = render(Discord);
    await waitFor(() => expect(getDiscordSettings).toHaveBeenCalledTimes(1));

    const creditsRow = getByText("Credits available").closest(".field-cell");
    const creditsToggle = creditsRow?.querySelector("input") as HTMLInputElement;
    expect(creditsRow?.classList.contains("unavailable")).toBe(true);
    expect(creditsToggle.disabled).toBe(true);
    expect(creditsToggle.checked).toBe(false);

    // The readout must not count a field the provider can never broadcast.
    expect(container.querySelector(".fc-den")?.textContent).toBe("/9");

    await fireEvent.click(creditsToggle);
    expect(setDiscordDisplayPrefs).not.toHaveBeenCalled();
  });

  it("hydrates privacy from Rust without writing local defaults back", async () => {
    discordSettings = {
      ...discordSettings,
      display_prefs: { ...defaultDisplayPrefs, show_branch: false },
    };
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { getByText } = render(Discord);

    await waitFor(() => expect(getDiscordSettings).toHaveBeenCalledTimes(1));
    const branchRow = getByText("Git branch").closest(".field-cell");
    const branchToggle = branchRow?.querySelector("input") as HTMLInputElement;
    expect(branchToggle.checked).toBe(false);
    expect(setDiscordDisplayPrefs).not.toHaveBeenCalled();
  });

  it("persists a branch toggle and refreshes the canonical payload", async () => {
    discordPreviewPayload = {
      provider: "claude",
      app_name: "Claude Code",
      details: "Thinking · pulse",
      state: "Claude Opus 4.8",
      large_image_key: "large",
      large_text: "Claude Code",
      small_image_key: null,
      small_text: null,
      has_session: true,
      duration_secs: 30,
    };
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container, getByText } = render(Discord);
    await waitFor(() => expect(getDiscordSettings).toHaveBeenCalledTimes(1));

    const branchToggle = getByText("Git branch")
      .closest(".field-cell")
      ?.querySelector("input") as HTMLInputElement;
    await fireEvent.change(branchToggle);

    await waitFor(() => {
      expect(setDiscordDisplayPrefs).toHaveBeenCalledWith(
        expect.objectContaining({ show_branch: false }),
      );
      expect(getDiscordPreview).toHaveBeenCalled();
      expect(container.querySelector(".dp-activity-details")?.textContent).toBe(
        "Thinking · pulse",
      );
    });
  });

  it("makes the backend-owned autosave lifecycle visible", async () => {
    let resolveSave!: (settings: DiscordSettings) => void;
    setDiscordDisplayPrefs.mockImplementationOnce(
      () => new Promise<DiscordSettings>((resolve) => {
        resolveSave = resolve;
      }),
    );
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { getByText, getByRole } = render(Discord);
    await waitFor(() => expect(getDiscordSettings).toHaveBeenCalledTimes(1));

    const branchToggle = getByText("Git branch")
      .closest(".field-cell")
      ?.querySelector("input") as HTMLInputElement;
    await fireEvent.change(branchToggle);

    expect(getByRole("status", { name: "Discord settings save status" }).textContent).toContain(
      "Saving changes",
    );
    resolveSave({
      ...discordSettings,
      display_prefs: { ...discordSettings.display_prefs, show_branch: false },
    });
    await waitFor(() => {
      expect(getByRole("status", { name: "Discord settings save status" }).textContent).toContain(
        "Saved automatically",
      );
    });
  });

  it("rolls a privacy toggle back when persistence fails", async () => {
    setDiscordDisplayPrefs.mockRejectedValueOnce(new Error("disk full"));
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { getByText, getByRole } = render(Discord);
    await waitFor(() => expect(getDiscordSettings).toHaveBeenCalledTimes(1));

    const branchToggle = getByText("Git branch")
      .closest(".field-cell")
      ?.querySelector("input") as HTMLInputElement;
    expect(branchToggle.checked).toBe(true);
    await fireEvent.change(branchToggle);

    await waitFor(() => expect(branchToggle.checked).toBe(true));
    expect(getByRole("status", { name: "Discord settings save status" }).textContent).toContain(
      "Save failed",
    );
  });

  it("lets Codex switch desktop identity through the persisted design control", async () => {
    const { provider } = await import("@/lib/provider");
    provider.set("codex");
    discordSettings = {
      ...discordSettings,
      provider: "codex",
      desktop_design: "codex_app",
      supports_desktop_design: true,
    };
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { getByRole } = render(Discord);
    await waitFor(() => expect(getDiscordSettings).toHaveBeenCalledTimes(1));

    await fireEvent.click(getByRole("button", { name: "ChatGPT App" }));

    await waitFor(() => {
      expect(setCodexDesktopDesign).toHaveBeenCalledWith("chatgpt_app");
      expect(getDiscordPreview).toHaveBeenCalled();
    });
  });

  it("persists accessible field reordering for Codex", async () => {
    const { provider } = await import("@/lib/provider");
    provider.set("codex");
    discordSettings = {
      ...discordSettings,
      provider: "codex",
      supports_field_order: true,
      supports_credits: true,
    };
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { getByRole } = render(Discord);
    await waitFor(() => expect(getDiscordSettings).toHaveBeenCalledTimes(1));

    await fireEvent.click(getByRole("button", { name: "Move Project name down" }));
    await waitFor(() => expect(setDiscordFieldOrder).toHaveBeenCalledTimes(1));
    expect(setDiscordFieldOrder.mock.calls[0][0].slice(0, 2)).toEqual(["branch", "project"]);
  });

  it("shows safe systems signals without exposing subagent names", async () => {
    const { sessions } = await import("@/lib/stores");
    const active = makeSession("active1", "active-project");
    active.has_thinking = true;
    active.workflow_label = "ULTRACODE";
    active.subagent_count = 1;
    active.subagents = [
      {
        agent_type: "secret-researcher",
        model: "Claude Opus 4.8",
        tokens: 10,
        cost: 0.01,
        activity: "Reading private.md",
      },
    ];
    sessions.set([active]);

    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await tick();

    const state = container.querySelector(".dp-activity-state")?.textContent ?? "";
    expect(state).toContain("ULTRACODE");
    expect(state).toContain("1 agent");
    expect(state).not.toContain("secret-researcher");
    expect(state).not.toContain("private.md");
  });

  it("does not label plain Claude thinking as a workflow", async () => {
    const { sessions } = await import("@/lib/stores");
    const active = makeSession("active1", "active-project");
    active.has_thinking = true;
    active.workflow_label = null;
    active.subagent_count = 0;
    sessions.set([active]);

    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await tick();

    const state = container.querySelector(".dp-activity-state")?.textContent ?? "";
    expect(state).not.toContain("workflow active");
    expect(state).not.toContain("ULTRACODE");
  });

  it("previews the active session first and ignores idle sessions", async () => {
    const { sessions } = await import("@/lib/stores");
    const idle = makeSession("idle1", "idle-project");
    idle.is_idle = true;
    const active = makeSession("active1", "active-project");
    sessions.set([idle, active]);

    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await tick();

    await waitFor(() => {
      expect(container.querySelector(".dp-activity-details")?.textContent).toContain("active-project");
    });
    expect(container.querySelector(".dp-activity-details")?.textContent).not.toContain("idle-project");
  });

  it("does not publish quota text without an authenticated access-route proof", async () => {
    const { accessSnapshot } = await import("@/lib/stores");
    accessSnapshot.set({
      routes: [{
        source: {
          id: "claude-unproved",
          kind: "claude_subscription",
          provider: "claude",
          auth_method: "oauth",
          proof: "none",
          plan: null,
        },
        availability: "available",
        freshness: "fresh",
        provenance: "session_jsonl",
        observed_at: "2026-08-02T12:00:00Z",
        fetched_at: "2026-08-02T12:00:00Z",
        expires_at: "2026-08-02T12:05:00Z",
        windows: [{
          key: "weekly",
          label: "Weekly",
          window_minutes: 10_080,
          used_percent: 99,
          remaining_percent: 1,
          resets_at: null,
        }],
        credits: null,
        extra_usage: null,
        error: null,
      }],
    });

    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await tick();

    expect(container.querySelector(".dp-activity-state")?.textContent).not.toContain("Weekly");
    expect(container.querySelector(".dp-activity-state")?.textContent).not.toContain("99%");
  });

  it("calls setDiscordEnabled when the master toggle is flipped off", async () => {
    const Discord = (await import("@/views/Discord.svelte")).default;
    const { container } = render(Discord);
    await tick();

    const toggle = container.querySelector(".big-toggle input") as HTMLInputElement;
    expect(toggle).not.toBeNull();
    await fireEvent.change(toggle);

    await waitFor(() => expect(setDiscordEnabled).toHaveBeenCalledWith(false));
  });

  describe("live preview theme-awareness", () => {
    /** Every colour-bearing rule in the Discord mock must resolve through the
     *  same app tokens used by Pulse panels in both themes. */
    const THEMED_SELECTORS = [
      ".dp-profile",
      ".dp-body",
      ".dp-avatar",
      ".dp-status-dot",
      ".dp-username",
      ".dp-handle",
      ".dp-separator",
      ".dp-section-title",
      ".dp-activity-card",
      ".dp-activity-header",
      ".dp-art-large",
      ".dp-art-small",
      ".dp-activity-name",
      ".dp-activity-details",
      ".dp-activity-elapsed",
    ];

    /** Svelte scopes component styles at build time and happy-dom does not
     *  materialize them, so the source `<style>` block is the honest place to
     *  assert on. */
    function componentCss(): string {
      // Vitest runs with the frontend package as cwd.
      const source = readFileSync(resolve(process.cwd(), "src/views/Discord.svelte"), "utf8");
      const style = source.match(/<style>([\s\S]*)<\/style>/);
      return style?.[1] ?? "";
    }

    it("keeps the stage and Discord preview rules free of hardcoded hex colours", () => {
      const css = componentCss();
      const previewCss = css.slice(css.indexOf("/* ── STAGE ── */"));
      expect(previewCss.length).toBeGreaterThan(0);
      expect(previewCss.match(/#[0-9a-fA-F]{3,6}\b/g)).toBeNull();
    });

    it("stacks field controls before the two-column layout can clip at app widths", () => {
      const css = componentCss();
      expect(css).toMatch(
        /@media \(max-width: 1180px\)[\s\S]*?\.field-grid\s*\{\s*grid-template-columns:\s*1fr;/,
      );
    });

    it("routes every Discord mock surface through the Pulse design tokens", () => {
      const css = componentCss();
      for (const selector of THEMED_SELECTORS) {
        const cls = selector.slice(1);
        const block = css.match(new RegExp(`\\.${cls}\\s*(,[^{]*)?\\{([^}]*)\\}`));
        expect(block, `${selector} rule must exist`).not.toBeNull();
        expect(block?.[2] ?? "", `${selector} must use an app theme token`).toMatch(
          /var\(--(?:surface-panel|bg-card|bg-elevated|border|text-primary|text-secondary|text-muted|success|radius|elev|shadow)/,
        );
      }
    });

    it("keeps the preview rendered in both dark and light themes", async () => {
      const Discord = (await import("@/views/Discord.svelte")).default;

      for (const theme of ["dark", "light"]) {
        document.documentElement.setAttribute("data-theme", theme);
        const { container, unmount } = render(Discord);
        await tick();

        expect(container.querySelector(".dp-profile"), theme).not.toBeNull();
        expect(container.querySelector(".dp-activity-card"), theme).not.toBeNull();
        expect(container.querySelector(".dp-username")?.textContent, theme).toContain("Tony");
        expect(container.querySelector(".dp-handle")?.textContent, theme).toContain("@xt0n1");
        unmount();
      }
      document.documentElement.removeAttribute("data-theme");
    });

    it("renders an intentional empty state when there is no session", async () => {
      const { sessions, discordPresencePreview } = await import("@/lib/stores");
      sessions.set([]);
      discordPresencePreview.set(null);

      const Discord = (await import("@/views/Discord.svelte")).default;
      const { container } = render(Discord);
      await tick();

      expect(container.querySelector(".dp-profile")).not.toBeNull();
      expect(container.querySelector(".dp-activity-details")?.textContent).toBe("No active session");
      expect(container.querySelector(".dp-activity-state")?.textContent).toBe("Idle");
    });

    it("marks the status dot offline when presence is paused", async () => {
      // The component prefers the backend settings payload over health for the
      // enabled flag, so pausing presence has to come through that fixture.
      discordSettings = { ...discordSettings, enabled: false };

      const Discord = (await import("@/views/Discord.svelte")).default;
      const { container } = render(Discord);
      await tick();
      await waitFor(() =>
        expect(container.querySelector(".dp-status-dot.offline")).not.toBeNull(),
      );

      expect(container.querySelector(".dp-status-dot.offline")).not.toBeNull();
    });
  });

  describe("avatar fallback", () => {
    it("starts from the real avatar URL", async () => {
      const Discord = (await import("@/views/Discord.svelte")).default;
      const { container } = render(Discord);
      await tick();
      const img = container.querySelector(".dp-avatar img") as HTMLImageElement;
      expect(img).not.toBeNull();
      expect(img.getAttribute("src")).toBe(discordUserFixture.avatar_url);
    });

    it("swaps to the always-resolvable default avatar when the custom hash 404s", async () => {
      const Discord = (await import("@/views/Discord.svelte")).default;
      const { container } = render(Discord);
      await tick();
      const img = container.querySelector(".dp-avatar img") as HTMLImageElement;
      await fireEvent.error(img);
      await tick();
      const after = container.querySelector(".dp-avatar img") as HTMLImageElement;
      expect(after.getAttribute("src")).toBe(discordUserFixture.avatar_default_url);
    });

    it("falls back to the Pulse mark when even the default avatar fails", async () => {
      const { discordUser } = await import("@/lib/stores");
      discordUser.set({ ...discordUserFixture, avatar_default_url: "" });
      const Discord = (await import("@/views/Discord.svelte")).default;
      const { container } = render(Discord);
      await tick();
      const img = container.querySelector(".dp-avatar img") as HTMLImageElement;
      await fireEvent.error(img);
      await tick();
      expect(container.querySelector(".dp-avatar img")).toBeNull();
      expect(container.querySelector(".dp-avatar svg")).not.toBeNull();
    });
  });
});
