<script lang="ts">
  import { onMount } from 'svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { CANISTER_IDS } from '$lib/config';
  import type { NicpRedemptionClaim } from '../../../../declarations/rumi_protocol_backend/rumi_protocol_backend.did';

  let redemptionClaim: NicpRedemptionClaim | null = null;
  let claimStatus = 'Checking on-chain migration status';
  let claimReadError = '';
  const claimStages: Record<string, string> = {
    Prepared: 'Prepared — withdrawal not submitted',
    FundingSubmitted: 'Custody transfer submitted — awaiting receipt',
    Funded: 'nICP in dedicated protocol custody — withdrawal not submitted',
    ApprovalSubmitted: 'Redemption allowance submitted — awaiting receipt',
    Approved: 'Allowance confirmed — withdrawal not submitted',
    WithdrawalSubmitted: 'Withdrawal submitted — verifying outcome',
    ClaimConfirmed: 'Redemption claim confirmed — awaiting ICP',
    Settled: 'ICP received in dedicated protocol custody',
  };
  onMount(async () => {
    try {
      const result = await publicActor.get_nicp_redemption_claim();
      redemptionClaim = result[0] ?? null;
      claimStatus = redemptionClaim
        ? (claimStages[Object.keys(redemptionClaim.stage)[0]] ?? 'Unknown migration status')
        : 'Planned — no migration recorded';
    } catch {
      claimStatus = 'Live migration status unavailable';
      claimReadError = 'The notice below describes the plan. A failed status lookup does not establish whether a transaction occurred.';
    }
  });

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
        const events: any[] = await publicActor.get_events({ start, length: pageSize, types: [], principal: [], collateral_token: [], time_range: [], min_size_e8s: [], admin_labels: [] });
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

<svelte:head><title>Transparency | Rumi Protocol</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Transparency</h1>

  <section class="doc-section" aria-labelledby="vault51-migration" id="vault-51-redemption">
    <h2 class="doc-heading" id="vault51-migration">Vault #51: nICP redemption migration</h2>
    <p><strong>November 1, 2026 · {claimStatus}</strong></p>
    {#if claimReadError}<p>{claimReadError}</p>{/if}
    <p>Rumi's founder has authorized migration of vault #51 from nICP collateral to a WaterNeuron redemption claim controlled by the protocol, followed by ICP held in the same dedicated protocol account when redemption settles. The verified status above distinguishes preparation, a submitted request, a confirmed claim, and receipt of ICP.</p>
    <p>The connected vault view on November 1 showed <strong>1,500 nICP</strong> and approximately <strong>2,871.08 icUSD of debt</strong>. Debt changes with repayments and interest. WaterNeuron's observed exchange rate implies approximately <strong>1,942.86 ICP</strong> before applicable deductions; the actual claim amount must come from the confirmed withdrawal.</p>
    <p>The migration preserves the vault's outstanding debt and accrued interest, with <strong>the existing nICP collateral parameters and dynamic interest-rate rules retained</strong>, including after ICP arrives. The fixed ICP amount is valued at the ICP price with the nICP haircut; it does not accrue further nICP staking rewards. This migration does not mint icUSD or forgive debt. Once the ICP has arrived and the debt is fully repaid, the vault owner can withdraw the settled ICP to the owner account.</p>
    <p><strong>Timing and risk:</strong> WaterNeuron's interface advertises a two-week delayed withdrawal with a transition warning. The short-term backing neuron queried on November 1 reported approximately <strong>151 days of dissolve delay</strong>. Two-week settlement is not confirmed. During a pending redemption, the claim would not be immediately available for ordinary liquidation or redemption by icUSD holders. ICP price declines, continuing interest, or a delayed or failed payout could create a shortfall. Protocol custody alone does not eliminate that risk.</p>
    <p><strong>Exception for vault #51:</strong> ordinary borrowing, collateral withdrawal, liquidation, and icUSD redemption against this vault are suspended while the claim or received ICP remains in dedicated custody. Repayment remains available and interest continues. The founder has personally committed to covering any shortfall and keeping the protocol whole. That commitment is not additional on-chain collateral or an independently funded guarantee.</p>
    {#if redemptionClaim}
      <p>Original collateral: <strong>{fmtIcusd(Number(redemptionClaim.original_nicp_e8s))} nICP</strong>. Recorded ICP amount: <strong>{fmtIcusd(Number(redemptionClaim.received_icp_e8s[0] ?? redemptionClaim.icp_due_e8s))} ICP</strong>{#if !('ClaimConfirmed' in redemptionClaim.stage) && !('Settled' in redemptionClaim.stage)} (provisional until the withdrawal is verified){/if}.</p>
      <p class="meta-text">Custodian: <code>{CANISTER_IDS.PROTOCOL}</code> · Dedicated subaccount: <code>{Array.from(redemptionClaim.escrow_subaccount).map(b => b.toString(16).padStart(2, '0')).join('')}</code></p>
      {#if redemptionClaim.withdrawal_id.length}<p>WaterNeuron withdrawal #{redemptionClaim.withdrawal_id[0]?.toString()}.</p>{/if}
      {#if redemptionClaim.burn_block.length}<p>nICP burn block #{redemptionClaim.burn_block[0]?.toString()} on ledger <a href="https://dashboard.internetcomputer.org/canister/buwm7-7yaaa-aaaar-qagva-cai" target="_blank" rel="noopener noreferrer">buwm7-7yaaa-aaaar-qagva-cai</a>.</p>{/if}
      {#if redemptionClaim.settlement_block.length}<p>ICP settlement block <a href={`https://dashboard.internetcomputer.org/transaction/${redemptionClaim.settlement_block[0]}`} target="_blank" rel="noopener noreferrer">#{redemptionClaim.settlement_block[0]?.toString()}</a>.</p>{/if}
      {#if redemptionClaim.release?.[0]}
        {#if redemptionClaim.release[0].block.length}<p>Owner withdrawal confirmed in ICP ledger block #{redemptionClaim.release[0].block[0]?.toString()}.</p>{:else}<p>Owner withdrawal initiated; its outcome is unconfirmed. The claim is no longer counted as collateral.</p>{/if}
      {/if}
      {#if redemptionClaim.last_error.length}<p>Operator reconciliation required: {redemptionClaim.last_error[0]}</p>{/if}
    {/if}
    <p class="meta-text">References: <a href="https://forum.dfinity.org/t/motion-wind-down-waterneuron/76274" target="_blank" rel="noopener noreferrer">WaterNeuron wind-down proposal</a> · <a href="https://waterneuron.fi/" target="_blank" rel="noopener noreferrer">WaterNeuron redemption interface</a> · <a href="https://dashboard.internetcomputer.org/neuron/13680855657433416220" target="_blank" rel="noopener noreferrer">Backing neuron</a></p>
  </section>

  <section class="doc-section">
    <p>The section below displays admin icUSD mint events with their stated reasons and linked ledger blocks for independent verification.</p>
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
    <p>The protocol canister is the icUSD minter. Admin minting and ordinary borrowing use the ledger's mint mechanism, but have different authorization and accounting paths. The vault #51 migration described above does not use either minting path.</p>
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
