<script lang="ts">
  import { onMount } from 'svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { CANISTER_IDS } from '$lib/config';

  interface AdminMintEvent {
    amount: number;
    to: string;
    reason: string;
    blockIndex: number;
    eventIndex: number;
  }

  let adminMints: AdminMintEvent[] = [];
  let loading = true;
  let error = '';
  let totalMinted = 0;
  let totalEvents = 0;

  onMount(async () => {
    try {
      let start = 0n;
      const pageSize = 2000n;
      let done = false;
      let eventIndex = 0;

      while (!done) {
        const events: any[] = await publicActor.get_events({ start, length: pageSize });
        for (const event of events) {
          if ('admin_mint' in event) {
            const e = event.admin_mint;
            adminMints.push({
              amount: Number(e.amount),
              to: typeof e.to === 'object' && e.to.toText ? e.to.toText() : String(e.to),
              reason: e.reason,
              blockIndex: Number(e.block_index),
              eventIndex,
            });
          }
          eventIndex++;
        }
        if (events.length < Number(pageSize)) {
          done = true;
        } else {
          start += pageSize;
        }
      }

      totalEvents = eventIndex;
      adminMints = adminMints;
      totalMinted = adminMints.reduce((sum, e) => sum + e.amount, 0);
    } catch (err) {
      console.error('Failed to load transparency data:', err);
      error = err instanceof Error ? err.message : 'Failed to load events';
    }
    loading = false;
  });

  function fmtIcusd(e8s: number): string {
    return (e8s / 1e8).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 4 });
  }

  function truncatePrincipal(p: string): string {
    if (p.length <= 20) return p;
    return p.slice(0, 10) + '...' + p.slice(-5);
  }

  function blockUrl(blockIndex: number): string {
    return `https://dashboard.internetcomputer.org/canister/${CANISTER_IDS.ICUSD_LEDGER}/block/${blockIndex}`;
  }
</script>

<svelte:head><title>Transparency - Rumi Protocol</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Transparency</h1>

  <section class="doc-section">
    <p>Every administrative action on Rumi Protocol is recorded as an on-chain event in the protocol's immutable event log. This page displays all admin icUSD mints with their stated reasons and linked ledger blocks for independent verification.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Guardrails</h2>
    <div class="guardrails">
      <div class="guardrail-item">
        <span class="guardrail-label">Per-mint cap</span>
        <span class="guardrail-value">1,500 icUSD</span>
      </div>
      <div class="guardrail-item">
        <span class="guardrail-label">Cooldown</span>
        <span class="guardrail-value">72 hours between mints</span>
      </div>
      <div class="guardrail-item">
        <span class="guardrail-label">Access</span>
        <span class="guardrail-value">Developer principal only</span>
      </div>
      <div class="guardrail-item">
        <span class="guardrail-label">Logging</span>
        <span class="guardrail-value">Every mint recorded on-chain with reason</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Admin Mints</h2>

    {#if loading}
      <p class="loading-text">Loading protocol events...</p>
    {:else if error}
      <p class="error-text">Error: {error}</p>
    {:else if adminMints.length === 0}
      <div class="empty-state">
        <p>No admin mints have been performed.</p>
        <p class="meta-text">Scanned {totalEvents.toLocaleString()} protocol events.</p>
      </div>
    {:else}
      <div class="summary">
        <div class="summary-item">
          <span class="summary-label">Total admin mints</span>
          <span class="summary-value">{adminMints.length}</span>
        </div>
        <div class="summary-item">
          <span class="summary-label">Total icUSD minted</span>
          <span class="summary-value">{fmtIcusd(totalMinted)} icUSD</span>
        </div>
        <div class="summary-item">
          <span class="summary-label">Events scanned</span>
          <span class="summary-value">{totalEvents.toLocaleString()}</span>
        </div>
      </div>

      <div class="mint-list">
        {#each adminMints as mint}
          <div class="mint-row">
            <div class="mint-field">
              <span class="field-label">Event #</span>
              <span class="field-value">{mint.eventIndex}</span>
            </div>
            <div class="mint-field">
              <span class="field-label">Amount</span>
              <span class="field-value amount">{fmtIcusd(mint.amount)} icUSD</span>
            </div>
            <div class="mint-field">
              <span class="field-label">Recipient</span>
              <span class="field-value principal" title={mint.to}>{truncatePrincipal(mint.to)}</span>
            </div>
            <div class="mint-field">
              <span class="field-label">Reason</span>
              <span class="field-value reason">{mint.reason}</span>
            </div>
            <div class="mint-field">
              <span class="field-label">Ledger Block</span>
              <a href={blockUrl(mint.blockIndex)} target="_blank" rel="noopener" class="field-value block-link">
                #{mint.blockIndex} ↗
              </a>
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Verification</h2>
    <p>Each admin mint creates an <code>icrc1_transfer</code> transaction on the icUSD ledger (<code>{CANISTER_IDS.ICUSD_LEDGER}</code>). Click any block link above to verify the transaction independently on the IC Dashboard. The protocol's event log is stored in stable memory and persists across canister upgrades.</p>
    <p>The protocol canister is the icUSD minter. When the minter calls <code>icrc1_transfer</code>, the ledger mints new tokens to the recipient. This is the same mechanism used for normal borrowing — admin mints are not a separate code path.</p>
  </section>
</article>

<style>
  .guardrails {
    display: grid; grid-template-columns: 1fr 1fr; gap: 0.75rem;
    margin-top: 0.75rem;
  }
  .guardrail-item {
    display: flex; flex-direction: column; gap: 0.25rem;
    padding: 0.75rem 1rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
  }
  .guardrail-label {
    font-size: 0.75rem; color: var(--rumi-text-muted);
    text-transform: uppercase; letter-spacing: 0.05em;
  }
  .guardrail-value {
    font-size: 0.875rem; color: var(--rumi-text-primary); font-weight: 500;
  }

  .summary {
    display: flex; gap: 1.5rem; margin-bottom: 1.5rem; flex-wrap: wrap;
  }
  .summary-item { display: flex; flex-direction: column; gap: 0.15rem; }
  .summary-label { font-size: 0.75rem; color: var(--rumi-text-muted); text-transform: uppercase; letter-spacing: 0.05em; }
  .summary-value { font-size: 1.1rem; color: var(--rumi-text-primary); font-weight: 600; }

  .mint-list { display: flex; flex-direction: column; gap: 0.75rem; }
  .mint-row {
    display: grid; grid-template-columns: 80px 140px 1fr 1fr 120px;
    gap: 0.75rem; align-items: start;
    padding: 1rem; background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border); border-radius: 0.5rem;
  }
  .mint-field { display: flex; flex-direction: column; gap: 0.15rem; }
  .field-label { font-size: 0.7rem; color: var(--rumi-text-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .field-value { font-size: 0.8125rem; color: var(--rumi-text-primary); word-break: break-all; }
  .field-value.amount { color: var(--rumi-action); font-weight: 600; }
  .field-value.principal { font-family: monospace; font-size: 0.75rem; }
  .field-value.reason { color: var(--rumi-text-secondary); }
  .block-link {
    color: var(--rumi-purple-accent); text-decoration: none;
    font-family: monospace; font-size: 0.8125rem;
  }
  .block-link:hover { text-decoration: underline; }

  .loading-text { color: var(--rumi-text-secondary); font-style: italic; }
  .error-text { color: var(--rumi-danger); }
  .empty-state {
    padding: 2rem; text-align: center;
    background: var(--rumi-bg-surface1); border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
  }
  .empty-state p { color: var(--rumi-text-secondary); margin: 0.25rem 0; }
  .meta-text { font-size: 0.8125rem; color: var(--rumi-text-muted); }

  @media (max-width: 768px) {
    .guardrails { grid-template-columns: 1fr; }
    .mint-row { grid-template-columns: 1fr; }
    .summary { flex-direction: column; gap: 0.75rem; }
  }
</style>
