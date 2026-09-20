<script lang="ts">
  import { onMount, type Component } from "svelte";
  import TopBar from "./components/TopBar.svelte";
  import AccessSourceBar from "./components/AccessSourceBar.svelte";
  import Toast from "./components/Toast.svelte";
  import UpdateBanner from "./components/UpdateBanner.svelte";
  import Dashboard from "./views/Dashboard.svelte";
  import Sessions from "./views/Sessions.svelte";
  import Costs from "./views/Costs.svelte";
  import Reports from "./views/Reports.svelte";
  import Discord from "./views/Discord.svelte";
  import Settings from "./views/Settings.svelte";
  import Accounts from "./views/Accounts.svelte";
  import {
    currentView,
    invalidateLiveSnapshotForProviderChange,
    loadDiscordUser,
    poll,
    startSnapshotSync,
    stopSnapshotSync,
  } from "./lib/stores";
  import { providerRevision } from "./lib/provider";
  import { fly } from "svelte/transition";
  import { setTheme } from "@tauri-apps/api/app";

  type ViewId = "dashboard" | "sessions" | "costs" | "reports" | "discord" | "accounts" | "settings";
  const views: Record<ViewId, Component<any>> = {
    dashboard: Dashboard,
    sessions: Sessions,
    costs: Costs,
    reports: Reports,
    discord: Discord,
    accounts: Accounts,
    settings: Settings,
  } as const;
  let activeViewId = $derived(
    ($currentView in views ? $currentView : "dashboard") as ViewId,
  );
  let ActiveView = $derived(views[activeViewId]);
  let scrollFrame: HTMLDivElement;
  let mainContent: HTMLElement;
  $effect(() => {
    void activeViewId;
    if (scrollFrame) scrollFrame.scrollTop = 0;
    if (mainContent) mainContent.scrollTop = 0;
  });

  const initialTheme: "dark" | "light" =
    localStorage.getItem("pulse-theme") === "light" ? "light" : "dark";
  let theme = $state<"dark" | "light">(initialTheme);

  function applyTheme(next: "dark" | "light"): void {
    theme = next;
    document.documentElement.setAttribute("data-theme", next);
    document.documentElement.style.colorScheme = next;
    localStorage.setItem("pulse-theme", next);
    void setTheme(next).catch(() => undefined);
  }

  applyTheme(initialTheme);

  function toggleTheme(): void {
    applyTheme(theme === "dark" ? "light" : "dark");
  }

  onMount(() => {
    applyTheme(theme);
    startSnapshotSync();
    void loadDiscordUser();
    const discordUserRefreshTimer = setInterval(() => void loadDiscordUser(), 60_000);
    let firstProviderRevision = true;
    const unsubscribeProviderRevision = providerRevision.subscribe(() => {
      if (firstProviderRevision) {
        firstProviderRevision = false;
        return;
      }
      invalidateLiveSnapshotForProviderChange();
      void poll();
    });
    return () => {
      clearInterval(discordUserRefreshTimer);
      unsubscribeProviderRevision();
      stopSnapshotSync();
    };
  });
</script>

<div class="main-wrapper" bind:this={scrollFrame}>
  <TopBar onToggleTheme={toggleTheme} />
  {#if activeViewId !== "accounts"}<AccessSourceBar />{/if}
  <main class="main-content" bind:this={mainContent}>
    {#key activeViewId}
      <div class="view-host" in:fly={{ y: 4, duration: 80 }}>
        {#if activeViewId === "settings"}
          <ActiveView onToggleTheme={toggleTheme} currentTheme={theme} />
        {:else}
          <ActiveView />
        {/if}
      </div>
    {/key}
  </main>
</div>
<Toast />
<UpdateBanner />

<style>
  .main-wrapper {
    height: 100vh;
    height: 100dvh;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  .main-content {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    padding: var(--page-padding-block) var(--page-padding-inline);
    overflow-y: auto;
    overflow-x: hidden;
    min-width: 0;
    scrollbar-gutter: stable;
  }

  .view-host {
    flex: 1 0 auto;
    width: 100%;
    min-width: 0;
    min-height: 100%;
  }
  @media (max-width: 620px) {
    .main-wrapper { display: block; overflow-y: auto; }
    .main-content { overflow: visible; }
  }
</style>
