<script lang="ts">
  import { onMount } from 'svelte';
  import { protocolService } from '$lib/services/protocol';
  import { publicActor } from '$lib/services/protocol/apiClient';

  let liquidationBonus = 0;
  let recoveryTargetCr = 0;
  let borrowingFee = 0;
  let redemptionFeeFloor = 0;
  let redemptionFeeCeiling = 0;
  let maxPartialLiqRatio = 0;
  let ckstableRepayFee = 0;
  let loaded = false;

  function pct(ratio: number, decimals = 1): string {
    if (!ratio) return '—';
    return ((ratio - 1) * 100).toFixed(decimals) + '%';
  }
  function pctRaw(val: number, decimals = 1): string {
    if (val === undefined || val === null) return '—';
    return (val * 100).toFixed(decimals) + '%';
  }
  function crPct(val: number): string {
    if (!val) return '—';
    return (val * 100).toFixed(0) + '%';
  }

  onMount(async () => {
    try {
      const [status, bFee, rfFloor, rfCeil, maxPLR, ckFee] = await Promise.all([
        protocolService.getProtocolStatus(),
        publicActor.get_borrowing_fee() as Promise<number>,
        publicActor.get_redemption_fee_floor() as Promise<number>,
        publicActor.get_redemption_fee_ceiling() as Promise<number>,
        publicActor.get_max_partial_liquidation_ratio() as Promise<number>,
        publicActor.get_ckstable_repay_fee() as Promise<number>,
      ]);
      liquidationBonus = status.liquidationBonus;
      recoveryTargetCr = status.recoveryTargetCr;
      borrowingFee = Number(bFee);
      redemptionFeeFloor = Number(rfFloor);
      redemptionFeeCeiling = Number(rfCeil);
      maxPartialLiqRatio = Number(maxPLR);
      ckstableRepayFee = Number(ckFee);
    } catch (e) {
      console.error('Failed to fetch protocol parameters:', e);
    }
    loaded = true;
  });
</script>

<svelte:head><title>Protocol Parameters - Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Protocol Parameters</h1>
  <p class="doc-intro">Live values read directly from the Rumi Protocol canister. Parameters shown in <span class="live-indicator">purple</span> are configurable by the protocol admin and always reflect the current on-chain state.</p>

  {#if !loaded}
    <p class="doc-loading">Loading parameters from canister...</p>
  {:else}

  <section class="doc-section">
    <h2 class="doc-heading">Collateral & Liquidation</h2>
    <div class="params-table">
      <div class="param"><span class="param-label">Minimum Collateral Ratio</span><span class="param-val">133%</span></div>
      <div class="param"><span class="param-label">Recovery Mode Threshold</span><span class="param-val">150% (system-wide)</span></div>
      <div class="param"><span class="param-label">Recovery Target CR</span><span class="param-val live">{crPct(recoveryTargetCr)}</span></div>
      <div class="param"><span class="param-label">Liquidation Bonus</span><span class="param-val live">{pct(liquidationBonus)}</span></div>
      <div class="param"><span class="param-label">Max Partial Liquidation</span><span class="param-val live">{pctRaw(maxPartialLiqRatio, 0)} of debt</span></div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Fees</h2>
    <div class="params-table">
      <div class="param"><span class="param-label">Borrowing Fee</span><span class="param-val live">{pctRaw(borrowingFee)} (0% in Recovery)</span></div>
      <div class="param"><span class="param-label">Redemption Fee Floor</span><span class="param-val live">{pctRaw(redemptionFeeFloor)}</span></div>
      <div class="param"><span class="param-label">Redemption Fee Ceiling</span><span class="param-val live">{pctRaw(redemptionFeeCeiling)}</span></div>
      <div class="param"><span class="param-label">ckUSDT / ckUSDC Repay Fee</span><span class="param-val live">{pctRaw(ckstableRepayFee)}</span></div>
      <div class="param"><span class="param-label">ICP Ledger Fee</span><span class="param-val">0.0001 ICP</span></div>
      <div class="param"><span class="param-label">icUSD Ledger Fee</span><span class="param-val">0.001 icUSD</span></div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Minimums</h2>
    <div class="params-table">
      <div class="param"><span class="param-label">Minimum ICP Deposit</span><span class="param-val">0.001 ICP</span></div>
      <div class="param"><span class="param-label">Minimum Borrow Amount</span><span class="param-val">1 icUSD</span></div>
      <div class="param"><span class="param-label">Minimum Partial Repayment</span><span class="param-val">0.01 icUSD</span></div>
      <div class="param"><span class="param-label">Dust Threshold (auto-forgiven)</span><span class="param-val">0.000001 icUSD</span></div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Supported Tokens</h2>
    <div class="params-table">
      <div class="param"><span class="param-label">Collateral</span><span class="param-val">ICP</span></div>
      <div class="param"><span class="param-label">Stablecoin (minted)</span><span class="param-val">icUSD</span></div>
      <div class="param"><span class="param-label">Repayment & Liquidation</span><span class="param-val">icUSD, ckUSDT, ckUSDC</span></div>
      <div class="param"><span class="param-label">ckStable Depeg Rejection</span><span class="param-val">Outside $0.95 – $1.05</span></div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Oracle & Timing</h2>
    <div class="params-table">
      <div class="param"><span class="param-label">Price Source</span><span class="param-val">IC Exchange Rate Canister (XRC)</span></div>
      <div class="param"><span class="param-label">Price Fetch Interval</span><span class="param-val">300 seconds (+ on-demand for operations)</span></div>
      <div class="param"><span class="param-label">Read-Only Price Floor</span><span class="param-val">$0.01</span></div>
      <div class="param"><span class="param-label">Stuck Transfer Timeout</span><span class="param-val">15 minutes</span></div>
      <div class="param"><span class="param-label">Health Monitor Interval</span><span class="param-val">5 minutes</span></div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Protocol Modes</h2>
    <div class="params-table">
      <div class="param"><span class="param-label">General Availability</span><span class="param-val">Total CR &ge; 150%</span></div>
      <div class="param"><span class="param-label">Recovery</span><span class="param-val">Total CR &lt; 150%</span></div>
      <div class="param"><span class="param-label">Read-Only</span><span class="param-val">Total CR &lt; 100% or oracle failure</span></div>
    </div>
  </section>

  {/if}
</article>

<style>
  .params-table { display: flex; flex-direction: column; gap: 0.5rem; }
  .param {
    display: flex; justify-content: space-between; align-items: baseline;
    padding: 0.5rem 0; border-bottom: 1px solid var(--rumi-border);
  }
  .param:last-child { border-bottom: none; }
  .param-label { font-size: 0.8125rem; color: var(--rumi-text-secondary); }
  .param-val {
    font-family: 'Inter', sans-serif; font-size: 0.875rem;
    font-weight: 600; color: var(--rumi-text-primary);
    font-variant-numeric: tabular-nums;
  }
  .param-val.live { color: var(--rumi-action); }
  .live-indicator { color: var(--rumi-action); font-weight: 600; }
  .doc-loading {
    font-size: 0.875rem; color: var(--rumi-text-muted);
    text-align: center; padding: 2rem 0;
  }
</style>
