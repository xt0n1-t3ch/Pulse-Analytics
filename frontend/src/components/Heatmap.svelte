<script lang="ts">
    import type { HourlyActivity } from "../lib/api";

    let { data = [] }: { data: HourlyActivity[] } = $props();

    const CELL = 18;
    const HOURS = Array.from({ length: 24 }, (_, i) => i);
    const hourFormatter = new Intl.DateTimeFormat(undefined, {
        hour: "numeric",
        hour12: true,
    });
    const localTimeZone =
        Intl.DateTimeFormat().resolvedOptions().timeZone || "Local time";

    let maxCount = $derived(Math.max(...data.map((d) => d.session_count), 1));
    let totalSessions = $derived(
        data.reduce((sum, entry) => sum + entry.session_count, 0),
    );
    let activeHours = $derived(
        data.filter((entry) => entry.session_count > 0).length,
    );
    let peak = $derived.by(() =>
        data.reduce<HourlyActivity | null>(
            (best, entry) =>
                !best || entry.session_count > best.session_count
                    ? entry
                    : best,
            null,
        ),
    );

    function intensity(hour: number): number {
        const entry = data.find((d) => d.hour === hour);
        if (!entry) return 0;
        return entry.session_count / maxCount;
    }

    function cellColor(val: number): string {
        if (val === 0) return "var(--bg-elevated)";
        if (val < 0.25)
            return "color-mix(in srgb, var(--accent) 25%, transparent)";
        if (val < 0.5)
            return "color-mix(in srgb, var(--accent) 50%, transparent)";
        if (val < 0.75)
            return "color-mix(in srgb, var(--accent) 75%, transparent)";
        return "var(--accent)";
    }

    function hourLabel(h: number): string {
        // The backend has already grouped UTC timestamps into the machine's local
        // hour. Intl owns the user's AM/PM convention instead of hand-built `5p`.
        return hourFormatter.format(new Date(2026, 0, 1, h, 0, 0));
    }

    function sessionCount(hour: number): number {
        return data.find((d) => d.hour === hour)?.session_count ?? 0;
    }
</script>

<div class="heatmap">
    <div
        class="heatmap-grid"
        role="img"
        aria-label={`${totalSessions} sessions. Sessions by local hour: ${HOURS.map((hour) => `${hourLabel(hour)} ${sessionCount(hour)}`).join(", ")}`}
    >
        {#each HOURS as h}
            <div class="hour-slot">
                <div
                    class="heatmap-cell"
                    style="background:{cellColor(intensity(h))};height:{CELL}px"
                    title="{hourLabel(h)}: {sessionCount(h)} sessions"
                ></div>
                <span class="heatmap-label">{hourLabel(h)}</span>
            </div>
        {/each}
    </div>
    <div class="heatmap-summary">
        <span>{totalSessions} sessions</span>
        <span>{activeHours} active hours</span>
        <span>Peak {peak ? hourLabel(peak.hour) : "—"}</span>
        <span class="timezone">Local time · {localTimeZone}</span>
    </div>
    <div class="heatmap-legend">
        <span class="legend-text">Less</span>
        {#each [0, 0.25, 0.5, 0.75, 1] as v}
            <div class="legend-cell" style="background:{cellColor(v)}"></div>
        {/each}
        <span class="legend-text">More</span>
    </div>
</div>

<style>
    .heatmap {
        display: flex;
        flex-direction: column;
        gap: 6px;
        max-width: 100%;
        min-width: 0;
    }
    .heatmap-grid {
        display: grid;
        grid-template-columns: repeat(24, minmax(4px, 1fr));
        gap: 2px;
    }
    .heatmap-cell {
        border-radius: 3px;
        outline: 1px solid transparent;
        transition:
            background 0.2s ease,
            outline-color 0.15s ease;
        cursor: default;
    }
    .heatmap-cell:hover {
        outline-color: var(--border-hover);
    }
    .heatmap-label {
        font-size: 9px;
        color: var(--text-muted);
        font-weight: 500;
    }
    .heatmap-legend {
        display: flex;
        align-items: center;
        gap: 3px;
        margin-top: 4px;
    }
    .heatmap-summary {
        display: flex;
        flex-wrap: wrap;
        gap: 6px 12px;
        color: var(--text-secondary);
        font: 600 10px var(--font-mono);
    }
    .legend-text {
        font-size: 9px;
        color: var(--text-muted);
    }
    .legend-cell {
        width: 10px;
        height: 10px;
        border-radius: 2px;
    }
    .heatmap-grid {
        grid-template-columns: repeat(12, minmax(0, 1fr));
        gap: 10px 5px;
    }
    .hour-slot {
        display: grid;
        gap: 4px;
        text-align: center;
        min-width: 0;
    }
    .heatmap-label {
        font-size: 9px;
        white-space: nowrap;
    }
    .heatmap-summary {
        justify-content: center;
        font-family: var(--font-sans);
        font-weight: 400;
    }
    .heatmap-legend {
        justify-content: center;
    }
    @media (max-width: 420px) {
        .heatmap-grid {
            grid-template-columns: repeat(6, minmax(0, 1fr));
        }
    }
</style>
