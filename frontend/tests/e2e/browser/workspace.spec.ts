import { expect, test, type Route } from "@playwright/test";
import { readFileSync } from "node:fs";

const hybridFixture = JSON.parse(
  readFileSync(
    new URL("../../../../tests/fixtures/providers/hybrid/fixture.json", import.meta.url),
    "utf8",
  ),
) as { expected_dto: { routes: unknown[] } };

const session = {
  session_id: "workspace-live",
  session_name: "PropertyAlpha-Agent",
  project: "PropertyAlpha-Agent",
  model: "GPT-5.6 Sol · High",
  model_id: "gpt-5.6-sol",
  provider: "codex",
  context_window: "258.4K",
  cost: 2.51,
  cost_available: true,
  cost_basis: "exact",
  tokens: 208_100,
  input_tokens: 120_000,
  output_tokens: 18_100,
  cache_write_tokens: 0,
  cache_read_tokens: 70_000,
  context_used_tokens: 208_100,
  context_window_tokens: 258_400,
  branch: "improvements/analytics-functionality",
  activity: "Thinking",
  activity_target: "frontend/src/views/Dashboard.svelte",
  effort: "High",
  effort_explicit: true,
  is_idle: false,
  started_at: "2026-08-01T12:00:00Z",
  duration_secs: 900,
  has_thinking: true,
  workflow_label: null,
  subagent_count: 0,
  subagents: [],
  tokens_per_sec: 42,
  input_cost: 0.7,
  output_cost: 1.35,
  cache_write_cost: 0,
  cache_read_cost: 0.46,
  speed: "standard",
  fast: false,
  service_tier: null,
  app_name: "ChatGPT App",
  has_inflated_tokenizer: false,
};

const metrics = {
  total_cost: 2.51,
  cost_available: true,
  cost_basis: "exact",
  input_tokens: 120_000,
  pure_input_tokens: 120_000,
  output_tokens: 18_100,
  cache_write_tokens: 0,
  cache_read_tokens: 70_000,
  total_tokens: 208_100,
  session_count: 1,
  input_cost: 0.7,
  output_cost: 1.35,
  cache_write_cost: 0,
  cache_read_cost: 0.46,
  cache_hit_ratio: 36.8,
  models: [{ model: "GPT-5.6 Sol · High", sessions: 1, cost: 2.51, tokens: 208_100 }],
};

const appSnapshot = {
  revision: 1,
  health: {
    version: "1.7.0",
    uptime_seconds: 900,
    discord_status: "Connected",
    discord_enabled: true,
  },
  metrics,
  sessions: [session],
  rate_limits: null,
  discord_preview: {
    provider: "codex",
    app_name: "ChatGPT App",
    details: "Thinking · PropertyAlpha-Agent · $2.51",
    state: "GPT-5.6 Sol · High · Weekly 4%",
    large_image_key: "large",
    large_text: "ChatGPT App",
    small_image_key: null,
    small_text: null,
    has_session: true,
    duration_secs: 900,
  },
  discord_settings: {
    provider: "codex",
    enabled: true,
    status: "Connected",
    publisher: "pulse",
    display_prefs: {
      show_project: true,
      show_branch: true,
      show_model: true,
      show_activity: true,
      show_tokens: true,
      show_cost: true,
      show_limits: true,
      show_credits: true,
      show_context: true,
      show_systems: true,
    },
    desktop_design: "chatgpt_app",
    supports_desktop_design: true,
    supports_field_order: true,
    supports_credits: true,
    field_order: [],
  },
  plan: { provider: "codex", plan_key: "pro_20x", plan_name: "Pro 20x", detected: true },
  access: hybridFixture.expected_dto,
};

// Sessions and allowance proof are independent backend signals. Keep the
// active session in this negative access snapshot so the layout contract cannot
// hide useful live work just because every quota route is unproved.
const noProofSnapshot = {
  ...appSnapshot,
  access: {
    routes: [{
      source: {
        id: "openai-api:unproved",
        kind: "open_ai_api",
        provider: "openai",
        auth_method: "api_key",
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
      error: "Provider proof was not observed.",
    }],
  },
};

const analyticsSummary = {
  total_sessions: 1,
  total_cost: 2.51,
  total_tokens: 208_100,
  total_cache_read: 70_000,
  total_cache_write: 0,
  avg_duration_secs: 900,
  avg_tokens_per_session: 208_100,
  avg_cost_per_session: 2.51,
  top_project: "PropertyAlpha-Agent",
  top_model: "GPT-5.6 Sol · High",
  days_tracked: 1,
};

const responses: Record<string, unknown> = {
  get_app_snapshot: appSnapshot,
  get_discord_user: null,
  get_analytics_summary: analyticsSummary,
  get_session_history: [],
  get_cost_forecast: {
    spent_this_month: 2.51,
    days_elapsed: 1,
    days_in_month: 31,
    projected_monthly: 77.81,
    daily_average: 2.51,
  },
  get_hourly_activity: [{ hour: 12, session_count: 1, total_cost: 2.51 }],
  get_daily_stats: [],
  get_project_stats: [],
};

const noProofResponses: Record<string, unknown> = {
  ...responses,
  get_app_snapshot: noProofSnapshot,
};

async function fulfillBridgeFrom(
  payloads: Record<string, unknown>,
  route: Route,
): Promise<void> {
  const payload = route.request().postDataJSON() as { command?: string };
  const command = payload.command ?? "";
  if (!(command in payloads)) {
    await route.fulfill({ status: 404, contentType: "application/json", body: JSON.stringify({ error: `unhandled ${command}` }) });
    return;
  }
  await route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify(payloads[command]),
  });
}

async function fulfillBridge(route: Route): Promise<void> {
  return fulfillBridgeFrom(responses, route);
}

test("Home shows only proofed sources and exact metrics", async ({ page }) => {
  await page.setViewportSize({ width: 1488, height: 1058 });
  await page.route("**/__pulse_api", fulfillBridge);
  await page.goto("/");

  await expect(page.getByRole("navigation", { name: "Primary navigation" })).toBeVisible();
  const notificationBox = await page.getByRole("button", { name: "Notifications" }).boundingBox();
  const themeBox = await page.getByRole("button", { name: "Toggle theme" }).boundingBox();
  expect(notificationBox).not.toBeNull();
  expect(themeBox).not.toBeNull();
  expect(notificationBox!.width).toBe(themeBox!.width);
  expect(notificationBox!.height).toBe(themeBox!.height);
  expect(notificationBox!.y).toBe(themeBox!.y);
  await expect(page.getByText("Codex Pro 20x", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("Claude Max 20x", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("All providers", { exact: true })).toBeVisible();
  await expect(page.getByText("OpenAI", { exact: true })).toHaveCount(0);

  await expect(page.getByRole("heading", { name: "Provider limits" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Live workspace" })).toBeVisible();
  await expect(page.getByText("Work now", { exact: true })).toHaveCount(0);
  await expect(page.getByText("$2.51", { exact: true }).first()).toBeVisible();
  await expect(page.locator("body")).not.toContainText(">=$");
  await expect(page.locator("[data-source-inspector]")).toHaveCount(0);
  await expect(page.getByText("Source diagnostics", { exact: true })).toHaveCount(0);
  const homeBox = await page.locator("[data-dashboard-layout='direction-two']").boundingBox();
  expect(homeBox).not.toBeNull();
  expect(homeBox!.width / 1488).toBeGreaterThan(0.96);

  await page.getByRole("button", { name: /Claude Max 20x Subscription/ }).click();
  await expect(page.getByText("5h", { exact: true })).toBeVisible();
  await expect(page.getByText("Fable-only", { exact: true })).toBeVisible();
  await expect(page.getByText("GPT-5.3-Codex-Spark · Weekly", { exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: /Claude Max 20x Subscription/ })).toHaveAttribute("aria-pressed", "true");

  await expect(page.getByRole("button", { name: "Help" })).toHaveCount(0);

  await page.setViewportSize({ width: 390, height: 844 });
  await expect(page.locator("html")).toHaveJSProperty("scrollWidth", 390);
  await expect(page.locator("main")).toHaveJSProperty("scrollWidth", 380);

  await page.getByRole("button", { name: "Notifications" }).click();
  const panelBox = await page.getByRole("dialog", { name: "Notification center" }).boundingBox();
  expect(panelBox).not.toBeNull();
  expect(panelBox!.x).toBeGreaterThanOrEqual(0);
  expect(panelBox!.x + panelBox!.width).toBeLessThanOrEqual(390);
});

test("Home releases the allowance rail when every route lacks provider proof", async ({ page }) => {
  await page.setViewportSize({ width: 1488, height: 1058 });
  await page.route("**/__pulse_api", (route) => fulfillBridgeFrom(noProofResponses, route));
  await page.goto("/");

  await expect(page.getByRole("heading", { name: "Live workspace" })).toBeVisible();
  await expect(page.getByText("PropertyAlpha-Agent", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("No authenticated usage source", { exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Provider limits" })).toHaveCount(0);
  await expect(page.locator("[aria-label='Provider allowances']")).toHaveCount(0);

  const gridBox = await page.locator(".home-grid").boundingBox();
  const workBox = await page.locator(".work-now").boundingBox();
  expect(gridBox).not.toBeNull();
  expect(workBox).not.toBeNull();
  expect(workBox!.width / gridBox!.width).toBeGreaterThan(0.9);
});

test("all primary views stay inside a 390px viewport", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.route("**/__pulse_api", fulfillBridge);
  await page.goto("/");

  const views = ["Home", "Sessions", "Context", "Costs", "Reports", "Discord", "Settings"];
  for (const view of views) {
    if (view !== "Home") {
      await page.getByRole("navigation", { name: "Primary navigation" })
        .getByRole("button", { name: view, exact: true })
        .click();
    }
    await expect(page.locator(".main-content")).toBeVisible();
    await expect.poll(() => page.evaluate(() => {
      const main = document.querySelector(".main-content") as HTMLElement | null;
      return document.documentElement.scrollWidth <= 390
        && document.body.scrollWidth <= 390
        && (main == null || main.scrollWidth <= main.clientWidth);
    })).toBe(true);
    const overflow = await page.evaluate(() => ({
      document: document.documentElement.scrollWidth,
      body: document.body.scrollWidth,
      main: (document.querySelector(".main-content") as HTMLElement | null)?.scrollWidth ?? 0,
      mainClient: (document.querySelector(".main-content") as HTMLElement | null)?.clientWidth ?? 0,
    }));
    expect(overflow.document).toBeLessThanOrEqual(390);
    expect(overflow.body).toBeLessThanOrEqual(390);
    expect(overflow.main).toBeLessThanOrEqual(overflow.mainClient);
  }
});

test("localhost boots against the authenticated Rust backend without fixture fallback", async ({ request }) => {
  const response = await request.post("/__pulse_api", {
    data: { command: "get_app_snapshot", args: {} },
  });

  expect(response.ok()).toBe(true);
  const snapshot = await response.json() as {
    revision: number;
    health: { version: string };
    sessions: unknown[];
    access: { routes: unknown[] };
  };
  expect(snapshot.revision).toBeGreaterThan(0);
  expect(snapshot.health.version).toMatch(/^\d+\.\d+\.\d+$/);
  expect(Array.isArray(snapshot.sessions)).toBe(true);
  expect(Array.isArray(snapshot.access.routes)).toBe(true);
});
