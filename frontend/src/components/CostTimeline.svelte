<script lang="ts">
    /**
     * Daily monetary-value chart with inflections marked directly on the curve.
     *
     * Drawn as inline SVG rather than Chart.js: the series is small, the shape is
     * fixed, and rendering it ourselves keeps every colour on a theme token and
     * lets the inflection markers live in the same coordinate space as the line
     * instead of a separate overlay that can drift out of alignment.
     */
    import type { DailyCostPoint, InflectionPoint } from "../lib/api";
    import { fmtCost } from "../lib/utils";

    let {
        points = [],
        inflections = [],
    }: { points: DailyCostPoint[]; inflections: InflectionPoint[] } = $props();

    // viewBox units. The chart scales to its container via width:100%.
    const W = 1000;
    const H = 260;
    const PAD = { top: 18, right: 16, bottom: 30, left: 52 };
    const plotW = W - PAD.left - PAD.right;
    const plotH = H - PAD.top - PAD.bottom;

    let maxCost = $derived(Math.max(...points.map((p) => p.cost), 0));
    /** Round the axis ceiling up so the curve never touches the top edge. */
    let ceiling = $derived(maxCost <= 0 ? 1 : niceCeiling(maxCost));

    function niceCeiling(value: number): number {
        const magnitude = 10 ** Math.floor(Math.log10(value));
        return Math.ceil(value / magnitude) * magnitude;
    }

    function x(index: number): number {
        if (points.length <= 1) return PAD.left + plotW / 2;
        return PAD.left + (index / (points.length - 1)) * plotW;
    }

    function y(cost: number): number {
        return PAD.top + plotH - (cost / ceiling) * plotH;
    }

    let linePath = $derived(
        points
            .map(
                (p, i) =>
                    `${i === 0 ? "M" : "L"}${x(i).toFixed(2)},${y(p.cost).toFixed(2)}`,
            )
            .join(" "),
    );

    /** The line path closed down to the baseline, for the gradient fill. */
    let areaPath = $derived(
        points.length === 0
            ? ""
            : `${linePath} L${x(points.length - 1).toFixed(2)},${(PAD.top + plotH).toFixed(2)} L${x(0).toFixed(2)},${(PAD.top + plotH).toFixed(2)} Z`,
    );

    /** Four horizontal gridlines plus the baseline. */
    let gridLines = $derived(
        [0, 0.25, 0.5, 0.75, 1].map((f) => ({
            y: PAD.top + plotH - f * plotH,
            label: fmtCost(ceiling * f),
        })),
    );

    /**
     * Inflection days resolved to a plotted point. An inflection whose date is
     * not in the window has no coordinates and is dropped rather than clamped to
     * an edge, which would put a marker on a day it did not happen.
     */
    let markers = $derived(
        inflections
            .map((inf) => {
                const index = points.findIndex((p) => p.date === inf.date);
                return index === -1
                    ? null
                    : { inf, index, cx: x(index), cy: y(points[index].cost) };
            })
            .filter((m): m is NonNullable<typeof m> => m !== null),
    );

    /**
     * Label the single most significant inflection so the chart explains itself.
     *
     * "Most significant" is distance from the baseline in either direction: a
     * drop reports a multiplier below 1 (0.2x), which is just as large a shift
     * as a 5x spike. Ranking on the raw multiplier alone would always favour
     * spikes and hide the biggest drop.
     */
    function deviation(multiplier: number): number {
        return multiplier >= 1 ? multiplier : 1 / Math.max(multiplier, 0.0001);
    }

    let headline = $derived(
        markers.length === 0
            ? null
            : markers.reduce((a, b) =>
                  deviation(b.inf.multiplier) > deviation(a.inf.multiplier)
                      ? b
                      : a,
              ),
    );

    /**
     * Phrase a multiplier the way a reader would say it out loud.
     *
     * The detector's multiplier is already `today / baseline`, so a drop is
     * stated as the share of the baseline that disappeared. Reporting its
     * reciprocal instead would claim a 0.2 multiplier "fell 5× below" the
     * baseline, which is not what a fifth of the baseline means.
     */
    function describe(inf: InflectionPoint): string {
        if (inf.multiplier < 1) {
            const drop = (1 - Math.max(inf.multiplier, 0)) * 100;
            return `fell ${drop.toFixed(0)}% below the rolling baseline (${inf.multiplier.toFixed(2)}× baseline)`;
        }
        return `ran ${inf.multiplier.toFixed(1)}× the rolling baseline`;
    }

    /**
     * Evenly spaced date ticks. The final label is only kept when it clears the
     * previous one, otherwise the last two dates collide and overprint each
     * other at the right edge.
     */
    let ticks = $derived.by(() => {
        if (points.length === 0) return [];
        const target = 8;
        const step = Math.max(1, Math.round(points.length / target));
        const chosen = points.map((_, i) => i).filter((i) => i % step === 0);
        const last = points.length - 1;
        // Roughly the width of a "MM/DD" label in viewBox units.
        const minGap = plotW / (target * 1.6);
        if (last > 0 && x(last) - x(chosen[chosen.length - 1]) >= minGap) {
            chosen.push(last);
        }
        return chosen.map((i) => ({
            x: x(i),
            label: points[i].date.slice(5).replace("-", "/"),
        }));
    });

    let hover = $state<number | null>(null);
    let hovered = $derived(hover === null ? null : (points[hover] ?? null));

    function onMove(event: MouseEvent): void {
        const svg = event.currentTarget as SVGSVGElement;
        const rect = svg.getBoundingClientRect();
        if (rect.width === 0 || points.length === 0) return;
        // Map pointer position into viewBox units, then to the nearest sample.
        const vbX = ((event.clientX - rect.left) / rect.width) * W;
        const ratio = (vbX - PAD.left) / plotW;
        const index = Math.round(ratio * (points.length - 1));
        hover = Math.min(points.length - 1, Math.max(0, index));
    }
</script>

<div class="timeline">
    {#if points.length === 0}
        <div class="tl-empty">No monetary value recorded in this window.</div>
    {:else}
        <svg
            class="tl-svg"
            viewBox="0 0 {W} {H}"
            preserveAspectRatio="none"
            role="img"
            aria-label="Daily monetary value timeline"
            onmousemove={onMove}
            onmouseleave={() => (hover = null)}
        >
            <defs>
                <linearGradient id="tl-fill" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" class="tl-stop-top" />
                    <stop offset="100%" class="tl-stop-bottom" />
                </linearGradient>
            </defs>

            {#each gridLines as line}
                <line
                    class="tl-grid"
                    x1={PAD.left}
                    y1={line.y}
                    x2={W - PAD.right}
                    y2={line.y}
                />
                <text
                    class="tl-axis"
                    x={PAD.left - 8}
                    y={line.y + 3}
                    text-anchor="end">{line.label}</text
                >
            {/each}

            <path class="tl-area" d={areaPath} fill="url(#tl-fill)" />
            <path class="tl-line" d={linePath} />

            {#each markers as m}
                <rect
                    class="tl-marker"
                    x={m.cx - 4.5}
                    y={m.cy - 4.5}
                    width="9"
                    height="9"
                    transform="rotate(45 {m.cx} {m.cy})"
                />
            {/each}

            {#if hovered && hover !== null}
                <line
                    class="tl-cursor"
                    x1={x(hover)}
                    y1={PAD.top}
                    x2={x(hover)}
                    y2={PAD.top + plotH}
                />
                <circle
                    class="tl-cursor-dot"
                    cx={x(hover)}
                    cy={y(hovered.cost)}
                    r="3.5"
                />
            {/if}

            {#each ticks as tick}
                <text class="tl-axis" x={tick.x} y={H - 10} text-anchor="middle"
                    >{tick.label}</text
                >
            {/each}
        </svg>

        <div class="tl-footer">
            {#if hovered}
                <span class="tl-read">
                    <span class="tl-read-date">{hovered.date}</span>
                    <span class="tl-read-cost">{fmtCost(hovered.cost)}</span>
                    <span class="tl-read-meta"
                        >{hovered.sessions}
                        {hovered.sessions === 1 ? "session" : "sessions"}</span
                    >
                </span>
            {:else if headline}
                <span class="tl-legend">
                    <span class="tl-legend-mark" aria-hidden="true"></span>
                    Monetary-value inflection · {headline.inf.date}
                    {describe(headline.inf)}
                </span>
            {:else}
                <span class="tl-legend tl-legend-quiet"
                    >No monetary-value inflections — the observed value held to
                    its baseline.</span
                >
            {/if}
        </div>
    {/if}
</div>

<style>
    .timeline {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }

    .tl-svg {
        width: 100%;
        height: 260px;
        display: block;
        overflow: visible;
    }

    .tl-grid {
        stroke: var(--border);
        stroke-width: 1;
        stroke-dasharray: 3 5;
        vector-effect: non-scaling-stroke;
    }
    .tl-axis {
        fill: var(--text-muted);
        font-family: var(--font-mono);
        font-size: 11px;
    }

    .tl-stop-top {
        stop-color: var(--success);
        stop-opacity: 0.28;
    }
    .tl-stop-bottom {
        stop-color: var(--success);
        stop-opacity: 0;
    }

    .tl-line {
        fill: none;
        stroke: var(--success);
        stroke-width: 2;
        stroke-linejoin: round;
        stroke-linecap: round;
        vector-effect: non-scaling-stroke;
    }

    .tl-marker {
        fill: var(--warning);
        stroke: var(--bg-card);
        stroke-width: 1.5;
        vector-effect: non-scaling-stroke;
    }

    .tl-cursor {
        stroke: var(--border-hover);
        stroke-width: 1;
        vector-effect: non-scaling-stroke;
    }
    .tl-cursor-dot {
        fill: var(--success);
        stroke: var(--bg-card);
        stroke-width: 2;
        vector-effect: non-scaling-stroke;
    }

    .tl-footer {
        min-height: 18px;
    }

    .tl-read {
        display: inline-flex;
        align-items: baseline;
        gap: 10px;
        font-size: var(--fs-sm);
    }
    .tl-read-date {
        font-family: var(--font-mono);
        color: var(--text-muted);
    }
    .tl-read-cost {
        font-family: var(--font-mono);
        font-weight: 700;
        color: var(--text-primary);
    }
    .tl-read-meta {
        color: var(--text-secondary);
    }

    .tl-legend {
        min-width: 0;
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 8px;
        font-size: var(--fs-sm);
        color: var(--text-secondary);
        overflow-wrap: anywhere;
    }
    .tl-legend-quiet {
        color: var(--text-muted);
    }
    .tl-legend-mark {
        width: 8px;
        height: 8px;
        background: var(--warning);
        transform: rotate(45deg);
        flex-shrink: 0;
    }

    .tl-empty {
        padding: 48px 0;
        text-align: center;
        font-size: var(--fs-sm);
        color: var(--text-muted);
    }
</style>
