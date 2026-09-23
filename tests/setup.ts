import { vi } from "vitest";

const eventMocks = vi.hoisted(() => ({
  listen: vi.fn(async () => () => undefined),
  emit: vi.fn(async () => undefined),
}));

const helpers = vi.hoisted(() => {
  const emptyArrayCmds = new Set<string>([
    "get_live_sessions",
    "get_session_history",
    "get_session_history_filtered",
    "get_sessions_by_hour_range",
    "search_sessions",
    "get_top_sessions",
    "get_inflection_points",
    "get_hourly_activity",
    "get_daily_stats",
    "get_recommendations",
    "get_sessions_context_usage",
    "get_context_breakdowns",
  ]);
  const emptyReportsBundle = {
    provider: "claude",
    capabilities: { cache_health: true, model_routing: true, extra_usage: true },
    days: 30,
    total_sessions: 0,
    recommendations: [],
    daily_costs: [],
    trace_overview: {
      provider: "claude",
      provider_display: "Claude Code",
      instruction_file: "CLAUDE.md",
      fix_button_label: "Fix with Claude",
      session_store: "",
      global_state_source: "",
      traced_sessions: 0,
      total_sessions: 0,
      user_messages: 0,
      assistant_messages: 0,
      total_tool_calls: 0,
      total_compactions: 0,
      mcp_tool_calls: 0,
      cache_hit_ratio: 0,
      top_tools: [],
      telemetry_mermaid: "",
      cache_mermaid: "",
    },
    tool_frequency: {
      available: false,
      sessions_analyzed: 0,
      traced_sessions: 0,
      total_tool_calls: 0,
      avg_tools_per_session: 0,
      avg_tool_calls_per_hour: 0,
      mcp_tool_calls: 0,
      mcp_share_pct: 0,
      compact_gap_sessions: 0,
      diagnosis: "",
      top_tools: [],
    },
    prompt_complexity: {
      available: false,
      sessions_analyzed: 0,
      prompts_analyzed: 0,
      avg_complexity_score: 0,
      avg_specificity_score: 0,
      high_complexity_sessions: 0,
      low_specificity_sessions: 0,
      diagnosis: "",
      top_sessions: [],
    },
    session_health: {
      available: false,
      sessions_analyzed: 0,
      health_score: 0,
      grade: "A",
      avg_duration_minutes: 0,
      p90_duration_minutes: 0,
      long_session_pct: 0,
      avg_messages_per_session: 0,
      peak_overlap_pct: 0,
      compact_gap_pct: 0,
      diagnosis: "",
    },
    cache_health: {
      grade: "A",
      grade_label: "Excellent",
      color: "#62b462",
      hit_ratio: 0,
      trend_weighted_ratio: 0,
      total_cache_read: 0,
      total_cache_write: 0,
      total_input: 0,
      sessions_analyzed: 0,
      diagnosis: "",
    },
    model_routing: {
      total_sessions: 0,
      priced_sessions: 0,
      total_cost: 0,
      cost_basis: "unavailable",
      cost_sources: [],
      opus: { sessions: 0, priced_sessions: 0, cost: 0, cost_share_pct: 0, avg_cost_per_session: 0 },
      sonnet: { sessions: 0, priced_sessions: 0, cost: 0, cost_share_pct: 0, avg_cost_per_session: 0 },
      haiku: { sessions: 0, priced_sessions: 0, cost: 0, cost_share_pct: 0, avg_cost_per_session: 0 },
      other: { sessions: 0, priced_sessions: 0, cost: 0, cost_share_pct: 0, avg_cost_per_session: 0 },
      estimated_savings_if_rerouted: 0,
      diagnosis: "",
    },
    inflection_points: [],
  };
  const currentUpdate = {
    current_version: "0.0.0-test",
    latest_version: null,
    update_available: false,
    release_name: null,
    release_notes: null,
    release_url: "https://github.com/xt0n1-t3ch/Pulse-Analytics/releases",
    published_at: null,
    checked_at: "2026-06-10T00:00:00Z",
    assets: [],
  };
  return {
    pulseInvoke: async (cmd: string): Promise<unknown> => {
      if (emptyArrayCmds.has(cmd)) return [];
      if (cmd === "get_db_size") return 0;
      if (cmd === "get_active_provider") return "claude";
      if (cmd === "get_discord_preview") {
        return {
          provider: "claude",
          app_name: "Claude Code",
          details: "Claude Code",
          state: "Waiting for session",
          has_session: false,
          duration_secs: 0,
        };
      }
      if (cmd === "get_reports_bundle") return emptyReportsBundle;
      if (cmd === "check_app_update") return currentUpdate;
      if (cmd === "open_app_release_page") return undefined;
      return undefined;
    },
  };
});

const fakeWindow = {
  minimize: async () => undefined,
  maximize: async () => undefined,
  unmaximize: async () => undefined,
  isMaximized: async () => false,
  close: async () => undefined,
};

if (typeof globalThis !== "undefined") {
  if (!globalThis.localStorage) {
    const storage = new Map<string, string>();
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: {
        getItem: (key: string) => storage.get(key) ?? null,
        setItem: (key: string, value: string) => storage.set(key, String(value)),
        removeItem: (key: string) => storage.delete(key),
        clear: () => storage.clear(),
        key: (index: number) => Array.from(storage.keys())[index] ?? null,
        get length() {
          return storage.size;
        },
      },
    });
  }
  const internals = {
    invoke: vi.fn(helpers.pulseInvoke),
    transformCallback: (cb: unknown) => {
      const id = Math.floor(Math.random() * 1e9);
      (globalThis as Record<string, unknown>)[`_${id}`] = cb;
      return id;
    },
    convertFileSrc: (p: string) => p,
    metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
  };
  const eventInternals = { unregisterListener: () => undefined };
  (globalThis as Record<string, unknown>).__TAURI_INTERNALS__ = internals;
  (globalThis as Record<string, unknown>).__TAURI_EVENT_PLUGIN_INTERNALS__ = eventInternals;
  if (typeof window !== "undefined") {
    const w = window as unknown as Record<string, unknown>;
    w.__TAURI_INTERNALS__ = internals;
    w.__TAURI_EVENT_PLUGIN_INTERNALS__ = eventInternals;
    w.__TAURI__ = { window: { getCurrentWindow: () => fakeWindow } };
  }
}

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(helpers.pulseInvoke),
  convertFileSrc: vi.fn((p: string) => p),
}));

vi.mock("@tauri-apps/api/event", () => eventMocks);

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn(async () => "0.0.0-test"),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => fakeWindow),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => null),
  save: vi.fn(async () => null),
  confirm: vi.fn(async () => true),
  message: vi.fn(async () => undefined),
}));

vi.mock("@tauri-apps/plugin-fs", () => ({
  writeTextFile: vi.fn(async () => undefined),
  readTextFile: vi.fn(async () => ""),
  exists: vi.fn(async () => false),
}));

vi.mock("chart.js/auto", () => {
  class Chart {
    static defaults = { color: "", borderColor: "" };
    data: { labels: unknown[]; datasets: { data: unknown[] }[] } = {
      labels: [],
      datasets: [{ data: [] }],
    };
    destroy() {}
    update() {}
    resize() {}
  }
  return { Chart, default: Chart };
});
vi.mock("chart.js", () => {
  class Chart {
    static register() {}
    destroy() {}
    update() {}
    resize() {}
  }
  return { Chart, registerables: [], default: Chart };
});

if (!("randomUUID" in globalThis.crypto)) {
  let counter = 0;
  Object.defineProperty(globalThis.crypto, "randomUUID", {
    value: () => `00000000-0000-4000-8000-${(counter++).toString().padStart(12, "0")}`,
  });
}

if (typeof Element !== "undefined") {
  Element.prototype.animate = function animateStub() {
    const anim: Record<string, unknown> = {
      onfinish: null,
      oncancel: null,
      cancel() {},
      finish() {},
      play() {},
      pause() {},
      reverse() {},
      persist() {},
      updatePlaybackRate() {},
      currentTime: 0,
      startTime: 0,
      playbackRate: 1,
      effect: null,
      playState: "finished",
      finished: Promise.resolve(),
    };
    return anim as unknown as Animation;
  } as typeof Element.prototype.animate;
}

if (typeof window !== "undefined" && !window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    addListener: () => undefined,
    removeListener: () => undefined,
    dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}
