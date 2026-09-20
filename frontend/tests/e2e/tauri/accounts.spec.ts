import { chromium, expect, test } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";

test("real Accounts use native Tauri IPC without the browser proxy", async () => {
  test.skip(!process.env.PULSE_REAL_ACCOUNT_PROOF_DIR, "Requires an isolated real-data native run");
  const browser = await chromium.connectOverCDP(process.env.PULSE_TAURI_CDP_URL!);
  const page = browser.contexts()[0]?.pages()[0];
  if (!page) throw new Error("The isolated Pulse WebView is unavailable");
  let proxyRequests = 0;
  page.on("request", (request) => {
    if (request.url().includes("/__pulse_api")) proxyRequests++;
  });
  await expect(page.getByRole("button", { name: "Accounts", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Accounts", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Accounts", exact: true })).toBeVisible();
  const accounts = await page.evaluate(async () => {
    const api = (window as unknown as { __TAURI_INTERNALS__: { invoke: (name: string) => Promise<Array<{ provider: string; status: string; windows: Array<{ id: string }>; origin: string }>> } }).__TAURI_INTERNALS__;
    return api.invoke("list_accounts");
  });
  expect(accounts.some((account) => account.provider === "codex")).toBe(true);
  expect(accounts.some((account) => account.provider === "commandcode")).toBe(true);
  expect(proxyRequests).toBe(0);
  await expect(page.getByRole("button", { name: "Add account", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Add account", exact: true }).click();
  await expect(page.getByRole("combobox", { name: "Provider", exact: true })).toBeVisible();
  await page.getByRole("radio", { name: /Link a local profile/ }).check();
  await expect(page.getByRole("button", { name: "Choose profile folder" })).toBeVisible();
  await page.getByRole("button", { name: "Cancel", exact: true }).click();
  const output = process.env.PULSE_REAL_ACCOUNT_PROOF_DIR!;
  await mkdir(output, { recursive: true });
  await page.screenshot({ path: path.join(output, "native-accounts.png") });
  await writeFile(path.join(output, "native-proof.json"), JSON.stringify({ nativeIpc: true, proxyRequests, accounts: accounts.map(({ provider, status, windows, origin }) => ({ provider, status, origin, windows: windows.map(({ id }) => id) })) }, null, 2));
});
