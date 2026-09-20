import { fireEvent, render, waitFor } from "@testing-library/svelte";
import { get } from "svelte/store";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { accessSnapshot, selectedAnalyticsProviderScope, selectedAccessSourceId } from "@/lib/stores";
import { provider, setProvider } from "@/lib/provider";
import type { AccessRouteSnapshot } from "@/lib/access";
import AccessSourceBar from "@/components/AccessSourceBar.svelte";

vi.mock("@/lib/provider", async () => {
  const { writable } = await import("svelte/store");
  const provider = writable("codex");
  return { provider, setProvider: vi.fn(async (next: string) => { provider.set(next); }) };
});
function route(id: string, kind: AccessRouteSnapshot["source"]["kind"], product: AccessRouteSnapshot["source"]["provider"], plan: string | null = null): AccessRouteSnapshot {
  return {source:{id,kind,provider:product,plan,auth_method:"api_key",proof:"quota_response"},availability:"available",freshness:"fresh",provenance:"provider_api",observed_at:null,fetched_at:null,expires_at:null,windows:[],credits:null,extra_usage:null,local_history:{available:false,sessions:0},error:null};
}

describe("unified provider selector", () => {
  beforeEach(() => { vi.mocked(setProvider).mockClear(); provider.set("codex"); accessSnapshot.set({routes:[]}); selectedAnalyticsProviderScope.set("all"); selectedAccessSourceId.set("all"); });
  it("renders one selector with OpenCode even without account quotas", () => {
    const { container, getByRole } = render(AccessSourceBar);
    expect(container.querySelectorAll("section")).toHaveLength(1);
    expect(getByRole("button", {name:/OpenCode Desktop/})).toBeTruthy();
    expect(getByRole("button", {name:/All providers/})).toBeTruthy();
    expect(container.textContent).not.toContain("All sessions");
    expect(container.textContent).not.toContain("independent of account quotas");
  });
  it("manually selects native OpenCode for analytics and app context without inventing an account", async () => {
    const {getByRole}=render(AccessSourceBar);
    await fireEvent.click(getByRole("button",{name:/OpenCode Desktop/}));
    await waitFor(()=>expect(get(selectedAnalyticsProviderScope)).toBe("opencode"));
    expect(setProvider).toHaveBeenCalledWith("opencode");
    expect(get(selectedAccessSourceId)).toBe("local:opencode");
  });
  it("keeps the broadcaster when selecting the aggregate", async () => {
    provider.set("opencode"); selectedAnalyticsProviderScope.set("opencode");
    const {getByRole}=render(AccessSourceBar);
    await fireEvent.click(getByRole("button",{name:/All providers/}));
    expect(get(selectedAnalyticsProviderScope)).toBe("all"); expect(get(provider)).toBe("opencode");
    expect(setProvider).not.toHaveBeenCalled();
  });
  it("does not auto-switch when an account appears", () => {
    accessSnapshot.set({routes:[route("claude","claude_subscription","claude","max_5x")]});
    render(AccessSourceBar); expect(get(provider)).toBe("codex"); expect(setProvider).not.toHaveBeenCalled();
  });
  it("shows provider-reported plan labels", () => {
    accessSnapshot.set({routes:[route("codex","codex_subscription","codex","pro_20x")]});
    const {getByRole}=render(AccessSourceBar); expect(getByRole("button",{name:/Codex Pro 20x/})).toBeTruthy();
  });
  it("associates Go quotas with OpenCode rather than Codex", async () => {
    accessSnapshot.set({routes:[route("go","open_code_go","opencode")]});
    const {getByRole}=render(AccessSourceBar); await fireEvent.click(getByRole("button",{name:/OpenCode Go Go subscription/}));
    await waitFor(()=>expect(get(selectedAccessSourceId)).toBe("go"));
    expect(get(selectedAnalyticsProviderScope)).toBe("opencode");
  });
  it("does not expose unproved API routes as connected sources", () => {
    const api=route("api","open_ai_api","openai"); api.source.proof="none";
    accessSnapshot.set({routes:[api]}); const {container}=render(AccessSourceBar);
    expect(container.querySelector('[data-provider="openai"]')).toBeNull();
  });
  it("selects a proven API lane without changing the native broadcaster", async () => {
    accessSnapshot.set({routes:[route("api","open_ai_api","openai")]});
    const {getByRole}=render(AccessSourceBar); await fireEvent.click(getByRole("button",{name:/OpenAI API/}));
    expect(get(selectedAnalyticsProviderScope)).toBe("openai"); expect(get(provider)).toBe("codex");
  });
  it("keeps the previous selection when persistence fails", async () => {
    vi.mocked(setProvider).mockRejectedValueOnce(new Error("write failed"));
    const {getByRole}=render(AccessSourceBar); await fireEvent.click(getByRole("button",{name:/OpenCode Desktop/}));
    await waitFor(()=>expect(setProvider).toHaveBeenCalled()); expect(get(selectedAnalyticsProviderScope)).toBe("all");
  });
  it("provides a single labelled native mobile selector", () => {
    const {getByRole}=render(AccessSourceBar); const select=getByRole("combobox",{name:"Provider"});
    expect(Array.from(select.querySelectorAll("option")).map((option) => option.value)).toEqual(["claude", "codex", "opencode", "commandcode", "all"]);
  });
});
