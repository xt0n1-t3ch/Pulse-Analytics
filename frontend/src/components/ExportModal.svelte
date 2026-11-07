<script lang="ts">
    import { fly } from "svelte/transition";
    import { tick } from "svelte";
    import {
        type ExportColumn,
        type ExportFormat,
        type ExportDateConfig,
        defaultDateConfig,
        exportToString,
        saveExport,
    } from "../lib/export";
    import { addToast } from "../lib/stores";

    interface Props {
        open: boolean;
        title: string;
        defaultFilename: string;
        columns: ExportColumn[];
        rows: Record<string, unknown>[];
        onclose: () => void;
    }

    let { open, title, defaultFilename, columns, rows, onclose }: Props =
        $props();

    let format = $state<ExportFormat>("csv");
    let dateConfig = $state<ExportDateConfig>(defaultDateConfig());
    let localColumns = $state<ExportColumn[]>([]);
    let modalElement = $state<HTMLDivElement>();
    let previouslyFocused: HTMLElement | null = null;
    let wasOpen = false;

    $effect(() => {
        if (open) {
            localColumns = columns.map((c) => ({ ...c }));
        }
    });

    $effect(() => {
        if (open && !wasOpen) {
            previouslyFocused = document.activeElement as HTMLElement | null;
            void tick().then(() =>
                modalElement
                    ?.querySelector<HTMLElement>(
                        "button, input, select, [tabindex]:not([tabindex='-1'])",
                    )
                    ?.focus(),
            );
        } else if (!open && wasOpen) {
            previouslyFocused?.focus();
            previouslyFocused = null;
        }
        wasOpen = open;
    });

    function handleModalKey(event: KeyboardEvent): void {
        if (event.key === "Escape") {
            event.preventDefault();
            onclose();
            return;
        }
        if (event.key !== "Tab") return;
        if (!modalElement) return;
        const focusable = [
            ...modalElement.querySelectorAll<HTMLElement>(
                "button:not(:disabled), input:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex='-1'])",
            ),
        ];
        if (!focusable.length) return;
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        if (event.shiftKey && document.activeElement === first) {
            event.preventDefault();
            last.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
            event.preventDefault();
            first.focus();
        }
    }

    let previewRows = $derived(rows.slice(0, 3));
    let previewText = $derived(
        exportToString(previewRows, localColumns, format),
    );

    function doExport(): void {
        const content = exportToString(rows, localColumns, format);
        saveExport(content, format, defaultFilename);
        addToast(
            `Exported ${rows.length} rows as ${format.toUpperCase()}`,
            "success",
        );
        onclose();
    }

    function toggleAll(enabled: boolean): void {
        localColumns = localColumns.map((c) => ({ ...c, enabled }));
    }
</script>

{#if open}
    <div
        class="modal-backdrop"
        role="button"
        tabindex="-1"
        onclick={onclose}
        onkeydown={(e) => e.key === "Escape" && onclose()}
        transition:fly={{ duration: 150 }}
    >
        <div
            bind:this={modalElement}
            class="modal"
            role="dialog"
            tabindex="-1"
            aria-modal="true"
            aria-labelledby="export-modal-title"
            onclick={(e) => e.stopPropagation()}
            onkeydown={(e) => {
                e.stopPropagation();
                handleModalKey(e);
            }}
            transition:fly={{ y: 20, duration: 200 }}
        >
            <div class="modal-header">
                <h3 id="export-modal-title">{title}</h3>
                <button
                    class="close-btn"
                    aria-label="Close export dialog"
                    onclick={onclose}>✕</button
                >
            </div>

            <div class="modal-body">
                <div class="section">
                    <div class="section-label">Format</div>
                    <div class="format-row">
                        <label
                            class="radio-pill"
                            class:active={format === "csv"}
                        >
                            <input
                                type="radio"
                                bind:group={format}
                                value="csv"
                            /> CSV
                        </label>
                        <label
                            class="radio-pill"
                            class:active={format === "json"}
                        >
                            <input
                                type="radio"
                                bind:group={format}
                                value="json"
                            /> JSON
                        </label>
                        <label
                            class="radio-pill"
                            class:active={format === "tsv"}
                        >
                            <input
                                type="radio"
                                bind:group={format}
                                value="tsv"
                            /> TSV
                        </label>
                    </div>
                </div>

                <div class="section">
                    <div class="section-label">Date & Time</div>
                    <div class="date-grid">
                        <label class="check-row">
                            <input
                                type="checkbox"
                                bind:checked={dateConfig.includeDate}
                            />
                            <span>Date</span>
                        </label>
                        {#if dateConfig.includeDate}
                            <select bind:value={dateConfig.dateFormat}>
                                <option value="yyyy-mm-dd">YYYY-MM-DD</option>
                                <option value="dd/mm/yyyy">DD/MM/YYYY</option>
                                <option value="mm/dd/yyyy">MM/DD/YYYY</option>
                            </select>
                        {/if}
                        <label class="check-row">
                            <input
                                type="checkbox"
                                bind:checked={dateConfig.includeTime}
                            />
                            <span>Time</span>
                        </label>
                        {#if dateConfig.includeTime}
                            <div class="time-opts">
                                <select bind:value={dateConfig.timeFormat}>
                                    <option value="24h">24h</option>
                                    <option value="12h">12h (AM/PM)</option>
                                </select>
                                <label class="check-row small">
                                    <input
                                        type="checkbox"
                                        bind:checked={dateConfig.includeSeconds}
                                    />
                                    <span>Seconds</span>
                                </label>
                            </div>
                        {/if}
                    </div>
                </div>

                <div class="section">
                    <div class="section-label">
                        Columns
                        <span class="col-actions">
                            <button onclick={() => toggleAll(true)}>All</button>
                            <button onclick={() => toggleAll(false)}
                                >None</button
                            >
                        </span>
                    </div>
                    <div class="columns-grid">
                        {#each localColumns as col, i}
                            <label class="check-row">
                                <input
                                    type="checkbox"
                                    bind:checked={localColumns[i].enabled}
                                />
                                <span>{col.label}</span>
                            </label>
                        {/each}
                    </div>
                </div>

                <div class="section">
                    <div class="section-label">
                        Preview ({rows.length} rows total)
                    </div>
                    <pre class="preview">{previewText}</pre>
                </div>
            </div>

            <div class="modal-footer">
                <button class="btn secondary" onclick={onclose}>Cancel</button>
                <button class="btn primary" onclick={doExport}
                    >Export {rows.length} rows</button
                >
            </div>
        </div>
    </div>
{/if}

<style>
    .modal-backdrop {
        position: fixed;
        inset: 0;
        background: var(--modal-backdrop);
        display: flex;
        align-items: center;
        justify-content: center;
        z-index: 100;
        backdrop-filter: blur(4px);
    }
    .modal {
        background: var(--bg-card);
        border: 1px solid var(--border);
        border-radius: var(--radius-lg);
        width: min(520px, calc(100vw - 24px));
        max-height: min(80vh, calc(100vh - 24px));
        display: flex;
        flex-direction: column;
        box-shadow: var(--shadow-lg);
    }
    .modal-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        padding: 16px 20px;
        border-bottom: 1px solid var(--border);
    }
    .modal-header h3 {
        font-size: 14px;
        font-weight: 700;
    }
    .close-btn {
        width: 36px;
        height: 36px;
        font-size: 14px;
        color: var(--text-muted);
        border-radius: var(--radius-sm);
        transition: all 0.15s ease;
    }
    .close-btn:hover {
        color: var(--text-primary);
        background: var(--bg-elevated);
    }
    .modal-body {
        padding: 16px 20px;
        overflow-y: auto;
        display: flex;
        flex-direction: column;
        gap: 16px;
    }
    .modal-footer {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
        padding: 12px 20px;
        border-top: 1px solid var(--border);
    }

    .section-label {
        font-size: 10px;
        font-weight: 700;
        text-transform: uppercase;
        letter-spacing: 0.06em;
        color: var(--text-muted);
        margin-bottom: 8px;
        display: flex;
        justify-content: space-between;
        align-items: center;
    }

    .format-row {
        display: flex;
        gap: 6px;
    }
    .radio-pill {
        font-size: 12px;
        padding: 5px 14px;
        border: 1px solid var(--border);
        border-radius: var(--radius-sm);
        cursor: pointer;
        color: var(--text-secondary);
        transition: all 0.15s ease;
    }
    .radio-pill input {
        display: none;
    }
    .radio-pill.active {
        border-color: var(--accent);
        color: var(--accent);
        background: var(--accent-dim);
    }
    .radio-pill:hover {
        border-color: var(--border-hover);
    }

    .date-grid {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }
    .time-opts {
        display: flex;
        gap: 8px;
        align-items: center;
        margin-left: 22px;
    }

    .check-row {
        display: flex;
        align-items: center;
        gap: 6px;
        font-size: 12px;
        color: var(--text-secondary);
        cursor: pointer;
    }
    .check-row.small {
        font-size: 11px;
    }
    .check-row input[type="checkbox"] {
        accent-color: var(--accent);
        width: 14px;
        height: 14px;
    }

    .col-actions {
        display: flex;
        gap: 4px;
    }
    .col-actions button {
        font-size: 10px;
        color: var(--accent);
        padding: 2px 6px;
        border-radius: 3px;
        transition: background 0.15s ease;
    }
    .col-actions button:hover {
        background: var(--accent-dim);
    }

    .columns-grid {
        display: grid;
        grid-template-columns: 1fr 1fr;
        gap: 4px 12px;
    }

    .preview {
        font-family: var(--font-mono);
        font-size: 10px;
        background: var(--bg-primary);
        border: 1px solid var(--border);
        border-radius: var(--radius-sm);
        padding: 10px;
        overflow-x: auto;
        color: var(--text-secondary);
        max-height: 120px;
        white-space: pre;
        line-height: 1.5;
    }

    .btn {
        min-height: 38px;
        font-size: 12px;
        font-weight: 600;
        padding: 8px 16px;
        border-radius: var(--radius-sm);
        transition: all 0.15s ease;
    }
    .btn.secondary {
        color: var(--text-secondary);
        background: var(--bg-elevated);
    }
    .btn.secondary:hover {
        background: var(--bg-input);
        color: var(--text-primary);
    }
    .btn.primary {
        color: var(--accent-fg);
        background: var(--accent);
    }
    .btn.primary:hover {
        background: var(--accent-hover);
    }
    @media (max-width: 560px) {
        .modal-header,
        .modal-body,
        .modal-footer {
            padding-left: 14px;
            padding-right: 14px;
        }
        .columns-grid {
            grid-template-columns: 1fr;
        }
    }
</style>
