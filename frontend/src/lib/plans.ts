import type { Provider } from "./provider";

export interface PlanOption {
  value: string;
  label: string;
}

/** Provider-owned catalogs. Keeping these separate prevents a stale plan from
 * being rendered under the wrong product while a provider switch is settling. */
const PLAN_CATALOGS: Record<Provider, readonly PlanOption[]> = {
  commandcode: [],
  opencode: [],
  codex: [
    { value: "free", label: "Free" },
    { value: "go", label: "Go" },
    { value: "plus", label: "Plus" },
    { value: "business", label: "Business" },
    { value: "enterprise", label: "Enterprise" },
    { value: "edu", label: "Edu" },
    { value: "pro_5x", label: "Pro 5x" },
    { value: "pro_20x", label: "Pro 20x" },
  ],
  claude: [
    { value: "free", label: "Free" },
    { value: "pro", label: "Pro" },
    { value: "team", label: "Teams" },
    { value: "enterprise", label: "Enterprise" },
    { value: "max_5x", label: "Max 5x" },
    { value: "max_20x", label: "Max 20x" },
  ],
};

export function planOptionsFor(provider: Provider): PlanOption[] {
  return PLAN_CATALOGS[provider].map((plan) => ({ ...plan }));
}

export function planLabelForKey(provider: Provider, key: string): string | null {
  return PLAN_CATALOGS[provider].find((plan) => plan.value === key)?.label ?? null;
}

/** Access-route payloads currently carry a provider-reported label rather than
 * a key. Accept only an exact catalog label; unknown claims stay unlabelled. */
export function verifiedPlanLabel(provider: Provider, claim: string | null | undefined): string | null {
  const raw = claim?.trim();
  if (!raw) return null;
  const normalized = raw.toLowerCase();
  const keyed = PLAN_CATALOGS[provider].find(
    (plan) => plan.value.toLowerCase() === normalized,
  );
  if (keyed) return keyed.label;

  const productPrefix = provider === "codex" ? /^codex\s+/i : /^(?:claude(?:\s+code)?)\s+/i;
  const withoutProduct = raw.replace(productPrefix, "").toLowerCase();
  return PLAN_CATALOGS[provider].find((plan) => (
    plan.label.toLowerCase() === normalized
    || plan.label.toLowerCase() === withoutProduct
  ))?.label ?? null;
}
