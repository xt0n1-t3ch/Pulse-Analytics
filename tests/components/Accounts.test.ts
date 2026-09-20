import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor, within } from "@testing-library/svelte";
import { get } from "svelte/store";
import Accounts from "@/views/Accounts.svelte";
import { provider } from "@/lib/provider";
import { accountPlan, type AccountSnapshot } from "@/lib/accounts";

const calls = vi.hoisted(() => ({
  list: vi.fn(), discover: vi.fn(), connect: vi.fn(), remove: vi.fn(), refresh: vi.fn(), cancel: vi.fn(),
}));
vi.mock("@/lib/accounts", async (original) => ({
  ...await original<typeof import("@/lib/accounts")>(),
  listAccounts: calls.list, discoverAccounts: calls.discover, connectAccount: calls.connect,
  removeAccount: calls.remove, refreshAccount: calls.refresh, cancelAccountConnection: calls.cancel,
}));
function account(id: string, overrides: Partial<AccountSnapshot> = {}): AccountSnapshot {
  return {
    id, provider: "claude", label: "Local account", origin: "local",
    identity: { subject: id, name: null, email: id + "@example.test", organization: null, verified: true },
    plan: "max_5x", status: "connected", observed_at: new Date().toISOString(),
    expires_at: new Date(Date.now() + 60000).toISOString(), retry_at: null,
    windows: [], balances: [], reset_credits: null, individual_limits: [], error: null,
    notice: null, refreshing: false, auth_url: null, ...overrides,
  };
}
beforeEach(() => {
  vi.clearAllMocks();
  provider.set("claude");
  const rows = [account("claude"), account("codex", {
    provider: "codex", plan: "pro",
    windows: [{ id: "weekly", label: "Weekly usage", scope: "Account", duration_minutes: 10080, used: null, limit: null, unit: null, used_percent: 25, resets_at: null }],
    balances: [{ id: "credits", label: "Credits", value: "0", unit: "credits", expires_at: null }],
    reset_credits: { availableCount: 3, credits: null },
  })];
  calls.list.mockResolvedValue(rows); calls.discover.mockResolvedValue(rows);
  calls.connect.mockResolvedValue("new-account"); calls.remove.mockResolvedValue(undefined);
  calls.refresh.mockResolvedValue(undefined); calls.cancel.mockResolvedValue(undefined);
});
afterEach(cleanup);

describe("Accounts", () => {
  it("keeps Codex's reported weekly-only windows and banked resets without switching the coding provider", async () => {
    const view = render(Accounts);
    const button = await view.findByRole("button", { name: /codex@example.test/ });
    await fireEvent.click(button);
    const detail = within(view.getByRole("region", { name: "Account details for codex@example.test" }));
    expect(detail.getByText("Weekly usage")).toBeTruthy();
    expect(detail.getByText(/Pro 20x/)).toBeTruthy();
    expect(detail.queryByText("5-hour usage")).toBeNull();
    expect(detail.getByText("3 available")).toBeTruthy();
    const balances = within(detail.getByRole("region", { name: "Credits & balances" }));
    expect(balances.getByText("0")).toBeTruthy();
    expect(get(provider)).toBe("claude");
  });

  it("links an explicit profile without sending an API key or changing the provider", async () => {
    const view = render(Accounts);
    await view.findByRole("button", { name: /claude@example.test/ });
    await fireEvent.click(view.getByRole("button", { name: "Add account", exact: true }));
    await fireEvent.change(view.getByLabelText("Provider", { exact: true }), { target: { value: "codex" } });
    await fireEvent.click(view.getByRole("radio", { name: /Link a local profile/ }));
    await fireEvent.input(view.getByLabelText("Profile folder"), { target: { value: "C:/profiles/codex" } });
    await fireEvent.submit(view.container.querySelector("form")!);
    await waitFor(() => expect(calls.connect).toHaveBeenCalledWith(expect.objectContaining({
      provider: "codex", method: "local", path: "C:/profiles/codex", api_key: undefined,
    })));
    expect(get(provider)).toBe("claude");
  });

  it("requires a scoped confirmation before unlinking an account", async () => {
    const view = render(Accounts);
    await view.findByRole("button", { name: /claude@example.test/ });
    await fireEvent.click(view.getByRole("button", { name: "Remove from Pulse", exact: true }));
    expect(calls.remove).not.toHaveBeenCalled();
    const dialog = within(view.getByRole("dialog"));
    expect(dialog.getByText("Your coding apps will stay signed in. Your session history stays in Pulse.")).toBeTruthy();
    calls.list.mockResolvedValue([account("codex", { provider: "codex" })]);
    await fireEvent.click(dialog.getByRole("button", { name: "Remove from Pulse" }));
    await waitFor(() => expect(calls.remove).toHaveBeenCalledWith("claude"));
  });

  it("reconnects a Pulse-owned account with its existing identifier", async () => {
    const expired = account("managed", { origin: "managed", status: "expired", error: "Token expired" });
    calls.discover.mockResolvedValue([expired]); calls.list.mockResolvedValue([expired]);
    const view = render(Accounts);
    await fireEvent.click(await view.findByRole("button", { name: "Connect again" }));
    expect(view.getByRole("heading", { name: "Reconnect account" })).toBeTruthy();
    await fireEvent.submit(view.container.querySelector("form")!);
    await waitFor(() => expect(calls.connect).toHaveBeenCalledWith(expect.objectContaining({
      account_id: "managed", provider: "claude", method: "browser",
    })));
  });

  it("shows provider unavailability without fabricated allowance counters", async () => {
    calls.discover.mockResolvedValue([account("go", { provider: "opencode", status: "unavailable", error: "OpenCode Go subscription required.", plan: null })]);
    const view = render(Accounts);
    expect(await view.findByText("OpenCode Go subscription required.")).toBeTruthy();
    expect(view.queryAllByRole("progressbar")).toHaveLength(0);
    expect(view.getByText("No balance is reported by this provider.")).toBeTruthy();
  });
  it("keeps two Claude accounts separate and preserves Fable-only and stale observations", async () => {
    const first = account("first", { windows: [{ id: "fable", label: "Fable only", scope: "Fable", duration_minutes: 10080, used: null, limit: null, unit: null, used_percent: 12, resets_at: null }] });
    const second = account("second", { status: "stale", windows: [], balances: [{ id: "monthly", label: "Monthly credits", value: "0", unit: "credits", expires_at: null, cycle_ends_at: "2026-10-01T00:00:00Z" }] });
    calls.discover.mockResolvedValue([first, second]); calls.list.mockResolvedValue([first, second]);
    const view = render(Accounts);
    expect(await view.findByText("Fable only")).toBeTruthy();
    await fireEvent.click(view.getByRole("button", { name: /second@example.test/ }));
    expect(view.queryByText("Fable only")).toBeNull();
    expect(view.getAllByText("Last known usage").length).toBeGreaterThan(0);
    expect(view.getByText(/Cycle ends/)).toBeTruthy();
    expect(view.queryByText(/^Expires /)).toBeNull();
    expect(view.queryAllByRole("progressbar")).toHaveLength(0);
  });

  it("labels the distinct native Codex Pro tiers and Command Code plans", () => {
    expect(accountPlan(account("pro", { provider: "codex", plan: "pro" }))).toBe("Pro 20x");
    expect(accountPlan(account("lite", { provider: "codex", plan: "pro_lite" }))).toBe("Pro 5x");
    expect(accountPlan(account("goat", { provider: "commandcode", plan: "individual-goat" }))).toBe("GOAT");
  });

});
