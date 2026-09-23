export function fmtTokens(n: number): string {
  if (n >= 1e9) return (n / 1e9).toFixed(1) + "B";
  if (n >= 1e6) return (n / 1e6).toFixed(1) + "M";
  if (n >= 1e3) return (n / 1e3).toFixed(1) + "K";
  return String(n);
}

export function fmtCost(n: number): string {
  return "$" + n.toFixed(2);
}

export type MonetaryValueKind = "provider-billed" | "api-equivalent" | "mixed" | "unknown";

/** Classify aggregate monetary provenance before choosing user-facing copy.
 * Arithmetic can be exact while still representing API-equivalent value rather
 * than a provider invoice, so `CostBasis` alone is not enough for this label. */
export function monetaryValueKind(sources: readonly string[]): MonetaryValueKind {
  let hasProviderBilling = false;
  let hasApiEquivalent = false;
  let hasOther = false;

  for (const rawSource of sources) {
    const source = rawSource.trim().toLowerCase().replaceAll("-", "_");
    if (!source || source === "unknown") continue;
    if (source === "provider_billed") {
      hasProviderBilling = true;
    } else if (
      source.includes("api_equivalent")
      || source === "session_calculated"
      || source === "legacy_calculated"
      || source === "live_session"
      || source.includes("pricing")
    ) {
      hasApiEquivalent = true;
    } else {
      hasOther = true;
    }
  }

  if (hasProviderBilling && !hasApiEquivalent && !hasOther) return "provider-billed";
  if (hasApiEquivalent && !hasProviderBilling && !hasOther) return "api-equivalent";
  if (hasProviderBilling || hasApiEquivalent || hasOther) return "mixed";
  return "unknown";
}

export function monetaryValueLabel(sources: readonly string[]): string {
  if (sources.length > 0 && sources.every((source) => source === "opencode_reported")) return "OpenCode-reported value";
  const kind = monetaryValueKind(sources);
  if (kind === "provider-billed") return "Provider-billed spend";
  if (kind === "api-equivalent") return "API-equivalent value";
  return "Known monetary value";
}

/** Exact-cost renderer shared by every consumer surface. A transport-level
 * zero is not evidence of a measured zero, so unproved values stay neutral. */
export function fmtExactCost(n: number, available: boolean): string {
  return available ? fmtCost(n) : "—";
}

export function fmtDuration(secs: number): string {
  if (secs < 60) return secs + "s";
  if (secs < 3600) return Math.floor(secs / 60) + "m";
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return h + "h " + m + "m";
}

export function fmtTps(tps: number): string {
  if (tps >= 1000) return (tps / 1000).toFixed(1) + "K/s";
  return tps.toFixed(0) + "/s";
}

export function fmtPct(n: number): string {
  return Math.round(n) + "%";
}

export function usageColor(pct: number): "normal" | "warning" | "danger" {
  if (pct > 80) return "danger";
  if (pct > 50) return "warning";
  return "normal";
}

function parseResetDate(raw: string): Date | null {
  const iso = Date.parse(raw);
  if (!isNaN(iso)) return new Date(iso);
  const match = raw.match(/(\d{1,2}):(\d{2})\s*UTC/);
  if (!match) return null;
  const now = new Date();
  const d = new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate(), +match[1], +match[2]));
  if (d.getTime() <= now.getTime()) d.setUTCDate(d.getUTCDate() + 1);
  return d;
}

/// Format an ISO-8601 / RFC3339 timestamp (or `HH:MM` legacy) as local `HH:MM`.
/// Falls back to `—` if the input is missing or unparseable.
export function fmtClock(raw: string | null | undefined): string {
  if (!raw) return "—";
  if (/^\d{1,2}:\d{2}$/.test(raw)) return raw;
  const d = new Date(raw);
  if (Number.isNaN(d.getTime())) return raw;
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

const RESET_DATE_FORMAT = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
  year: "numeric",
});

const RESET_TIME_FORMAT = new Intl.DateTimeFormat("en-US", {
  hour: "numeric",
  minute: "2-digit",
  hour12: true,
});

export function formatResetDateTime(raw: string): string {
  const reset = parseResetDate(raw);
  if (!reset) return raw;
  return `Resets ${RESET_DATE_FORMAT.format(reset)} ${RESET_TIME_FORMAT.format(reset)}`;
}

export type ActivityType =
  | "thinking"
  | "editing"
  | "reading"
  | "running"
  | "waiting"
  | "idle";

export function classifyActivity(activity: string): ActivityType {
  const a = activity.toLowerCase();
  if (a.includes("thinking")) return "thinking";
  if (a.includes("edit")) return "editing";
  if (a.includes("read")) return "reading";
  if (a.includes("running") || a.includes("command")) return "running";
  if (a.includes("waiting")) return "waiting";
  return "idle";
}
