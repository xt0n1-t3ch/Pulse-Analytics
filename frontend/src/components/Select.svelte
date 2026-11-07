<script lang="ts">
    import { onMount, onDestroy, tick } from "svelte";

    interface Option {
        value: string;
        label: string;
        hint?: string;
    }

    let {
        value = $bindable(""),
        options,
        onchange,
        ariaLabel,
        variant = "rail",
        placeholder,
    }: {
        value?: string;
        options: Option[];
        onchange?: (v: string) => void;
        ariaLabel?: string;
        variant?: "rail" | "inline";
        placeholder?: string;
    } = $props();

    let open = $state(false);
    let wrap = $state<HTMLDivElement | null>(null);
    let btn = $state<HTMLButtonElement | null>(null);
    let pop = $state<HTMLDivElement | null>(null);
    let focused = $state(-1);
    let popStyle = $state("");
    let dropUp = $state(false);

    let current = $derived(options.find((o) => o.value === value) ?? null);

    async function positionPop(): Promise<void> {
        if (!btn) return;
        const r = btn.getBoundingClientRect();
        const vw = window.innerWidth;
        const vh = window.innerHeight;
        const gap = 6;
        const maxH = 280;
        const below = vh - r.bottom - 12;
        const above = r.top - 12;
        dropUp = below < 200 && above > below;
        const height = Math.min(maxH, dropUp ? above : below);
        const top = dropUp ? Math.max(8, r.top - gap - height) : r.bottom + gap;
        const width = Math.max(r.width, 180);
        const left = Math.min(vw - width - 8, Math.max(8, r.left));
        popStyle = `top:${top}px; left:${left}px; width:${width}px; max-height:${height}px;`;
    }

    async function openMenu(): Promise<void> {
        open = true;
        focused = Math.max(
            0,
            options.findIndex((o) => o.value === value),
        );
        await tick();
        await positionPop();
        queueMicrotask(() => {
            const el = pop?.querySelector<HTMLButtonElement>(
                `[data-idx="${focused}"]`,
            );
            el?.focus({ preventScroll: true });
            el?.scrollIntoView({ block: "nearest" });
        });
    }

    function closeMenu(restoreFocus = true): void {
        open = false;
        if (restoreFocus) btn?.focus();
    }

    function toggle(): void {
        if (open) closeMenu();
        else void openMenu();
    }

    function pick(v: string): void {
        value = v;
        open = false;
        onchange?.(v);
        btn?.focus();
    }

    function handleClickOutside(e: MouseEvent): void {
        if (!open) return;
        const t = e.target as Node;
        if (wrap && wrap.contains(t)) return;
        if (pop && pop.contains(t)) return;
        open = false;
    }

    function onWinScrollResize(): void {
        if (!open) return;
        void positionPop();
    }

    function onKey(e: KeyboardEvent): void {
        if (!open) {
            if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
                e.preventDefault();
                void openMenu();
            }
            return;
        }
        if (e.key === "Escape") {
            e.preventDefault();
            closeMenu();
        } else if (e.key === "Tab") {
            closeMenu(false);
        } else if (e.key === "ArrowDown") {
            e.preventDefault();
            focused = (focused + 1) % options.length;
            pop?.querySelector<HTMLButtonElement>(
                `[data-idx="${focused}"]`,
            )?.focus({ preventScroll: true });
        } else if (e.key === "ArrowUp") {
            e.preventDefault();
            focused = (focused - 1 + options.length) % options.length;
            pop?.querySelector<HTMLButtonElement>(
                `[data-idx="${focused}"]`,
            )?.focus({ preventScroll: true });
        } else if (e.key === "Enter") {
            e.preventDefault();
            if (focused >= 0) pick(options[focused].value);
        } else if (e.key === "Home") {
            e.preventDefault();
            focused = 0;
            pop?.querySelector<HTMLButtonElement>(`[data-idx="0"]`)?.focus({
                preventScroll: true,
            });
        } else if (e.key === "End") {
            e.preventDefault();
            focused = options.length - 1;
            pop?.querySelector<HTMLButtonElement>(
                `[data-idx="${focused}"]`,
            )?.focus({ preventScroll: true });
        }
    }

    onMount(() => {
        document.addEventListener("mousedown", handleClickOutside);
        window.addEventListener("scroll", onWinScrollResize, true);
        window.addEventListener("resize", onWinScrollResize);
    });
    onDestroy(() => {
        document.removeEventListener("mousedown", handleClickOutside);
        window.removeEventListener("scroll", onWinScrollResize, true);
        window.removeEventListener("resize", onWinScrollResize);
    });
</script>

<div
    class="sel-wrap"
    class:variant-inline={variant === "inline"}
    bind:this={wrap}
>
    <button
        type="button"
        class="sel-btn"
        class:open
        class:placeholder={!current}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        onclick={toggle}
        onkeydown={onKey}
        bind:this={btn}
    >
        <span class="sel-val">{current?.label ?? placeholder ?? ""}</span>
        <svg
            class="sel-chev"
            width="10"
            height="6"
            viewBox="0 0 10 6"
            fill="none"
            aria-hidden="true"
        >
            <path
                d="M1 1l4 4 4-4"
                stroke="currentColor"
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
            />
        </svg>
    </button>

    {#if open}
        <div
            class="sel-pop"
            class:drop-up={dropUp}
            role="listbox"
            tabindex="-1"
            style={popStyle}
            onkeydown={onKey}
            bind:this={pop}
        >
            <div class="sel-pop-inner">
                {#each options as o, i}
                    <button
                        type="button"
                        role="option"
                        aria-selected={o.value === value}
                        class="sel-opt"
                        class:active={o.value === value}
                        data-idx={i}
                        tabindex="-1"
                        onclick={() => pick(o.value)}
                        onmouseenter={() => (focused = i)}
                    >
                        <span class="sel-opt-mark" aria-hidden="true">
                            {#if o.value === value}
                                <svg
                                    width="11"
                                    height="11"
                                    viewBox="0 0 24 24"
                                    fill="none"
                                    stroke="currentColor"
                                    stroke-width="3"
                                    stroke-linecap="round"
                                    stroke-linejoin="round"
                                >
                                    <polyline points="20 6 9 17 4 12" />
                                </svg>
                            {/if}
                        </span>
                        <span class="sel-opt-label">{o.label}</span>
                        {#if o.hint}
                            <span class="sel-opt-hint">{o.hint}</span>
                        {/if}
                    </button>
                {/each}
            </div>
        </div>
    {/if}
</div>

<style>
    .sel-wrap {
        position: relative;
        width: 100%;
        font-family: var(--font-sans);
    }

    /* ── trigger: rail variant — editorial underline ── */
    .sel-btn {
        width: 100%;
        display: inline-flex;
        align-items: center;
        justify-content: space-between;
        gap: 10px;
        padding: 8px 0 9px;
        background: transparent;
        border: 0;
        border-bottom: 1px solid var(--border);
        color: var(--text-primary);
        font-size: var(--fs-base);
        font-weight: 600;
        letter-spacing: var(--letter-tight);
        text-align: left;
        cursor: pointer;
        transition:
            border-color 0.15s var(--ease),
            color 0.15s var(--ease);
        font-variant-numeric: tabular-nums;
    }
    .sel-btn:hover {
        border-bottom-color: var(--border-hover);
    }
    .sel-btn:focus-visible {
        outline: none;
        border-bottom-color: var(--text-primary);
        box-shadow: 0 1px 0 0 var(--text-primary);
    }
    .sel-btn.open {
        border-bottom-color: var(--text-primary);
        color: var(--text-primary);
    }
    .sel-btn.placeholder {
        color: var(--text-muted);
        font-weight: 500;
    }

    .sel-chev {
        color: var(--text-muted);
        transition:
            transform 0.2s var(--ease),
            color 0.15s var(--ease);
        flex-shrink: 0;
    }
    .sel-btn:hover .sel-chev {
        color: var(--text-secondary);
    }
    .sel-btn.open .sel-chev {
        transform: rotate(180deg);
        color: var(--text-primary);
    }

    /* ── trigger: inline variant — framed input ── */
    .variant-inline .sel-btn {
        border: 1px solid var(--border);
        border-radius: var(--radius-full);
        corner-shape: round;
        background: var(--bg-input);
        padding: 8px 12px;
        min-height: 32px;
    }
    .variant-inline .sel-btn:hover {
        border-color: var(--border-hover);
        background: var(--bg-elevated);
    }
    .variant-inline .sel-btn.open {
        border-color: var(--border-strong);
        background: var(--bg-elevated);
        box-shadow: var(--shadow-xs);
    }

    .sel-val {
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
        flex: 1;
    }

    /* ── popup: fixed-position portal-like, escapes all overflow ancestors ── */
    .sel-pop {
        position: fixed;
        z-index: 200;
        pointer-events: auto;
        animation: selFade 0.14s var(--ease-out);
        transform-origin: top left;
        filter: drop-shadow(0 16px 40px rgba(0, 0, 0, 0.55));
    }
    .sel-pop.drop-up {
        transform-origin: bottom left;
        animation-name: selFadeUp;
    }
    @keyframes selFade {
        from {
            opacity: 0;
            transform: translateY(-4px) scale(0.985);
        }
        to {
            opacity: 1;
            transform: translateY(0) scale(1);
        }
    }
    @keyframes selFadeUp {
        from {
            opacity: 0;
            transform: translateY(4px) scale(0.985);
        }
        to {
            opacity: 1;
            transform: translateY(0) scale(1);
        }
    }

    .sel-pop-inner {
        background: var(--bg-elevated);
        border: 1px solid var(--border-strong);
        border-radius: var(--radius-md);
        padding: 4px;
        max-height: inherit;
        overflow-y: auto;
        overscroll-behavior: contain;
    }

    .sel-opt {
        width: 100%;
        display: grid;
        grid-template-columns: 14px 1fr auto;
        align-items: center;
        gap: 9px;
        padding: 8px 10px 8px 9px;
        font-size: var(--fs-base);
        font-weight: 500;
        color: var(--text-secondary);
        background: transparent;
        border: 0;
        border-radius: var(--radius-sm);
        cursor: pointer;
        text-align: left;
        transition:
            background 0.12s var(--ease),
            color 0.12s var(--ease);
        font-variant-numeric: tabular-nums;
        letter-spacing: -0.005em;
    }
    .sel-opt:hover,
    .sel-opt:focus-visible {
        outline: none;
        background: var(--bg-card-hover);
        color: var(--text-primary);
        box-shadow: none;
    }
    .sel-opt.active {
        color: var(--text-primary);
        background: var(--bg-card-hover);
    }
    .sel-opt.active:hover {
        background: var(--border);
    }

    .sel-opt-mark {
        display: inline-flex;
        align-items: center;
        justify-content: center;
        width: 14px;
        height: 14px;
        color: var(--text-primary);
        flex-shrink: 0;
    }

    .sel-opt-label {
        min-width: 0;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .sel-opt-hint {
        font-size: var(--fs-xs);
        color: var(--text-muted);
        font-family: var(--font-mono);
        letter-spacing: var(--letter-wide);
    }
</style>
