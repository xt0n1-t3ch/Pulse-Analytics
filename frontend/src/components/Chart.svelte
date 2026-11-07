<script lang="ts">
    import { onMount } from "svelte";
    import { Chart, type ChartConfiguration } from "chart.js/auto";

    let {
        config,
        updateData,
    }: {
        config: ChartConfiguration;
        updateData?: (chart: Chart) => void;
    } = $props();

    let canvas: HTMLCanvasElement;
    let chart: Chart | null = null;

    function syncChartTheme(): void {
        const styles = getComputedStyle(document.documentElement);
        const text = styles.getPropertyValue("--text-muted").trim();
        const border = styles.getPropertyValue("--border").trim();
        Chart.defaults.color = text;
        Chart.defaults.borderColor = border;
        if (chart) {
            chart.options.color = text;
            chart.options.borderColor = border;
            chart.update("none");
        }
    }

    onMount(() => {
        syncChartTheme();
        chart = new Chart(canvas, config);
        const observer = new MutationObserver(syncChartTheme);
        observer.observe(document.documentElement, {
            attributes: true,
            attributeFilter: ["data-theme"],
        });
        return () => {
            observer.disconnect();
            chart?.destroy();
        };
    });

    $effect(() => {
        if (chart && updateData) {
            updateData(chart);
            chart.update("none");
        }
    });
</script>

<div class="chart-wrap">
    <canvas bind:this={canvas}></canvas>
</div>

<style>
    .chart-wrap {
        position: relative;
        width: 100%;
        height: 100%;
    }
</style>
