<script lang="ts">
    import { currentView } from "../lib/stores";
    import PulseMark from "./PulseMark.svelte";

    const navItems = [
        { id: "dashboard", icon: "grid", title: "Dashboard" },
        { id: "sessions", icon: "messages", title: "Sessions" },
        { id: "costs", icon: "dollar", title: "Costs" },
        { id: "reports", icon: "reports", title: "Reports" },
        { id: "discord", icon: "discord", title: "Discord" },
    ];

    function switchView(id: string): void {
        currentView.set(id);
    }
</script>

<aside class="sidebar">
    <div class="sidebar-logo" title="Pulse" aria-label="Pulse">
        <PulseMark size={20} accent="var(--accent)" />
        <span class="sidebar-wordmark">Pulse</span>
    </div>

    <nav class="sidebar-nav">
        {#each navItems as item}
            <button
                class="nav-btn"
                class:active={$currentView === item.id}
                title={item.title}
                aria-label={item.title}
                onclick={() => switchView(item.id)}
            >
                {#if item.icon === "grid"}
                    <svg
                        width="20"
                        height="20"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        ><rect x="3" y="3" width="7" height="7" rx="1" /><rect
                            x="14"
                            y="3"
                            width="7"
                            height="7"
                            rx="1"
                        /><rect x="3" y="14" width="7" height="7" rx="1" /><rect
                            x="14"
                            y="14"
                            width="7"
                            height="7"
                            rx="1"
                        /></svg
                    >
                {:else if item.icon === "messages"}
                    <svg
                        width="20"
                        height="20"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        ><path
                            d="M21 15a2 2 0 01-2 2H7l-4 4V5a2 2 0 012-2h14a2 2 0 012 2z"
                        /></svg
                    >
                {:else if item.icon === "dollar"}
                    <svg
                        width="20"
                        height="20"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        ><line x1="12" y1="1" x2="12" y2="23" /><path
                            d="M17 5H9.5a3.5 3.5 0 000 7h5a3.5 3.5 0 010 7H6"
                        /></svg
                    >
                {:else if item.icon === "reports"}
                    <svg
                        width="20"
                        height="20"
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        ><path
                            d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z"
                        /><polyline points="14 2 14 8 20 8" /><line
                            x1="9"
                            y1="14"
                            x2="15"
                            y2="14"
                        /><line x1="9" y1="18" x2="15" y2="18" /><line
                            x1="9"
                            y1="10"
                            x2="10"
                            y2="10"
                        /></svg
                    >
                {:else if item.icon === "discord"}
                    <svg
                        width="20"
                        height="20"
                        viewBox="0 0 127.14 96.36"
                        fill="currentColor"
                        ><path
                            d="M107.7,8.07A105.15,105.15,0,0,0,81.47,0a72.06,72.06,0,0,0-3.36,6.83A97.68,97.68,0,0,0,49,6.83,72.37,72.37,0,0,0,45.64,0,105.89,105.89,0,0,0,19.39,8.09C2.79,32.65-1.71,56.6.54,80.21h0A105.73,105.73,0,0,0,32.71,96.36,77.7,77.7,0,0,0,39.6,85.25a68.42,68.42,0,0,1-10.85-5.18c.91-.66,1.8-1.34,2.66-2a75.57,75.57,0,0,0,64.32,0c.87.71,1.76,1.39,2.66,2a68.68,68.68,0,0,1-10.87,5.19,77,77,0,0,0,6.89,11.1A105.25,105.25,0,0,0,126.6,80.22h0C129.24,52.84,122.09,29.11,107.7,8.07ZM42.45,65.69C36.18,65.69,31,60,31,53s5-12.74,11.43-12.74S54,46,53.89,53,48.84,65.69,42.45,65.69Zm42.24,0C78.41,65.69,73.25,60,73.25,53s5-12.74,11.44-12.74S96.23,46,96.12,53,91.08,65.69,84.69,65.69Z"
                        /></svg
                    >
                {/if}
                <span class="nav-label">{item.title}</span>
            </button>
        {/each}
    </nav>

    <div class="sidebar-bottom">
        <button
            class="nav-btn"
            class:active={$currentView === "settings"}
            title="Settings"
            aria-label="Settings"
            onclick={() => switchView("settings")}
        >
            <svg
                width="20"
                height="20"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                ><circle cx="12" cy="12" r="3" /><path
                    d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-2 2 2 2 0 01-2-2v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83 0 2 2 0 010-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 01-2-2 2 2 0 012-2h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 010-2.83 2 2 0 012.83 0l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 012-2 2 2 0 012 2v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 0 2 2 0 010 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 012 2 2 2 0 01-2 2h-.09a1.65 1.65 0 00-1.51 1z"
                /></svg
            >
            <span class="nav-label">Settings</span>
        </button>
    </div>
</aside>

<style>
    .sidebar {
        position: fixed;
        left: 0;
        top: 0;
        bottom: 0;
        width: var(--sidebar-width);
        background: var(--bg-secondary);
        border-right: 1px solid var(--border);
        display: flex;
        flex-direction: column;
        align-items: stretch;
        padding: 0 10px 12px;
        z-index: 100;
    }

    .sidebar-logo {
        height: var(--topbar-height);
        width: 100%;
        display: flex;
        align-items: center;
        justify-content: flex-start;
        gap: 10px;
        padding: 0 12px;
        border-bottom: 1px solid var(--border);
        color: var(--text-primary);
        margin-bottom: 10px;
    }

    .sidebar-wordmark {
        font-size: 14px;
        font-weight: 800;
        letter-spacing: -0.02em;
        text-transform: uppercase;
    }

    .sidebar-nav {
        display: flex;
        flex-direction: column;
        gap: 4px;
        flex: 1;
        width: 100%;
    }

    .sidebar-bottom {
        margin-top: auto;
        padding-top: 10px;
        border-top: 1px solid var(--border);
        width: 100%;
        display: flex;
        justify-content: stretch;
    }

    .nav-btn {
        width: 100%;
        min-width: 0;
        height: 42px;
        display: flex;
        align-items: center;
        justify-content: flex-start;
        gap: 11px;
        padding: 0 12px;
        border-radius: var(--radius-md);
        color: var(--text-muted);
        transition: all 0.2s var(--ease);
    }

    .nav-btn:hover {
        color: var(--text-secondary);
        background: var(--bg-elevated);
    }

    .nav-btn.active {
        color: var(--accent);
        background: var(--accent-dim);
    }

    .nav-btn.active::before {
        content: "";
        width: 2px;
        height: 18px;
        margin-left: -12px;
        background: var(--accent);
        border-radius: var(--radius-full);
        corner-shape: round;
    }

    .nav-label {
        overflow: hidden;
        font-size: 12px;
        font-weight: 600;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    @media (max-width: 900px) {
        .sidebar {
            padding-inline: 8px;
        }
        .sidebar-logo {
            justify-content: center;
            padding: 0;
        }
        .sidebar-wordmark,
        .nav-label {
            display: none;
        }
        .nav-btn {
            width: 46px;
            justify-content: center;
            padding: 0;
            margin-inline: auto;
        }
        .nav-btn.active::before {
            position: absolute;
            left: 0;
            margin: 0;
        }
    }
</style>
