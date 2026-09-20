import { invoke } from "./api";
import type { Provider } from "./provider";
import { verifiedPlanLabel } from "./plans";
import type { RateLimitResetCreditsSummary, IndividualSpendLimit } from "./access";

export interface AccountWindow {
  id: string;
  label: string;
  scope: string;
  duration_minutes: number | null;
  used: number | null;
  limit: number | null;
  unit: string | null;
  used_percent: number | null;
  resets_at: string | null;
}
export interface AccountBalance {
  id: string;
  label: string;
  value: string;
  unit: string;
  expires_at: string | null;
  cycle_ends_at?: string | null;
}
export interface AccountSnapshot {
  id: string;
  provider: Provider;
  label: string;
  origin: "local" | "managed";
  identity: { subject: string | null; name: string | null; email: string | null; organization: string | null; verified: boolean };
  plan: string | null;
  status: "pending" | "connecting" | "awaiting_login" | "connected" | "stale" | "expired" | "cancelled" | "error" | "unavailable";
  observed_at: string | null;
  expires_at: string | null;
  retry_at: string | null;
  windows: AccountWindow[];
  balances: AccountBalance[];
  reset_credits: RateLimitResetCreditsSummary | null;
  individual_limits: IndividualSpendLimit[];
  error: string | null;
  notice: string | null;
  refreshing: boolean;
  auth_url: string | null;
}
export interface ConnectAccountRequest {
  account_id?: string;
  provider: Provider;
  method: "local" | "browser" | "api_key";
  label?: string;
  path?: string;
  api_key?: string;
}
export const listAccounts = (): Promise<AccountSnapshot[]> => invoke("list_accounts");
export const discoverAccounts = (): Promise<AccountSnapshot[]> => invoke("discover_accounts");
export const connectAccount = (request: ConnectAccountRequest): Promise<string> => invoke("connect_account", { request });
export const removeAccount = (accountId: string): Promise<void> => invoke("remove_account", { accountId });
export const refreshAccount = (accountId: string): Promise<void> => invoke("refresh_account", { accountId });
export const cancelAccountConnection = (accountId: string): Promise<void> => invoke("cancel_account_connection", { accountId });

export function accountName(account: AccountSnapshot): string {
  return account.identity.email ?? account.identity.name ?? account.label;
}
export function accountPlan(account: AccountSnapshot): string | null {
  if (account.provider === "codex") {
    if (account.plan === "pro") return "Pro 20x";
    if (account.plan === "pro_lite" || account.plan === "prolite") return "Pro 5x";
  }
  if (account.provider === "commandcode") {
    const names: Record<string, string> = { "individual-go": "Go", "individual-goat": "GOAT", "individual-pro": "Pro", "individual-pro-v1": "Pro", "individual-max": "Max", "individual-ultra": "Ultra", "individual-provider": "Provider", "teams-pro": "Teams Pro" };
    return account.plan ? names[account.plan] ?? account.plan : null;
  }
  return verifiedPlanLabel(account.provider, account.plan) ?? account.plan;
}
export function accountStatus(account: AccountSnapshot): string {
  if (account.refreshing) return "Refreshing";
  const labels: Record<AccountSnapshot["status"], string> = {
    pending: "Checking account", connecting: "Starting sign-in", awaiting_login: "Complete sign-in",
    connected: "Connected", stale: "Last known usage", expired: "Sign-in expired", cancelled: "Sign-in cancelled",
    error: "Could not refresh", unavailable: "Unavailable",
  };
  return labels[account.status] ?? "Unavailable";
}
export function windowDuration(minutes: number | null): string {
  if (!minutes) return "";
  if (minutes === 300) return "5 hours";
  if (minutes === 10080) return "Weekly";
  if (minutes >= 1440 && minutes % 1440 === 0) return String(minutes / 1440) + " days";
  if (minutes % 60 === 0) return String(minutes / 60) + " hours";
  return String(minutes) + " minutes";
}
