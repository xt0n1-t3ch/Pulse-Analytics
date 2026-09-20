<script lang="ts">
  import { onMount } from "svelte";
  import { IconPlus, IconRefresh, IconX, IconExternalLink, IconUnlink, IconFolder, IconCheck, IconClock, IconAlertCircle, IconRotateClockwise, IconBuilding, IconShieldLock } from "@tabler/icons-svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { hasTauriIpc } from "../lib/api";
  import { PROVIDERS, type Provider } from "../lib/provider";
  import { rpArtFor } from "../lib/rpArt";
  import { addToast } from "../lib/stores";
  import {
    accountName, accountPlan, accountStatus, windowDuration, listAccounts, discoverAccounts,
    connectAccount, removeAccount, refreshAccount, cancelAccountConnection,
    type AccountSnapshot, type ConnectAccountRequest,
  } from "../lib/accounts";

  const providers: Provider[] = ["claude", "codex", "opencode", "commandcode"];
  let accounts = $state<AccountSnapshot[]>([]);
  let selectedId = $state<string | null>(null);
  let loading = $state(true);
  let error = $state("");
  let adding = $state(false);
  let submitting = $state(false);
  let now = $state(Date.now());
  let addProvider = $state<Provider>("claude");
  let method = $state<ConnectAccountRequest["method"]>("browser");
  let connectionName = $state("");
  let reconnectId = $state<string | undefined>(undefined);
  let profilePath = $state("");
  let apiKey = $state("");
  let formError = $state("");
  let removeCandidate = $state<AccountSnapshot | null>(null);
  let removeDialog: HTMLDialogElement;
  let removing = $state(false);
  let stopped = false;
  let inFlight = false;

  let selected = $derived(accounts.find((account) => account.id === selectedId) ?? null);
  let groups = $derived(providers.map((provider) => ({ provider, accounts: accounts.filter((account) => account.provider === provider) })));
  let connected = $derived(accounts.filter((account) => account.status === "connected").length);

  async function reload(discover = false): Promise<void> {
    if (inFlight) return;
    inFlight = true;
    try {
      const result = await (discover ? discoverAccounts() : listAccounts());
      if (stopped) return;
      accounts = result;
      if (!result.some((account) => account.id === selectedId)) selectedId = result[0]?.id ?? null;
      error = "";
    } catch (cause) {
      if (!stopped) error = cause instanceof Error ? cause.message : String(cause);
    } finally {
      if (!stopped) loading = false;
      inFlight = false;
    }
  }

  onMount(() => {
    stopped = false;
    void reload(true);
    const timer = setInterval(() => { now = Date.now(); void reload(); }, 2000);
    return () => { stopped = true; clearInterval(timer); apiKey = ""; };
  });

  function pick(id: string): void { selectedId = id; adding = false; formError = ""; apiKey = ""; }
  function startAdd(provider: Provider = "claude"): void {
    reconnectId = undefined;
    addProvider = provider;
    method = provider === "opencode" ? "api_key" : "browser";
    connectionName = ""; profilePath = ""; apiKey = ""; formError = ""; adding = true;
  }
  function reconnect(account: AccountSnapshot): void {
    startAdd(account.provider);
    reconnectId = account.origin === "managed" ? account.id : undefined;
    connectionName = account.label;
  }
  function changeAddProvider(value: string): void {
    reconnectId = undefined;
    addProvider = value as Provider;
    method = addProvider === "opencode" ? "api_key" : "browser";
    apiKey = ""; profilePath = ""; formError = "";
  }
  function timeLabel(value: string | null): string {
    if (!value) return "Not reported";
    const date = new Date(value);
    return Number.isFinite(date.getTime()) ? date.toLocaleString([], { dateStyle: "medium", timeStyle: "short" }) : "Not reported";
  }
  function resetLabel(value: string | null): string {
    if (!value) return "Reset not reported";
    const seconds = (new Date(value).getTime() - now) / 1000;
    if (!Number.isFinite(seconds)) return "Reset not reported";
    if (seconds <= 0) return "Awaiting reset update";
    if (seconds >= 86400) return "Resets in " + Math.ceil(seconds / 86400) + " days";
    if (seconds >= 3600) return "Resets in " + Math.ceil(seconds / 3600) + " hours";
    return "Resets in " + Math.max(1, Math.ceil(seconds / 60)) + " min";
  }
  function number(value: number | string): string {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 }).format(parsed) : String(value);
  }
  function waiting(account: AccountSnapshot): boolean { return ["pending", "connecting", "awaiting_login"].includes(account.status); }
  function connectionLabel(account: AccountSnapshot): string {
    const defaultName = account.provider === "opencode" ? "OpenCode Go" : PROVIDERS[account.provider].label;
    if (account.status === "unavailable") return "Usage unavailable";
    return account.label !== defaultName && account.label !== "Personal account" && account.label !== "Local account" ? account.label : accountPlan(account) ?? "";
  }

  async function refreshSelected(): Promise<void> {
    if (!selected) return;
    try { await refreshAccount(selected.id); await reload(); }
    catch (cause) { addToast(String(cause), "danger"); }
  }
  async function browseProfile(): Promise<void> {
    try { const path = await open({ directory: true, multiple: false }); if (typeof path === "string") profilePath = path; }
    catch { formError = "Could not open the profile folder picker."; }
  }
  async function addConnection(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (submitting) return;
    submitting = true; formError = "";
    try {
      const id = await connectAccount({
        account_id: reconnectId, provider: addProvider, method, label: connectionName.trim() || undefined,
        path: method === "local" ? profilePath.trim() : undefined,
        api_key: method === "api_key" ? apiKey : undefined,
      });
      selectedId = id; adding = false; apiKey = "";
      await reload();
    } catch (cause) { formError = cause instanceof Error ? cause.message : String(cause); }
    finally { submitting = false; }
  }
  function requestRemove(account: AccountSnapshot): void { removeCandidate = account; removeDialog.showModal(); }
  async function confirmRemove(): Promise<void> {
    if (!removeCandidate || removing) return;
    removing = true;
    try {
      await removeAccount(removeCandidate.id);
      removeDialog.close();
      await reload();
      addToast("Account removed from Pulse. Your coding apps are unchanged.", "success");
    } catch (cause) { addToast(String(cause), "danger"); }
    finally { removing = false; }
  }
  async function cancelSignIn(account: AccountSnapshot): Promise<void> {
    try { await cancelAccountConnection(account.id); await reload(); }
    catch (cause) { addToast(String(cause), "danger"); }
  }
</script>

<section class="accounts-page" aria-labelledby="accounts-title">
  <header class="accounts-heading">
    <div>
      <h1 id="accounts-title">Accounts</h1>
      <p>Usage and credits across your coding accounts.</p>
    </div>
    <button class="add-button" type="button" onclick={() => startAdd()}><IconPlus size={17} />Add account</button>
  </header>

  {#if error}<div class="page-error" role="alert"><IconAlertCircle size={18} /><span>{error}</span><button type="button" onclick={() => reload(true)}>Try again</button></div>{/if}
  {#if loading}
    <div class="loading-state" role="status"><IconRefresh size={19} />Discovering your local accounts…</div>
  {:else}
    <div class="accounts-workspace">
      <aside class="account-index" aria-label="Connected accounts">
        <div class="index-summary"><span>{accounts.length} {accounts.length === 1 ? "connection" : "connections"}</span><span>{connected} connected</span></div>
        {#each groups as group (group.provider)}
          <section class="provider-group" aria-label={PROVIDERS[group.provider].label}>
            <div class="group-heading">
              <img src={rpArtFor(group.provider, group.provider === "codex" ? "codex-app" : undefined).large} alt="" />
              <h2>{group.provider === "opencode" ? "OpenCode Go" : PROVIDERS[group.provider].label}</h2>
              <button class="icon-button" type="button" aria-label={"Add " + PROVIDERS[group.provider].label + " account"} onclick={() => startAdd(group.provider)}><IconPlus size={15} /></button>
            </div>
            {#if group.accounts.length}
              {#each group.accounts as account (account.id)}
                <button class="account-row" class:inactive={account.status === "unavailable"} class:selected={selectedId === account.id && !adding} type="button" aria-pressed={selectedId === account.id && !adding} onclick={() => pick(account.id)}>
                  <span class="account-row-copy"><strong title={accountName(account)}>{account.provider === "opencode" && account.status === "unavailable" && !account.identity.verified ? "No active Go plan" : accountName(account)}</strong>{#if connectionLabel(account)}<small>{connectionLabel(account)}</small>{/if}</span>
                  <span class="status-dot" class:connected={account.status === "connected"} class:attention={["error", "expired"].includes(account.status)} aria-label={accountStatus(account)}></span>
                </button>
              {/each}
            {:else}
              <button class="empty-provider" type="button" onclick={() => startAdd(group.provider)}>Connect account<IconPlus size={14} /></button>
            {/if}
          </section>
        {/each}
        <div class="index-note"><IconShieldLock size={18} /><div><strong>Your agents stay signed in</strong><p>Manage connections here without changing their logins.</p></div></div>
      </aside>

      <div class="account-content">
        <label class="mobile-account-picker">Account
          <select aria-label="Selected account" value={selectedId ?? ""} onchange={(event) => pick(event.currentTarget.value)}>
            {#each accounts as account (account.id)}<option value={account.id}>{PROVIDERS[account.provider].label + " · " + accountName(account)}</option>{/each}
          </select>
        </label>
        {#if adding}
          <section class="connect-panel" aria-labelledby="connect-title">
            <header class="detail-heading"><div><h2 id="connect-title">{reconnectId ? "Reconnect account" : "Add an account"}</h2><p>Link a local profile or create a separate Pulse connection.</p></div><button class="icon-button" aria-label="Close add account" type="button" onclick={() => { adding = false; apiKey = ""; }}><IconX size={19} /></button></header>
            <form onsubmit={addConnection} class="connect-form">
              <label>Provider<select value={addProvider} onchange={(event) => changeAddProvider(event.currentTarget.value)}>{#each providers as item}<option value={item}>{item === "opencode" ? "OpenCode Go" : PROVIDERS[item].label}</option>{/each}</select></label>
              <fieldset>
                <legend>Connection</legend>
                <label class="method-choice"><input type="radio" name="connection-method" checked={method !== "local"} onchange={() => { method = addProvider === "opencode" ? "api_key" : "browser"; apiKey = ""; }} /><span><strong>Connect for Pulse</strong><small>Keep this connection separate from your coding apps.</small></span></label>
                <label class="method-choice"><input type="radio" name="connection-method" checked={method === "local"} disabled={Boolean(reconnectId)} onchange={() => { method = "local"; apiKey = ""; }} /><span><strong>Link a local profile</strong><small>Read an existing profile without changing its login.</small></span></label>
              </fieldset>
              <label>Connection name <span class="optional">Optional</span><input bind:value={connectionName} maxlength="100" autocomplete="off" placeholder="Personal, work, or another label" /></label>
              {#if method === "local"}
                <div class="profile-field"><label for="account-profile-path">Profile folder</label><div class="path-control"><input id="account-profile-path" bind:value={profilePath} required autocomplete="off" placeholder="Absolute path to the provider profile" />{#if hasTauriIpc()}<button type="button" class="icon-button" aria-label="Choose profile folder" onclick={browseProfile}><IconFolder size={18} /></button>{/if}</div></div>
              {:else}
                {#if addProvider === "commandcode"}
                  <label>Sign-in method<select bind:value={method}><option value="browser">Sign in with your browser</option><option value="api_key">Use an API key</option></select></label>
                {/if}
                {#if method === "api_key"}
                  <label>{addProvider === "opencode" ? "OpenCode Go API key" : "Command Code API key"}<input bind:value={apiKey} type="password" required autocomplete="off" spellcheck="false" placeholder="Enter your provider key" /></label>
                  <p class="form-note">Pulse stores this key in your protected local profile. It is not saved in analytics or browser storage.</p>
                {:else}
                  <p class="form-note">Continue with the provider's sign-in page. This creates a separate Pulse connection.</p>
                {/if}
              {/if}
              {#if formError}<p class="form-error" role="alert">{formError}</p>{/if}
              <div class="form-actions"><button type="button" class="secondary-button" disabled={submitting} onclick={() => { adding = false; apiKey = ""; }}>Cancel</button><button type="submit" class="add-button" disabled={submitting}>{submitting ? "Connecting…" : method === "local" ? "Link profile" : method === "browser" ? "Continue to sign in" : "Connect account"}</button></div>
            </form>
          </section>
        {:else if selected}
          <section class="account-detail" aria-label={"Account details for " + accountName(selected)}>
            <header class="detail-heading identity-heading">
              <img class="identity-mark" src={rpArtFor(selected.provider, selected.provider === "codex" ? "codex-app" : undefined).large} alt="" />
              <div class="identity-copy"><h2>{selected.identity.name ?? accountName(selected)}</h2>{#if selected.identity.email && selected.identity.name}<p>{selected.identity.email}</p>{/if}<span class="identity-meta">{selected.provider === "opencode" ? "OpenCode Go" : PROVIDERS[selected.provider].label}{#if selected.plan}<span class="meta-divider">/</span>{accountPlan(selected)}{/if}</span></div>
              <button class="secondary-button refresh-button" type="button" disabled={selected.refreshing || waiting(selected)} onclick={refreshSelected}><IconRefresh size={16} /><span>{selected.refreshing ? "Refreshing" : "Refresh"}</span></button>
            </header>
            <div class="connection-strip">
              <span class="connection-state" class:good={selected.status === "connected"}>{#if selected.status === "connected"}<IconCheck size={15} />{:else if waiting(selected)}<IconClock size={15} />{:else}<IconAlertCircle size={15} />{/if}{accountStatus(selected)}</span>
              {#if selected.identity.organization}<span class="organization"><IconBuilding size={14} />{selected.identity.organization}</span>{/if}
            </div>
            {#if selected.error}<div class="account-alert" role="status"><IconAlertCircle size={18} /><div><strong>{selected.status === "expired" ? "Sign in again to refresh usage" : "Usage is unavailable"}</strong><p>{selected.error}</p>{#if selected.retry_at}<small>Next automatic check: {timeLabel(selected.retry_at)}</small>{/if}<button class="secondary-button" type="button" onclick={() => { if (selected) reconnect(selected); }}>Connect again</button></div></div>{/if}
            {#if waiting(selected) || selected.status === "cancelled"}
              <div class="sign-in-panel"><h3>{selected.status === "cancelled" ? "Sign-in cancelled" : "Complete your sign-in"}</h3><p>{selected.status === "pending" ? "Checking the provider for account and usage details." : "Your coding apps keep their current accounts."}</p>
                {#if selected.auth_url}<a class="add-button" href={selected.auth_url} target="_blank" rel="noopener noreferrer">Continue sign-in<IconExternalLink size={16} /></a>{/if}
                {#if selected.status === "cancelled"}<button class="secondary-button" type="button" onclick={() => { if (selected) reconnect(selected); }}>Try sign-in again</button>{/if}
                {#if selected.status === "connecting" || selected.status === "awaiting_login"}<button class="secondary-button" type="button" onclick={() => cancelSignIn(selected)}>Cancel sign-in</button>{/if}
              </div>
            {/if}

            <section class="usage-section" aria-labelledby="allowances-title">
              <div class="section-heading"><h3 id="allowances-title">Allowances</h3><span>{selected.status === "stale" ? "Last known values" : "Provider-reported limits"}</span></div>
              {#if selected.windows.length}
                <div class="allowance-list">
                  {#each selected.windows as window (window.id)}
                    <div class="allowance-row">
                      <div class="allowance-label"><strong>{window.label}</strong><span>{windowDuration(window.duration_minutes)}{#if window.scope && window.scope !== "Account" && window.scope !== window.label}{#if windowDuration(window.duration_minutes)}<span class="meta-divider">·</span>{/if}{window.scope}{/if}</span></div>
                      <div class="allowance-value">{#if window.used_percent !== null}<strong>{number(window.used_percent)}<small>% used</small></strong>{:else}<strong>Not reported</strong>{/if}{#if window.used !== null && window.limit !== null}<span>{number(window.used)} / {number(window.limit)} {window.unit ?? ""}</span>{/if}</div>
                      {#if window.used_percent !== null}<progress max="100" value={Math.min(100, Math.max(0, window.used_percent))} aria-label={window.label + " used"} class:high={window.used_percent >= 90}></progress>{/if}
                      <span class="reset-time" title={timeLabel(window.resets_at)}><IconClock size={12} />{resetLabel(window.resets_at)}</span>
                    </div>
                  {/each}
                </div>
              {:else}<p class="section-empty">{waiting(selected) ? "Limits appear after the provider responds." : "No allowance windows are reported for this connection."}</p>{/if}
            </section>

            <section class="usage-section" aria-labelledby="credits-title">
              <div class="section-heading"><h3 id="credits-title">Credits & balances</h3><span>Kept in their original units</span></div>
              {#if selected.balances.length}
                <dl class="balance-list">{#each selected.balances as balance (balance.id)}<div><dt>{balance.label}{#if balance.cycle_ends_at}<small>Cycle ends {timeLabel(balance.cycle_ends_at)}</small>{/if}{#if balance.expires_at}<small>Expires {timeLabel(balance.expires_at)}</small>{/if}</dt><dd>{balance.unit === "USD" ? "$" : ""}{number(balance.value)}{#if balance.unit !== "USD" && balance.value !== "Unlimited"}<span>{balance.unit}</span>{/if}</dd></div>{/each}</dl>
              {:else}<p class="section-empty">No balance is reported by this provider.</p>{/if}
              {#if selected.notice}<p class="source-note">{selected.notice}</p>{/if}
            </section>

            {#if selected.reset_credits}
              <section class="usage-section" aria-labelledby="resets-title">
                <div class="section-heading"><h3 id="resets-title">Banked resets</h3><strong class="reset-count">{selected.reset_credits.availableCount} available</strong></div>
                {#if selected.reset_credits.credits?.length}
                  <ul class="reset-list">{#each selected.reset_credits.credits as credit (credit.id)}<li><span class="reset-symbol"><IconRotateClockwise size={21} /></span><div class="reset-copy"><strong>{credit.title ?? "Rate-limit reset"}</strong><span>{credit.status === "available" ? "Ready to use" : credit.status}</span></div><small>{credit.expiresAt ? "Expires " + timeLabel(new Date(credit.expiresAt * 1000).toISOString()) : "Expiry not reported"}</small></li>{/each}</ul>
                {:else}<p class="section-empty">{selected.reset_credits.availableCount > 0 ? "The provider reports the count but not individual reset details." : "No resets are currently available."}</p>{/if}
              </section>
            {/if}
            {#if selected.individual_limits.length}
              <section class="usage-section"><div class="section-heading"><h3>Individual spend limits</h3></div><dl class="balance-list">{#each selected.individual_limits as limit (limit.limitId)}<div><dt>{limit.limitId}<small>{resetLabel(limit.resetsAt)}</small></dt><dd>{number(limit.remainingPercent)}<span>% remaining</span></dd></div>{/each}</dl></section>
            {/if}

            <footer class="account-footer"><span>Last checked {timeLabel(selected.observed_at)}</span><button class="unlink-button" type="button" onclick={() => requestRemove(selected)}><IconUnlink size={15} />Remove from Pulse</button></footer>
          </section>
        {:else}
          <div class="empty-accounts"><h2>Your accounts belong here</h2><p>Connect Claude, Codex, OpenCode Go, or Command Code to see their native usage and balances.</p><button class="add-button" type="button" onclick={() => startAdd()}><IconPlus size={17} />Add your first account</button></div>
        {/if}
      </div>
    </div>
  {/if}
</section>

<dialog bind:this={removeDialog} class="remove-dialog" aria-labelledby="remove-title" onclose={() => { removeCandidate = null; }}>
  <h2 id="remove-title">Remove this connection?</h2><p>{removeCandidate ? accountName(removeCandidate) : ""}</p><p>Your coding apps will stay signed in. Your session history stays in Pulse.</p>
  <div class="form-actions"><button type="button" class="secondary-button" disabled={removing} onclick={() => removeDialog.close()}>Keep connection</button><button type="button" class="add-button" disabled={removing} onclick={confirmRemove}>{removing ? "Removing…" : "Remove from Pulse"}</button></div>
</dialog>

<style>
  .accounts-page { width:100%; max-width:1480px; margin-inline:auto; color:var(--text-primary); }
  .accounts-heading { display:flex; align-items:center; justify-content:space-between; gap:24px; padding:8px 0 26px; }
  h1,h2,h3,p { margin:0; } h1 { font-size:30px; font-weight:650; letter-spacing:-.025em; } h2 { font-size:21px; font-weight:620; letter-spacing:-.015em; } h3 { font-size:14px; font-weight:650; }
  .accounts-heading p,.detail-heading p { margin-top:7px; color:var(--text-secondary); font-size:13px; line-height:1.6; }
  button,a,input,select { font:inherit; } button,a { -webkit-app-region:no-drag; }
  button { cursor:pointer; } button:disabled { cursor:wait; opacity:.55; }
  .add-button,.secondary-button { display:inline-flex; align-items:center; justify-content:center; gap:8px; min-height:38px; padding:9px 14px; border-radius:var(--radius-sm); border:1px solid var(--border-strong); font-size:12px; font-weight:600; text-decoration:none; white-space:nowrap; }
  .add-button { background:var(--text-primary); color:var(--bg-primary); border-color:var(--text-primary); }
  .add-button:hover { opacity:.85; } .secondary-button { background:transparent; color:var(--text-primary); } .secondary-button:hover { background:var(--bg-elevated); }
  button:focus-visible,a:focus-visible,input:focus-visible,select:focus-visible { outline:2px solid var(--accent); outline-offset:3px; }
  .accounts-workspace { display:grid; grid-template-columns:minmax(210px,280px) minmax(0,1fr); border-top:1px solid var(--border); min-height:clamp(680px,calc(100dvh - 200px),1040px); }
  .account-index { padding:26px 22px 24px 0; border-right:1px solid var(--border); min-width:0; }
  .index-summary { display:flex; justify-content:space-between; gap:8px; color:var(--text-secondary); font-size:11px; padding-bottom:23px; font-variant-numeric:tabular-nums; }
  .provider-group + .provider-group { margin-top:29px; } .group-heading { display:flex; align-items:center; gap:9px; margin-bottom:9px; padding-left:8px; }
  .group-heading img { width:22px; height:22px; border-radius:5px; object-fit:contain; } .group-heading h2 { font-size:12px; letter-spacing:0; font-weight:650; flex:1; }
  .icon-button { display:inline-flex; align-items:center; justify-content:center; width:30px; height:30px; border:0; border-radius:var(--radius-sm); color:var(--text-secondary); background:transparent; flex:0 0 auto; }
  .icon-button:hover { background:var(--bg-elevated); color:var(--text-primary); }
  .account-row { display:flex; align-items:center; gap:12px; width:100%; padding:14px 12px; border:1px solid transparent; border-radius:var(--radius-sm); color:var(--text-primary); background:transparent; text-align:left; }
  .account-row.inactive { background:var(--bg-elevated); border-color:var(--border); border-style:dashed; color:var(--text-secondary); } .account-row:hover { background:var(--bg-card-hover); } .account-row.selected { border-color:var(--border-strong); background:var(--bg-elevated); }
  .account-row-copy { display:grid; gap:5px; min-width:0; flex:1; } .account-row strong { font-size:13px; font-weight:550; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; } .account-row small { font-size:11px; color:var(--text-secondary); }
  .status-dot { width:6px; height:6px; border-radius:50%; background:var(--text-muted); flex-shrink:0; } .status-dot.connected { background:var(--success); } .status-dot.attention { background:var(--warning); }
  .empty-provider { display:flex; align-items:center; justify-content:space-between; gap:12px; width:100%; border:0; padding:9px 10px; background:transparent; color:var(--text-secondary); font-size:12px; }
  .empty-provider:hover { color:var(--text-primary); } .index-note { display:flex; align-items:flex-start; gap:10px; margin:32px 0 0; padding:16px 10px; border-top:1px solid var(--border); font-size:11px; line-height:1.7; color:var(--text-secondary); } .index-note :global(svg) { flex-shrink:0; margin-top:2px; } .index-note strong { display:block; color:var(--text-primary); font-size:11px; font-weight:550; margin-bottom:4px; }
  .account-content { min-width:0; display:flex; flex-direction:column; padding:25px 0 16px 34px; } .account-detail { flex:1; display:flex; flex-direction:column; } .detail-heading { display:flex; align-items:flex-start; gap:16px; }
  .detail-heading > div { min-width:0; flex:1; } .identity-mark { width:45px; height:45px; object-fit:contain; border-radius:10px; flex-shrink:0; }
  .identity-copy h2,.identity-copy p { overflow-wrap:anywhere; } .identity-copy p { margin-top:4px; } .identity-meta { display:flex; flex-wrap:wrap; align-items:center; gap:8px; margin-top:8px; font-size:12px; color:var(--text-secondary); }
  .meta-divider { margin-inline:5px; color:var(--text-muted); } .refresh-button { margin-left:auto; flex-shrink:0; }
  .connection-strip { display:flex; align-items:center; flex-wrap:wrap; gap:12px 20px; margin:23px 0 28px; font-size:11px; color:var(--text-secondary); }
  .connection-state { display:inline-flex; align-items:center; gap:7px; padding:7px 10px; border-radius:var(--radius-sm); background:var(--bg-elevated); border:1px solid var(--border); font-weight:550; } .connection-state.good { color:var(--success); background:color-mix(in srgb,var(--success) 7%,var(--bg-primary)); border-color:color-mix(in srgb,var(--success) 24%,var(--border)); } .organization { display:flex; align-items:center; gap:7px; overflow-wrap:anywhere; }
  .usage-section { border-top:1px solid var(--border); padding:clamp(24px,3vh,40px) 0; } .section-heading { display:flex; justify-content:space-between; align-items:center; flex-wrap:wrap; gap:8px; margin-bottom:20px; }
  .section-heading > span { color:var(--text-secondary); font-size:11px; } .allowance-list { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); column-gap:30px; row-gap:25px; }
  .allowance-row { min-height:104px; align-content:space-between; display:grid; grid-template-columns:minmax(0,1fr) auto; gap:9px 12px; min-width:0; } .allowance-label { display:grid; align-content:start; gap:5px; min-width:0; } .allowance-label strong { font-size:14px; font-weight:550; overflow-wrap:anywhere; } .allowance-label > span { color:var(--text-secondary); font-size:10px; }
  .allowance-value { display:grid; gap:4px; text-align:right; font-variant-numeric:tabular-nums; } .allowance-value strong { font-size:24px; font-weight:550; white-space:nowrap; } .allowance-value small { font-size:10px; color:var(--text-secondary); font-weight:400; padding-left:4px; } .allowance-value > span { font-size:10px; color:var(--text-secondary); }
  progress { grid-column:1/-1; width:100%; height:6px; border:0; border-radius:5px; overflow:hidden; background:var(--bg-elevated); appearance:none; }
  progress::-webkit-progress-bar { background:var(--bg-elevated); border-radius:5px; } progress::-webkit-progress-value { background:var(--text-primary); border-radius:5px; } progress::-moz-progress-bar { background:var(--text-primary); border-radius:5px; }
  progress.high::-webkit-progress-value { background:var(--warning); } progress.high::-moz-progress-bar { background:var(--warning); }
  .allowance-row:only-child,.allowance-row:last-child:nth-child(odd) { grid-column:1/-1; } .reset-time { grid-column:1/-1; display:flex; align-items:center; gap:5px; color:var(--text-secondary); font-size:10px; }
  .balance-list { margin:0; display:grid; gap:22px; } .balance-list > div { display:flex; justify-content:space-between; align-items:start; gap:20px; }
  dt { font-size:12px; font-weight:500; } dt small { display:block; margin-top:5px; font-size:10px; color:var(--text-secondary); font-weight:400; } dd { margin:0; font-size:18px; font-weight:550; font-variant-numeric:tabular-nums; text-align:right; } dd span { margin-left:6px; font-size:11px; font-weight:400; color:var(--text-secondary); }
  .reset-count { font-size:12px; font-weight:550; padding:5px 9px; background:var(--bg-elevated); border-radius:var(--radius-sm); }
  .reset-list { list-style:none; padding:0; margin:0; display:grid; grid-template-columns:repeat(auto-fit,minmax(180px,1fr)); gap:14px; }
  .reset-list li { display:grid; grid-template-columns:auto 1fr; align-items:center; gap:15px 12px; padding:18px; background:var(--bg-elevated); border-radius:var(--radius-md); min-width:0; }
  .reset-symbol { width:38px; height:38px; display:grid; place-items:center; color:var(--text-primary); border:1px solid var(--border-strong); border-radius:var(--radius-sm); }
  .reset-copy { display:grid; gap:5px; } .reset-copy strong { font-size:13px; font-weight:600; } .reset-copy span { font-size:11px; color:var(--text-secondary); }
  .reset-list small { grid-column:1/-1; color:var(--text-secondary); font-size:11px; line-height:1.6; }

  .section-empty,.source-note { font-size:12px; line-height:1.7; color:var(--text-secondary); max-width:70ch; } .source-note { margin-top:14px; font-size:11px; }
  .account-footer { margin-top:auto; display:flex; justify-content:space-between; align-items:center; flex-wrap:wrap; gap:16px; border-top:1px solid var(--border); padding-top:20px; }
  .account-footer > span { font-size:11px; color:var(--text-secondary); } .unlink-button { display:flex; align-items:center; gap:6px; border:0; padding:5px 0; color:var(--text-secondary); background:none; font-size:11px; } .unlink-button:hover { color:var(--text-primary); }
  .connect-panel { max-width:640px; } .connect-form { margin-top:28px; display:grid; gap:22px; } .connect-form > label { display:grid; gap:9px; font-size:12px; font-weight:550; }
  .profile-field { display:grid; gap:9px; font-size:12px; font-weight:550; }
  input:not([type="radio"]),select { width:100%; min-width:0; min-height:41px; padding:10px 12px; border:1px solid var(--border-strong); border-radius:var(--radius-sm); color:var(--text-primary); background:var(--bg-input); font-size:12px; box-sizing:border-box; caret-color:var(--text-primary); }
  input::placeholder { color:var(--text-secondary); font-weight:400; } .optional { color:var(--text-secondary); font-size:10px; font-weight:400; }
  fieldset { border:0; padding:0; margin:0; display:grid; gap:9px; } legend { padding:0; margin-bottom:12px; font-size:12px; font-weight:550; }
  .method-choice { display:flex; align-items:center; gap:12px; border:1px solid var(--border); border-radius:var(--radius-sm); padding:14px; cursor:pointer; } .method-choice:has(input:checked) { background:var(--bg-elevated); border-color:var(--border-strong); } .method-choice input { accent-color:var(--text-primary); flex-shrink:0; } .method-choice span { display:grid; gap:5px; } .method-choice strong { font-size:12px; font-weight:550; } .method-choice small { color:var(--text-secondary); font-size:11px; line-height:1.5; }
  .form-note { color:var(--text-secondary); font-size:11px; line-height:1.7; } .form-actions { display:flex; justify-content:flex-end; gap:10px; margin-top:7px; } .path-control { display:flex; align-items:center; gap:6px; } .form-error { font-size:12px; line-height:1.6; color:var(--danger); }
  .account-alert,.page-error { display:flex; align-items:flex-start; gap:10px; padding:15px; margin-bottom:22px; border:1px solid var(--border-strong); border-radius:var(--radius-md); background:var(--bg-elevated); } .account-alert > :global(svg) { flex-shrink:0; color:var(--warning); } .account-alert strong { font-size:12px; } .account-alert p { margin-top:5px; font-size:12px; line-height:1.6; color:var(--text-secondary); } .account-alert small { display:block; margin-top:7px; color:var(--text-secondary); font-size:10px; }
  .page-error { font-size:12px; align-items:center; } .page-error span { flex:1; } .page-error button { color:var(--text-primary); background:none; border:0; text-decoration:underline; text-underline-offset:3px; }
  .sign-in-panel { padding:20px 0 26px; } .sign-in-panel p { color:var(--text-secondary); font-size:12px; line-height:1.6; margin:9px 0 18px; } .sign-in-panel > button { margin-left:8px; }
  .loading-state { min-height:280px; display:flex; justify-content:center; align-items:center; gap:10px; color:var(--text-secondary); font-size:13px; } .empty-accounts { display:grid; justify-items:start; gap:18px; padding:44px 10px; } .empty-accounts p { max-width:45ch; color:var(--text-secondary); font-size:13px; line-height:1.7; }
  .mobile-account-picker { display:none; } .remove-dialog { width:min(440px,calc(100vw - 48px)); border:1px solid var(--border-strong); border-radius:var(--radius-lg); padding:26px; color:var(--text-primary); background:var(--bg-primary); } .remove-dialog::backdrop { background:rgba(0,0,0,.65); } .remove-dialog p { margin-top:14px; font-size:13px; line-height:1.6; overflow-wrap:anywhere; color:var(--text-secondary); } .remove-dialog .form-actions { margin-top:24px; }
  @media(max-width:1000px) { .accounts-workspace { grid-template-columns:220px minmax(0,1fr); } .account-content { padding-left:24px; } .allowance-list { grid-template-columns:1fr; } .identity-heading { flex-wrap:wrap; } .refresh-button { min-height:32px; padding:7px 10px; } }
  @media(max-width:700px) { .accounts-heading { align-items:flex-start; gap:16px; padding-bottom:21px; } h1 { font-size:25px; } .accounts-heading p { font-size:12px; } .accounts-heading .add-button { font-size:11px; padding:8px 10px; } .accounts-workspace { display:block; min-height:0; } .account-index { display:none; } .account-content { padding:20px 0 10px; } .mobile-account-picker { display:grid; gap:8px; margin-bottom:25px; color:var(--text-secondary); font-size:11px; } h2 { font-size:19px; } .identity-mark { width:37px; height:37px; } .identity-heading { gap:11px; } .identity-heading .identity-copy { flex:1 0 calc(100% - 48px); } .refresh-button { margin-left:48px; } .connection-strip { gap:10px; margin-block:18px 23px; } .section-heading > span { font-size:10px; } .allowance-list { gap:24px; } .usage-section { padding-block:21px; } .form-actions { flex-wrap:wrap; } .remove-dialog { padding:21px; } }
</style>
