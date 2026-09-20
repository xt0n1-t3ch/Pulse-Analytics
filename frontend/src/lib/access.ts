export type AccessKind =
  | "codex_subscription"
  | "open_ai_api"
  | "claude_subscription"
  | "anthropic_api"
  | "open_code_local"
  | "open_code_go"
  | "command_code_local"
  | "command_code_subscription";

export type AccessProvider = "codex" | "claude" | "openai" | "anthropic" | "opencode" | "commandcode";
export type AnalyticsProviderScope = AccessProvider | "opencode" | "all";
export type AccessProof =
  | "authenticated_probe"
  | "quota_response"
  | "none";
export type AccessAvailability = "available" | "unavailable";
export type AccessFreshness = "fresh" | "stale" | "unknown";
export type AccessUnavailableReason =
  | "expired"
  | "not_configured"
  | "probe_failed"
  | "no_data"
  | "other";

export interface AccessSource {
  id: string;
  kind: AccessKind;
  provider: AccessProvider;
  auth_method: "oauth" | "api_key" | "app_server" | "none";
  proof: AccessProof;
  plan: string | null;
}

export interface AccessQuotaWindow {
  key: string;
  label: string | null;
  window_minutes: number | null;
  used_percent?: number | null;
  remaining_percent?: number | null;
  resets_at?: string | null;
}

export interface RateLimitResetCredit {
  id: string;
  resetType: string;
  status: string;
  grantedAt: number;
  expiresAt: number | null;
  title: string | null;
  description: string | null;
}

export interface RateLimitResetCreditsSummary {
  availableCount: number;
  credits: RateLimitResetCredit[] | null;
}

export interface IndividualSpendLimit {
  limitId: string;
  limit: string | null;
  used: string | null;
  remainingPercent: number;
  resetsAt: string | null;
}

export interface AccessRouteSnapshot {
  source: AccessSource;
  availability: AccessAvailability;
  freshness: AccessFreshness;
  provenance: "provider_api" | "app_server" | "memory_cache" | "session_jsonl" | "none";
  observed_at: string | null;
  fetched_at: string | null;
  expires_at: string | null;
  windows: AccessQuotaWindow[];
  credits: { balance: string | null; has_credits: boolean; unlimited: boolean } | null;
  extra_usage: {
    enabled: boolean;
    used: number | null;
    limit: number | null;
    utilization: number | null;
  } | null;
  rateLimitResetCredits?: RateLimitResetCreditsSummary;
  individualSpendLimits?: IndividualSpendLimit[];
  local_history: {
    available: boolean;
    sessions: number;
  };
  error: string | null;
  /** Classified reason a route is unavailable, derived once by the backend.
   *  Absent when the route is available. */
  unavailable_reason?: AccessUnavailableReason | null;
}

export interface AccessSnapshot {
  routes: AccessRouteSnapshot[];
}

const KIND_LABELS: Record<
  AccessKind,
  { product: string; access: "Subscription" | "API" | "Local" }
> = {
  command_code_local: { product: "Command Code", access: "Local" },
  command_code_subscription: { product: "Command Code", access: "Subscription" },
  open_code_go: { product: "OpenCode Go", access: "Subscription" },
  open_code_local: { product: "OpenCode", access: "Local" },
  codex_subscription: { product: "Codex", access: "Subscription" },
  open_ai_api: { product: "OpenAI", access: "API" },
  claude_subscription: { product: "Claude", access: "Subscription" },
  anthropic_api: { product: "Anthropic", access: "API" },
};

export function accessKindLabel(kind: AccessKind): {
  product: string;
  access: "Subscription" | "API" | "Local";
} {
  return KIND_LABELS[kind];
}

export function accessSourceName(source: AccessSource): string {
  const product = accessKindLabel(source.kind).product;
  const provider = source.provider === "codex" || source.provider === "claude"
    ? source.provider
    : null;
  const plan = provider ? verifiedPlanLabel(provider, source.plan) : null;
  if (!plan) return product;
  return `${product} ${plan}`;
}

/** Quota and provider-health consumers require provider evidence. */
export function authenticatedAccessRoutes(
  routes: AccessRouteSnapshot[],
): AccessRouteSnapshot[] {
  return routes.filter((route) => route.source.proof !== "none");
}

/** Historical analytics remain useful when a subscription token expires.
 * Unproved API lanes stay hidden unless they actually own local history. */
export function displayableAccessRoutes(
  routes: AccessRouteSnapshot[],
): AccessRouteSnapshot[] {
  return routes.filter(
    (route) => route.source.proof !== "none" || route.local_history?.available === true,
  );
}

/** @deprecated Use the capability-specific selector instead. */
export const visibleAccessRoutes = authenticatedAccessRoutes;

/**
 * Access quotas are keyed by stable source id, while historical analytics are
 * intentionally aggregated by provider. `all` is the only cross-provider
 * scope; an unknown source fails closed to that explicit aggregate instead of
 * borrowing a neighboring product's telemetry.
 */
export function analyticsProviderScopeForSelection(
  selectedSourceId: string,
  routes: AccessRouteSnapshot[],
): AnalyticsProviderScope | null {
  if (selectedSourceId === "all") return "all";
  return routes.find((route) => route.source.id === selectedSourceId)?.source.provider ?? null;
}

/** API lanes are separate providers. OpenAI must not reuse Codex subscription
 * sessions, and Anthropic API must not reuse Claude subscription sessions. */
export function providerMatchesAnalyticsScope(
  sessionProvider: string,
  scope: AnalyticsProviderScope,
): boolean {
  return scope === "all" || sessionProvider === scope;
}

export function windowLabel(
  window: Pick<AccessQuotaWindow, "key" | "label" | "window_minutes">,
): string {
  if (window.label?.trim()) return window.label.trim();

  const minutes = window.window_minutes;
  if (minutes && minutes > 0) {
    if (minutes % 10_080 === 0) return `${minutes / 10_080}w`;
    if (minutes % 1_440 === 0) return `${minutes / 1_440}d`;
    if (minutes % 60 === 0) return `${minutes / 60}h`;
    return `${minutes}m`;
  }

  return window.key
    .split(/[_-]+/)
    .filter(Boolean)
    .map((part) => part[0]?.toUpperCase() + part.slice(1))
    .join(" ");
}

export interface AllowancePresentation {
  percent: number;
  direction: "remaining" | "used";
}

/** Codex communicates headroom (100 → 0), while Claude communicates
 * consumption (0 → 100). Both values must already exist in the provider DTO;
 * the UI never guesses a missing complement. */
export function allowancePresentation(
  route: Pick<AccessRouteSnapshot, "availability" | "freshness" | "source">,
  window: Pick<AccessQuotaWindow, "used_percent" | "remaining_percent">,
): AllowancePresentation | null {
  if (
    route.availability !== "available"
    || route.freshness !== "fresh"
  ) {
    return null;
  }

  const direction = route.source.provider === "codex" ? "remaining" : "used";
  const value = direction === "remaining"
    ? window.remaining_percent
    : window.used_percent;
  if (value == null || !Number.isFinite(value)) return null;
  return {
    percent: Math.min(100, Math.max(0, value)),
    direction,
  };
}
import { verifiedPlanLabel } from "./plans";
