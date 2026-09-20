import { fireEvent, render } from "@testing-library/svelte";
import { afterEach, describe, expect, it } from "vitest";
import { accessSnapshot, selectedAccessSourceId, sourceInspectorExpanded } from "@/lib/stores";
import AllowanceRail from "@/components/AllowanceRail.svelte";
import DataSourceInspector from "@/components/DataSourceInspector.svelte";

describe("Home provider surfaces", () => {
  afterEach(() => {
    accessSnapshot.set(null);
    selectedAccessSourceId.set("all");
    sourceInspectorExpanded.set(false);
  });

  it("renders the empty allowance rail without a reactive update loop", () => {
    const { getByText } = render(AllowanceRail);
    expect(getByText("No live limits yet")).toBeTruthy();
  });

  it("renders the empty source inspector without a reactive update loop", () => {
    const { getByRole, getByText } = render(DataSourceInspector);
    void fireEvent.click(getByRole("button", { name: /Source diagnostics/ }));
    expect(getByText("No provider route has been discovered yet.")).toBeTruthy();
  });

  it("explains failed provider probes without exposing them as selectable sources", async () => {
    accessSnapshot.set({
      routes: [{
        source: {
          id: "claude-subscription:default",
          kind: "claude_subscription",
          provider: "claude",
          auth_method: "oauth",
          proof: "none",
          plan: null,
        },
        availability: "unavailable",
        freshness: "unknown",
        provenance: "none",
        observed_at: null,
        fetched_at: null,
        expires_at: null,
        windows: [],
        credits: null,
        extra_usage: null,
        error: "token expired",
      }],
    });

    const { getByRole, getByText } = render(DataSourceInspector);
    await fireEvent.click(getByRole("button", { name: /Source diagnostics/ }));
    expect(getByText("Claude Subscription")).toBeTruthy();
    expect(getByText("token expired")).toBeTruthy();
    expect(getByText(/no provider proof/i)).toBeTruthy();
  });

  it("shows all failed probes even when an authenticated source is selected", async () => {
    accessSnapshot.set({
      routes: [
        {
          source: {
            id: "codex-subscription:default",
            kind: "codex_subscription",
            provider: "codex",
            auth_method: "app_server",
            proof: "quota_response",
            plan: "Pro 20x",
          },
          availability: "available",
          freshness: "fresh",
          provenance: "provider_api",
          observed_at: "2026-08-03T08:00:00Z",
          fetched_at: "2026-08-03T08:00:01Z",
          expires_at: "2026-08-03T08:00:31Z",
          windows: [],
          credits: null,
          extra_usage: null,
          error: null,
        },
        {
          source: {
            id: "claude-subscription:default",
            kind: "claude_subscription",
            provider: "claude",
            auth_method: "oauth",
            proof: "none",
            plan: null,
          },
          availability: "unavailable",
          freshness: "unknown",
          provenance: "none",
          observed_at: null,
          fetched_at: null,
          expires_at: null,
          windows: [],
          credits: null,
          extra_usage: null,
          error: "token expired",
        },
      ],
    });
    selectedAccessSourceId.set("codex-subscription:default");

    const { getByRole, getByText } = render(DataSourceInspector);
    await fireEvent.click(getByRole("button", { name: /Source diagnostics/ }));

    expect(getByText("Codex Subscription")).toBeTruthy();
    expect(getByText("Claude Subscription")).toBeTruthy();
    expect(getByText("token expired")).toBeTruthy();
  });

  it("shows only the selected provider allowance without fabricating windows", () => {
    accessSnapshot.set({
      routes: [
        {
          source: { id: "codex", kind: "codex_subscription", provider: "codex", auth_method: "app_server", proof: "quota_response", plan: "Pro 20x" },
          availability: "available", freshness: "fresh", provenance: "app_server",
          observed_at: null, fetched_at: null, expires_at: null,
          windows: [{ key: "weekly", label: "Weekly", window_minutes: 10080, used_percent: 4, remaining_percent: 96 }],
          credits: null, extra_usage: null, error: null,
        },
        {
          source: { id: "claude", kind: "claude_subscription", provider: "claude", auth_method: "oauth", proof: "quota_response", plan: "Max 20x" },
          availability: "available", freshness: "fresh", provenance: "provider_api",
          observed_at: null, fetched_at: null, expires_at: null,
          windows: [
            { key: "five_hour", label: "5h", window_minutes: 300, used_percent: 24, remaining_percent: 76 },
            { key: "weekly", label: "Weekly", window_minutes: 10080, used_percent: 58, remaining_percent: 42 },
            { key: "fable", label: "Fable", window_minutes: null, used_percent: 11, remaining_percent: 89 },
          ],
          credits: null, extra_usage: null, error: null,
        },
      ],
    });
    selectedAccessSourceId.set("claude");

    const { container } = render(AllowanceRail);
    expect(container.textContent).toContain("Claude Max 20x");
    expect(container.textContent).toContain("5-hour limit");
    expect(container.textContent).toContain("24% used");
    expect(container.textContent).toContain("Weekly");
    expect(container.textContent).toContain("Fable");
    expect(container.textContent).not.toContain("Codex Pro 20x");
    expect(container.textContent).not.toContain("Provider current");
    expect(container.textContent).not.toContain("provider api");
  });

  it("fails stale API spend closed instead of presenting cached usage as current", () => {
    accessSnapshot.set({
      routes: [{
        source: {
          id: "openai-api",
          kind: "open_ai_api",
          provider: "openai",
          auth_method: "api_key",
          proof: "authenticated_probe",
          plan: null,
        },
        availability: "available",
        freshness: "stale",
        provenance: "memory_cache",
        observed_at: "2026-08-01T14:00:00Z",
        fetched_at: "2026-08-01T14:00:01Z",
        expires_at: "2026-08-01T14:05:01Z",
        windows: [],
        credits: null,
        extra_usage: { used: 87.42, limit: 200, currency: "USD" },
        error: null,
      }],
    });

    const { container } = render(AllowanceRail);
    expect(container.textContent).toContain("Month-to-date usage");
    expect(container.textContent).toContain("Unavailable");
    expect(container.textContent).not.toContain("$87.42");
  });

  it("shows provider-reported Codex reset credits separately from spend credits", () => {
    accessSnapshot.set({
      routes: [{
        source: {
          id: "codex-subscription:default",
          kind: "codex_subscription",
          provider: "codex",
          auth_method: "app_server",
          proof: "quota_response",
          plan: "Pro 20x",
        },
        availability: "available",
        freshness: "fresh",
        provenance: "app_server",
        observed_at: "2026-08-03T06:00:00Z",
        fetched_at: "2026-08-03T06:00:01Z",
        expires_at: "2026-08-03T06:00:31Z",
        windows: [{
          key: "weekly",
          label: "Weekly",
          window_minutes: 10080,
          used_percent: 52,
          remaining_percent: 48,
          resets_at: "2026-08-07T00:00:00Z",
        }],
        credits: null,
        rateLimitResetCredits: {
          availableCount: 1,
          credits: [{
            id: "full-reset-1",
            resetType: "full",
            status: "available",
            grantedAt: 1785736800,
            expiresAt: 1786514400,
            title: "Full reset",
            description: null,
          }],
        },
        extra_usage: null,
        error: null,
      }],
    });

    const { getByText, queryByText } = render(AllowanceRail);
    expect(getByText("1 reset available")).toBeTruthy();
    expect(getByText("Full reset")).toBeTruthy();
    expect(queryByText("Use reset")).toBeNull();
  });
  it("uses Command Code artwork and preserves its calendar monthly window", () => {
    accessSnapshot.set({ routes: [{
      source: { id: "commandcode:account", kind: "command_code_subscription", provider: "commandcode", auth_method: "api_key", proof: "quota_response", plan: "GOAT" },
      availability: "available", freshness: "fresh", provenance: "provider_api", observed_at: new Date().toISOString(), fetched_at: new Date().toISOString(), expires_at: null,
      windows: [{ key: "monthly", label: "Monthly limit", scope_id: "account", scope_kind: "global_account", window_minutes: 0, used_percent: 8, remaining_percent: 92, resets_at: null }],
      credits: null, extra_usage: null, error: null,
    }] });
    selectedAccessSourceId.set("all");
    const view = render(AllowanceRail);
    expect(view.getByText("Monthly limit")).toBeTruthy();
    expect(view.container.querySelector(".allowance-card header img")?.getAttribute("src")).toContain("commandcode.png");
  });

});
