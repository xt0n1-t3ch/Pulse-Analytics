<script lang="ts">
  import { onMount } from "svelte";
  import { fly } from "svelte/transition";
  import { addToast } from "../lib/stores";
  import { checkAppUpdate, openAppReleasePage, type AppUpdateAsset, type AppUpdateInfo } from "../lib/api";

  const SKIP_KEY = "pulse-update-skipped-version";
  const CHECK_EVENT = "pulse:check-updates";
  const FAKE_PARAM = "fakeUpdate";
  const RELEASE_TAG_BASE =
    "https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/tag/";

  let info = $state<AppUpdateInfo | null>(null);
  let visible = $state(false);
  let notesOpen = $state(false);
  let opening = $state(false);

  /** In-app install lifecycle. `idle` also covers "not attempted yet". */
  type InstallPhase = "idle" | "downloading" | "installing" | "relaunching" | "failed" | "relaunch-failed";
  let phase = $state<InstallPhase>("idle");
  let downloaded = $state(0);
  let downloadTotal = $state(0);
  let installError = $state<string | null>(null);

  let progressPct = $derived(
    downloadTotal > 0 ? Math.min(100, (downloaded / downloadTotal) * 100) : 0,
  );
  let busy = $derived(phase === "downloading" || phase === "installing" || phase === "relaunching");

  /** Human label for the release age, e.g. "today", "3d ago". */
  function publishedLabel(iso: string | null): string | null {
    if (!iso) return null;
    const then = new Date(iso).getTime();
    if (Number.isNaN(then)) return null;
    const days = Math.floor((Date.now() - then) / 86_400_000);
    if (days <= 0) return "released today";
    if (days === 1) return "released yesterday";
    if (days < 30) return `released ${days}d ago`;
    const months = Math.floor(days / 30);
    return `released ${months}mo ago`;
  }

  function formatSize(bytes: number): string {
    if (bytes <= 0) return "";
    const mb = bytes / 1_048_576;
    return mb >= 1 ? `${mb.toFixed(1)} MB` : `${Math.round(bytes / 1024)} KB`;
  }

  /** Best-guess host platform, used to surface one relevant installer. */
  function hostPlatform(): string | null {
    if (typeof navigator === "undefined") return null;
    const ua = `${navigator.userAgent} ${navigator.platform ?? ""}`.toLowerCase();
    if (ua.includes("win")) return "windows";
    if (ua.includes("mac")) return "macos";
    if (ua.includes("linux") || ua.includes("x11")) return "linux";
    return null;
  }

  let severity = $derived(info?.severity ?? "patch");
  let releaseAge = $derived(publishedLabel(info?.published_at ?? null));
  /** The single installer worth offering: this host's, when we can tell. */
  let hostAsset = $derived.by((): AppUpdateAsset | null => {
    const assets = info?.assets ?? [];
    const installers = assets.filter((a) => a.platform !== null);
    if (installers.length === 0) return null;
    const host = hostPlatform();
    return installers.find((a) => a.platform === host) ?? null;
  });
  /** Release notes trimmed of markdown noise for a compact preview. */
  let notesPreview = $derived.by((): string[] => {
    const raw = info?.release_notes ?? "";
    return raw
      .split("\n")
      .map((l) => l.replace(/^[\s*\-#]+/, "").trim())
      .filter((l) => l.length > 0)
      .slice(0, 3);
  });

  function skippedVersion(): string | null {
    return localStorage.getItem(SKIP_KEY);
  }

  function fakeVersionParam(): string | null {
    if (typeof window === "undefined") return null;
    const value = new URLSearchParams(window.location.search).get(FAKE_PARAM);
    return value && value.trim() ? value.trim() : null;
  }

  function synthFakeUpdate(version: string): AppUpdateInfo {
    const tag = version.replace(/^v/i, "");
    // The fake lane has no real installed version to diff against, so grade
    // the tag against the current 1.x line. Keeps ?fakeUpdate=2.0.0 rendering
    // as a major update rather than a hardcoded severity.
    const [major = 0, minor = 0] = tag.split(".").map((p) => Number.parseInt(p, 10) || 0);
    const severity = major > 1 ? "major" : minor > 0 ? "minor" : "patch";
    return {
      current_version: "dev",
      latest_version: version,
      update_available: true,
      release_name: `Pulse ${version}`,
      release_notes: "Simulated update from the local fakeUpdate lane.",
      release_url: `${RELEASE_TAG_BASE}v${tag}`,
      published_at: null,
      checked_at: new Date().toISOString(),
      assets: [],
      severity,
    };
  }

  async function fetchUpdate(): Promise<AppUpdateInfo | null> {
    const fake = fakeVersionParam();
    if (fake) return synthFakeUpdate(fake);
    return checkAppUpdate();
  }

  async function runCheck(force: boolean): Promise<void> {
    let next: AppUpdateInfo | null = null;
    try {
      next = await fetchUpdate();
    } catch {
      // Only the explicit manual check pays for a failure message; the
      // startup probe stays silent.
      if (force) addToast("Couldn't reach the update service.", "warning", 5000);
      return;
    }
    if (!next || !next.update_available || !next.latest_version) {
      if (force) addToast("You're already up to date.", "info", 4000);
      return;
    }
    if (!force && next.latest_version === skippedVersion()) return;
    info = next;
    notesOpen = false;
    visible = true;
  }

  function later(): void {
    visible = false;
  }

  function skipVersion(): void {
    if (info?.latest_version) {
      localStorage.setItem(SKIP_KEY, info.latest_version);
    }
    visible = false;
  }

  async function openRelease(): Promise<void> {
    if (!info) return;
    opening = true;
    try {
      await openAppReleasePage(info.release_url);
    } catch {
    } finally {
      opening = false;
    }
  }

  /**
   * Download and install the update in place, then restart.
   *
   * Runs through `@tauri-apps/plugin-updater`, which verifies the release
   * signature against the public key in `tauri.conf.json` before installing.
   * The plugin only resolves an update when the app is running as a bundled,
   * signed build, so in a browser or an unsigned dev build this fails cleanly
   * and we fall back to opening the release page rather than pretending the
   * install happened.
   */
  async function installUpdate(): Promise<void> {
    if (busy) return;
    installError = null;
    phase = "downloading";
    downloaded = 0;
    downloadTotal = 0;

    try {
      const { check } = await import("@tauri-apps/plugin-updater");
      const update = await check();
      if (!update) {
        // Nothing to install through the updater channel; hand off to GitHub.
        phase = "idle";
        await openRelease();
        return;
      }

      await update.downloadAndInstall((event) => {
        if (event.event === "Started") {
          downloadTotal = event.data.contentLength ?? 0;
        } else if (event.event === "Progress") {
          downloaded += event.data.chunkLength;
        } else if (event.event === "Finished") {
          phase = "installing";
        }
      });
      phase = "relaunching";
      // The Update click is the explicit effect checkpoint. Relaunch only
      // after the signed updater reports a successful install.
      const { relaunch } = await import("@tauri-apps/plugin-process");
      try {
        await relaunch();
      } catch (err) {
        phase = "relaunch-failed";
        installError = String(err);
      }
    } catch (err) {
      phase = "failed";
      installError = String(err);
    }
  }

  async function retryRelaunch(): Promise<void> {
    if (busy) return;
    installError = null;
    phase = "relaunching";
    try {
      const { relaunch } = await import("@tauri-apps/plugin-process");
      await relaunch();
    } catch (err) {
      phase = "relaunch-failed";
      installError = String(err);
    }
  }

  onMount(() => {
    void runCheck(false);
    const onForce = (): void => {
      runCheck(true);
    };
    window.addEventListener(CHECK_EVENT, onForce);
    return () => {
      window.removeEventListener(CHECK_EVENT, onForce);
    };
  });
</script>

{#if visible && info}
  <aside
    class="update-pop"
    class:sev-major={severity === "major"}
    class:sev-minor={severity === "minor"}
    role="status"
    aria-live="polite"
    aria-label="Application update available"
    in:fly={{ y: 16, duration: 320 }}
  >
    <header class="up-head">
      <span class="up-title">Update available</span>
      <span class="up-badge">{severity === "major" ? "Major" : severity === "minor" ? "Feature" : "Patch"} update</span>
      {#if releaseAge}
        <span class="up-age">{releaseAge}</span>
      {/if}
    </header>

    <div class="up-version" aria-label="Version change">
      <span class="up-from mono">{info.current_version}</span>
      <svg class="up-arrow" viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
        <path d="M5 12h13M13 6l6 6-6 6" />
      </svg>
      <span class="up-to mono">{info.latest_version}</span>
    </div>

    {#if info.release_name}
      <div class="up-name">{info.release_name}</div>
    {/if}

    {#if notesPreview.length > 0}
      <ul class="up-highlights">
        {#each notesPreview as line}
          <li>{line}</li>
        {/each}
      </ul>
    {/if}

    {#if info.release_notes}
      <button
        type="button"
        class="up-notes-toggle"
        aria-expanded={notesOpen}
        onclick={() => (notesOpen = !notesOpen)}
      >
        {notesOpen ? "Hide notes" : "Show notes"}
      </button>
      {#if notesOpen}
        <pre class="up-notes">{info.release_notes}</pre>
      {/if}
    {/if}

    {#if hostAsset}
      <div class="up-asset" title={hostAsset.name}>
        <span class="up-asset-name">{hostAsset.name}</span>
        {#if formatSize(hostAsset.size)}
          <span class="up-asset-size mono">{formatSize(hostAsset.size)}</span>
        {/if}
      </div>
    {/if}

    {#if busy}
      <div class="up-progress" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(progressPct)}>
        <div class="up-progress-head">
          <span class="up-progress-label">
            {phase === "installing" ? "Installing…" : phase === "relaunching" ? "Restarting…" : "Downloading…"}
          </span>
          {#if downloadTotal > 0}
            <span class="up-progress-size mono">
              {formatSize(downloaded)} / {formatSize(downloadTotal)}
            </span>
          {/if}
        </div>
        <div class="up-progress-track">
          <div
            class="up-progress-fill"
            class:indeterminate={downloadTotal === 0}
            style={downloadTotal > 0 ? `width:${progressPct}%` : undefined}
          ></div>
        </div>
      </div>
    {/if}

    {#if installError}
      <p class="up-error" role="alert">
        {phase === "relaunch-failed"
          ? "Update installed, but Pulse could not restart. Retry the restart."
          : "Update failed. Open the release page or try again."}
      </p>
    {/if}

    <div class="up-actions">
      <button type="button" class="up-btn up-ghost" onclick={later} disabled={busy} aria-label="Dismiss this update for now">
        Later
      </button>
      <button
        type="button"
        class="up-btn up-ghost"
        onclick={skipVersion}
        disabled={busy}
        aria-label={`Skip version ${info.latest_version}`}
      >
        Skip
      </button>
      {#if installError && phase !== "relaunch-failed"}
        <button type="button" class="up-btn up-ghost" onclick={openRelease} disabled={opening}>
          {opening ? "Opening…" : "Open release"}
        </button>
      {/if}
      <button
        type="button"
        class="up-btn up-primary"
        onclick={phase === "relaunch-failed" ? retryRelaunch : installUpdate}
        disabled={busy}
        aria-label={phase === "relaunch-failed" ? "Restart Pulse after the installed update" : "Download, install, and restart with the update"}
      >
        {phase === "relaunch-failed" ? "Retry restart" : installError ? "Retry update" : busy ? "Updating…" : "Update"}
      </button>
    </div>
  </aside>
{/if}

<style>
  .update-pop {
    position: fixed;
    top: calc(var(--topbar-height) + 12px);
    right: 16px;
    z-index: 9998;
    display: flex;
    flex-direction: column;
    gap: 12px;
    width: 340px;
    max-width: calc(100vw - var(--sidebar-width) - 32px);
    padding: 18px;
    background: var(--bg-card);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lg);
    backdrop-filter: blur(12px);
    overflow: hidden;
  }
  /* A tinted hairline along the top encodes update size without shouting. */
  .update-pop::before {
    content: '';
    position: absolute;
    inset: 0 0 auto 0;
    height: 3px;
    background: var(--success);
  }
  .update-pop.sev-minor::before { background: var(--info); }
  .update-pop.sev-major::before { background: var(--warning); }

  .up-head {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .up-title { width: 100%; color: var(--text-primary); font-size: var(--fs-md); font-weight: 700; letter-spacing: var(--letter-tight); }
  .up-badge {
    font-size: var(--fs-xs);
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: var(--letter-wider);
    color: var(--success);
    background: var(--success-dim);
    border: 1px solid color-mix(in srgb, var(--success) 26%, transparent);
    border-radius: var(--radius-full);
    padding: 3px 9px;
  }
  .sev-minor .up-badge {
    color: var(--info);
    background: var(--info-dim);
    border-color: color-mix(in srgb, var(--info) 26%, transparent);
  }
  .sev-major .up-badge {
    color: var(--warning);
    background: var(--warning-dim);
    border-color: color-mix(in srgb, var(--warning) 26%, transparent);
  }
  .up-age {
    margin-left: auto;
    font-size: var(--fs-xs);
    color: var(--text-muted);
  }

  .up-version {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .up-from,
  .up-to {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }
  .up-from {
    font-size: var(--fs-md);
    color: var(--text-muted);
    text-decoration: line-through;
    text-decoration-color: var(--border-hover);
  }
  .up-to {
    font-size: var(--fs-xl);
    font-weight: 700;
    letter-spacing: var(--letter-tight);
    color: var(--text-primary);
  }
  .up-arrow {
    width: 15px;
    height: 15px;
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .up-name {
    font-size: var(--fs-base);
    font-weight: 600;
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .up-highlights {
    display: flex;
    flex-direction: column;
    gap: 5px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .up-highlights li {
    position: relative;
    padding-left: 15px;
    font-size: var(--fs-sm);
    line-height: var(--lh-snug);
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .up-highlights li::before {
    content: '';
    position: absolute;
    left: 3px;
    top: 7px;
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: var(--border-hover);
  }

  .up-notes-toggle {
    align-self: flex-start;
    font-size: var(--fs-sm);
    font-weight: 500;
    color: var(--text-muted);
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
    text-decoration: underline;
    text-underline-offset: 3px;
    text-decoration-color: var(--border-hover);
    transition: color 0.15s var(--ease);
  }
  .up-notes-toggle:hover { color: var(--text-primary); }

  .up-notes {
    margin: 0;
    max-height: 160px;
    overflow-y: auto;
    padding: 10px 12px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    line-height: var(--lh-snug);
    color: var(--text-secondary);
    white-space: pre-wrap;
    word-break: break-word;
  }

  .up-asset {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px;
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }
  .up-asset-name {
    font-size: var(--fs-xs);
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .up-asset-size {
    margin-left: auto;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-muted);
    flex-shrink: 0;
  }

  .up-actions {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
    margin-top: 2px;
  }

  .up-progress { display: flex; flex-direction: column; gap: 7px; }
  .up-progress-head { display: flex; align-items: baseline; gap: 8px; }
  .up-progress-label {
    font-size: var(--fs-sm);
    font-weight: 600;
    color: var(--text-secondary);
  }
  .up-progress-size {
    margin-left: auto;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }
  .up-progress-track {
    height: 4px;
    border-radius: var(--radius-full);
    background: var(--bg-input);
    overflow: hidden;
  }
  .up-progress-fill {
    height: 100%;
    border-radius: var(--radius-full);
    background: var(--accent);
    transition: width 0.2s var(--ease);
  }
  /* Some servers omit Content-Length; show motion rather than a stuck 0%. */
  .up-progress-fill.indeterminate {
    width: 35%;
    animation: up-slide 1.1s var(--ease) infinite;
  }
  @keyframes up-slide {
    0%   { transform: translateX(-100%); }
    100% { transform: translateX(320%); }
  }
  @media (prefers-reduced-motion: reduce) {
    .up-progress-fill.indeterminate { animation: none; width: 100%; opacity: 0.5; }
  }

  .up-error {
    font-size: var(--fs-sm);
    color: var(--warning);
  }

  .up-btn:disabled { opacity: 0.5; cursor: default; }
  .up-btn {
    font-family: inherit;
    font-size: var(--fs-sm);
    font-weight: 600;
    padding: 7px 13px;
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background 0.15s var(--ease), color 0.15s var(--ease), border-color 0.15s var(--ease);
  }
  .up-ghost {
    background: transparent;
    border: 1px solid transparent;
    color: var(--text-muted);
  }
  .up-ghost:hover {
    color: var(--text-primary);
    border-color: var(--border-strong);
  }
  .up-primary {
    background: var(--accent);
    border: 1px solid var(--accent);
    color: var(--accent-fg);
  }
  .up-primary:hover:not(:disabled) { background: var(--accent-hover); }
  .up-primary:disabled { opacity: 0.6; cursor: default; }

  @media (max-width: 560px) {
    .update-pop {
      left: 12px;
      right: 12px;
      width: auto;
      max-width: none;
    }
  }
</style>
