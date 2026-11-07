<script lang="ts">
    /**
     * One status pill for the whole app. `state` maps to a semantic color so a
     * live source, a stale window, a waiting broadcast, an expired credential,
     * and a paused feature never each invent their own chip treatment.
     */
    export type StatusPillState =
        "live" | "stale" | "waiting" | "expired" | "paused" | "neutral";

    let {
        state = "neutral",
        label,
        title,
        pulse = false,
    }: {
        state?: StatusPillState;
        label: string;
        title?: string;
        pulse?: boolean;
    } = $props();
</script>

<span class="status-pill" data-state={state} title={title ?? label}>
    <span class="sp-beacon" class:pulse={pulse && state === "live"}></span>
    {label}
</span>

<style>
    .status-pill {
        display: inline-flex;
        align-items: center;
        gap: 7px;
        min-height: 24px;
        padding: 0 10px 0 9px;
        border-radius: var(--radius-full);
        corner-shape: round;
        font-size: var(--fs-xs);
        font-weight: 650;
        letter-spacing: var(--letter-tight);
        white-space: nowrap;
        color: var(--text-secondary);
        background: var(--bg-elevated);
        border: 1px solid var(--border);
        font-variant-numeric: tabular-nums;
    }

    .sp-beacon {
        position: relative;
        width: 6px;
        height: 6px;
        border-radius: 50%;
        corner-shape: round;
        background: var(--text-muted);
        flex-shrink: 0;
    }

    .status-pill[data-state="live"] {
        color: var(--success);
        background: var(--success-dim);
        border-color: color-mix(in srgb, var(--success) 34%, transparent);
    }
    .status-pill[data-state="live"] .sp-beacon {
        background: var(--success);
    }

    .status-pill[data-state="stale"] {
        color: var(--text-secondary);
        background: var(--bg-elevated);
        border-color: var(--border-strong);
    }
    .status-pill[data-state="stale"] .sp-beacon {
        background: var(--text-muted);
    }

    .status-pill[data-state="waiting"] {
        color: var(--warning);
        background: var(--warning-dim);
        border-color: color-mix(in srgb, var(--warning) 34%, transparent);
    }
    .status-pill[data-state="waiting"] .sp-beacon {
        background: var(--warning);
    }

    .status-pill[data-state="expired"] {
        color: var(--danger);
        background: var(--danger-dim);
        border-color: color-mix(in srgb, var(--danger) 32%, transparent);
    }
    .status-pill[data-state="expired"] .sp-beacon {
        background: var(--danger);
    }

    .status-pill[data-state="paused"] {
        color: var(--text-muted);
        background: var(--surface-panel-soft);
        border-color: var(--border);
    }
    .status-pill[data-state="paused"] .sp-beacon {
        background: var(--text-placeholder);
    }

    .sp-beacon.pulse::after {
        content: "";
        position: absolute;
        inset: -3px;
        border-radius: 50%;
        corner-shape: round;
        border: 1.5px solid var(--success);
        animation: sp-ping 2s var(--ease-out) infinite;
    }
    @keyframes sp-ping {
        0% {
            transform: scale(0.7);
            opacity: 0.9;
        }
        70% {
            transform: scale(1.7);
            opacity: 0;
        }
        100% {
            transform: scale(1.7);
            opacity: 0;
        }
    }
    @media (prefers-reduced-motion: reduce) {
        .sp-beacon.pulse::after {
            animation: none;
            opacity: 0.35;
        }
    }
</style>
