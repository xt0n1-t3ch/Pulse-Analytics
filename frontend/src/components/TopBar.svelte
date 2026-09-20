<script lang="ts">
  import {
    IconMaximize,
    IconMinus,
    IconSun,
    IconX,
  } from "@tabler/icons-svelte";
  import PulseMark from "./PulseMark.svelte";
  import NotificationCenter from "./NotificationCenter.svelte";
  import {
    currentView,
    health,
    selectedAccessDiagnostics,
  } from "../lib/stores";
  import { verifiedPlanLabel } from "../lib/plans";
  import { accessKindLabel } from "../lib/access";

  let { onToggleTheme }: { onToggleTheme: () => void } = $props();

  const navItems = [
    { id: "dashboard", label: "Home" },
    { id: "sessions", label: "Sessions" },
    { id: "costs", label: "Costs" },
    { id: "reports", label: "Reports" },
    { id: "discord", label: "Discord" },
    { id: "accounts", label: "Accounts" },
    { id: "settings", label: "Settings" },
  ] as const;

  /** Header identity is the authenticated subscription — product and plan —
   *  not the live session's model string. It reads from the single selected
   *  subscription route so it matches the source the user picked. */
  let subscriptionRoute = $derived(
    $selectedAccessDiagnostics.length === 1
    && ($selectedAccessDiagnostics[0].source.kind === "codex_subscription"
      || $selectedAccessDiagnostics[0].source.kind === "claude_subscription")
      ? $selectedAccessDiagnostics[0]
      : null,
  );
  let activeProductLabel = $derived(
    subscriptionRoute ? accessKindLabel(subscriptionRoute.source.kind).product : null,
  );
  let activePlanLabel = $derived.by(() => {
    if (!subscriptionRoute) return null;
    const provider = subscriptionRoute.source.kind === "codex_subscription" ? "codex" : "claude";
    return verifiedPlanLabel(provider, subscriptionRoute.source.plan);
  });

  function minimize(): void {
    window.__TAURI__?.window.getCurrentWindow().minimize();
  }

  function toggleMaximize(): void {
    const win = window.__TAURI__?.window.getCurrentWindow();
    win?.isMaximized().then((maximized: boolean) => maximized ? win.unmaximize() : win.maximize());
  }

  function close(): void {
    window.__TAURI__?.window.getCurrentWindow().close();
  }
</script>

<header class="app-header" data-tauri-drag-region>
  <div class="brand">
    <PulseMark size={22} accent="var(--accent)" />
    <span>
      <strong>Pulse</strong>
      <small>Code Analytics</small>
    </span>
  </div>

  <nav class="app-nav" aria-label="Primary navigation">
    {#each navItems as item}
      <button
        class:active={$currentView === item.id}
        aria-current={$currentView === item.id ? "page" : undefined}
        onclick={() => currentView.set(item.id)}
      >
        {item.label}
      </button>
    {/each}
  </nav>

  <div class="header-context">
    {#if activeProductLabel || activePlanLabel || $health}
      <span class="identity-line">
        {#if activeProductLabel}<span class="il-product">{activeProductLabel}</span>{/if}
        {#if activePlanLabel}<span class="il-sep">·</span><span class="il-plan">{activePlanLabel}</span>{/if}
        {#if $health}<span class="il-sep">·</span><span class="il-version">v{$health.version}</span>{/if}
      </span>
    {/if}
  </div>

  <div class="header-actions">
    <NotificationCenter />
    <button title="Toggle dark or light" aria-label="Toggle dark or light" onclick={onToggleTheme}>
      <IconSun size={17} stroke={1.7} />
    </button>
    <span class="action-divider"></span>
    <button title="Minimize" aria-label="Minimize" onclick={minimize}><IconMinus size={16} stroke={1.7} /></button>
    <button title="Maximize" aria-label="Maximize" onclick={toggleMaximize}><IconMaximize size={15} stroke={1.7} /></button>
    <button class="close" title="Close" aria-label="Close" onclick={close}><IconX size={17} stroke={1.7} /></button>
  </div>
</header>

<style>
  .app-header {
    height: var(--topbar-height);
    display: flex;
    align-items: stretch;
    gap: 22px;
    padding: 0 10px 0 20px;
    color: var(--text-secondary);
    background: color-mix(in srgb, var(--bg-primary) 96%, transparent);
    border-bottom: 1px solid var(--border);
    user-select: none;
    -webkit-app-region: drag;
  }

  .brand {
    min-width: 128px;
    display: flex;
    align-items: center;
    gap: 9px;
    color: var(--text-primary);
  }

  .brand span {
    display: grid;
    line-height: 1.05;
  }

  .brand strong { font-size: 13px; font-weight: 700; letter-spacing: -0.015em; }
  .brand small { margin-top: 3px; color: var(--text-muted); font-size: 9px; }

  .app-nav {
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 3px;
    -webkit-app-region: no-drag;
  }

  .app-nav button {
    position: relative;
    min-height: 32px;
    padding: 0 10px;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    font-size: 11px;
    font-weight: 500;
    white-space: nowrap;
  }

  .app-nav button:hover { color: var(--text-primary); background: var(--bg-elevated); }
  .app-nav button.active { color: var(--text-primary); background: var(--surface-raised); border-color: var(--text-muted); font-weight: 650; }
  .app-nav button:focus-visible { outline: 2px solid var(--text-primary); outline-offset: 2px; }

  .header-context {
    min-width: 0;
    margin-left: auto;
    display: flex;
    align-items: center;
    gap: 9px;
  }

  .identity-line {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    max-width: 260px;
    overflow: hidden;
    padding: 4px 10px;
    background: var(--surface-panel-soft);
    border: 1px solid var(--border);
    border-radius: var(--radius-full);
    font-size: 10px;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .il-product { color: var(--text-primary); font-weight: 650; }
  .il-plan { color: var(--provider-accent); font-weight: 650; }
  .il-version { color: var(--text-muted); font-family: var(--font-mono); }
  .il-sep { color: var(--border-strong); }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 3px;
    -webkit-app-region: no-drag;
  }

  .header-actions button {
    width: 30px;
    height: 30px;
    display: grid;
    place-items: center;
    color: var(--text-muted);
    border-radius: var(--radius-md);
    transition: color 140ms var(--ease), background 140ms var(--ease);
  }

  .header-actions button:hover { color: var(--text-primary); background: var(--bg-elevated); }
  .header-actions button.close:hover { color: var(--on-danger); background: var(--danger); }
  .action-divider { width: 1px; height: 16px; margin: 0 5px; background: var(--border); }
  @media (max-width: 1080px) {
    .header-context { display: none; }
    .app-header { gap: 10px; padding-left: 12px; }
    .app-nav button { padding-inline: 7px; }
  }

  @media (max-width: 800px) {
    .app-header {
      height: var(--topbar-height);
      display: grid;
      grid-template:
        "brand spacer actions" 55px
        "nav nav nav" 41px
        / auto 1fr auto;
      gap: 0;
      padding: 0 8px;
    }

    .brand { grid-area: brand; min-width: 0; padding-inline: 4px; }
    .header-context { display: none; }
    .header-actions { grid-area: actions; }
    .app-nav {
      grid-area: nav;
      min-width: 0;
      overflow-x: auto;
      overflow-y: hidden;
      scrollbar-width: none;
      border-top: 1px solid var(--divider);
    }
    .app-nav::-webkit-scrollbar { display: none; }
    .app-nav button { min-width: max-content; padding-inline: 12px; }
  }

  @media (max-width: 620px) {
    .app-header { height: auto; flex-shrink: 0; grid-template: "brand spacer actions" 44px "nav nav nav" auto / auto 1fr auto; }
    .app-nav { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); overflow: visible; padding-block: 3px; }
    .app-nav button { min-width: 0; min-height: 36px; padding-inline: 4px; text-align: center; }
    .brand span { display: grid; }
    .brand small { display: none; }
  }
</style>
