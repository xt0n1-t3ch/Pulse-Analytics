import { describe, it, expect } from "vitest";
import {
  fmtTokens,
  fmtCost,
  fmtExactCost,
  fmtDuration,
  fmtTps,
  fmtPct,
  usageColor,
  classifyActivity,
  fmtClock,
  formatResetDateTime,
  monetaryValueKind,
  monetaryValueLabel,
} from "@/lib/utils";

describe("fmtTokens", () => {
  it("uses billions instead of rendering four-digit millions", () => {
    expect(fmtTokens(11_330_800_000)).toBe("11.3B");
    expect(fmtTokens(1_245_400_000)).toBe("1.2B");
  });

  it("formats millions, thousands, and units", () => {
    expect(fmtTokens(2_500_000)).toBe("2.5M");
    expect(fmtTokens(1_500)).toBe("1.5K");
    expect(fmtTokens(42)).toBe("42");
  });
});

describe("fmtCost", () => {
  it("renders a two-decimal dollar amount", () => {
    expect(fmtCost(3)).toBe("$3.00");
    expect(fmtCost(12.345)).toBe("$12.35");
  });
});

describe("fmtExactCost", () => {
  it("keeps a measured zero distinct from an unavailable estimate", () => {
    expect(fmtExactCost(0, true)).toBe("$0.00");
    expect(fmtExactCost(0, false)).toBe("—");
  });

  it("never emits lower-bound or approximate cost notation", () => {
    expect(fmtExactCost(0.454, true)).toBe("$0.45");
    expect(fmtExactCost(0.454, false)).not.toMatch(/[>=~]/);
  });
});

describe("monetary value provenance", () => {
  it("does not present API-equivalent arithmetic as provider-billed spend", () => {
    expect(monetaryValueKind(["anthropic_api_equivalent", "session-calculated"]))
      .toBe("api-equivalent");
    expect(monetaryValueLabel(["anthropic_api_equivalent"]))
      .toBe("API-equivalent value");
  });

  it("reserves spend language for provider billing and labels mixed sources neutrally", () => {
    expect(monetaryValueLabel(["provider_billed"])).toBe("Provider-billed spend");
    expect(monetaryValueLabel(["provider_billed", "api_equivalent"]))
      .toBe("Known monetary value");
    expect(monetaryValueLabel([])).toBe("Known monetary value");
  });
});

describe("fmtDuration", () => {
  it("renders seconds, minutes, and hour+minute tiers", () => {
    expect(fmtDuration(45)).toBe("45s");
    expect(fmtDuration(90)).toBe("1m");
    expect(fmtDuration(3 * 3600 + 25 * 60)).toBe("3h 25m");
  });
});

describe("fmtTps", () => {
  it("switches to K/s above a thousand", () => {
    expect(fmtTps(820)).toBe("820/s");
    expect(fmtTps(2_400)).toBe("2.4K/s");
  });
});

describe("fmtPct", () => {
  it("rounds to a whole percent", () => {
    expect(fmtPct(83.4)).toBe("83%");
    expect(fmtPct(99.6)).toBe("100%");
  });
});

describe("usageColor", () => {
  it("maps the three severity tiers", () => {
    expect(usageColor(20)).toBe("normal");
    expect(usageColor(60)).toBe("warning");
    expect(usageColor(95)).toBe("danger");
  });
});

describe("classifyActivity", () => {
  it("maps activity strings to canonical types", () => {
    expect(classifyActivity("Thinking")).toBe("thinking");
    expect(classifyActivity("Editing file.ts")).toBe("editing");
    expect(classifyActivity("Reading foo")).toBe("reading");
    expect(classifyActivity("Running command")).toBe("running");
    expect(classifyActivity("Waiting on input")).toBe("waiting");
    expect(classifyActivity("doing nothing")).toBe("idle");
  });
});

describe("fmtClock", () => {
  it("passes through HH:MM and falls back to an em dash on empty input", () => {
    expect(fmtClock("14:30")).toBe("14:30");
    expect(fmtClock(null)).toBe("—");
    expect(fmtClock(undefined)).toBe("—");
  });
});

describe("formatResetDateTime", () => {
  it("renders the reset instant as an absolute local date and time", () => {
    const localReset = new Date(2026, 6, 22, 23, 16);
    expect(formatResetDateTime(localReset.toISOString())).toBe("Resets Jul 22, 2026 11:16 PM");
  });

  it("falls back to the raw value when the reset timestamp is invalid", () => {
    expect(formatResetDateTime("not-a-date")).toBe("not-a-date");
  });
});
