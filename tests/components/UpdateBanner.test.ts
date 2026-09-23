import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, waitFor, fireEvent } from "@testing-library/svelte";
import { get } from "svelte/store";
import type { AppUpdateInfo } from "@/lib/api";
import { toasts } from "@/lib/stores";

const SKIP_KEY = "pulse-update-skipped-version";

function makeUpdate(overrides: Partial<AppUpdateInfo> = {}): AppUpdateInfo {
  return {
    current_version: "1.1.0",
    latest_version: "1.2.0",
    update_available: true,
    release_name: "Pulse 1.2.0",
    release_notes: "Faster reports and a new updater.",
    release_url: "https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/tag/v1.2.0",
    published_at: "2026-06-01T00:00:00Z",
    checked_at: "2026-06-10T00:00:00Z",
    assets: [],
    severity: "minor",
    ...overrides,
  };
}

const checkAppUpdate = vi.fn<[], Promise<AppUpdateInfo>>();
const openAppReleasePage = vi.fn<[(string | null | undefined)?], Promise<void>>();

/** Stand-ins for the Tauri updater/process plugins. Hoisted so the dynamic
 *  `import()` inside the component resolves to these, not the real modules. */
const { updaterCheck, relaunch } = vi.hoisted(() => ({
  updaterCheck: vi.fn(),
  relaunch: vi.fn(async () => undefined),
}));

vi.mock("@tauri-apps/plugin-updater", () => ({ check: updaterCheck }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch }));

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...actual,
    checkAppUpdate: () => checkAppUpdate(),
    openAppReleasePage: (url?: string | null) => openAppReleasePage(url),
  };
});

function setSearch(search: string): void {
  window.history.replaceState(null, "", search || window.location.pathname);
}

async function loadBanner() {
  return (await import("@/components/UpdateBanner.svelte")).default;
}

describe("UpdateBanner.svelte", () => {
  beforeEach(() => {
    checkAppUpdate.mockReset();
    openAppReleasePage.mockReset();
    openAppReleasePage.mockResolvedValue(undefined);
    updaterCheck.mockReset();
    relaunch.mockReset().mockResolvedValue(undefined);
    localStorage.clear();
    setSearch("");
  });

  afterEach(() => {
    setSearch("");
  });

  it("renders the popup when an update is available", async () => {
    checkAppUpdate.mockResolvedValue(makeUpdate());
    const UpdateBanner = await loadBanner();
    const { findByText } = render(UpdateBanner);

    expect(await findByText("Feature update")).toBeTruthy();
    expect(await findByText("1.1.0")).toBeTruthy();
    expect(await findByText("1.2.0")).toBeTruthy();
    expect(checkAppUpdate).toHaveBeenCalledTimes(1);
  });

  it("stays hidden when there is no update", async () => {
    checkAppUpdate.mockResolvedValue(makeUpdate({ update_available: false, latest_version: null }));
    const UpdateBanner = await loadBanner();
    const { container } = render(UpdateBanner);

    await waitFor(() => expect(checkAppUpdate).toHaveBeenCalledTimes(1));
    expect(container.querySelector(".update-pop")).toBeNull();
  });

  it("reports a failed forced check while keeping the startup probe silent", async () => {
    checkAppUpdate.mockRejectedValue(new Error("offline"));
    const UpdateBanner = await loadBanner();
    const { container } = render(UpdateBanner);

    await waitFor(() => expect(checkAppUpdate).toHaveBeenCalledTimes(1));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(container.querySelector(".update-pop")).toBeNull();
    expect(get(toasts).map((t) => t.message)).not.toContain("Couldn't reach the update service.");

    window.dispatchEvent(new CustomEvent("pulse:check-updates"));
    await waitFor(() =>
      expect(get(toasts).map((t) => t.message)).toContain("Couldn't reach the update service."),
    );
  });

  it("tells the user they are current when a forced check finds nothing", async () => {
    checkAppUpdate.mockResolvedValue(makeUpdate({ update_available: false, latest_version: null }));
    const UpdateBanner = await loadBanner();
    render(UpdateBanner);
    await waitFor(() => expect(checkAppUpdate).toHaveBeenCalledTimes(1));

    window.dispatchEvent(new CustomEvent("pulse:check-updates"));
    await waitFor(() =>
      expect(get(toasts).map((t) => t.message)).toContain("You're already up to date."),
    );
  });

  it("Later dismisses the popup without persisting a skip", async () => {
    checkAppUpdate.mockResolvedValue(makeUpdate());
    const UpdateBanner = await loadBanner();
    const { container, findByText } = render(UpdateBanner);

    await findByText("Feature update");
    await fireEvent.click(await findByText("Later"));

    await waitFor(
      () => expect(container.querySelector(".update-pop")).toBeNull(),
      { timeout: 2000 },
    );
    expect(localStorage.getItem(SKIP_KEY)).toBeNull();
  });

  it("Skip version persists the latest version to localStorage", async () => {
    checkAppUpdate.mockResolvedValue(makeUpdate());
    const UpdateBanner = await loadBanner();
    const { container, findByText } = render(UpdateBanner);

    await findByText("Feature update");
    await fireEvent.click(await findByText("Skip"));

    expect(localStorage.getItem(SKIP_KEY)).toBe("1.2.0");
    await waitFor(
      () => expect(container.querySelector(".update-pop")).toBeNull(),
      { timeout: 2000 },
    );
  });

  it("does not show a previously skipped version on auto-check", async () => {
    localStorage.setItem(SKIP_KEY, "1.2.0");
    checkAppUpdate.mockResolvedValue(makeUpdate());
    const UpdateBanner = await loadBanner();
    const { container } = render(UpdateBanner);

    await waitFor(() => expect(checkAppUpdate).toHaveBeenCalledTimes(1));
    expect(container.querySelector(".update-pop")).toBeNull();
  });

  it("forces a skipped version to show on the pulse:check-updates event", async () => {
    localStorage.setItem(SKIP_KEY, "1.2.0");
    checkAppUpdate.mockResolvedValue(makeUpdate());
    const UpdateBanner = await loadBanner();
    const { container, findByText } = render(UpdateBanner);

    await waitFor(() => expect(checkAppUpdate).toHaveBeenCalledTimes(1));
    expect(container.querySelector(".update-pop")).toBeNull();

    window.dispatchEvent(new CustomEvent("pulse:check-updates"));

    expect(await findByText("Feature update")).toBeTruthy();
  });

  it("installs in-app instead of opening the release page", async () => {
    checkAppUpdate.mockResolvedValue(makeUpdate());
    const downloadAndInstall = vi.fn(async (onEvent: (e: unknown) => void) => {
      onEvent({ event: "Started", data: { contentLength: 1000 } });
      onEvent({ event: "Progress", data: { chunkLength: 1000 } });
      onEvent({ event: "Finished" });
    });
    updaterCheck.mockResolvedValue({ downloadAndInstall });

    const UpdateBanner = await loadBanner();
    const { findByText } = render(UpdateBanner);

    await findByText("Feature update");
    await fireEvent.click(await findByText("Update"));

    await waitFor(() => expect(downloadAndInstall).toHaveBeenCalledTimes(1));
    // The whole point: no trip to GitHub.
    expect(openAppReleasePage).not.toHaveBeenCalled();
    await waitFor(() => expect(relaunch).toHaveBeenCalledTimes(1));
  });

  it("uses one approved Update action to install the signed release and relaunch", async () => {
    checkAppUpdate.mockResolvedValue(makeUpdate());
    const downloadAndInstall = vi.fn(async () => undefined);
    updaterCheck.mockResolvedValue({ downloadAndInstall });

    const UpdateBanner = await loadBanner();
    const { findByText } = render(UpdateBanner);

    expect(await findByText("Update available")).toBeTruthy();
    await fireEvent.click(await findByText("Update"));

    await waitFor(() => expect(downloadAndInstall).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(relaunch).toHaveBeenCalledTimes(1));
    expect(openAppReleasePage).not.toHaveBeenCalled();
  });

  it("restarts the app as the final step of the approved Update action", async () => {
    checkAppUpdate.mockResolvedValue(makeUpdate());
    updaterCheck.mockResolvedValue({ downloadAndInstall: vi.fn(async () => undefined) });

    const UpdateBanner = await loadBanner();
    const { findByText } = render(UpdateBanner);

    await findByText("Feature update");
    await fireEvent.click(await findByText("Update"));

    await waitFor(() => expect(relaunch).toHaveBeenCalledTimes(1));
  });

  it("retries only the restart after the signed update is already installed", async () => {
    checkAppUpdate.mockResolvedValue(makeUpdate());
    const downloadAndInstall = vi.fn(async () => undefined);
    updaterCheck.mockResolvedValue({ downloadAndInstall });
    relaunch.mockRejectedValueOnce(new Error("restart blocked")).mockResolvedValueOnce(undefined);

    const UpdateBanner = await loadBanner();
    const { findByText } = render(UpdateBanner);

    await findByText("Feature update");
    await fireEvent.click(await findByText("Update"));
    expect(await findByText("Retry restart")).toBeTruthy();
    await fireEvent.click(await findByText("Retry restart"));

    await waitFor(() => expect(relaunch).toHaveBeenCalledTimes(2));
    expect(downloadAndInstall).toHaveBeenCalledTimes(1);
  });

  it("falls back to the release page when the updater channel has nothing", async () => {
    checkAppUpdate.mockResolvedValue(makeUpdate());
    updaterCheck.mockResolvedValue(null);

    const UpdateBanner = await loadBanner();
    const { findByText } = render(UpdateBanner);

    await findByText("Feature update");
    await fireEvent.click(await findByText("Update"));

    await waitFor(() => expect(openAppReleasePage).toHaveBeenCalledTimes(1));
    expect(openAppReleasePage).toHaveBeenCalledWith(
      "https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/tag/v1.2.0",
    );
  });

  /** A failed install must stay honest: surface the error and offer the
   *  manual route rather than silently claiming success. */
  it("surfaces a failed install and offers the release page as a fallback", async () => {
    checkAppUpdate.mockResolvedValue(makeUpdate());
    updaterCheck.mockRejectedValue(new Error("signature mismatch"));

    const UpdateBanner = await loadBanner();
    const { findByText } = render(UpdateBanner);

    await findByText("Feature update");
    await fireEvent.click(await findByText("Update"));

    expect(await findByText(/Update failed/)).toBeTruthy();
    expect(await findByText("Open release")).toBeTruthy();
    expect(await findByText("Retry update")).toBeTruthy();
  });

  it("synthesizes a fake update from ?fakeUpdate without calling the backend", async () => {
    setSearch("?fakeUpdate=9.9.9");
    const UpdateBanner = await loadBanner();
    const { findByText } = render(UpdateBanner);

    // 9.9.9 is a major jump from the 1.x line the fake lane grades against.
    expect(await findByText("Major update")).toBeTruthy();
    expect(await findByText("dev")).toBeTruthy();
    expect(await findByText("9.9.9")).toBeTruthy();
    expect(checkAppUpdate).not.toHaveBeenCalled();

    await fireEvent.click(await findByText("Update"));
    await waitFor(() => expect(openAppReleasePage).toHaveBeenCalledTimes(1));
    expect(openAppReleasePage).toHaveBeenCalledWith(
      "https://github.com/xt0n1-t3ch/Pulse-Analytics/releases/tag/v9.9.9",
    );
  });

  it("grades the fakeUpdate lane by its tag instead of a fixed severity", async () => {
    setSearch("?fakeUpdate=2.0.0");
    const UpdateBanner = await loadBanner();
    const { container, findByText } = render(UpdateBanner);

    expect(await findByText("Major update")).toBeTruthy();
    expect(container.querySelector(".update-pop.sev-major")).not.toBeNull();
  });

  it("labels the update by the severity the backend reported", async () => {
    const cases: Array<[AppUpdateInfo["severity"], string]> = [
      ["patch", "Patch update"],
      ["minor", "Feature update"],
      ["major", "Major update"],
    ];
    for (const [severity, label] of cases) {
      checkAppUpdate.mockResolvedValue(makeUpdate({ severity }));
      const UpdateBanner = await loadBanner();
      const { findByText, unmount } = render(UpdateBanner);
      expect(await findByText(label)).toBeTruthy();
      unmount();
      checkAppUpdate.mockReset();
    }
  });

  it("tints the popup by severity so the size of the jump is visible at a glance", async () => {
    checkAppUpdate.mockResolvedValue(makeUpdate({ severity: "major" }));
    const UpdateBanner = await loadBanner();
    const { container, findByText } = render(UpdateBanner);

    await findByText("Major update");
    expect(container.querySelector(".update-pop.sev-major")).not.toBeNull();
  });

  it("surfaces only the installer for the host platform", async () => {
    // The host platform is derived from the user agent, which differs between
    // a Windows dev machine and a Linux CI runner. Pin it so the assertion
    // tests the selection logic rather than the machine running the suite.
    const originalAgent = Object.getOwnPropertyDescriptor(navigator, "userAgent");
    Object.defineProperty(navigator, "userAgent", {
      configurable: true,
      value: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
    });

    checkAppUpdate.mockResolvedValue(
      makeUpdate({
        assets: [
          {
            name: "Pulse_1.2.0_x64-setup.exe",
            download_url: "https://example.invalid/Pulse.exe",
            size: 8_388_608,
            content_type: "application/octet-stream",
            platform: "windows",
          },
          {
            name: "pulse_1.2.0_amd64.deb",
            download_url: "https://example.invalid/pulse.deb",
            size: 7_340_032,
            content_type: "application/octet-stream",
            platform: "linux",
          },
          {
            name: "checksums.txt",
            download_url: "https://example.invalid/checksums.txt",
            size: 128,
            content_type: "text/plain",
            platform: null,
          },
        ],
      }),
    );
    const UpdateBanner = await loadBanner();
    const { container, findByText, queryByText } = render(UpdateBanner);

    await findByText("Feature update");
    const asset = container.querySelector(".up-asset");
    // With the host pinned to Windows, exactly the .exe must surface: not the
    // Linux build, and not the checksum file.
    expect(asset).not.toBeNull();
    expect(asset?.textContent).toContain("Pulse_1.2.0_x64-setup.exe");
    expect(asset?.textContent).toContain("8.0 MB");
    expect(asset?.textContent).not.toContain("amd64.deb");
    expect(queryByText("checksums.txt")).toBeNull();

    if (originalAgent) {
      Object.defineProperty(navigator, "userAgent", originalAgent);
    }
  });

  it("previews at most three release-note highlights with markdown stripped", async () => {
    checkAppUpdate.mockResolvedValue(
      makeUpdate({
        release_notes: "## What's new\n- First thing\n* Second thing\n- Third thing\n- Fourth thing",
      }),
    );
    const UpdateBanner = await loadBanner();
    const { container, findByText } = render(UpdateBanner);

    await findByText("Feature update");
    const items = Array.from(container.querySelectorAll(".up-highlights li")).map(
      (li) => li.textContent,
    );
    expect(items).toEqual(["What's new", "First thing", "Second thing"]);
    expect(items.join(" ")).not.toContain("#");
    expect(items.join(" ")).not.toContain("- ");
  });
});
