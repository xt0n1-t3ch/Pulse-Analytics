<script lang="ts">
  import commandCodeMark from "../assets/rp/commandcode.png";
  import OpenCodeMark from "./OpenCodeMark.svelte";
  import codexMark from "../assets/rp/codex-app.png";
  import claudeMark from "../assets/rp/claude.svg";
  import openCodeMark from "../assets/rp/opencode-v2.png";
  import openAiMark from "../assets/rp/chatgpt-app.jpg";
  import { IconStack2, IconActivityHeartbeat } from "@tabler/icons-svelte";
  import { accessSnapshot, backendConnection, currentView, selectedAccessSourceId, selectedAnalyticsProviderScope, sourceInspectorExpanded, addToast } from "../lib/stores";
  import { accessSourceName, displayableAccessRoutes, type AnalyticsProviderScope, type AccessRouteSnapshot } from "../lib/access";
  import { provider, setProvider, type Provider } from "../lib/provider";

  type Entry = { id: AnalyticsProviderScope; name: string; subtitle: string; image: string | null; route?: AccessRouteSnapshot; state: string };
  let pending = $state(false);
  let routes = $derived(displayableAccessRoutes($accessSnapshot?.routes ?? []));
  function routeState(route: AccessRouteSnapshot | undefined): string {
    if (!route || route.source.proof === "none") return route?.local_history.available ? "history" : "local";
    if (route.source.proof === "authenticated_probe") return "live";
    return route.availability === "available" && route.freshness === "fresh" ? "live" : "attention";
  }
  let entries = $derived.by<Entry[]>(() => {
    const native: Entry[] = (["claude", "codex", "opencode", "commandcode"] as const).map((id) => {
      const route = routes.find((route) => route.source.provider === id && (route.source.kind === `${id}_subscription` || (id === "commandcode" && route.source.kind === "command_code_subscription") || (id === "opencode" && route.source.kind === "open_code_go")));
      return { id, name: route && route.source.proof !== "none" ? accessSourceName(route.source) : id === "claude" ? "Claude" : id === "codex" ? "Codex" : id === "commandcode" ? "Command Code" : "OpenCode",
        subtitle: id === "opencode" ? route && route.source.proof !== "none" ? "Go subscription" : "Desktop · CLI · OpenChamber" : route?.source.proof !== "none" && route ? "Subscription" : "Local sessions",
        image: id === "claude" ? claudeMark : id === "codex" ? codexMark : id === "commandcode" ? commandCodeMark : openCodeMark, route, state: routeState(route) };
    });
    for (const route of routes.filter((route) => route.source.kind.endsWith("_api"))) {
      native.push({ id: route.source.provider, name: accessSourceName(route.source), subtitle: "API", image: route.source.kind === "open_ai_api" ? openAiMark : claudeMark, route, state: routeState(route) });
    }
    native.push({id:"all", name:"All providers", subtitle:"Combined analytics", image:null, state:"aggregate"});
    return native;
  });
  const nativeIds = new Set<string>(["claude","codex","opencode","commandcode"]);
  async function selectSource(id: AnalyticsProviderScope): Promise<void> {
    if (pending) return;
    pending = true;
    try {
      if (nativeIds.has(id) && $provider !== id) await setProvider(id as Provider);
      selectedAnalyticsProviderScope.set(id);
      const entry = entries.find((entry) => entry.id === id);
      selectedAccessSourceId.set(id === "all" ? "all" : entry?.route?.source.id ?? `local:${id}`);
    } catch { addToast("Could not switch provider. Your previous selection is unchanged.", "danger"); }
    finally { pending = false; }
  }
  let attention = $derived($backendConnection === "disconnected" || ($accessSnapshot?.routes ?? []).some((route) => {
    if (route.source.proof === "authenticated_probe") return false;
    if (route.source.proof !== "none") return route.availability !== "available" || route.freshness !== "fresh";
    return route.local_history.available;
  }));
  function inspect(): void { sourceInspectorExpanded.set(true); currentView.set("settings"); }
</script>

<section class="access-bar" aria-label="Provider selection">
  <label class="mobile-provider-picker"><span>Provider</span>
    <select aria-label="Provider" value={$selectedAnalyticsProviderScope} disabled={pending}
      onchange={(event) => void selectSource(event.currentTarget.value as AnalyticsProviderScope)}>
      {#each entries as entry}<option value={entry.id}>{entry.name}</option>{/each}
    </select>
  </label>
  <div class="source-list" role="group" aria-label="Select provider">
    {#each entries as entry (entry.id)}
      <button class="source-card" class:selected={$selectedAnalyticsProviderScope === entry.id}
        type="button" data-access-source={entry.route?.source.id ?? entry.id} data-provider={entry.id}
        aria-pressed={$selectedAnalyticsProviderScope === entry.id} disabled={pending} onclick={() => selectSource(entry.id)}>
        {#if entry.id === "opencode"}<OpenCodeMark />{:else if entry.image}<img src={entry.image} alt="" />{:else}<span class="aggregate-mark" aria-hidden="true"><IconStack2 size={20} /></span>{/if}
        <span class="source-copy"><strong>{entry.name}</strong><small>{entry.subtitle}</small></span>
        {#if entry.state === "live" || entry.state === "attention"}<span class="source-dot" class:attention={entry.state === "attention"} aria-label={entry.state === "live" ? "Live" : "Needs attention"}></span>{/if}
      </button>
    {/each}
  </div>

</section>

<style>
  .access-bar { display:flex; align-items:center; gap:14px; padding:10px 20px; border-bottom:1px solid var(--border); background:var(--bg-secondary); flex:0 0 auto; }
  .source-list { min-width:0; flex:1; display:grid; grid-template-columns:repeat(auto-fit,minmax(180px,1fr)); gap:8px; }
  .source-card { min-width:0; display:grid; grid-template-columns:28px minmax(0,1fr) auto; align-items:center; gap:10px; padding:10px 12px; border:1px solid var(--border); border-radius:var(--radius-md); color:var(--text-secondary); text-align:left; background:var(--bg-card); min-height:56px; }
  .source-card:hover { background:var(--bg-card-hover); border-color:var(--border-hover); }
  .source-card.selected { color:var(--text-primary); border-color:var(--accent); background:var(--bg-elevated); }
  .source-card:focus-visible, select:focus-visible { outline:2px solid var(--accent); outline-offset:2px; }
  .source-card img, .aggregate-mark { width:28px; height:28px; object-fit:contain; border-radius:6px; }
  .aggregate-mark { display:grid; place-items:center; color:var(--text-secondary); }
  .source-copy { display:grid; gap:3px; min-width:0; }
  .source-copy strong { font-size:12px; font-weight:650; overflow-wrap:anywhere; }
  .source-copy small { font-size:10px; color:var(--text-secondary); line-height:1.4; }
  .source-dot { width:6px; height:6px; border-radius:50%; background:var(--success); }
  .source-dot.attention { background:var(--warning); }
  .mobile-provider-picker { display:none; }
  @media(max-width:620px) {
    .access-bar { padding:10px 12px; gap:8px; }
    .source-list { display:none; }
    .mobile-provider-picker { display:grid; gap:5px; flex:1; min-width:0; font-size:12px; color:var(--text-secondary); }
    .mobile-provider-picker select { width:100%; min-width:0; min-height:40px; padding:8px 10px; color:var(--text-primary); background:var(--bg-input); border:1px solid var(--border-strong); border-radius:var(--radius-sm); font:inherit; }
    }
</style>
