<script lang="ts">
    import type { Snippet } from "svelte";

    let {
        label,
        value,
        extra,
    }: { label: string; value: string; extra?: Snippet } = $props();
</script>

<div class="stat-card">
    <span class="stat-label">{label}</span>
    <div class="stat-row">
        <span class="stat-value">{value}</span>
        {#if extra}
            <div class="stat-extra">{@render extra()}</div>
        {/if}
    </div>
</div>

<style>
    .stat-card {
        position: relative;
        background: transparent;
        border: 0;
        border-radius: 0;
        padding: 16px 18px;
        display: flex;
        flex-direction: column;
        gap: 6px;
        transition: all 0.2s var(--ease);
        min-width: 0;
    }

    .stat-card:hover {
        background: var(--accent-dim);
    }

    .stat-label {
        font-size: var(--fs-xs);
        font-weight: 600;
        text-transform: uppercase;
        letter-spacing: 0.06em;
        color: var(--text-secondary);
    }

    .stat-row {
        display: flex;
        align-items: flex-end;
        justify-content: space-between;
        gap: 8px;
    }

    .stat-value {
        font-size: clamp(22px, 2.3vw, 30px);
        font-weight: 700;
        letter-spacing: -0.03em;
        color: var(--text-primary);
        font-variant-numeric: tabular-nums;
    }

    .stat-extra {
        display: flex;
        align-items: flex-end;
        flex-shrink: 0;
    }
</style>
