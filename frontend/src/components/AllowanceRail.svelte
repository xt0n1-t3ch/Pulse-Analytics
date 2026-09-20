<script lang="ts">
  import OpenCodeMark from "./OpenCodeMark.svelte";
  import { IconShieldCheck } from "@tabler/icons-svelte";
  import commandCodeMark from "../assets/rp/commandcode.png";
  import codexMark from "../assets/rp/codex-app.png";
  import claudeMark from "../assets/rp/claude.svg";
  import openCodeMark from "../assets/rp/opencode-v2.png";
  import openAiMark from "../assets/rp/chatgpt-app.jpg";
  import { selectedAccessRoutes, selectedAccessDiagnostics } from "../lib/stores";
  import {
    accessKindLabel,
    accessSourceName,
    allowancePresentation,
    windowLabel,
    type AccessKind,
    type AccessQuotaWindow,
  } from "../lib/access";
  import { formatResetDateTime } from "../lib/utils";

  let routes = $derived($selectedAccessRoutes);

  /** When the selected source has no authenticated proof but still owns local
   *  history (an expired Claude subscription), name the reason instead of
   *  falling back to the generic "waiting for proof" empty state. Historical
   *  analytics keep working; only the live allowance card is gated. */
  let localOnly = $derived.by(() => {
    if (routes.length > 0) return null;
    const candidate = $selectedAccessDiagnostics.find(
      (route) => route.source.proof === "none" && route.local_history.available,
    );
    return candidate ?? null;
  });
  let localOnlyExpired = $derived(localOnly?.unavailable_reason === "expired");
  let localOnlyName = $derived(localOnly ? accessSourceName(localOnly.source) : "");
  let localOnlySessions = $derived(localOnly?.local_history.sessions ?? 0);
  let signInHint = $derived.by(() => {
    if (!localOnly) return "";
    const kind = localOnly.source.kind;
    if (kind === "claude_subscription") return "Sign in to Claude Code to see live limits.";
    if (kind === "codex_subscription") return "Sign in to Codex to see live limits.";
    return "Sign in to the provider to see live limits.";
  });

  function markFor(kind: AccessKind): string {
    if (kind === "command_code_subscription" || kind === "command_code_local") return commandCodeMark;
    if (kind === "open_code_go") return openCodeMark;
    if (kind === "claude_subscription" || kind === "anthropic_api") return claudeMark;
    if (kind === "open_ai_api") return openAiMark;
    return codexMark;
  }

  function formatExpiry(epochSeconds: number | null): string | null {
    if (epochSeconds == null || !Number.isFinite(epochSeconds)) return null;
    const date = new Date(epochSeconds * 1000);
    if (!Number.isFinite(date.getTime())) return null;
    return new Intl.DateTimeFormat(undefined, {
      month: "short",
      day: "numeric",
      hour: "numeric",
      minute: "2-digit",
    }).format(date);
  }

  function canonicalWindowLabel(window: AccessQuotaWindow): string {
    if (window.key === "monthly" || window.key === "commandcode:monthly") return "Monthly limit";
    if (window.key.startsWith("opencode-go:") && window.label) return window.label;
    const minutes = window.window_minutes;
    if (minutes == null || !Number.isFinite(minutes) || minutes <= 0) {
      return `${windowLabel(window)} limit`;
    }
    if (minutes === 300) return "5-hour limit";
    if (minutes === 10_080) return "Weekly limit";
    if (minutes === 43_200) return "30-day limit";

    if (minutes % 1_440 === 0) {
      const days = minutes / 1_440;
      return `${days} ${days === 1 ? "day" : "days"} limit`;
    }

    const hours = minutes / 60;
    return `${Number.isInteger(hours) ? hours : hours.toFixed(1)} ${hours === 1 ? "hour" : "hours"} limit`;
  }

  function canonicalWindowTitle(window: AccessQuotaWindow): string {
    if (window.window_minutes === 10_080) return "Weekly usage limit";
    return canonicalWindowLabel(window);
  }

  function modelWindowLabel(window: AccessQuotaWindow): string | null {
    const label = window.label?.trim();
    const durationLabel = /^(?:weekly|[0-9]+(?:[ -]?(?:hour|day|week|month)s?|[hdw]))(?: (?:limit|usage))?$/i;
    if (!label || label === canonicalWindowLabel(window) || durationLabel.test(label)) return null;
    return label;
  }

</script>

<section class="allowance-rail" aria-label="Provider allowances">
  <div class="rail-head">
    <div>
      <h2>Provider limits</h2>
      <p>Live usage limits from your signed-in providers.</p>
    </div>
  </div>

  {#if localOnly}
    <div class="allowance-local" class:expired={localOnlyExpired}>
      <img src={markFor(localOnly.source.kind)} alt="" />
      <div class="al-copy">
        <div class="al-head">
          <strong>{localOnlyName}</strong>
          <span class="al-pill" class:expired={localOnlyExpired}>
            {localOnlyExpired ? "Session expired" : "Sign in required"}
          </span>
        </div>
        <span class="al-sub">
          {localOnlyExpired
            ? `${signInHint} Historical analytics stay available from ${localOnlySessions.toLocaleString()} local sessions.`
            : `${signInHint} ${localOnlySessions.toLocaleString()} local sessions remain available.`}
        </span>
      </div>
    </div>
  {:else if routes.length === 0}
    <div class="allowance-empty">
      <IconShieldCheck size={24} stroke={1.45} aria-hidden="true" />
      <div>
        <strong>No live limits yet</strong>
        <span>Sign in to a provider to see usage limits here.</span>
      </div>
    </div>
  {:else}
    <div class="allowance-list">
      {#each routes as route (route.source.id)}
        {@const label = accessKindLabel(route.source.kind)}
        <article class="allowance-card">
          <header>
            {#if route.source.kind === "open_code_go"}<OpenCodeMark />{:else}<img src={markFor(route.source.kind)} alt="" />{/if}
            <div>
              <strong>{accessSourceName(route.source)}</strong>
              <span>{label.access}</span>
            </div>
            <i class:available={route.availability === "available" && route.freshness === "fresh"}></i>
          </header>

          {#if route.windows.length > 0}
            <div class="window-list">
              {#each route.windows as window, index (`${route.source.id}:${window.key}:${window.window_minutes ?? "native"}:${index}`)}
                {@const presentation = allowancePresentation(route, window)}
                {@const primaryLabel = canonicalWindowLabel(window)}
                {@const modelLabel = modelWindowLabel(window)}
                <section class="window-row">
                  <div class="window-copy">
                    <div class="window-name">
                      <span title={canonicalWindowTitle(window)}>{primaryLabel}</span>
                      {#if modelLabel}<small>{modelLabel}</small>{/if}
                    </div>
                    <strong>
                      {presentation == null
                        ? "Unavailable"
                        : `${Math.round(presentation.percent)}% ${presentation.direction}`}
                    </strong>
                  </div>
                  <div
                    class="window-meter"
                    role="img"
                    aria-label={`${primaryLabel} ${modelLabel ? `${modelLabel} ` : ""}${presentation == null ? "unavailable" : `${Math.round(presentation.percent)}% ${presentation.direction}`}`}
                  >
                    <span style={`width:${presentation?.percent ?? 0}%`}></span>
                  </div>
                  <small>{window.resets_at ? formatResetDateTime(window.resets_at) : "Reset not reported"}</small>
                </section>
              {/each}
            </div>
          {:else if !route.extra_usage}
            <div class="no-allowance">
              Authenticated, but this source exposes no allowance counters.
            </div>
          {/if}

          {#if route.extra_usage && route.extra_usage.used != null}
            <div class="api-summary">
              <span>Month-to-date usage</span>
              <strong>
                {route.availability !== "available"
                  || route.freshness !== "fresh"
                  || route.extra_usage.used == null
                  ? "Unavailable"
                  : `$${route.extra_usage.used.toFixed(2)}`}
              </strong>
            </div>
          {/if}

          {#if route.availability === "available"
            && route.freshness === "fresh"
            && (route.individualSpendLimits?.length ?? 0) > 0}
            <section class="spend-limits" aria-label="Individual spend limits">
              {#each route.individualSpendLimits ?? [] as limit (limit.limitId)}
                <div class="spend-limit">
                  <span>{limit.limitId}</span>
                  <strong>
                    {limit.used != null && limit.limit != null
                      ? `${limit.used} of ${limit.limit}`
                      : `${Math.round(limit.remainingPercent)}% remaining`}
                  </strong>
                  {#if limit.resetsAt}
                    <small>{formatResetDateTime(limit.resetsAt)}</small>
                  {/if}
                </div>
              {/each}
            </section>
          {/if}

          {#if route.availability === "available"
            && route.freshness === "fresh"
            && route.rateLimitResetCredits
            && route.rateLimitResetCredits.availableCount > 0}
            <section class="reset-entitlement" aria-label="Usage limit resets">
              <div class="reset-summary">
                <span>Usage limit resets</span>
                <strong>
                  {route.rateLimitResetCredits.availableCount}
                  {route.rateLimitResetCredits.availableCount === 1 ? "reset" : "resets"} available
                </strong>
              </div>
              {#each route.rateLimitResetCredits.credits ?? [] as credit (credit.id)}
                {@const expiry = formatExpiry(credit.expiresAt)}
                <div class="reset-credit">
                  <span>{credit.title?.trim() || "Quota reset"}</span>
                  {#if expiry}<small>Expires {expiry}</small>{/if}
                </div>
              {/each}
            </section>
          {/if}
        </article>
      {/each}
    </div>
  {/if}
</section>

<style>
  .allowance-rail {
    min-width: 0;
    height: auto;
    padding: 20px;
    /* Column of the shared Dashboard home-grid card. It contributes no border,
       radius, or shadow of its own; the parent grid owns the surface. */
    background: transparent;
  }

  .rail-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 18px;
  }

  .rail-head h2 { font-size: 19px; letter-spacing: -0.03em; }
  .rail-head p { max-width: 260px; margin-top: 5px; color: var(--text-muted); font-size: 11px; line-height: 1.45; }

  .allowance-list { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(250px, 100%), 1fr)); gap: 0 24px; align-items: start; }
  .allowance-card {
    min-width: 0;
    padding: 15px 0;
    border-top: 1px solid var(--divider);
  }
  .allowance-card:last-child { padding-bottom: 0; }

  .allowance-card header {
    display: grid;
    grid-template-columns: 32px minmax(0, 1fr) 8px;
    align-items: center;
    gap: 9px;
    padding: 0 0 11px;
  }

  .allowance-card header img { width: 30px; height: 30px; object-fit: contain; border-radius: 8px; }
  .allowance-card header div { min-width: 0; display: grid; gap: 2px; }
  .allowance-card header strong { overflow: hidden; font-size: 12px; text-overflow: ellipsis; white-space: nowrap; }
  .allowance-card header span { color: var(--text-muted); font-size: 10px; }
  .allowance-card i { width: 6px; height: 6px; background: var(--text-placeholder); border-radius: 50%; }
  .allowance-card i.available { background: var(--success); }

  .window-list { display: grid; }
  .window-row { display: grid; gap: 8px; padding: 11px 0; border-top: 1px solid var(--divider); }
  .window-row:last-child { border-bottom: 0; }
  .window-copy { display: flex; align-items: flex-start; justify-content: space-between; gap: 8px; }
  .window-name { min-width: 0; display: grid; gap: 2px; }
  .window-copy span { color: var(--text-secondary); font-size: 11px; }
  .window-name small { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .window-copy strong { flex: 0 0 auto; font-size: 12px; font-variant-numeric: tabular-nums; }
  .window-meter { height: 6px; overflow: hidden; background: var(--meter-track); border-radius: var(--radius-full); }
  .window-meter span { display: block; height: 100%; background: var(--info); border-radius: inherit; }
  .window-row small { color: var(--text-muted); font-size: 9px; }

  .api-summary,
  .no-allowance {
    padding: 11px 0 0;
    color: var(--text-muted);
    font-size: 9px;
  }

  .api-summary { display: flex; align-items: center; justify-content: space-between; }
  .api-summary strong { color: var(--text-primary); font-size: 12px; }

  .spend-limits {
    display: grid;
    gap: 9px;
    padding-top: 12px;
    border-top: 1px solid var(--divider);
  }

  .spend-limit { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 4px 10px; }
  .spend-limit span,
  .spend-limit small { color: var(--text-muted); font-size: 9px; }
  .spend-limit strong { font-size: 11px; font-variant-numeric: tabular-nums; }
  .spend-limit small { grid-column: 1 / -1; }

  .reset-entitlement {
    display: grid;
    gap: 9px;
    padding-top: 12px;
    border-top: 1px solid var(--divider);
  }
  .reset-summary,
  .reset-credit {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }
  .reset-summary span,
  .reset-credit span { color: var(--text-secondary); font-size: 11px; }
  .reset-summary strong {
    color: var(--success);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }
  .reset-credit small { color: var(--text-muted); font-size: 9px; }

  .allowance-empty {
    min-height: 92px;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 0 2px;
    color: var(--text-muted);
    border-top: 1px solid var(--divider);
  }
  .allowance-empty :global(svg) { flex: 0 0 auto; color: var(--info); }
  .allowance-empty div { display: grid; gap: 4px; }
  .allowance-empty strong { color: var(--text-secondary); font-size: 11px; }
  .allowance-empty span { font-size: 9px; line-height: 1.5; }

  .allowance-local {
    display: grid;
    grid-template-columns: 34px minmax(0, 1fr);
    gap: 12px;
    align-items: start;
    padding: 16px 0 2px;
    border-top: 1px solid var(--divider);
  }
  .allowance-local img { width: 32px; height: 32px; object-fit: contain; border-radius: 8px; }
  .al-copy { min-width: 0; display: grid; gap: 5px; }
  .al-head { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .al-head strong { font-size: 12px; color: var(--text-primary); }
  .al-pill {
    display: inline-flex;
    align-items: center;
    padding: 2px 8px;
    border-radius: var(--radius-full);
    font-size: 9px;
    font-weight: 700;
    letter-spacing: var(--letter-wide);
    text-transform: uppercase;
    color: var(--warning);
    background: var(--warning-dim);
    border: 1px solid color-mix(in srgb, var(--warning) 30%, transparent);
  }
  .al-pill.expired {
    color: var(--danger);
    background: var(--danger-dim);
    border-color: color-mix(in srgb, var(--danger) 30%, transparent);
  }
  .al-sub { color: var(--text-muted); font-size: 10px; line-height: 1.5; }



  @media (max-width: 620px) {
    .allowance-card { padding-right: 0; }
    .allowance-card + .allowance-card { padding-left: 0; border-left: 0; }
  }
</style>
