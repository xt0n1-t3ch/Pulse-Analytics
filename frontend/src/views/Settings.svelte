<script lang="ts">
  import { onMount } from "svelte";
  import {
    health,
    rateLimits,
    planInfo,
    addToast,
    accessSnapshot,
    selectedAccessSourceId,
    selectedAnalyticsProviderScope,
    opencodeDiagnostics,
    refreshDiscordPresencePreview,
  } from "../lib/stores";
  import { provider, providerProfile, setProvider, PROVIDERS, type Provider } from "../lib/provider";
  import { planLabelForKey, planOptionsFor } from "../lib/plans";
  import { setPlanOverride, exportAllData, clearHistory, getDbSize, getPlanInfo, getAnalyticsSummary, getAppSettings, setCloseToTray } from "../lib/api";
  import type { AnalyticsSummary } from "../lib/api";
  import PulseMark from "../components/PulseMark.svelte";
  import OpenCodeMark from "../components/OpenCodeMark.svelte";
  import SegmentedControl from "../components/SegmentedControl.svelte";
  import Select from "../components/Select.svelte";
  import IconDownload from "@tabler/icons-svelte/icons/download";
  import IconRefresh from "@tabler/icons-svelte/icons/refresh";
  import IconTrash from "@tabler/icons-svelte/icons/trash";

  let {
    onToggleTheme,
    currentTheme,
  }: {
    onToggleTheme: () => void;
    currentTheme: string;
  } = $props();

  let planOverrideValue = $state("auto");
  let planSaving = $state(false);
  let planSavedFlash = $state(false);
  let planSavedTimer: ReturnType<typeof setTimeout> | null = null;
  let settingsError = $state<string | null>(null);
  let providerGeneration = 0;
  let planGeneration = 0;
  let analyticsGeneration = 0;
  let planMutation: Promise<void> = Promise.resolve();

  let closeToTray = $state(true);
  let closeToTraySaving = $state(false);
  let exportPending = $state(false);

  onMount(() => {
    getAppSettings()
      .then((settings) => { closeToTray = settings.close_to_tray; })
      .catch(() => {
        addToast("Could not load the window close setting. The default was kept.", "warning", 5000);
      });
  });

  async function handleCloseToTray(next: boolean): Promise<void> {
    if (next === closeToTray || closeToTraySaving) return;
    const previous = closeToTray;
    closeToTray = next;
    closeToTraySaving = true;
    try {
      await setCloseToTray(next);
    } catch (error) {
      closeToTray = previous;
      addToast(`Could not save the close preference: ${String(error)}`, "danger", 4000);
    } finally {
      closeToTraySaving = false;
    }
  }

  $effect(() => {
    if (!$planInfo) return;
    if ($planInfo.provider !== $provider) return;
    if (planSaving) return;
    // The backend returns a canonical plan key, so the select round-trips
    // directly without fragile label matching.
    planOverrideValue = $planInfo.detected ? "auto" : ($planInfo.plan_key || "auto");
  });

  async function handleProviderChange(val: string): Promise<void> {
    const nextProvider = val as Provider;
    if (nextProvider === $provider) return;
    const generation = ++providerGeneration;
    const providerPlanGeneration = ++planGeneration;
    settingsError = null;
    planSaving = false;
    planInfo.set(null);
    try {
      await setProvider(nextProvider);
    } catch {
      if (generation !== providerGeneration || providerPlanGeneration !== planGeneration) return;
      settingsError = "Provider selection could not be saved.";
      return;
    }
    try {
      const fresh = await getPlanInfo();
      if (generation !== providerGeneration || providerPlanGeneration !== planGeneration) return;
      if (fresh.provider === nextProvider) {
        planInfo.set(fresh);
      void refreshDiscordPresencePreview();
        return;
      }
      throw new Error("provider plan response did not match the selected provider");
    } catch {
      if (generation !== providerGeneration) return;
      settingsError = "Provider was saved, but its plan details could not be refreshed.";
    }
  }

  let providerOptions = $derived(
    Object.values(PROVIDERS).map((p) => ({ value: p.id, label: p.productName })),
  );

  let planOptions = $derived.by(() => {
    return [{ value: "auto", label: "Auto-detect" }, ...planOptionsFor($provider)];
  });

  let planLabelFor = $derived((key: string): string =>
    planOptions.find((o) => o.value === key)?.label ?? key,
  );

  let dbSizeBytes = $state<number | null>(null);
  let confirmClear = $state(false);
  let clearResult = $state<string | null>(null);
  let summary = $state<AnalyticsSummary | null>(null);
  let dataLoading = $state(true);
  let dataError = $state<string | null>(null);
  let clearPending = $state(false);

  async function loadLocalAnalytics(): Promise<void> {
    const generation = ++analyticsGeneration;
    const scope = $selectedAnalyticsProviderScope;
    dataLoading = true;
    dataError = null;
    summary = null;
    try {
      const [nextSize, nextSummary] = await Promise.all([
        getDbSize(),
        getAnalyticsSummary(scope),
      ]);
      if (generation !== analyticsGeneration || scope !== $selectedAnalyticsProviderScope) return;
      dbSizeBytes = nextSize;
      summary = nextSummary;
    } catch (error) {
      if (generation !== analyticsGeneration) return;
      dbSizeBytes = null;
      summary = null;
      dataError = error instanceof Error && error.message
        ? `Local analytics unavailable. ${error.message}`
        : "Local analytics unavailable. The database did not return a complete response.";
    } finally {
      if (generation === analyticsGeneration) dataLoading = false;
    }
  }

  onMount(() => {
    void loadLocalAnalytics();
  });
  let previousAnalyticsScope: string | undefined;
  $effect(() => {
    const scope = $selectedAnalyticsProviderScope;
    if (previousAnalyticsScope !== undefined && scope !== previousAnalyticsScope) {
      void loadLocalAnalytics();
    }
    previousAnalyticsScope = scope;
  });

  async function handlePlanChange(val: string): Promise<void> {
    const generation = ++planGeneration;
    const selectedProvider = $provider;
    const previousValue = planOverrideValue;
    const previousPlan = $planInfo;
    planOverrideValue = val;
    planSaving = true;
    settingsError = null;

    if ($planInfo) {
      if (val === "auto") {
        planInfo.set({ ...$planInfo, detected: true });
      } else {
        planInfo.set({ ...$planInfo, plan_key: val, plan_name: planLabelFor(val), detected: false });
      }
    }
    try {
      const mutation = planMutation.then(() =>
        (selectedProvider === "opencode" || selectedProvider === "commandcode") ? Promise.resolve() : setPlanOverride(val === "auto" ? "" : val, selectedProvider)
      );
      planMutation = mutation.catch(() => undefined);
      await mutation;
      const fresh = await getPlanInfo();
      if (generation !== planGeneration || selectedProvider !== $provider) return;
      if (fresh.provider !== selectedProvider) {
        throw new Error("plan response did not match the selected provider");
      }
      planInfo.set(fresh);
      void refreshDiscordPresencePreview();
      planSavedFlash = true;
      if (planSavedTimer) clearTimeout(planSavedTimer);
      planSavedTimer = setTimeout(() => { planSavedFlash = false; }, 1800);
    } catch {
      if (generation !== planGeneration) return;
      planOverrideValue = previousValue;
      planInfo.set(previousPlan);
      settingsError = "Plan override could not be saved.";
    } finally {
      if (generation === planGeneration) planSaving = false;
    }
  }

  function checkForUpdates(): void {
    window.dispatchEvent(new CustomEvent("pulse:check-updates"));
    addToast("Checking for updates…", "info");
  }

  function fmtBytes(b: number): string {
    if (b < 1024) return b + " B";
    if (b < 1024 * 1024) return (b / 1024).toFixed(1) + " KB";
    return (b / (1024 * 1024)).toFixed(1) + " MB";
  }

  async function handleExport(): Promise<void> {
    if (exportPending) return;
    exportPending = true;
    try {
      const data = await exportAllData($selectedAnalyticsProviderScope);
      const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `pulse-export-${new Date().toISOString().slice(0, 10)}.json`;
      a.click();
      URL.revokeObjectURL(url);
    } catch (error) {
      addToast(`Analytics export failed: ${String(error)}`, "danger", 5000);
    } finally {
      exportPending = false;
    }
  }

  async function handleClear(): Promise<void> {
    if (clearPending) return;
    clearPending = true;
    try {
      const deleted = await clearHistory($selectedAnalyticsProviderScope);
      clearResult = `Cleared ${deleted} sessions`;
      confirmClear = false;
      await loadLocalAnalytics();
      setTimeout(() => { clearResult = null; }, 3000);
    } catch (error) {
      dataError = error instanceof Error && error.message
        ? `Local analytics unavailable. ${error.message}`
        : "Local analytics unavailable. History could not be cleared.";
      addToast(`History clear failed: ${String(error)}`, "danger", 5000);
    } finally {
      clearPending = false;
    }
  }

  function friendlyPlatform(raw: string): string {
    const normalized = raw.toLowerCase();
    if (normalized.includes("mac") || normalized.includes("darwin")) return "macOS";
    if (normalized.startsWith("win") || normalized.includes("windows")) return "Windows";
    if (normalized.includes("linux") || normalized.includes("x11")) return "Linux";
    return raw;
  }

  let platformName = $derived.by(() => {
    const navigatorWithUserAgentData = navigator as Navigator & {
      userAgentData?: { platform?: string };
    };
    const raw = navigatorWithUserAgentData.userAgentData?.platform ?? navigator.platform;
    return friendlyPlatform(raw);
  });

  let discordStatus = $derived(($health?.discord_status ?? "—").toLowerCase());
  let discordTone = $derived(
    discordStatus.includes("connect") && !discordStatus.includes("dis") ? "ok"
    : discordStatus === "—" ? "muted"
    : "warn"
  );

  let sessionTotal = $derived(summary?.total_sessions ?? null);
  let activePlanInfo = $derived($planInfo?.provider === $provider ? $planInfo : null);
  let activePlanLabel = $derived.by(() => {
    if (!activePlanInfo?.plan_key) return "Not reported";
    return planLabelForKey($provider, activePlanInfo.plan_key) ?? "Not reported";
  });
  let isManual = $derived.by(() => !!activePlanInfo && !activePlanInfo.detected);
  let planStateLabel = $derived.by(() => {
    if (planSaving) return "Saving";
    if (!activePlanInfo) return "Detecting";
    return activePlanInfo.detected ? "Auto" : activePlanInfo.plan_key ? "Manual override" : "Not reported";
  });
</script>

<div class="settings-view app-view">
  <div class="view-header">
    <div class="settings-title">
      <h2 class="view-title">Settings</h2>
      <span class="version-chip">{$health?.version ? `v${$health.version}` : "Version unavailable"}</span>
    </div>
    <button type="button" class="btn check-updates-btn" onclick={checkForUpdates} aria-label="Check for application updates">
      <IconRefresh size={13} stroke={2.2} aria-hidden="true" />
      Check for updates
    </button>
  </div>

  <!-- IDENTITY — editorial masthead + control strip -->
  <section class="identity-card" style="--provider-accent: {$providerProfile.accent}">
    <div class="identity-top">
      <div class="it-lead">
        <div class="it-mark" aria-hidden="true">
          {#if $provider === "opencode"}<OpenCodeMark />{:else}<PulseMark size={28} />{/if}
        </div>
        <div class="it-text">
          <div class="it-line">
            <span class="it-product" style="color: {$providerProfile.accent}">{$providerProfile.productName}</span>
            {#if $provider !== "opencode" && $provider !== "commandcode"}<span class="it-sep">·</span>
            <span class="it-plan">{activePlanInfo ? activePlanLabel : "Detecting plan…"}</span>{/if}
          </div>
          <span class="it-sub">
            Broadcasting as <strong>{$providerProfile.label}</strong>
            <span class="it-dim">·</span>
            telemetry from <span class="mono">{$providerProfile.homeDir}</span>
          </span>
        </div>
      </div>
      <div class="settings-state" role="status">
        <span>{planSavedFlash ? "Plan saved" : isManual && $provider !== "opencode" && $provider !== "commandcode" ? "Using your selected plan" : ($provider === "opencode" || $provider === "commandcode") ? "Account details are available in Accounts" : "Plan detected from your account"}</span>
        <span>{$health?.discord_status === "Controlled by external daemon" ? "Desktop app controls publication" : $health?.discord_status === "Connected" ? "Discord connected" : $health?.discord_status?.startsWith("Waiting for OpenCode") ? "Waiting for OpenCode session" : "Discord connection pending"}</span>
      </div>
    </div>

    <div class="rail">
      <div class="rail-ctrl rail-ctrl-select">
        <span class="rail-k">Provider</span>
        <Select
          value={$provider}
          options={providerOptions}
          onchange={handleProviderChange}
          ariaLabel="Active provider"
        />
      </div>

      {#if $provider !== "opencode" && $provider !== "commandcode"}
      <div class="rail-ctrl rail-ctrl-select">
        <span class="rail-k">Plan override</span>
        <Select
          value={planOverrideValue}
          options={planOptions}
          onchange={handlePlanChange}
          ariaLabel="Plan override"
        />
      </div>
      {/if}

      <div class="rail-ctrl">
        <span class="rail-k">Appearance</span>
        <SegmentedControl
          value={currentTheme}
          options={[{ value: "dark", label: "Dark" }, { value: "light", label: "Light" }]}
          onchange={(next) => { if (next !== currentTheme) onToggleTheme(); }}
          ariaLabel="Theme"
          size="md"
        />
      </div>

      <div class="rail-ctrl">
        <span class="rail-k">Window close</span>
        <SegmentedControl
          value={closeToTray ? "tray" : "quit"}
          options={[{ value: "tray", label: "Minimize to tray" }, { value: "quit", label: "Quit" }]}
          onchange={(next) => handleCloseToTray(next === "tray")}
          ariaLabel="Window close behavior"
          disabled={closeToTraySaving}
          size="md"
        />
      </div>
    </div>
    {#if settingsError}
      <div class="settings-error" role="alert">{settingsError}</div>
    {/if}
  </section>

  {#if $provider === "opencode"}
    <section class="s-card" aria-label="OpenCode connection status">
      <header class="s-card-head"><div class="head-text"><h3 class="s-card-title">OpenCode local sessions</h3>
        <p class="s-card-desc">OpenChamber, OpenCode Desktop and CLI share local session storage. Account quotas are not inferred.</p></div></header>
      <div class="s-rows"><div class="s-row"><span class="s-label">Configuration</span><span class="s-desc">~/.claude/pulse-opencode.json · database_paths accepts additional SQLite files.</span></div>
      {#each $opencodeDiagnostics as diagnostic}<p class="settings-error" role="status">{diagnostic}</p>{/each}
      {#if $opencodeDiagnostics.length === 0}<p class="s-row">No OpenCode reader errors reported.</p>{/if}</div>
    </section>
  {/if}
  <div class="settings-grid">
    <section class="s-card">
      <header class="s-card-head">
        <div class="head-accent" aria-hidden="true"></div>
        <div class="head-text">
          <h3 class="s-card-title">Data sources</h3>
          <p class="s-card-desc">Where Pulse reads session, usage, and presence signals from.</p>
        </div>
      </header>
      <div class="s-rows">
        <div class="s-row">
          <div class="s-info">
            <span class="s-label">Sessions Directory</span>
            <span class="s-desc">Local transcripts parsed by Pulse.</span>
          </div>
          <span class="s-value mono truncate">{$providerProfile.sessionsPath}</span>
        </div>
        <div class="s-row">
          <div class="s-info">
            <span class="s-label">Rate Limit Source</span>
            <span class="s-desc">How usage quotas are fetched.</span>
          </div>
          <span class="s-value mono truncate">{$provider === "opencode"
            ? $accessSnapshot?.routes.some((route) => route.source.kind === "open_code_go" && route.source.proof === "quota_response" && route.availability === "available" && route.freshness === "fresh")
              ? "OpenCode Go usage API" : "OpenCode Go quotas unavailable"
            : $rateLimits?.source ?? "—"}</span>
        </div>
        <div class="s-row">
          <div class="s-info">
            <span class="s-label">Instruction File</span>
            <span class="s-desc">Top-level memory file read by {$providerProfile.productName}.</span>
          </div>
          <span class="s-value mono">{$providerProfile.instructionFile}</span>
        </div>
      </div>
    </section>

    <section class="s-card">
      <header class="s-card-head">
        <div class="head-accent" aria-hidden="true"></div>
        <div class="head-text">
          <h3 class="s-card-title">Data management</h3>
          <p class="s-card-desc">Export or reset the local analytics database. Destructive actions are irreversible.</p>
        </div>
      </header>
      <div class="dm-body">
        {#if dataError}
          <div class="dm-error" role="alert">
            <span>Local analytics unavailable. {dataError.replace(/^Local analytics unavailable\.\s*/, "")}</span>
            <button type="button" onclick={loadLocalAnalytics}>Retry</button>
          </div>
        {/if}
        <div class="dm-stats">
          <div class="dm-stat">
            <span class="dm-key">Database</span>
            <span class="dm-val mono">{dataLoading ? "Loading…" : dbSizeBytes === null ? "Unavailable" : fmtBytes(dbSizeBytes)}</span>
            <span class="dm-sub mono">pulse-analytics.db</span>
          </div>
          <div class="dm-stat">
            <span class="dm-key">Sessions</span>
            <span class="dm-val">{dataLoading ? "Loading…" : sessionTotal === null ? "Unavailable" : sessionTotal.toLocaleString()}</span>
            <span class="dm-sub">tracked locally</span>
          </div>
        </div>
        <div class="dm-actions">
          <button class="btn" onclick={handleExport} disabled={dataLoading || !!dataError || exportPending}>
            <IconDownload size={12} stroke={2.2} aria-hidden="true" />
            {exportPending ? "Exporting…" : "Export JSON"}
          </button>
          {#if confirmClear}
            <button class="btn btn-danger" onclick={handleClear} disabled={clearPending}>
              {clearPending ? "Clearing…" : "Confirm clear"}
            </button>
            <button class="btn btn-ghost" onclick={() => confirmClear = false}>Cancel</button>
          {:else}
            <button class="btn btn-danger" onclick={() => confirmClear = true} disabled={dataLoading || !!dataError}>
              <IconTrash size={12} stroke={2.2} aria-hidden="true" />
              Clear history
            </button>
          {/if}
        </div>
      </div>
      {#if clearResult}
        <div class="clear-result">{clearResult}</div>
      {/if}
    </section>
  </div>

  <div class="meta-strip">
    <div class="meta-cell">
      <span class="meta-key">Engine</span>
      <span class="meta-val">cc-discord-presence</span>
    </div>
    <div class="meta-cell">
      <span class="meta-key">Runtime</span>
      <span class="meta-val">Tauri 2.0 · Svelte 5</span>
    </div>
    <div class="meta-cell">
      <span class="meta-key">Platform</span>
      <span class="meta-val mono">{platformName}</span>
    </div>
  </div>
</div>

<style>
  .settings-view {
    display: flex;
    flex-direction: column;
    gap: var(--page-gap);
    width: 100%;
    animation: fadeIn 0.3s var(--ease-out);
  }

  .view-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 24px;
    flex-wrap: wrap;
  }
  .settings-title { display: flex; align-items: center; gap: 10px; }
  .version-chip { padding: 3px 8px; color: var(--text-muted); border: 1px solid var(--border); border-radius: var(--radius-full); font: 600 10px var(--font-mono); }
  .view-title {
    font-size: var(--fs-2xl);
    font-weight: 700;
    letter-spacing: var(--letter-tighter);
    color: var(--text-primary);
  }
  .check-updates-btn {
    flex-shrink: 0;
    padding: 7px 12px;
    font-size: var(--fs-sm);
    font-weight: 500;
  }

  /* ── IDENTITY — flat, Dashboard-aligned; no overflow clip so portal menus escape ── */
  .identity-card {
    position: relative;
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    transition: border-color 0.18s var(--ease);
  }
  .identity-card:hover { border-color: var(--border-hover); }

  .identity-top {
    display: grid;
    grid-template-columns: 1fr auto;
    align-items: center;
    gap: 18px;
    padding: 22px 24px 20px;
  }
  .it-lead {
    display: flex;
    align-items: center;
    gap: 14px;
    min-width: 0;
  }
  @media (max-width: 760px) {
    .view-header { flex-direction: column; gap: 12px; }
    .check-updates-btn { width: 100%; justify-content: center; }
    .identity-top { grid-template-columns: 1fr; }
    }

  .it-mark {
    width: 40px;
    height: 40px;
    border-radius: var(--radius-sm);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    color: var(--provider-accent);
    flex-shrink: 0;
  }
  .it-text { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
  .it-line {
    display: inline-flex;
    align-items: baseline;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 19px;
    font-weight: 700;
    letter-spacing: var(--letter-tight);
    line-height: 1.15;
    color: var(--text-primary);
  }
  .it-product { font-weight: 700; }
  .it-sep { color: var(--border-strong); font-weight: 400; }
  .it-plan { color: var(--text-secondary); font-weight: 500; }
  .it-sub {
    font-size: 13px;
    color: var(--text-secondary);
    line-height: var(--lh-snug);
  }
  .it-sub strong { font-weight: 600; color: var(--text-secondary); }
  .it-sub .it-dim { margin: 0 5px; color: var(--border-strong); }
  .it-sub .mono { font-family: var(--font-mono); font-size: 11px; color: var(--text-secondary); }


  .rail {
    position: relative;
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    border-top: 1px solid var(--border);
    background: var(--panel-sheen), var(--surface-panel);
    border-bottom-left-radius: var(--radius-lg);
    border-bottom-right-radius: var(--radius-lg);
    overflow: visible;
  }
  @media (max-width: 900px) {
    .rail { grid-template-columns: 1fr 1fr; }
  }
  @media (max-width: 560px) {
    .rail { grid-template-columns: 1fr; }
  }
  .rail-ctrl {
    display: flex;
    flex-direction: column;
    gap: 9px;
    padding: 16px 22px 18px;
    border-left: 1px solid var(--border);
    min-width: 0;
  }
  .rail-ctrl:first-child { border-left: none; }
  @media (max-width: 900px) {
    .rail-ctrl:nth-child(2n+1) { border-left: none; }
    .rail-ctrl:nth-child(n+3) { border-top: 1px solid var(--border); }
  }
  @media (max-width: 560px) {
    .rail-ctrl { border-left: none; }
    .rail-ctrl + .rail-ctrl { border-top: 1px solid var(--border); }
  }
  .rail-k {
    font-family: var(--font-mono);
    font-size: 9.5px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: var(--letter-wider);
    color: var(--text-muted);
  }

  .rail-ctrl :global(.segmented) { width: 100%; }
  .rail-ctrl :global(.seg-opt) { flex: 1; justify-content: center; }

  /* ── sub-cards ── */
  .settings-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
    align-items: start;
  }
  @media (max-width: 900px) {
    .settings-grid { grid-template-columns: 1fr; }
  }

  .s-card {
    position: relative;
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    display: flex;
    flex-direction: column;
    transition: border-color 0.18s var(--ease);
    overflow: hidden;
  }
  .s-card:hover { border-color: var(--border-hover); }

  .s-card-head {
    position: relative;
    padding: 16px 20px 14px;
    border-bottom: 1px solid var(--border);
  }
  .head-accent { display: none; }
  .head-text { min-width: 0; }
  .s-card-title {
    font-size: var(--fs-md);
    font-weight: 600;
    letter-spacing: var(--letter-tight);
    color: var(--text-primary);
    margin: 0 0 3px;
  }
  .s-card-desc {
    font-size: 13px;
    color: var(--text-secondary);
    line-height: var(--lh-snug);
    margin: 0;
  }

  .s-rows { display: flex; flex-direction: column; }
  .s-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 14px;
    padding: 14px 20px;
    border-top: 1px solid var(--border);
    min-height: 56px;
    transition: background 0.15s var(--ease);
  }
  .s-row:first-child { border-top: none; }
  .s-row:hover { background: var(--bg-card-hover); }

  .s-info { display: flex; flex-direction: column; gap: 3px; min-width: 0; flex: 1; }
  .s-label {
    font-size: var(--fs-base);
    font-weight: 500;
    color: var(--text-primary);
    letter-spacing: var(--letter-tight);
  }
  .s-desc {
    font-size: 13px;
    color: var(--text-secondary);
    line-height: var(--lh-snug);
  }

  .s-value {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 5px 10px;
    font-size: var(--fs-sm);
    color: var(--text-secondary);
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    white-space: nowrap;
    max-width: 60%;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .s-value.mono { font-family: var(--font-mono); font-size: 11px; }
  .s-value.truncate { min-width: 0; }

  /* ── data management ── */
  .dm-body {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 18px;
    padding: 20px;
    flex: 1;
  }
  .dm-error {
    flex: 0 0 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 14px;
    padding-bottom: 14px;
    color: var(--danger);
    font-size: var(--fs-xs);
    border-bottom: 1px solid color-mix(in srgb, var(--danger) 30%, var(--border));
  }
  .dm-error button {
    flex: 0 0 auto;
    padding: 5px 10px;
    color: var(--text-primary);
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }
  .dm-stats { display: flex; gap: 24px; align-items: stretch; flex-wrap: wrap; }
  .dm-stat {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    padding-right: 24px;
    border-right: 1px solid var(--border);
  }
  .dm-stat:last-child { border-right: none; padding-right: 0; }
  .dm-key {
    font-family: var(--font-mono);
    font-size: 9.5px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: var(--letter-wider);
    color: var(--text-muted);
  }
  .dm-val {
    font-size: 26px;
    font-weight: 700;
    color: var(--text-primary);
    font-variant-numeric: tabular-nums;
    letter-spacing: -0.02em;
    line-height: 1.05;
  }
  .dm-val.mono { font-family: var(--font-mono); }
  .dm-sub { font-size: 11px; color: var(--text-muted); }
  .dm-sub.mono { font-family: var(--font-mono); }
  .dm-actions { display: inline-flex; gap: 6px; flex-wrap: wrap; justify-content: flex-end; }
  .dm-actions button:disabled { cursor: not-allowed; opacity: 0.45; }
  @media (max-width: 560px) {
    .s-row { align-items: flex-start; flex-direction: column; }
    .s-value { max-width: 100%; }
    .dm-body { flex-direction: column; align-items: stretch; }
    .dm-actions { justify-content: stretch; }
    .dm-actions button { flex: 1; }
  }
  .settings-error {
    padding: 9px 22px;
    color: var(--danger);
    background: var(--danger-dim);
    border-top: 1px solid color-mix(in srgb, var(--danger) 32%, var(--border));
    border-bottom-left-radius: var(--radius-lg);
    border-bottom-right-radius: var(--radius-lg);
    font-size: var(--fs-xs);
    font-weight: 600;
  }

  .clear-result {
    padding: 9px 20px;
    font-size: var(--fs-sm);
    color: var(--success);
    font-weight: 500;
    background: var(--success-dim);
    border-top: 1px solid color-mix(in srgb, var(--success) 25%, var(--border));
  }

  /* ── meta footer strip ── */
  .meta-strip {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 0;
    padding: 2px 0 0;
    border-top: 1px dashed var(--border);
    margin-top: 4px;
  }
  @media (max-width: 720px) {
    .meta-strip { grid-template-columns: 1fr; }
  }
  .meta-cell {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 12px 20px 0;
    border-right: 1px dashed var(--border);
  }
  .meta-cell:last-child { border-right: none; }
  @media (max-width: 720px) {
    .meta-cell { border-right: none; }
  }
  .meta-key {
    font-family: var(--font-mono);
    font-size: 9.5px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: var(--letter-wider);
    color: var(--text-muted);
  }
  .meta-val {
    font-size: var(--fs-sm);
    font-weight: 500;
    color: var(--text-secondary);
  }
  .meta-val.mono { font-family: var(--font-mono); font-size: var(--fs-xs); }

  .dm-actions :global(.btn) {
    padding: 7px 12px;
    font-size: var(--fs-sm);
    font-weight: 500;
  }
  .it-line { flex-wrap: wrap; }
  @media (max-width: 620px) {
    .identity-top, .rail-ctrl { padding: 16px; }
    .it-sub, .s-value, .settings-error { white-space: normal; overflow-wrap: anywhere; }
    .s-card-head, .s-row { padding-inline: 16px; }
    .meta-strip { gap: 10px; }
  }
  .settings-state { display:grid; gap:5px; color:var(--text-secondary); text-align:right; font-size:12px; line-height:1.5; }
  @media(max-width:760px) { .settings-state { text-align:left; } }
</style>
