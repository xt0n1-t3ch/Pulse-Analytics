<script lang="ts">
    let {
        size = 22,
        showPulse = true,
        accent = "currentColor",
    }: { size?: number; showPulse?: boolean; accent?: string } = $props();
</script>

<svg
    width={size}
    height={size}
    viewBox="0 0 32 32"
    fill="none"
    aria-hidden="true"
    class="pulse-mark"
>
    <!-- P stem + bowl — geometric, bold. Stroke-based so it inherits currentColor cleanly. -->
    <path
        d="M 9 7 H 18.5 C 23 7 25.5 9.8 25.5 13.6 C 25.5 17.4 23 20.2 18.5 20.2 H 13.5 V 25 H 9 Z M 13.5 10.6 V 16.6 H 18 C 20 16.6 21 15.6 21 13.6 C 21 11.6 20 10.6 18 10.6 Z"
        fill={accent}
    />
    {#if showPulse}
        <path
            d="M 6 27 L 10.5 27 L 12 24 L 14 30 L 15.5 26 L 18 27 L 26 27"
            fill="none"
            stroke={accent}
            stroke-width="1.4"
            stroke-linecap="round"
            stroke-linejoin="round"
            opacity="0.75"
        />
    {/if}
</svg>

<style>
    .pulse-mark {
        display: block;
        flex-shrink: 0;
    }
</style>
