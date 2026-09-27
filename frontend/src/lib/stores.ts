import { writable, derived } from "svelte/store";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  HealthResponse,
  MetricsResponse,
  SessionInfo,
  RateLimitInfo,
  DiscordUserInfo,
  DiscordPresencePreview,
  DiscordSettings,
  DiscordDisplayPrefs,
  PlanInfo,
  AppSnapshot,
} from "./api";
import {
  getAppSnapshot,
  getDiscordPreview,
  getDiscordSettings,
  getDiscordUser,
  hasTauriIpc,
} from "./api";
import {
  authenticatedAccessRoutes,
  displayableAccessRoutes,
  type AnalyticsProviderScope,
  type AccessRouteSnapshot,
  type AccessSnapshot,
} from "./access";

export const health = writable<HealthResponse | null>(null);
export const metrics = writable<MetricsResponse | null>(null);
export const sessions = writable<SessionInfo[]>([]);
export const rateLimits = writable<RateLimitInfo | null>(null);
export const discordUser = writable<DiscordUserInfo | null>(null);
export const discordPresencePreview = writable<DiscordPresencePreview | null>(null);
export const discordSettings = writable<DiscordSettings | null>(null);
export const planInfo = writable<PlanInfo | null>(null);
export const accessSnapshot = writable<AccessSnapshot | null>(null);
export const backendConnection = writable<"connecting" | "live" | "disconnected">("connecting");
export const selectedAccessSourceId = writable<string>("all");
export const sourceInspectorExpanded = writable(false);
export const selectedAnalyticsProviderScope = writable<AnalyticsProviderScope>("all");
export const opencodeDiagnostics = writable<string[]>([]);
export const orionDiagnostics = writable<string[]>([]);

export const selectedAccessRoutes = derived(
  [accessSnapshot, selectedAccessSourceId],
  ([$snapshot, $selectedId]): AccessRouteSnapshot[] => {
    const routes = authenticatedAccessRoutes($snapshot?.routes ?? []);
    if ($selectedId === "all") return routes;
    return routes.filter((route) => route.source.id === $selectedId);
  },
);
/** Diagnostic routes include failed probes so Source health can explain why a
 *  provider is unavailable. They remain separate from proof-gated selectors
 *  and allowance cards, which must never promote a configured key to a source. */
export const selectedAccessDiagnostics = derived(
  [accessSnapshot, selectedAccessSourceId],
  ([$snapshot, $selectedId]): AccessRouteSnapshot[] => {
    const routes = $snapshot?.routes ?? [];
    if ($selectedId === "all") return routes;
    return routes.filter((route) => route.source.id === $selectedId);
  },
);
export const currentView = writable<string>("dashboard");

export interface DiscordPreviewSettings {
  showProject: boolean;
  showBranch: boolean;
  showModel: boolean;
  showActivity: boolean;
  showTokens: boolean;
  showCost: boolean;
  showLimits: boolean;
  showCredits: boolean;
  showContext: boolean;
  showSystems: boolean;
}

export const discordPreview = writable<DiscordPreviewSettings>({
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

export function displayPrefsToPreview(prefs: DiscordDisplayPrefs): DiscordPreviewSettings {
  return {
    showProject: prefs.show_project,
    showBranch: prefs.show_branch,
    showModel: prefs.show_model,
    showActivity: prefs.show_activity,
    showTokens: prefs.show_tokens,
    showCost: prefs.show_cost,
    showLimits: prefs.show_limits,
    showCredits: prefs.show_credits,
    showContext: prefs.show_context,
    showSystems: prefs.show_systems,
  };
}

export function previewToDisplayPrefs(preview: DiscordPreviewSettings): DiscordDisplayPrefs {
  return {
    show_project: preview.showProject,
    show_branch: preview.showBranch,
    show_model: preview.showModel,
    show_activity: preview.showActivity,
    show_tokens: preview.showTokens,
    show_cost: preview.showCost,
    show_limits: preview.showLimits,
    show_credits: preview.showCredits,
    show_context: preview.showContext,
    show_systems: preview.showSystems,
  };
}

export function applyDiscordSettings(settings: DiscordSettings): void {
  discordSettings.set(settings);
  discordPreview.set(displayPrefsToPreview(settings.display_prefs));
}

export async function loadDiscordSettings(): Promise<DiscordSettings> {
  const settings = await getDiscordSettings();
  applyDiscordSettings(settings);
  return settings;
}

export interface Toast {
  id: number;
  message: string;
  type: "info" | "warning" | "danger" | "success";
}

let toastId = 0;
export const toasts = writable<Toast[]>([]);

export function addToast(
  message: string,
  type: Toast["type"] = "info",
  duration = 5000,
): void {
  const id = ++toastId;
  toasts.update((t) => [...t.slice(-2), { id, message, type }]);
  setTimeout(() => {
    toasts.update((t) => t.filter((x) => x.id !== id));
  }, duration);
}

export const activeSessions = derived(sessions, ($s) =>
  $s.filter((s) => !s.is_idle),
);

let snapshotSequence = 0;
let pollInFlight: Promise<void> | null = null;
let pollPending = false;

export function poll(): Promise<void> {
  if (pollInFlight) {
    pollPending = true;
    return pollInFlight;
  }
  const startedAtSequence = snapshotSequence;
  pollInFlight = getAppSnapshot()
    .then((snapshot) => {
      // A push event received after this poll began is newer than the response.
      if (startedAtSequence === snapshotSequence) applySnapshot(snapshot);
    })
    .catch((error) => {
      // Stale-while-revalidate: keep the last coherent snapshot on screen, but
      // revoke its live status. Provider changes still clear because crossing
      // that trust boundary must never show one provider as another.
      if (startedAtSequence === snapshotSequence) backendConnection.set("disconnected");
      console.warn("Snapshot error:", error);
    })
    .finally(() => {
      pollInFlight = null;
      if (pollPending) {
        pollPending = false;
        void poll();
      }
    });
  return pollInFlight;
}

/** Provider selection changes invalidate every proof-bound live field before
 *  the replacement snapshot is requested. Incrementing the sequence also
 *  prevents an already-running poll for the prior provider from winning. */
export function invalidateLiveSnapshotForProviderChange(): void {
  snapshotSequence++;
  clearLiveSnapshot("connecting");
}

function applySnapshot(snapshot: AppSnapshot): void {
    snapshotSequence++;
    backendConnection.set(snapshot.sync_state === "syncing" ? "connecting" : "live");
    health.set(snapshot.health);
    metrics.set(snapshot.metrics);
    sessions.set(snapshot.sessions);
    discordPresencePreview.set(snapshot.discord_preview);
    rateLimits.set(snapshot.rate_limits);
    planInfo.set(snapshot.plan);
    accessSnapshot.set(snapshot.access);
  opencodeDiagnostics.set(snapshot.opencode_diagnostics ?? []);
  orionDiagnostics.set(snapshot.orion_diagnostics ?? []);
    applyDiscordSettings(snapshot.discord_settings);
}

/** Clears provider-bound state when changing trust domains. Transient polling
 * failures use stale-while-revalidate instead and mark the retained snapshot
 * disconnected, so tables and cards do not blink out between retries. */
function clearLiveSnapshot(
  connection: "connecting" | "disconnected" = "disconnected",
): void {
  backendConnection.set(connection);
  health.set(null);
  metrics.set(null);
  sessions.set([]);
  discordPresencePreview.set(null);
  rateLimits.set(null);
  planInfo.set(null);
  accessSnapshot.set(null);
}

export async function refreshDiscordPresencePreview(): Promise<void> {
  try {
    discordPresencePreview.set(await getDiscordPreview());
  } catch (e) {
    console.warn("Discord preview:", e);
  }
}

let discordUserRequestSequence = 0;
let discordUserAppliedSequence = 0;

export async function loadDiscordUser(): Promise<void> {
  const requestSequence = ++discordUserRequestSequence;
  try {
    const user = await getDiscordUser();
    // A completed stale response stays usable when its newer sibling failed.
    if (requestSequence > discordUserAppliedSequence) {
      discordUserAppliedSequence = requestSequence;
      discordUser.set(user);
    }
  } catch (e) {
    console.warn("Discord user:", e);
  }
}

let snapshotUnlisten: Promise<UnlistenFn> | null = null;
let snapshotPollTimer: ReturnType<typeof setInterval> | null = null;
let snapshotSyncGeneration = 0;

/** How often the browser fallback re-reads the snapshot. Matches the backend
 *  poll interval, so the reviewed UI moves at the same cadence as the app. */
const SNAPSHOT_POLL_MS = 5000;

/**
 * Polls for snapshots when push events are unavailable.
 *
 * Tauri's event transport only exists inside the Pulse webview. Opened in a
 * plain browser for UI review, `listen` throws and the UI would otherwise show
 * a single frozen snapshot for the rest of the session.
 */
function startSnapshotPolling(): void {
  if (snapshotPollTimer) return;
  void poll();
  snapshotPollTimer = setInterval(() => void poll(), SNAPSHOT_POLL_MS);
}

export function startSnapshotSync(): void {
  if (snapshotUnlisten) return;
  const generation = ++snapshotSyncGeneration;
  if (!hasTauriIpc()) {
    startSnapshotPolling();
    return;
  }
  snapshotUnlisten = listen<AppSnapshot>("pulse://snapshot", (event) => {
    applySnapshot(event.payload);
  });
  void snapshotUnlisten
    .then((unlisten) => {
      if (generation !== snapshotSyncGeneration) {
        unlisten();
        return;
      }
      return poll();
    })
    .catch((error) => {
      if (generation !== snapshotSyncGeneration) return;
      snapshotUnlisten = null;
      console.warn("Snapshot listener:", error);
      startSnapshotPolling();
    });
}

export function stopSnapshotSync(): void {
  snapshotSyncGeneration++;
  snapshotSequence++;
  pollPending = false;
  if (snapshotPollTimer) {
    clearInterval(snapshotPollTimer);
    snapshotPollTimer = null;
  }
  const pendingUnlisten = snapshotUnlisten;
  snapshotUnlisten = null;
  void pendingUnlisten?.then((unlisten) => unlisten());
}
