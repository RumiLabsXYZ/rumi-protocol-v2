<script lang="ts">
  import type { Principal } from '@dfinity/principal';
  import type { DepositView, ReleaseView } from '$declarations/rumi_icusd_sui_bridge/rumi_icusd_sui_bridge.did';
  import { formatE8s } from '$lib/services/baseBridge/amounts';
  import { captureSessionGuard } from '$lib/services/baseBridge/bridgeActor';
  import { formatTimestampNs, shortAddress, shortHash } from '$lib/services/baseBridge/status';
  import type { SuiBridgeConfig } from '$lib/services/suiBridge/config';
  import { explorerTxUrl } from '$lib/services/suiBridge/networks';
  import { getAnonymousSuiBridge } from '$lib/services/suiBridge/suiActor';
  import { describeSuiDeposit, describeSuiRelease } from '$lib/services/suiBridge/status';

  interface Props {
    principal: Principal | null;
    config: SuiBridgeConfig;
    /** Connected Sui account. When set, releases sent from it are listed too (held claims, re-mints). */
    suiAddress: string | null;
    /** Bump to reload (after a transfer changes state). */
    version: number;
  }

  let { principal, config, suiAddress, version }: Props = $props();

  const PAGE = 20;
  let deposits = $state.raw<DepositView[] | null>(null);
  let releases = $state.raw<ReleaseView[] | null>(null);
  let senderListingFailed = $state(false);
  let loading = $state(false);
  let error = $state('');
  let requestSeq = 0;

  const principalText = $derived(principal && !principal.isAnonymous() ? principal.toText() : '');

  /** Every call here is a public query with an anonymous actor, so all wallet types see the same history. */
  async function load() {
    const owner = principal;
    if (!owner || owner.isAnonymous()) return;
    const requestId = ++requestSeq;
    const guard = captureSessionGuard();
    const sender = suiAddress;
    loading = true;
    error = '';
    try {
      const reader = await getAnonymousSuiBridge();
      const [ownDeposits, owned] = await Promise.all([
        reader.list_deposits_for_owner({ owner, start_after: [], limit: PAGE }),
        reader.list_releases_for_owner({ owner, start_after_seq: [], limit: PAGE }),
      ]);
      let bySender: ReleaseView[] = [];
      let failed = false;
      if (sender) {
        try {
          bySender = await reader.list_releases_for_sui_sender({ sui_sender: sender, start_after_seq: [], limit: PAGE });
        } catch {
          failed = true;
        }
      }
      if (requestId !== requestSeq || !guard()) return;
      const merged = new Map<string, ReleaseView>();
      for (const release of [...owned, ...bySender]) merged.set(release.return_id, release);
      deposits = ownDeposits;
      releases = [...merged.values()].sort((a, b) => (a.created_at_ns < b.created_at_ns ? 1 : -1));
      senderListingFailed = failed;
    } catch (cause) {
      if (requestId === requestSeq) error = `Could not load your bridge history: ${cause instanceof Error ? cause.message : String(cause)}`;
    } finally {
      if (requestId === requestSeq) loading = false;
    }
  }

  $effect(() => {
    version;
    suiAddress;
    if (principalText) {
      void load();
    } else {
      requestSeq += 1;
      deposits = null;
      releases = null;
    }
  });
</script>

<section class="history" aria-label="Bridge history">
  <div class="head">
    <h2>Your bridge history</h2>
    <button type="button" class="btn-secondary small" onclick={load} disabled={loading || !principalText}>{loading ? 'Refreshing...' : 'Refresh'}</button>
  </div>

  {#if !principalText}
    <p class="empty">Connect your ICP wallet to see your bridge history.</p>
  {:else}
    {#if error}<p class="error" role="alert">{error}</p>{/if}

    <h3>Deposits to Sui</h3>
    {#if deposits && deposits.length > 0}
      <ul>
        {#each deposits as deposit (deposit.id)}
          {@const display = describeSuiDeposit(deposit, { principal: principalText })}
          <li>
            <div class="line">
              <span>#{deposit.id} · {formatE8s(deposit.amount_e8s)} icUSD</span>
              <span class="status" class:good={display.tone === 'success'} class:warn={display.tone === 'warning'} class:bad={display.tone === 'error'}>{display.label}</span>
            </div>
            <div class="sub">
              <span>{formatTimestampNs(deposit.created_at_ns)}</span>
              <span>to <code title={deposit.sui_recipient}>{shortAddress(deposit.sui_recipient)}</code></span>
              {#if display.digest}<a href={explorerTxUrl(config.network, display.digest)} target="_blank" rel="noopener noreferrer" title={display.digest}>Delivered in {shortAddress(display.digest)}</a>{/if}
            </div>
          </li>
        {/each}
      </ul>
    {:else if deposits}
      <p class="empty">No deposits yet.</p>
    {:else if loading}
      <p class="empty">Loading...</p>
    {/if}

    <h3>Releases to the Internet Computer</h3>
    {#if suiAddress}
      <p class="note">Includes transfers sent from your connected Sui account, such as held claims and re-mints.</p>
    {/if}
    {#if senderListingFailed}
      <p class="note">Transfers sent from your Sui account could not be listed right now. Refresh to try again.</p>
    {/if}
    {#if releases && releases.length > 0}
      <ul>
        {#each releases as release (release.return_id)}
          {@const display = describeSuiRelease(release)}
          <li>
            <div class="line">
              <span>{formatE8s(release.amount_e8s)} icUSD returned from Sui</span>
              <span class="status" class:good={display.tone === 'success'} class:warn={display.tone === 'warning'} class:bad={display.tone === 'error'}>{display.label}</span>
            </div>
            <div class="sub">
              <span>{formatTimestampNs(release.created_at_ns)}</span>
              <span>return <code title={release.return_id}>{shortHash(release.return_id)}</code></span>
              <span>to <code title={release.destination.owner.toText()}>{shortHash(release.destination.owner.toText(), 8, 5)}</code></span>
              <a href={explorerTxUrl(config.network, release.tx_digest)} target="_blank" rel="noopener noreferrer" title={release.tx_digest}>Burn {shortAddress(release.tx_digest)}</a>
            </div>
          </li>
        {/each}
      </ul>
    {:else if releases}
      <p class="empty">No releases yet.</p>
    {:else if loading}
      <p class="empty">Loading...</p>
    {/if}
  {/if}
</section>

<style>
  .history { display:flex; flex-direction:column; gap:0.4rem; }
  .head { display:flex; justify-content:space-between; align-items:center; gap:0.5rem; }
  h2 { margin:0; font-size:1.1rem; color:var(--rumi-text-primary); }
  h3 { margin:0.9rem 0 0.2rem; font-size:0.85rem; color:var(--rumi-text-secondary); font-weight:600; }
  ul { margin:0; padding:0; list-style:none; display:flex; flex-direction:column; gap:0.5rem; }
  li { padding:0.65rem 0.8rem; border:1px solid var(--rumi-border); border-radius:0.5rem; background:var(--rumi-bg-primary); }
  .line { display:flex; flex-wrap:wrap; justify-content:space-between; gap:0.4rem; font-size:0.82rem; color:var(--rumi-text-primary); }
  .sub { display:flex; flex-wrap:wrap; gap:0.3rem 1rem; margin-top:0.25rem; font-size:0.72rem; color:var(--rumi-text-muted); }
  .sub a { color:var(--rumi-action-bright); }
  .status { font-size:0.75rem; color:var(--rumi-text-secondary); }
  .status.good { color:var(--rumi-safe); }
  .status.warn { color:#ffd9a8; }
  .status.bad { color:#ffb3c5; }
  .empty, .note { margin:0.2rem 0; font-size:0.78rem; color:var(--rumi-text-muted); }
  .error { margin:0.3rem 0; font-size:0.78rem; color:#ffb3c5; }
  .small { padding:0.35rem 0.75rem; font-size:0.75rem; }
  code { font-size:0.7rem; }
  button:disabled { opacity:0.45; cursor:not-allowed; }
</style>
