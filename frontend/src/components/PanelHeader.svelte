<script lang="ts">
    import type { Snippet } from "svelte";

    /**
     * Shared header for panels and views: an optional provider-accent kicker, a
     * title, an optional subtitle, and a right-aligned slot for a control
     * (SegmentedControl, StatusPill, button…). Standardizes the title rhythm the
     * design-qa gate calls out ("consistent title rhythm across all routes").
     */
    let {
        kicker,
        title,
        subtitle,
        as = "h2",
        control,
    }: {
        kicker?: string;
        title: string;
        subtitle?: string;
        as?: "h1" | "h2" | "h3";
        control?: Snippet;
    } = $props();
</script>

<div class="panel-header">
    <div class="ph-text">
        {#if kicker}<span class="kicker">{kicker}</span>{/if}
        {#if as === "h1"}
            <h1 class="ph-title">{title}</h1>
        {:else if as === "h3"}
            <h3 class="ph-title">{title}</h3>
        {:else}
            <h2 class="ph-title">{title}</h2>
        {/if}
        {#if subtitle}<p class="ph-sub">{subtitle}</p>{/if}
    </div>
    {#if control}
        <div class="ph-control">{@render control()}</div>
    {/if}
</div>

<style>
    .panel-header {
        display: flex;
        align-items: flex-start;
        justify-content: space-between;
        gap: 18px;
        flex-wrap: wrap;
    }
    .ph-text {
        min-width: 0;
        display: flex;
        flex-direction: column;
        gap: 4px;
    }
    .ph-title {
        font-size: var(--fs-lg);
        font-weight: 700;
        letter-spacing: var(--letter-tight);
        color: var(--text-primary);
        line-height: var(--lh-tight);
    }
    h1.ph-title {
        font-size: var(--fs-2xl);
        letter-spacing: var(--letter-tighter);
    }
    h3.ph-title {
        font-size: var(--fs-md);
    }
    .ph-sub {
        font-size: var(--fs-sm);
        color: var(--text-muted);
        line-height: var(--lh-snug);
        max-width: 62ch;
    }
    .ph-control {
        display: inline-flex;
        align-items: center;
        gap: 10px;
        flex-shrink: 0;
        flex-wrap: wrap;
    }
    .kicker {
        margin-bottom: 2px;
    }
</style>
