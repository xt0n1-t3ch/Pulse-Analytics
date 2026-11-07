<script lang="ts">
    import type { Snippet } from "svelte";
    import StatCard from "./StatCard.svelte";

    /**
     * Standard metric strip: a bordered matte row of stat cells with tabular
     * numerals and an optional provider-accent kicker. Replaces the per-view
     * `<div class="stats-row metric-strip">` + loose StatCard usage so every
     * KPI band shares the same divider rhythm and column collapse behavior.
     */
    export interface Metric {
        label: string;
        value: string;
    }

    let {
        metrics = [],
        kicker,
        children,
    }: {
        metrics?: Metric[];
        kicker?: string;
        children?: Snippet;
    } = $props();
</script>

<div class="metric-strip-wrap">
    {#if kicker}<span class="kicker metric-strip-kicker">{kicker}</span>{/if}
    <div class="metric-strip">
        {#if children}
            {@render children()}
        {:else}
            {#each metrics as metric (metric.label)}
                <StatCard label={metric.label} value={metric.value} />
            {/each}
        {/if}
    </div>
</div>

<style>
    .metric-strip-wrap {
        display: grid;
        gap: 10px;
        min-width: 0;
    }
    .metric-strip-kicker {
        margin-left: 2px;
    }
</style>
