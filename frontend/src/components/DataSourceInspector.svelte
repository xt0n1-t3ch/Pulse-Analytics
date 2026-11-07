<script lang="ts">
    import { accessSnapshot, sourceInspectorExpanded } from "../lib/stores";
    import { accessKindLabel } from "../lib/access";

    let routes = $derived($accessSnapshot?.routes ?? []);
</script>

<section class="source-inspector" data-source-inspector>
    <button
        class="inspector-head"
        aria-expanded={$sourceInspectorExpanded}
        onclick={() => sourceInspectorExpanded.update((value) => !value)}
    >
        <span
            ><strong>Source diagnostics</strong><small
                >Proof, reachability, and freshness</small
            ></span
        >
        <span>{$sourceInspectorExpanded ? "Collapse" : "Inspect routes"}</span>
    </button>

    {#if $sourceInspectorExpanded}
        <div class="route-grid">
            {#if routes.length === 0}
                <div class="route-empty">
                    No provider route has been discovered yet.
                </div>
            {:else}
                {#each routes as route, index (route.source.id)}
                    {@const label = accessKindLabel(route.source.kind)}
                    {@const isProven = route.source.proof !== "none"}
                    <article class="route-card">
                        <header>
                            <span class="route-rank">{index + 1}</span>
                            <strong>{label.product} {label.access}</strong>
                            {#if route.availability === "available" && isProven}
                                <span class="status-copy ok">Authenticated</span
                                >
                            {:else}
                                <span class="status-copy bad">Unavailable</span>
                            {/if}
                        </header>
                        <p class="route-proof">
                            {route.source.auth_method.replaceAll("_", " ")}
                            · {isProven
                                ? route.source.proof.replaceAll("_", " ")
                                : "no provider proof"}
                        </p>
                        {#if route.error}
                            <p class="route-error">{route.error}</p>
                        {/if}
                        <footer>
                            <span>Freshness</span>
                            <strong class:ok={route.freshness === "fresh"}
                                >{route.freshness}</strong
                            >
                        </footer>
                    </article>
                {/each}
            {/if}
        </div>
    {/if}
</section>

<style>
    .source-inspector {
        overflow: hidden;
        background: var(--bg-secondary);
        border: 1px solid var(--border);
        border-radius: var(--radius-lg);
    }

    .inspector-head {
        width: 100%;
        min-height: 42px;
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 16px;
        padding: 9px 14px;
        border-bottom: 1px solid var(--border);
        text-align: left;
    }

    .inspector-head > span {
        display: inline-flex;
        align-items: center;
        gap: 7px;
    }
    .inspector-head strong {
        font-size: 11px;
    }
    .inspector-head small,
    .inspector-head > span:last-child {
        color: var(--text-muted);
        font-size: 9px;
    }
    .route-grid {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(210px, 1fr));
        gap: 10px;
        padding: 12px;
    }

    .route-card {
        min-height: 116px;
        display: grid;
        gap: 9px;
        padding: 12px;
        background: var(--bg-card);
        border: 1px solid var(--border);
        border-radius: var(--radius-md);
    }

    .route-card header {
        display: grid;
        grid-template-columns: 20px minmax(0, 1fr) 16px;
        align-items: center;
        gap: 7px;
    }
    .route-card header strong {
        font-size: 10px;
    }
    .route-rank {
        width: 19px;
        height: 19px;
        display: grid;
        place-items: center;
        color: var(--success);
        background: var(--success-dim);
        border-radius: 50%;
        corner-shape: round;
        font-size: 9px;
    }
    .route-card p {
        color: var(--text-muted);
        font-size: 9px;
    }
    .route-proof {
        text-transform: capitalize;
    }
    .route-error {
        color: var(--text-secondary) !important;
        line-height: 1.45;
        overflow-wrap: anywhere;
    }
    .route-card footer {
        display: flex;
        align-items: center;
        justify-content: space-between;
        color: var(--text-muted);
        font-size: 9px;
    }
    .route-card footer strong {
        color: var(--text-secondary);
        text-transform: capitalize;
    }
    .status-copy {
        font-size: 8px;
        text-align: right;
    }
    .ok {
        color: var(--success) !important;
    }
    .bad {
        color: var(--danger) !important;
    }
    .route-empty {
        padding: 18px;
        color: var(--text-muted);
        font-size: 10px;
    }
</style>
