<script lang="ts">
  import { provider, setProvider, PROVIDERS, type Provider } from "../lib/provider";

  let { compact = false }: { compact?: boolean } = $props();

  const options: Provider[] = ["claude", "codex", "opencode", "commandcode"];

  function pick(id: Provider): void {
    if (id !== $provider) void setProvider(id);
  }

  let activeIndex = $derived(options.indexOf($provider));
</script>

<div
  class="provider-toggle"
  class:compact
  role="tablist"
  aria-label="Provider"
  style="--active-accent: {PROVIDERS[$provider].accent}"
>
  <span
    class="indicator"
    aria-hidden="true"
    style="transform: translateX({activeIndex * 100}%)"
  ></span>

  {#each options as id}
    {@const p = PROVIDERS[id]}
    {@const active = $provider === id}
    <button
      type="button"
      role="tab"
      class="seg"
      class:active
      aria-selected={active}
      title={p.productName}
      onclick={() => pick(id)}
      style="--seg-accent: {p.accent}"
    >
      <span class="dot" aria-hidden="true"></span>
      <span class="label">{p.label}</span>
    </button>
  {/each}
</div>

<style>
  .provider-toggle {
    position: relative;
    display: inline-grid;
    grid-template-columns: repeat(4, 1fr);
    padding: 3px;
    background: var(--surface-panel-soft);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    height: var(--control-height);
    -webkit-app-region: no-drag;
    isolation: isolate;
  }
  .provider-toggle:hover { border-color: color-mix(in srgb, var(--active-accent) 45%, var(--border-hover)); }

  .indicator {
    position: absolute;
    top: 3px;
    left: 3px;
    width: calc(25% - 1.5px);
    height: calc(100% - 6px);
    background: var(--surface-raised);
    border-radius: var(--radius-sm);
    box-shadow:
      inset 0 0 0 1px color-mix(in srgb, var(--active-accent) 32%, var(--border-strong)),
      0 1px 4px rgba(0, 0, 0, 0.35);
    transition: transform 0.26s var(--ease-out);
    z-index: 0;
  }

  .seg {
    position: relative;
    z-index: 1;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    padding: 0 13px;
    min-width: 78px;
    color: var(--text-muted);
    background: transparent;
    font-size: var(--fs-sm);
    font-weight: 600;
    letter-spacing: 0.01em;
    line-height: 1;
    transition: color 0.18s var(--ease);
    white-space: nowrap;
  }
  .seg:hover { color: var(--text-secondary); }
  .seg.active { color: var(--text-primary); }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-muted);
    flex-shrink: 0;
    transition: background 0.22s var(--ease), transform 0.22s var(--ease);
  }
  .seg.active .dot {
    background: var(--seg-accent);
    transform: scale(1.08);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--seg-accent) 15%, transparent);
  }

  .label { line-height: 1; }

  .compact { height: 26px; }
  .compact .seg { min-width: 0; padding: 0 10px; }
</style>
