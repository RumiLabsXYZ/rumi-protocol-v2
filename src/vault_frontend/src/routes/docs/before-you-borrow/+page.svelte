<script lang="ts">
  import { onMount } from 'svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { collateralStore } from '$lib/stores/collateralStore';
  import type { CollateralInfo } from '$lib/services/types';
  import { get } from 'svelte/store';

  let borrowingFeePct = '0.5';
  let ckstableFeePct = '0.05';
  let collaterals: CollateralInfo[] = [];

  $: collateralSymbols = collaterals.map(c => c.symbol).join(', ') || 'ICP';
  // Use the lowest liquidation CR across all collaterals for the general warning
  $: lowestLiqPct = collaterals.length > 0
    ? Math.min(...collaterals.map(c => c.liquidationCr * 100)).toFixed(0)
    : '133';

  onMount(async () => {
    try {
      const [bFee, ckFee] = await Promise.all([
        publicActor.get_borrowing_fee() as Promise<number>,
        publicActor.get_ckstable_repay_fee() as Promise<number>,
      ]);
      borrowingFeePct = (Number(bFee) * 100).toFixed(1);
      ckstableFeePct = (Number(ckFee) * 100).toFixed(2);

      await collateralStore.fetchSupportedCollateral();
      const state = get(collateralStore);
      collaterals = state.collaterals;
    } catch (e) {
      console.error('Failed to fetch fees:', e);
    }
  });
</script>

<svelte:head><title>Before You Borrow - Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Before You Borrow</h1>

  <section class="doc-section">
    <h2 class="doc-heading">How Borrowing Works</h2>
    <p>You deposit collateral into a vault, then borrow icUSD against it. The icUSD is minted at the time of borrowing — it doesn't come from a pool. Your collateral stays locked in the vault until you repay the debt and withdraw it.</p>
    <p>Each vault is independent. You can have multiple vaults, each with its own collateral and debt. The protocol currently supports <strong>{collateralSymbols}</strong> as collateral{collaterals.length > 1 ? ' — each with its own parameters' : ''}.</p>
  </section>

  {#if collaterals.length > 0}
  <section class="doc-section">
    <h2 class="doc-heading">Per-Collateral Requirements</h2>
    {#each collaterals as c (c.principal)}
      <div class="collateral-card">
        <h3 class="collateral-label">{c.symbol}</h3>
        <div class="params-table">
          <div class="param">
            <span class="param-label">Liquidation Ratio</span>
            <span class="param-val live">{(c.liquidationCr * 100).toFixed(0)}%</span>
          </div>
          <div class="param">
            <span class="param-label">Borrowing Threshold (Min CR)</span>
            <span class="param-val live">{(c.minimumCr * 100).toFixed(0)}%</span>
          </div>
          <div class="param">
            <span class="param-label">Minimum Borrow</span>
            <span class="param-val live">{c.minVaultDebt > 0 ? `${c.minVaultDebt / 1e8} icUSD` : '—'}</span>
          </div>
        </div>
      </div>
    {/each}
  </section>
  {/if}

  <section class="doc-section">
    <h2 class="doc-heading">Fees</h2>
    <p>A one-time borrowing fee of {borrowingFeePct}% is deducted from the icUSD you borrow. If you borrow 100 icUSD, you receive {(100 - parseFloat(borrowingFeePct)).toFixed(1)} icUSD and owe 100 icUSD. There is no ongoing interest. The fee drops to 0% if the protocol enters Recovery mode.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Repaying Your Debt</h2>
    <p>You can repay your icUSD debt at any time — in full or partially. You can also repay using <strong>ckUSDT</strong> or <strong>ckUSDC</strong> instead of icUSD. Stablecoin repayments are treated at a 1:1 rate with icUSD, minus a {ckstableFeePct}% conversion fee. The protocol checks the stablecoin's live price and rejects repayment if it has depegged outside the $0.95–$1.05 range.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Managing Collateral</h2>
    <p>You can add more collateral to your vault at any time to improve your collateral ratio. You can also <strong>withdraw collateral partially</strong> — taking some out while keeping the vault open, as long as your CR stays above the borrowing threshold.</p>
    <p>The maximum you can withdraw is: <code>current collateral - (debt &times; min ratio &divide; collateral price)</code>. The protocol calculates this for you and rejects withdrawals that would put your vault at risk.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Closing Your Vault</h2>
    <p>To close a vault, you must first repay all outstanding icUSD debt, then withdraw your collateral. The protocol also offers a <strong>withdraw-and-close</strong> operation that does both steps atomically in a single call. Dust amounts below 0.000001 icUSD are forgiven automatically on close.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">What You Should Understand</h2>
    <p>Collateral assets are volatile. A sharp price drop can push your vault below the liquidation threshold faster than you can react. There is no grace period and no notification system — liquidation is immediate and automated.</p>
    <p>Higher collateral ratios give you more buffer. A vault at 200% can absorb a much larger price drop than one at 140%.</p>
    <p>Your vault's collateral can also be affected by redemptions. When icUSD holders redeem and protocol reserves are insufficient, collateral is taken from the lowest-CR vaults. See <a href="/docs/redemptions" class="doc-link">Redemptions</a> for details.</p>
    <p>The protocol allows only one operation per user at a time. If you submit a second transaction before the first completes, it will be rejected. Wait for confirmations before taking another action.</p>
    <p>This protocol is in beta. See the <a href="/docs/beta" class="doc-link">beta disclaimer</a> and <a href="/docs/risks" class="doc-link">risk documentation</a> for full details.</p>
  </section>
</article>

<style>
  .collateral-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1rem 1.25rem;
    margin-bottom: 0.75rem;
  }
  .collateral-label {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.9375rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    margin-bottom: 0.5rem;
  }
  .params-table { display: flex; flex-direction: column; gap: 0.25rem; }
  .param {
    display: flex; justify-content: space-between; align-items: baseline;
    padding: 0.25rem 0; border-bottom: 1px solid var(--rumi-border);
  }
  .param:last-child { border-bottom: none; }
  .param-label { font-size: 0.8125rem; color: var(--rumi-text-secondary); }
  .param-val {
    font-family: 'Inter', sans-serif; font-size: 0.875rem;
    font-weight: 600; color: var(--rumi-text-primary);
    font-variant-numeric: tabular-nums;
  }
  .param-val.live { color: var(--rumi-action); }
</style>
