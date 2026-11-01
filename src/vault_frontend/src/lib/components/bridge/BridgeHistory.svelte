<script lang="ts">
  import type { Principal } from '@dfinity/principal';
  import { formatE8s } from '$lib/services/baseBridge/amounts';
  import type { BridgeConfig } from '$lib/services/baseBridge/chains';
  import { displayAddress } from '$lib/services/baseBridge/eip55';
  import { captureSessionGuard, getAnonymousBridge } from '$lib/services/baseBridge/bridgeActor';
  import {
    describeDeposit,
    describeRelease,
    formatTimestampNs,
    loadBridgeHistory,
    shortAddress,
    shortHash,
    type BridgeHistory,
  } from '$lib/services/baseBridge/status';
  import TxLink from './TxLink.svelte';

  interface Props {
    principal: Principal | null;
    config: BridgeConfig;
    /** Connected Base account. When set, releases sent from it are listed too (held claims, re-mints). */
    baseAccount: string | null;
    /** Bump to reload (after a transfer changes state). */
    version: number;
  }

  let { principal, config, baseAccount, version }: Props = $props();

  let history = $state.raw<BridgeHistory | null>(null);
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
    const sender = baseAccount;
    loading = true;
    error = '';
    try {
      const reader = await getAnonymousBridge();
      const result = await loadBridgeHistory(reader, { owner, baseSender: sender });
      if (requestId === requestSeq && guard()) history = result;
    } catch (cause) {
      if (requestId === requestSeq) error = `Could not load your bridge history: ${cause instanceof Error ? cause.message : String(cause)}`;
    } finally {
      if (requestId === requestSeq) loading = false;
    }
  }

  $effect(() => {
    version;
    baseAccount;
    if (principalText) {
      void load();
    } else {
      requestSeq += 1;
      history = null;
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

    <h3>Deposits to Base</h3>
    {#if history && history.deposits.length > 0}
      <ul>
        {#each history.deposits as deposit (deposit.id)}
          {@const display = describeDeposit(deposit, { principal: principalText, awaitingGas: config.minterAwaitingGas })}
          <li>
            <div class="line">
              <span>#{deposit.id} · {formatE8s(deposit.amount_e8s)} icUSD</span>
              <span class="status" class:good={display.tone === 'success'} class:warn={display.tone === 'warning'} class:bad={display.tone === 'error'}>{display.label}</span>
            </div>
            <div class="sub">
              <span>{formatTimestampNs(deposit.created_at_ns)}</span>
              <span>to <code title={displayAddress(deposit.base_recipient)}>{shortAddress(displayAddress(deposit.base_recipient))}</code></span>
              {#if deposit.delivered[0]}<TxLink hash={deposit.delivered[0].tx_hash} chain={config.chain} label="Delivered in" />{/if}
            </div>
          </li>
        {/each}
      </ul>
    {:else if history}
      <p class="empty">No deposits yet.</p>
    {:else if loading}
      <p class="empty">Loading...</p>
    {/if}

    <h3>Releases to the Internet Computer</h3>
    {#if baseAccount}
      <p class="note">Includes transfers sent from your connected Base account, such as held claims and re-mints.</p>
    {/if}
    {#if history && history.senderListingFailed}
      <p class="note">Transfers sent from your Base account could not be listed right now. Refresh to try again.</p>
    {/if}
    {#if history && history.releases.length > 0}
      <ul>
        {#each history.releases as release (release.transfer_id)}
          {@const display = describeRelease(release)}
          <li>
            <div class="line">
              <span>{formatE8s(release.amount_e8s)} icUSD burned on Base</span>
              <span class="status" class:good={display.tone === 'success'} class:warn={display.tone === 'warning'} class:bad={display.tone === 'error'}>{display.label}</span>
            </div>
            <div class="sub">
              <span>{formatTimestampNs(release.created_at_ns)}</span>
              <span>transfer <code title={release.transfer_id}>{shortHash(release.transfer_id)}</code></span>
              {#if release.destination[0]}<span>to <code title={release.destination[0].owner.toText()}>{shortHash(release.destination[0].owner.toText(), 8, 5)}</code></span>{/if}
              <TxLink hash={release.base_tx_hash} chain={config.chain} label="Burn" />
            </div>
          </li>
        {/each}
      </ul>
    {:else if history}
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
