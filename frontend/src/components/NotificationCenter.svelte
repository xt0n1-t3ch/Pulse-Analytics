<script lang="ts">
    import { onMount, tick } from "svelte";
    import { emit, listen, type UnlistenFn } from "@tauri-apps/api/event";
    import { IconBell, IconX } from "@tabler/icons-svelte";
    import {
        dismissNotification,
        dismissAllNotifications,
        restoreNotifications,
        markNotificationUnread,
        markAllNotificationsUnread,
        getNotifications,
        getUnreadNotificationCount,
        markAllNotificationsRead,
        markNotificationRead,
    } from "../lib/api";
    import SegmentedControl from "./SegmentedControl.svelte";
    import type { PulseNotification } from "../lib/api";
    import {
        accessSnapshot,
        backendConnection,
        currentView,
        selectedAccessSourceId,
        selectedAnalyticsProviderScope,
    } from "../lib/stores";
    import { authenticatedAccessRoutes } from "../lib/access";
    import { provider, setProvider, type Provider } from "../lib/provider";

    let open = $state(false);
    let onlyUnread = $state(false);
    let confirmClear = $state(false);
    type UndoInfo = { token: string; count: number };
    const undoStorageKey = "pulse-notification-undo";
    function savedUndo(): UndoInfo | null {
        try {
            const value = JSON.parse(
                localStorage.getItem(undoStorageKey) ?? "null",
            );
            return value &&
                typeof value.token === "string" &&
                Number.isFinite(Date.parse(value.token)) &&
                Number.isInteger(value.count) &&
                value.count > 0
                ? value
                : null;
        } catch {
            return null;
        }
    }
    let undoInfo = $state<UndoInfo | null>(savedUndo());
    let rootElement = $state<HTMLDivElement>();
    let triggerElement = $state<HTMLButtonElement>();
    let closeElement = $state<HTMLButtonElement>();
    function closePanel(restoreFocus = true): void {
        confirmClear = false;
        open = false;
        if (restoreFocus) triggerElement?.focus();
    }

    let loading = $state(false);
    let mutating = $state(false);
    let errorMessage = $state<string | null>(null);
    let notifications = $state<PulseNotification[]>([]);
    let visibleNotifications = $derived(
        onlyUnread
            ? notifications.filter((item) => !item.read_at)
            : notifications,
    );

    let unreadCount = $state(0);
    let refreshGeneration = 0;

    function relativeTime(iso: string): string {
        const timestamp = Date.parse(iso);
        if (!Number.isFinite(timestamp)) return "Recently";
        const seconds = Math.max(
            0,
            Math.floor((Date.now() - timestamp) / 1000),
        );
        if (seconds < 60) return "Now";
        const minutes = Math.floor(seconds / 60);
        if (minutes < 60) return `${minutes}m`;
        const hours = Math.floor(minutes / 60);
        if (hours < 24) return `${hours}h`;
        return `${Math.floor(hours / 24)}d`;
    }

    function destination(
        notification: PulseNotification,
    ): "dashboard" | "discord" | "settings" {
        if (notification.kind === "discord_connectivity") return "discord";
        if (notification.kind === "provider_health") return "settings";
        return "dashboard";
    }

    async function refresh(): Promise<void> {
        const generation = ++refreshGeneration;
        try {
            const [nextNotifications, nextUnread] = await Promise.all([
                getNotifications(50),
                getUnreadNotificationCount(),
            ]);
            if (generation !== refreshGeneration) return;
            notifications = nextNotifications;
            unreadCount = nextUnread;
            errorMessage = null;
        } catch {
            if (generation !== refreshGeneration) return;
            // Preserve the last provider-backed snapshot. Clearing it would make a
            // transport failure look like an authoritative empty notification feed.
            errorMessage =
                $backendConnection === "disconnected"
                    ? "Pulse is disconnected. These are your last saved alerts."
                    : "Pulse could not load notification history. Showing the last loaded alerts.";
        }
    }

    async function openPanel(): Promise<void> {
        open = true;
        loading = true;
        await refresh();
        loading = false;
        await tick();
        closeElement?.focus();
    }

    async function toggle(): Promise<void> {
        if (open) {
            closePanel();
            return;
        }
        await openPanel();
    }

    async function openNotification(
        notification: PulseNotification,
    ): Promise<void> {
        // Navigation is the primary action. A persistence failure must not strand
        // the user inside the notification center.
        if (
            notification.kind === "quota_threshold" ||
            notification.kind === "quota_reset" ||
            notification.kind === "provider_health"
        ) {
            const matchingRoute = authenticatedAccessRoutes(
                $accessSnapshot?.routes ?? [],
            ).find((route) => route.source.provider === notification.provider);
            if (!matchingRoute) {
                errorMessage =
                    "The notification source is no longer available.";
                return;
            }
            const nextProvider: Provider | null =
                matchingRoute.source.kind === "claude_subscription"
                    ? "claude"
                    : matchingRoute.source.kind === "codex_subscription"
                      ? "codex"
                      : matchingRoute.source.kind === "open_code_go"
                        ? "opencode"
                        : null;
            if (nextProvider && nextProvider !== $provider) {
                try {
                    await setProvider(nextProvider);
                } catch {
                    errorMessage =
                        "Pulse could not switch to the notification provider.";
                    return;
                }
            }
            selectedAccessSourceId.set(matchingRoute.source.id);
            selectedAnalyticsProviderScope.set(matchingRoute.source.provider);
        }
        currentView.set(destination(notification));
        open = false;
        if (notification.read_at) return;
        try {
            await markNotificationRead(notification.id);
            void emit("pulse://notification-state-changed").catch(
                () => undefined,
            );
            await refresh();
        } catch {
            errorMessage =
                "The read status could not be confirmed. Refresh to check the saved state.";
        }
    }

    async function markAllRead(): Promise<void> {
        if (mutating) return;
        mutating = true;
        try {
            await markAllNotificationsRead();
            void emit("pulse://notification-state-changed").catch(
                () => undefined,
            );
            await refresh();
        } catch {
            errorMessage =
                "The read update could not be confirmed. Refresh to check the saved state.";
        } finally {
            mutating = false;
        }
    }

    async function dismiss(
        event: MouseEvent,
        notification: PulseNotification,
    ): Promise<void> {
        event.stopPropagation();
        if (mutating) return;
        mutating = true;
        try {
            await dismissNotification(notification.id);
            void emit("pulse://notification-state-changed").catch(
                () => undefined,
            );
            await refresh();
        } catch {
            errorMessage =
                "The dismissal could not be confirmed. Refresh to check the list.";
        } finally {
            mutating = false;
        }
    }

    async function mutate(
        action: () => Promise<unknown>,
        failure: string,
    ): Promise<void> {
        if (mutating) return;
        mutating = true;
        try {
            await action();
            void emit("pulse://notification-state-changed").catch(
                () => undefined,
            );
            await refresh();
        } catch {
            errorMessage = failure;
        } finally {
            mutating = false;
        }
    }

    function toggleRead(notification: PulseNotification): void {
        void mutate(
            () =>
                notification.read_at
                    ? markNotificationUnread(notification.id)
                    : markNotificationRead(notification.id),
            "The read change could not be confirmed. Refresh to check the saved state.",
        );
    }

    function markAllUnread(): void {
        void mutate(
            markAllNotificationsUnread,
            "Pulse could not mark your alerts as unread. No confirmation was received.",
        );
    }

    function clearAll(): void {
        confirmClear = false;
        void mutate(async () => {
            const result = await dismissAllNotifications();
            if (result.count > 0) {
                undoInfo = { token: result.undo_token, count: result.count };
                localStorage.setItem(undoStorageKey, JSON.stringify(undoInfo));
            }
        }, "The clear action could not be confirmed. Refresh the list before trying again.");
    }

    function undoClear(): void {
        const previous = undoInfo;
        if (!previous) return;
        void mutate(async () => {
            await restoreNotifications(previous.token);
            undoInfo = null;
            localStorage.removeItem(undoStorageKey);
        }, "Pulse could not restore the cleared alerts. Try Undo again.");
    }

    onMount(() => {
        const unlisteners: UnlistenFn[] = [];
        let destroyed = false;
        const onKey = (event: KeyboardEvent) => {
            if (open && event.key === "Escape") {
                event.preventDefault();
                closePanel();
            }
        };
        const onOutside = (event: PointerEvent) => {
            if (
                open &&
                event.target instanceof Node &&
                !rootElement?.contains(event.target)
            )
                closePanel(false);
        };
        window.addEventListener("keydown", onKey);
        document.addEventListener("pointerdown", onOutside);
        const interval = window.setInterval(() => void refresh(), 5_000);
        void listen("pulse://notification", () => void refresh())
            .then((stop) => {
                if (destroyed) stop();
                else unlisteners.push(stop);
            })
            .catch(() => undefined);
        void listen("pulse://open-notifications", () => void openPanel())
            .then((stop) => {
                if (destroyed) stop();
                else unlisteners.push(stop);
            })
            .catch(() => undefined);
        void refresh();
        return () => {
            destroyed = true;
            window.removeEventListener("keydown", onKey);
            document.removeEventListener("pointerdown", onOutside);
            window.clearInterval(interval);
            for (const unlisten of unlisteners) unlisten();
        };
    });
</script>

<div class="notification-center" bind:this={rootElement}>
    <button
        class="notification-trigger"
        bind:this={triggerElement}
        class:active={open}
        title="Notifications"
        aria-label={unreadCount > 0
            ? `Notifications, ${unreadCount} unread`
            : "Notifications"}
        aria-expanded={open}
        aria-haspopup="dialog"
        onclick={toggle}
    >
        <IconBell size={17} stroke={1.7} />
        {#if unreadCount > 0}<span class="unread-badge"
                >{unreadCount > 99 ? "99+" : unreadCount}</span
            >{/if}
    </button>
    {#if open}
        <div
            class="notification-panel"
            role="dialog"
            aria-label="Notification center"
        >
            <header>
                <div>
                    <h2>Notifications</h2>
                    <p>
                        {loading
                            ? "Reading your local history…"
                            : `${unreadCount} unread · ${notifications.length} shown`}
                    </p>
                </div>
                <button
                    class="panel-close"
                    type="button"
                    aria-label="Close notifications"
                    bind:this={closeElement}
                    onclick={() => closePanel()}><IconX size={18} /></button
                >
            </header>
            <div class="notification-filters">
                <SegmentedControl
                    ariaLabel="Notification filter"
                    value={onlyUnread ? "unread" : "all"}
                    options={[
                        { value: "all", label: "All activity" },
                        {
                            value: "unread",
                            label: `Unread${unreadCount > 0 ? ` (${unreadCount})` : ""}`,
                        },
                    ]}
                    onchange={(value) => (onlyUnread = value === "unread")}
                />
            </div>
            <div
                class="bulk-actions"
                role="group"
                aria-label="Notification actions"
            >
                <button
                    type="button"
                    disabled={loading ||
                        mutating ||
                        !!errorMessage ||
                        unreadCount === 0}
                    onclick={markAllRead}>Mark all read</button
                >
                <button
                    type="button"
                    disabled={loading ||
                        mutating ||
                        !!errorMessage ||
                        notifications.length === 0}
                    onclick={markAllUnread}>Mark all unread</button
                >
                <button
                    type="button"
                    class="clear-all"
                    disabled={loading ||
                        mutating ||
                        !!errorMessage ||
                        notifications.length === 0}
                    onclick={() => (confirmClear = true)}>Clear all</button
                >
            </div>
            {#if confirmClear}
                <div
                    class="confirmation"
                    role="group"
                    aria-label="Confirm clearing notifications"
                >
                    <div>
                        <strong>Clear all notifications?</strong>
                        <p>
                            This removes them from this list. You can undo the
                            last confirmed clear.
                        </p>
                    </div>
                    <div class="confirmation-actions">
                        <button
                            type="button"
                            onclick={() => (confirmClear = false)}
                            >Cancel</button
                        ><button
                            type="button"
                            class="clear-all"
                            onclick={clearAll}>Clear notifications</button
                        >
                    </div>
                </div>
            {/if}
            {#if undoInfo}
                <div class="undo-bar" role="status">
                    <span>{undoInfo.count} notifications cleared</span><button
                        type="button"
                        disabled={mutating}
                        onclick={undoClear}>Undo</button
                    >
                </div>
            {/if}
            {#if errorMessage}
                <div class="sync-error" role="alert">
                    <div>
                        <strong
                            >{$backendConnection === "disconnected"
                                ? "Waiting for Pulse"
                                : "History could not be updated"}</strong
                        >
                        <p>{errorMessage}</p>
                    </div>
                    <button
                        type="button"
                        disabled={loading}
                        onclick={() => void refresh()}>Retry</button
                    >
                </div>
            {/if}
            <div
                class="notification-list"
                aria-live="polite"
                aria-busy={loading || mutating}
            >
                {#if loading}
                    <div class="empty-state">Loading notifications…</div>
                {:else if visibleNotifications.length === 0}
                    <div class="empty-state">
                        <strong
                            >{onlyUnread
                                ? "No unread notifications"
                                : "All clear"}</strong
                        ><span
                            >{onlyUnread
                                ? "Your alerts are up to date."
                                : "Quota resets and provider alerts will appear here."}</span
                        >
                    </div>
                {:else}
                    {#each visibleNotifications as notification (notification.id)}
                        <div
                            class="notification-row"
                            class:unread={!notification.read_at}
                        >
                            <button
                                class="event-action"
                                onclick={() => openNotification(notification)}
                            >
                                <span
                                    class="event-mark"
                                    data-kind={notification.kind}
                                    aria-hidden="true"
                                ></span>
                                <span class="event-copy"
                                    ><span class="event-title"
                                        >{notification.title}</span
                                    ><span class="event-body"
                                        >{notification.body}</span
                                    ><span class="event-meta"
                                        >{notification.provider ?? "Pulse"} · {relativeTime(
                                            notification.created_at,
                                        )}{#if notification.action}
                                            · {notification.action}{/if}</span
                                    ></span
                                >
                            </button>
                            <div class="item-actions">
                                <button
                                    type="button"
                                    disabled={mutating || !!errorMessage}
                                    aria-label={`${notification.read_at ? "Mark unread" : "Mark read"}: ${notification.title}`}
                                    onclick={() => toggleRead(notification)}
                                    >{notification.read_at
                                        ? "Mark unread"
                                        : "Mark read"}</button
                                >
                                <button
                                    class="dismiss"
                                    type="button"
                                    aria-label={`Dismiss ${notification.title}`}
                                    disabled={mutating || !!errorMessage}
                                    onclick={(event) =>
                                        dismiss(event, notification)}
                                    ><IconX size={15} /></button
                                >
                            </div>
                        </div>
                    {/each}
                {/if}
            </div>
        </div>
    {/if}
</div>

<style>
    .notification-center {
        position: relative;
        width: 30px;
        height: 30px;
        display: grid;
        place-items: center;
        flex: 0 0 30px;
        -webkit-app-region: no-drag;
    }
    .notification-trigger {
        position: relative;
        width: 30px;
        height: 30px;
        display: grid;
        place-items: center;
        color: var(--text-muted);
        border-radius: var(--radius-md);
    }
    .notification-trigger:hover,
    .notification-trigger.active {
        color: var(--text-primary);
        background: var(--bg-elevated);
    }
    .unread-badge {
        position: absolute;
        top: 2px;
        right: 1px;
        min-width: 14px;
        height: 14px;
        padding: 0 3px;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        color: var(--on-danger);
        background: var(--notification-badge-bg);
        border: 2px solid var(--bg-primary);
        border-radius: var(--radius-full);
        corner-shape: round;
        font-size: 8px;
        font-weight: 800;
        line-height: 1;
    }
    .notification-panel {
        position: fixed;
        z-index: 120;
        top: calc(var(--topbar-height) + 8px);
        right: 12px;
        width: min(420px, calc(100vw - 24px));
        max-height: min(740px, calc(100dvh - 76px));
        display: flex;
        flex-direction: column;
        overflow: hidden;
        color: var(--text-primary);
        background: var(--bg-primary);
        border: 1px solid var(--border-hover);
        border-radius: var(--radius-lg);
        box-shadow: var(--elev-3);
    }
    .notification-panel header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
        padding: 16px;
    }
    h2 {
        margin: 0;
        font-size: 17px;
        font-weight: 650;
    }
    header p {
        margin-top: 4px;
        font-size: 12px;
        color: var(--text-muted);
    }
    .panel-close {
        display: grid;
        place-items: center;
        width: 32px;
        height: 32px;
        color: var(--text-secondary);
        border-radius: var(--radius-sm);
    }
    .panel-close:hover {
        background: var(--bg-elevated);
    }
    .notification-filters {
        padding: 0 16px 12px;
    }
    .notification-filters :global(.segmented) {
        display: flex;
        width: 100%;
        height: 36px;
    }
    .notification-filters :global(.seg-opt) {
        flex: 1;
        justify-content: center;
    }
    .bulk-actions {
        display: grid;
        grid-template-columns: repeat(3, minmax(0, 1fr));
        gap: 6px;
        padding: 0 16px 14px;
        border-bottom: 1px solid var(--divider);
    }
    .bulk-actions button,
    .confirmation-actions button {
        min-height: 34px;
        padding: 6px;
        color: var(--text-primary);
        background: var(--surface-panel-soft);
        border: 1px solid var(--border);
        border-radius: var(--radius-sm);
        font-size: 11px;
        line-height: 1.3;
    }
    .bulk-actions button:hover:not(:disabled),
    .confirmation-actions button:hover {
        background: var(--bg-elevated);
        border-color: var(--text-muted);
    }
    button.clear-all {
        color: var(--danger);
    }
    button:disabled {
        opacity: 0.45;
        cursor: not-allowed;
    }
    .confirmation,
    .sync-error {
        display: flex;
        flex-direction: column;
        gap: 10px;
        padding: 14px 16px;
        background: var(--surface-panel-soft);
        border-bottom: 1px solid var(--divider);
    }
    .confirmation strong,
    .sync-error strong {
        font-size: 12px;
    }
    .confirmation p,
    .sync-error p {
        margin-top: 4px;
        color: var(--text-secondary);
        font-size: 12px;
        line-height: 1.45;
    }
    .confirmation-actions {
        display: flex;
        justify-content: flex-end;
        gap: 8px;
    }
    .confirmation-actions button {
        padding-inline: 12px;
    }
    .sync-error {
        flex-direction: row;
        align-items: center;
    }
    .sync-error > div {
        flex: 1;
        min-width: 0;
    }
    .sync-error button,
    .undo-bar button {
        min-height: 32px;
        padding: 5px 10px;
        color: var(--text-primary);
        border: 1px solid var(--border-strong);
        border-radius: var(--radius-sm);
        font-size: 12px;
    }
    .undo-bar {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 10px;
        padding: 10px 16px;
        border-bottom: 1px solid var(--divider);
        color: var(--text-secondary);
        font-size: 12px;
    }
    .notification-list {
        min-height: 0;
        overflow-y: auto;
        overscroll-behavior: contain;
    }
    .notification-row {
        border-bottom: 1px solid var(--divider);
        padding: 12px 16px 8px;
    }
    .notification-row:last-child {
        border-bottom: 0;
    }
    .notification-row.unread {
        background: var(--surface-panel-soft);
    }
    .event-action {
        width: 100%;
        min-width: 0;
        display: grid;
        grid-template-columns: 7px minmax(0, 1fr);
        align-items: start;
        gap: 10px;
        color: var(--text-primary);
        text-align: left;
    }
    .event-mark {
        width: 6px;
        height: 6px;
        margin-top: 5px;
        border-radius: 50%;
        corner-shape: round;
        background: var(--text-muted);
    }
    .event-mark[data-kind="quota_reset"] {
        background: var(--success);
    }
    .event-mark[data-kind="quota_threshold"] {
        background: var(--warning);
    }
    .event-copy {
        display: grid;
        gap: 5px;
        min-width: 0;
    }
    .event-title {
        font-size: 13px;
        font-weight: 650;
        line-height: 1.35;
        overflow-wrap: anywhere;
    }
    .event-body {
        color: var(--text-secondary);
        font-size: 12px;
        line-height: 1.45;
    }
    .event-meta {
        color: var(--text-muted);
        font-size: 11px;
        line-height: 1.4;
    }
    .item-actions {
        display: flex;
        align-items: center;
        justify-content: flex-end;
        gap: 8px;
        margin-top: 4px;
    }
    .item-actions button {
        min-height: 28px;
        padding: 3px 7px;
        color: var(--text-secondary);
        font-size: 11px;
        border-radius: var(--radius-sm);
    }
    .item-actions button:hover:not(:disabled) {
        color: var(--text-primary);
        background: var(--bg-elevated);
    }
    .dismiss {
        display: grid;
        place-items: center;
        min-width: 28px;
    }
    .empty-state {
        min-height: 120px;
        display: grid;
        align-content: center;
        justify-items: center;
        gap: 7px;
        padding: 24px 16px;
        text-align: center;
        color: var(--text-muted);
        font-size: 12px;
    }
    .empty-state strong {
        font-size: 14px;
        color: var(--text-primary);
    }
    .notification-panel button:focus-visible {
        outline: 2px solid var(--text-primary);
        outline-offset: 2px;
    }
    @media (max-width: 620px) {
        .notification-panel {
            top: 52px;
            left: 12px;
            right: 12px;
            width: auto;
            max-height: calc(100dvh - 64px);
        }
    }
</style>
