<script lang="ts">
    import { usageColor } from "../lib/utils";

    let {
        label,
        pct,
        remainingPct = null,
        meta = "",
        sublabel = "",
    }: {
        label: string;
        pct: number;
        remainingPct?: number | null;
        meta?: string;
        sublabel?: string;
    } = $props();

    let color = $derived(usageColor(pct));
    let clampedWidth = $derived(Math.min(pct, 100));
</script>

<div class="bar-group">
    <div class="bar-top">
        <div class="bar-label-col">
            <span class="bar-label">{label}</span>
            {#if sublabel}
                <span class="bar-sublabel">{sublabel}</span>
            {/if}
            {#if meta}
                <span class="bar-meta">{meta}</span>
            {/if}
        </div>
        <span
            class="bar-values"
            class:warning={color === "warning"}
            class:danger={color === "danger"}
        >
            {#if remainingPct != null}<strong
                    >{Math.round(remainingPct)}% available</strong
                >{/if}
            <span>{Math.round(pct)}% used</span>
        </span>
    </div>
    <div class="bar-track">
        <div
            class="bar-fill"
            class:warning={color === "warning"}
            class:danger={color === "danger"}
            style="width: {clampedWidth}%"
        ></div>
    </div>
</div>

<style>
    .bar-group {
        display: flex;
        flex-direction: column;
        gap: 6px;
        padding: 6px 0;
    }

    .bar-top {
        display: flex;
        justify-content: space-between;
        align-items: flex-start;
    }

    .bar-label-col {
        display: flex;
        flex-direction: column;
        gap: 2px;
    }

    .bar-label {
        font-weight: 600;
        font-size: 13px;
        color: var(--text-primary);
    }

    .bar-sublabel {
        font-size: 11px;
        color: var(--text-muted);
    }

    .bar-meta {
        font-size: 11px;
        color: var(--text-muted);
    }

    .bar-values {
        display: flex;
        flex-direction: column;
        align-items: flex-end;
        gap: 1px;
        font-weight: 600;
        font-size: 12px;
        font-variant-numeric: tabular-nums;
        color: var(--text-secondary);
        white-space: nowrap;
    }
    .bar-values strong {
        color: var(--text-primary);
        font-size: 14px;
    }
    .bar-values span {
        color: var(--text-muted);
    }
    .bar-values.warning strong {
        color: var(--warning);
    }
    .bar-values.danger strong {
        color: var(--danger);
    }

    .bar-track {
        height: 8px;
        background: var(--bg-elevated);
        border-radius: var(--radius-full);
        corner-shape: round;
        overflow: hidden;
    }

    .bar-fill {
        height: 100%;
        border-radius: var(--radius-full);
        corner-shape: round;
        background: var(--accent);
        transition: width 600ms var(--ease);
    }

    .bar-fill.warning {
        background: var(--warning);
    }

    .bar-fill.danger {
        background: var(--danger);
    }
</style>
