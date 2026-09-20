import { chromium, expect, test } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

test("installed production app uses embedded UI and real native providers", async () => {
  test.skip(!process.env.PULSE_INSTALLED_PROOF_DIR, "Requires the installed production app");
  const browser = await chromium.connectOverCDP(process.env.PULSE_TAURI_CDP_URL!);
  const page = browser.contexts()[0]?.pages()[0];
  if (!page) throw new Error("Installed Pulse did not expose its WebView");
  await page.waitForLoadState("domcontentloaded");
  expect(new URL(page.url()).hostname).toBe("tauri.localhost");
  let devRequests = 0;
  const errors: string[] = [];
  page.on("request", (request) => {
    if (/127\.0\.0\.1:(1420|1421|1430|1431)|localhost:(1420|1421|1430|1431)|__pulse_api/.test(request.url())) devRequests++;
  });
  page.on("pageerror", (error) => errors.push(error.message));
  const read = (command: string) => page.evaluate(async (name) => {
    const api = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string) => Promise<any> } }).__TAURI_INTERNALS__;
    return api.invoke(name);
  }, command);
  const health = await read("get_health");
  expect(health.version).toBe(process.env.PULSE_EXPECTED_VERSION);
  await expect(page.getByRole("navigation", { name: "Primary navigation" })).toBeVisible();
  for (const route of ["Sessions", "Costs", "Reports", "Discord", "Accounts", "Settings"]) {
    await page.getByRole("button", { name: route, exact: true }).click();
    await expect(page.locator("main")).toBeVisible();
  }
  await page.getByRole("button", { name: "Accounts", exact: true }).click();
  await expect.poll(async () => {
    const accounts = await read("list_accounts");
    return accounts.some((account: any) => account.provider === "commandcode" && account.status === "connected");
  }, { timeout: 30000 }).toBe(true);
  const accounts = await read("list_accounts");
  const command = accounts.find((account: any) => account.provider === "commandcode");
  expect(command.windows.map((window: any) => window.id)).toEqual(expect.arrayContaining(["fiveHour", "weekly", "monthly"]));
  const codex = accounts.find((account: any) => account.provider === "codex");
  expect(codex).toBeTruthy();
  const settings = await read("get_discord_settings");
  const preview = await read("get_discord_preview");
  if (settings.provider === "codex" && settings.display_prefs.show_cost && preview.has_session) {
    expect(preview.state).toMatch(/\$\d/);
    expect(preview.state).not.toMatch(/partial|~\$/i);
  }
  const output = process.env.PULSE_INSTALLED_PROOF_DIR!;
  await mkdir(output, { recursive: true });
  await page.screenshot({ path: path.join(output, "installed-accounts.png") });
  expect(devRequests).toBe(0);
  expect(errors).toEqual([]);
  await writeFile(path.join(output, "installed-runtime-proof.json"), JSON.stringify({
    url: page.url(), version: health.version, nativeIpc: true, devRequests, errors,
    accounts: accounts.map((account: any) => ({ provider: account.provider, status: account.status, plan: account.plan, windows: account.windows.map((window: any) => window.id) })),
    discord: { provider: settings.provider, status: settings.status, enabled: settings.enabled, monetaryFieldEnabled: settings.display_prefs.show_cost, hasSession: preview.has_session, currencyOnlyCost: /\$\d/.test(preview.state) && !/partial|~\$/i.test(preview.state) },
  }, null, 2));
});
