<script lang="ts" generics="T extends string">
    /**
     * Shared segmented control. Replaces the ad-hoc `.preset-seg` (Discord),
     * window pills (Reports), and identity toggle (Codex/ChatGPT App) so every
     * inline mutually-exclusive choice reads and behaves the same.
     */
    interface Option {
        value: T;
        label: string;
    }

    let {
        options,
        value,
        onchange,
        disabled = false,
        ariaLabel,
        role = "group",
        size = "md",
    }: {
        options: Option[];
        value: T;
        onchange: (value: T) => void;
        disabled?: boolean;
        ariaLabel: string;
        role?: "group" | "tablist";
        size?: "sm" | "md";
    } = $props();
</script>

<div class="segmented" data-size={size} {role} aria-label={ariaLabel}>
    {#each options as option (option.value)}
        <button
            type="button"
            class="seg-opt"
            class:active={value === option.value}
            role={role === "tablist" ? "tab" : undefined}
            aria-selected={role === "tablist"
                ? value === option.value
                : undefined}
            aria-pressed={role === "group" ? value === option.value : undefined}
            {disabled}
            onclick={() => onchange(option.value)}>{option.label}</button
        >
    {/each}
</div>

<style>
    .segmented {
        display: inline-flex;
        padding: 3px;
        gap: 2px;
        background: var(--bg-input);
        border: 1px solid var(--border);
        border-radius: var(--radius-full);
        corner-shape: round;
        height: 32px;
        flex-shrink: 0;
    }
    .segmented[data-size="sm"] {
        height: 28px;
    }

    .seg-opt {
        display: inline-flex;
        align-items: center;
        padding: 0 14px;
        font-size: var(--fs-sm);
        font-weight: 600;
        line-height: 1;
        color: var(--text-muted);
        background: transparent;
        border-radius: var(--radius-full);
        corner-shape: round;
        transition:
            background 0.15s var(--ease),
            color 0.15s var(--ease);
    }
    .segmented[data-size="sm"] .seg-opt {
        padding: 0 11px;
        font-size: var(--fs-xs);
    }

    .seg-opt:hover:not(:disabled) {
        color: var(--text-secondary);
    }
    .seg-opt.active {
        color: var(--text-primary);
        background: var(--bg-card-hover);
        box-shadow: var(--shadow-xs);
    }
    .seg-opt:disabled {
        opacity: 0.4;
        cursor: not-allowed;
    }
</style>
