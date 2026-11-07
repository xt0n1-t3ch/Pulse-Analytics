import { fireEvent, render, waitFor } from "@testing-library/svelte";
import { get } from "svelte/store";
import { tick } from "svelte";
import { beforeEach, describe, expect, it } from "bun:test";
import { vi } from "../bun-setup";
import NotificationCenter from "@/components/NotificationCenter.svelte";
import {
    accessSnapshot,
    currentView,
    selectedAccessSourceId,
} from "@/lib/stores";

const api = vi.hoisted(() => ({
    getNotifications: vi.fn(),
    getUnreadNotificationCount: vi.fn(),
    markNotificationRead: vi.fn(),
    markAllNotificationsRead: vi.fn(),
    markNotificationUnread: vi.fn(),
    markAllNotificationsUnread: vi.fn(),
    dismissAllNotifications: vi.fn(),
    restoreNotifications: vi.fn(),
    dismissNotification: vi.fn(),
    persistActiveProvider: vi.fn(),
    getProviderCopy: vi.fn(),
}));

const tauriEvents = vi.hoisted(() => ({
    listeners: new Map<string, () => void>(),
    emit: vi.fn(async () => undefined),
    listen: vi.fn(async (event: string, handler: () => void) => {
        tauriEvents.listeners.set(event, handler);
        return () => tauriEvents.listeners.delete(event);
    }),
}));

vi.mock("@/lib/api", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/api")>()),
    ...api,
}));

vi.mock("@tauri-apps/api/event", () => ({
    emit: tauriEvents.emit,
    listen: tauriEvents.listen,
}));

const notification = {
    id: 42,
    kind: "quota_threshold" as const,
    provider: "claude",
    key: "claude:weekly:80",
    title: "Claude weekly usage reached 80%",
    body: "Provider-reported weekly allowance is now above the configured threshold.",
    action: "Open quota details",
    created_at: "2026-08-01T14:00:00Z",
    read_at: null,
    dismissed_at: null,
};

describe("NotificationCenter", () => {
    beforeEach(async () => {
        localStorage.removeItem("pulse-notification-undo");
        const { provider } = await import("@/lib/provider");
        provider.set("claude");
        tauriEvents.listeners.clear();
        tauriEvents.emit.mockClear();
        tauriEvents.listen.mockClear();
        currentView.set("sessions");
        selectedAccessSourceId.set("all");
        accessSnapshot.set({
            routes: [
                {
                    local_history: { available: false, sessions: 0 },
                    source: {
                        id: "claude-subscription",
                        kind: "claude_subscription",
                        provider: "claude",
                        auth_method: "oauth",
                        proof: "quota_response",
                        plan: "Max 20x",
                    },
                    availability: "available",
                    freshness: "fresh",
                    provenance: "provider_api",
                    observed_at: "2026-08-01T14:00:00Z",
                    fetched_at: "2026-08-01T14:00:01Z",
                    expires_at: "2026-08-01T14:00:31Z",
                    windows: [],
                    credits: null,
                    extra_usage: null,
                    error: null,
                },
            ],
        });
        for (const mock of Object.values(api)) mock.mockReset();
        api.getNotifications.mockResolvedValue([notification]);
        api.getUnreadNotificationCount.mockResolvedValue(1);
        api.markNotificationRead.mockResolvedValue(true);
        api.markAllNotificationsRead.mockResolvedValue(1);
        api.dismissNotification.mockResolvedValue(true);
        api.markNotificationUnread.mockResolvedValue(true);
        api.markAllNotificationsUnread.mockResolvedValue(1);
        api.dismissAllNotifications.mockResolvedValue({
            count: 1,
            undo_token: "2026-09-05T03:00:00Z",
        });
        api.restoreNotifications.mockResolvedValue(1);
        api.persistActiveProvider.mockResolvedValue(undefined);
        api.getProviderCopy.mockResolvedValue({});
    });

    it("loads durable notifications and routes quota actions to Home", async () => {
        const { getByRole, getByText } = render(NotificationCenter);

        await waitFor(() =>
            expect(getByRole("button", { name: /1 unread/ })).toBeTruthy(),
        );
        await fireEvent.click(getByRole("button", { name: /1 unread/ }));
        await waitFor(() =>
            expect(getByText("Claude weekly usage reached 80%")).toBeTruthy(),
        );

        const row = getByText("Claude weekly usage reached 80%").closest(
            "button",
        );
        expect(row).toBeTruthy();
        await fireEvent.click(row!);
        expect(api.markNotificationRead).toHaveBeenCalledWith(42);
        expect(get(currentView)).toBe("dashboard");
    });

    it("switches the active provider before opening a notification from another lane", async () => {
        const { provider } = await import("@/lib/provider");
        const { getByRole, getByText } = render(NotificationCenter);

        await waitFor(() =>
            expect(getByRole("button", { name: /1 unread/ })).toBeTruthy(),
        );
        await fireEvent.click(getByRole("button", { name: /1 unread/ }));
        await waitFor(() => expect(getByText(notification.title)).toBeTruthy());
        provider.set("codex");
        await tick();
        expect(get(provider)).toBe("codex");
        await fireEvent.click(getByText(notification.title).closest("button")!);

        await waitFor(() => expect(get(provider)).toBe("claude"));
        expect(get(selectedAccessSourceId)).toBe("claude-subscription");
    });

    it("marks every unread event and refreshes the backend count", async () => {
        const { getByRole } = render(NotificationCenter);
        await waitFor(() =>
            expect(getByRole("button", { name: /1 unread/ })).toBeTruthy(),
        );
        await fireEvent.click(getByRole("button", { name: /1 unread/ }));
        await fireEvent.click(getByRole("button", { name: "Mark all read" }));

        expect(api.markAllNotificationsRead).toHaveBeenCalledOnce();
        expect(api.getUnreadNotificationCount).toHaveBeenCalled();
    });

    it("preserves the last provider-backed snapshot when a refresh fails", async () => {
        const { getByRole, getByText } = render(NotificationCenter);
        await waitFor(() =>
            expect(getByRole("button", { name: /1 unread/ })).toBeTruthy(),
        );
        await fireEvent.click(getByRole("button", { name: /1 unread/ }));
        await waitFor(() => expect(getByText(notification.title)).toBeTruthy());

        api.getNotifications.mockRejectedValueOnce(
            new Error("native database unavailable"),
        );
        tauriEvents.listeners.get("pulse://notification")?.();

        await waitFor(() => expect(getByRole("alert")).toBeTruthy());
        expect(getByText(notification.title)).toBeTruthy();
        expect(getByRole("button", { name: /1 unread/ })).toBeTruthy();
    });

    it("ignores a stale refresh that resolves after a newer snapshot", async () => {
        let resolveStale!: (rows: (typeof notification)[]) => void;
        api.getNotifications
            .mockImplementationOnce(
                () =>
                    new Promise((resolve) => {
                        resolveStale = resolve;
                    }),
            )
            .mockResolvedValueOnce([
                { ...notification, id: 84, title: "Newest provider event" },
            ]);
        api.getUnreadNotificationCount.mockResolvedValue(1);

        const { getByRole, getByText, queryByText } =
            render(NotificationCenter);
        await fireEvent.click(getByRole("button", { name: "Notifications" }));
        await waitFor(() =>
            expect(getByText("Newest provider event")).toBeTruthy(),
        );

        resolveStale([{ ...notification, title: "Stale provider event" }]);
        await Promise.resolve();
        expect(queryByText("Stale provider event")).toBeNull();
    });

    it("navigates even when marking a notification as read fails", async () => {
        api.markNotificationRead.mockRejectedValueOnce(
            new Error("write failed"),
        );
        const { getByRole, getByText } = render(NotificationCenter);
        await waitFor(() =>
            expect(getByRole("button", { name: /1 unread/ })).toBeTruthy(),
        );
        await fireEvent.click(getByRole("button", { name: /1 unread/ }));
        await waitFor(() => expect(getByText(notification.title)).toBeTruthy());
        await fireEvent.click(getByText(notification.title).closest("button")!);

        expect(get(currentView)).toBe("dashboard");
    });

    it("opens from the native tray event and refreshes the durable feed", async () => {
        const { getByRole, getByText } = render(NotificationCenter);
        await waitFor(() =>
            expect(
                tauriEvents.listeners.has("pulse://open-notifications"),
            ).toBe(true),
        );

        tauriEvents.listeners.get("pulse://open-notifications")?.();

        await waitFor(() =>
            expect(
                getByRole("dialog", { name: "Notification center" }),
            ).toBeTruthy(),
        );
        await waitFor(() => expect(getByText(notification.title)).toBeTruthy());
    });
    it("supports unread filtering and Escape with focus restoration", async () => {
        api.getNotifications.mockResolvedValue([
            notification,
            {
                ...notification,
                id: 43,
                title: "Read event",
                read_at: "2026-08-01T01:00:00Z",
            },
        ]);
        const { getByRole, getByText, queryByText, queryByRole } =
            render(NotificationCenter);
        await waitFor(() =>
            expect(getByRole("button", { name: /1 unread/ })).toBeTruthy(),
        );
        const trigger = getByRole("button", { name: /1 unread/ });
        await fireEvent.click(trigger);
        await waitFor(() => expect(getByText("Read event")).toBeTruthy());
        await fireEvent.click(getByRole("button", { name: "Unread (1)" }));
        expect(queryByText("Read event")).toBeNull();
        await fireEvent.keyDown(window, { key: "Escape" });
        expect(queryByRole("dialog")).toBeNull();
        expect(document.activeElement).toBe(trigger);
    });

    it("keeps read/unread/clear actions visible and restores a confirmed clear", async () => {
        api.getNotifications.mockResolvedValue([
            { ...notification, read_at: "2026-09-05T02:00:00Z" },
        ]);
        api.getUnreadNotificationCount.mockResolvedValue(0);
        const { getByRole, getByText } = render(NotificationCenter);
        await fireEvent.click(getByRole("button", { name: "Notifications" }));
        await waitFor(() => expect(getByText(notification.title)).toBeTruthy());
        expect(getByRole("button", { name: "Mark all read" })).toBeTruthy();
        await fireEvent.click(
            getByRole("button", { name: `Mark unread: ${notification.title}` }),
        );
        await waitFor(() =>
            expect(api.markNotificationUnread).toHaveBeenCalledWith(42),
        );
        await fireEvent.click(getByRole("button", { name: "Mark all unread" }));
        await waitFor(() =>
            expect(api.markAllNotificationsUnread).toHaveBeenCalledOnce(),
        );
        await fireEvent.click(getByRole("button", { name: "Clear all" }));
        expect(api.dismissAllNotifications).not.toHaveBeenCalled();
        await fireEvent.click(getByRole("button", { name: "Cancel" }));
        await fireEvent.click(getByRole("button", { name: "Clear all" }));
        await fireEvent.click(
            getByRole("button", { name: "Clear notifications" }),
        );
        await waitFor(() =>
            expect(getByRole("button", { name: "Undo" })).toBeTruthy(),
        );
        await fireEvent.click(getByRole("button", { name: "Undo" }));
        await waitFor(() =>
            expect(api.restoreNotifications).toHaveBeenCalledWith(
                "2026-09-05T03:00:00Z",
            ),
        );
    });
});
