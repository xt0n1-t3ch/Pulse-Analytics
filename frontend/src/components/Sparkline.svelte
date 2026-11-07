<script lang="ts">
    let {
        data = [],
        color = "var(--accent)",
        width = 80,
        height = 24,
    }: {
        data: number[];
        color?: string;
        width?: number;
        height?: number;
    } = $props();

    let points = $derived.by(() => {
        if (data.length < 2) return "";
        const max = Math.max(...data, 1);
        const min = Math.min(...data, 0);
        const range = max - min || 1;
        const step = width / (data.length - 1);
        return data
            .map(
                (v, i) =>
                    `${i * step},${height - ((v - min) / range) * (height - 2) - 1}`,
            )
            .join(" ");
    });

    let trend = $derived.by(() => {
        if (data.length < 2) return 0;
        const recent =
            data.slice(-3).reduce((a, b) => a + b, 0) /
            Math.min(3, data.length);
        const older =
            data.slice(0, 3).reduce((a, b) => a + b, 0) /
            Math.min(3, data.length);
        if (older === 0) return 0;
        return ((recent - older) / older) * 100;
    });
</script>

{#if data.length >= 2}
    <div class="sparkline-wrap">
        <svg
            {width}
            {height}
            viewBox="0 0 {width} {height}"
            class="sparkline-svg"
        >
            <polyline
                {points}
                fill="none"
                stroke={color}
                stroke-width="1.5"
                stroke-linecap="round"
                stroke-linejoin="round"
            />
        </svg>
        {#if trend !== 0}
            <span
                class="spark-trend"
                class:up={trend > 0}
                class:down={trend < 0}
            >
                {trend > 0 ? "+" : ""}{trend.toFixed(0)}%
            </span>
        {/if}
    </div>
{/if}

<style>
    .sparkline-wrap {
        display: inline-flex;
        align-items: center;
        gap: 4px;
    }
    .sparkline-svg {
        display: block;
    }
    .spark-trend {
        font-size: 10px;
        font-weight: 600;
        font-variant-numeric: tabular-nums;
    }
    .spark-trend.up {
        color: var(--success);
    }
    .spark-trend.down {
        color: var(--danger);
    }
</style>
