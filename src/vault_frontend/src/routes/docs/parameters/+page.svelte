<script lang="ts">
  import { onMount } from 'svelte';
  import { protocolService } from '$lib/services/protocol';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { collateralStore } from '$lib/stores/collateralStore';
  import { get } from 'svelte/store';

  let liquidationBonus = 0;
  let recoveryTargetCr = 0;
  let recoveryModeThreshold = 0;
  let borrowingFee = 0;
  let redemptionFeeFloor = 0;
  let redemptionFeeCeiling = 0;
  let ckstableRepayFee = 0;

  // Per-collateral values (ICP defaults)
  let liquidationRatio = 0;
  let borrowThreshold = 0;
  let interestRateApr = 0;
  let minVaultDebt = 0;
  let debtCeiling = 0;
  let ledgerFee = 0;

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
      // Fetch global parameters and per-collateral config in parallel
      const [status, bFee, rfFloor, rfCeil, ckFee] = await Promise.all([
        protocolService.getProtocolStatus(),
        publicActor.get_borrowing_fee() as Promise<number>,
        publicActor.get_redemption_fee_floor() as Promise<number>,
        publicActor.get_redemption_fee_ceiling() as Promise<number>,
        publicActor.get_ckstable_repay_fee() as Promise<number>,
      ]);

      liquidationBonus = status.liquidationBonus;
      recoveryTargetCr = status.recoveryTargetCr;
      recoveryModeThreshold = status.recoveryModeThreshold;
      borrowingFee = Number(bFee);
      redemptionFeeFloor = Number(rfFloor);
      redemptionFeeCeiling = Number(rfCeil);
      ckstableRepayFee = Number(ckFee);

      // Load per-collateral config (ICP values)
      await collateralStore.fetchSupportedCollateral();
      const state = get(collateralStore);
      const icpConfig = state.collaterals.find(c => c.symbol === 'ICP');
      if (icpConfig) {
        liquidationRatio = icpConfig.liquidationCr;
        borrowThreshold = icpConfig.minimumCr;
        interestRateApr = icpConfig.interestRateApr;
        minVaultDebt = icpConfig.minVaultDebt / 1e8; // e8s → icUSD
        debtCeiling = icpConfig.debtCeiling;
        ledgerFee = icpConfig.ledgerFee / 1e8; // e8s → ICP
        // Override global with per-collateral if available
        if (icpConfig.borrowingFee !== undefined) borrowingFee = icpConfig.borrowingFee;
      }
    } catch (e) {
      console.error('Failed to fetch protocol parameters:', e);
    }
    loaded = true;
  });
</script>

<svelte:head><title>Protocol Parameters - Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Protocol Parameters</h1>
  <p class="doc-intro">Live values read directly from the Rumi Protocol canister. Parameters shown in <span class="live-indicator">teal</span> are configurable by the protocol admin and always reflect the current on-chain state.</p>

  {#if !loaded}
    <p class="doc-loading">Loading parameters from canister...</p>
  {:else}

  <section class="doc-section">
    <h2 class="doc-heading">Collateral & Liquidation (ICP)</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Borrowing Threshold (Min CR) <span class="tip" title="The minimum collateral ratio required to open a vault or borrow more icUSD. Your vault must be above this ratio to mint new debt.">?</span></span>
        <span class="param-val live">{crPct(borrowThreshold)}</span>
      </div>
      <div class="param">
        <span class="param-label">Liquidation Ratio <span class="tip" title="If your vault's collateral ratio drops below this level, it becomes eligible for liquidation. Liquidators can repay your debt and claim your collateral at a bonus.">?</span></span>
        <span class="param-val live">{crPct(liquidationRatio)}</span>
      </div>
      <div class="param">
        <span class="param-label">Recovery Mode Threshold <span class="tip" title="The system-wide collateral ratio that triggers Recovery Mode. Calculated as a debt-weighted average of all collateral types' borrowing thresholds. When the total system CR falls below this, the protocol enters Recovery Mode and the liquidation threshold rises.">?</span></span>
        <span class="param-val live">{crPct(recoveryModeThreshold)} (system-wide, debt-weighted)</span>
      </div>
      <div class="param">
        <span class="param-label">Recovery Target CR <span class="tip" title="During Recovery Mode, partially liquidated vaults are restored to this collateral ratio. Only enough debt is repaid to bring the vault back to this level.">?</span></span>
        <span class="param-val live">{crPct(recoveryTargetCr)}</span>
      </div>
      <div class="param">
        <span class="param-label">Liquidation Bonus <span class="tip" title="The extra collateral a liquidator receives as an incentive. For example, 15% means liquidators get collateral worth 115% of the debt they repay.">?</span></span>
        <span class="param-val live">{pct(liquidationBonus)}</span>
      </div>
      <div class="param">
        <span class="param-label">Partial Liquidation <span class="tip" title="In Recovery Mode, vaults between the Liquidation Ratio and Borrowing Threshold are not fully liquidated. Instead, only enough debt is repaid to restore the vault to the Recovery Target CR.">?</span></span>
        <span class="param-val">Restores vault CR to Recovery Target</span>
      </div>
      {#if debtCeiling > 0 && debtCeiling < Number.MAX_SAFE_INTEGER}
        <div class="param">
          <span class="param-label">Debt Ceiling <span class="tip" title="The maximum total icUSD that can be borrowed against this collateral type across all vaults.">?</span></span>
          <span class="param-val live">{(debtCeiling / 1e8).toLocaleString()} icUSD</span>
        </div>
      {:else}
        <div class="param">
          <span class="param-label">Debt Ceiling <span class="tip" title="The maximum total icUSD that can be borrowed against this collateral type. Currently there is no cap.">?</span></span>
          <span class="param-val live">Unlimited</span>
        </div>
      {/if}
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Fees</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Borrowing Fee <span class="tip" title="A one-time fee deducted from the icUSD you mint. For example, if 0.5%, borrowing 100 icUSD means you receive 99.5 icUSD but owe 100. Drops to 0% during Recovery Mode.">?</span></span>
        <span class="param-val live">{pctRaw(borrowingFee)} (0% in Recovery)</span>
      </div>
      <div class="param">
        <span class="param-label">Interest Rate (APR) <span class="tip" title="Annual interest charged on outstanding vault debt. Currently 0% — Rumi charges no ongoing interest.">?</span></span>
        <span class="param-val live">{pctRaw(interestRateApr)}</span>
      </div>
      <div class="param">
        <span class="param-label">Redemption Fee Floor <span class="tip" title="The minimum fee charged when redeeming icUSD for collateral. The actual fee starts here and increases with redemption volume, then decays back over time.">?</span></span>
        <span class="param-val live">{pctRaw(redemptionFeeFloor)}</span>
      </div>
      <div class="param">
        <span class="param-label">Redemption Fee Ceiling <span class="tip" title="The maximum the redemption fee can reach, no matter how much redemption activity occurs.">?</span></span>
        <span class="param-val live">{pctRaw(redemptionFeeCeiling)}</span>
      </div>
      <div class="param">
        <span class="param-label">ckUSDT / ckUSDC Repay Fee <span class="tip" title="A small fee applied when repaying vault debt or liquidating with ckUSDT or ckUSDC instead of icUSD. Compensates for potential stablecoin price variance.">?</span></span>
        <span class="param-val live">{pctRaw(ckstableRepayFee, 2)}</span>
      </div>
      <div class="param">
        <span class="param-label">ICP Ledger Fee <span class="tip" title="The network fee charged by the ICP ledger for each transfer. This is an Internet Computer system fee, not a Rumi fee.">?</span></span>
        <span class="param-val">{ledgerFee > 0 ? `${ledgerFee} ICP` : '0.0001 ICP'}</span>
      </div>
      <div class="param">
        <span class="param-label">icUSD Ledger Fee <span class="tip" title="The network fee charged by the icUSD ledger for each transfer.">?</span></span>
        <span class="param-val">0.001 icUSD</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Minimums</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Minimum ICP Deposit <span class="tip" title="The smallest amount of ICP you can deposit when opening a vault.">?</span></span>
        <span class="param-val">0.001 ICP</span>
      </div>
      <div class="param">
        <span class="param-label">Minimum Borrow Amount <span class="tip" title="The smallest amount of icUSD you can borrow. Vaults with debt below this amount cannot be created.">?</span></span>
        <span class="param-val live">{minVaultDebt > 0 ? `${minVaultDebt} icUSD` : '—'}</span>
      </div>
      <div class="param">
        <span class="param-label">Minimum Partial Repayment <span class="tip" title="The smallest repayment amount accepted by the protocol.">?</span></span>
        <span class="param-val">0.01 icUSD</span>
      </div>
      <div class="param">
        <span class="param-label">Dust Threshold (auto-forgiven) <span class="tip" title="Debt amounts smaller than this are automatically forgiven when closing a vault, to avoid rounding issues.">?</span></span>
        <span class="param-val">0.000001 icUSD</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Supported Tokens</h2>
    <div class="params-table">
      <div class="param"><span class="param-label">Collateral</span><span class="param-val">ICP</span></div>
      <div class="param"><span class="param-label">Stablecoin (minted)</span><span class="param-val">icUSD</span></div>
      <div class="param"><span class="param-label">Repayment & Liquidation</span><span class="param-val">icUSD, ckUSDT, ckUSDC</span></div>
      <div class="param">
        <span class="param-label">ckStable Depeg Rejection <span class="tip" title="If ckUSDT or ckUSDC is trading outside this range, the protocol rejects it for repayment or liquidation to protect against depeg events.">?</span></span>
        <span class="param-val">Outside $0.95 – $1.05</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Oracle & Timing</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Price Source <span class="tip" title="The Internet Computer's built-in Exchange Rate Canister, which aggregates prices from multiple exchanges.">?</span></span>
        <span class="param-val">IC Exchange Rate Canister (XRC)</span>
      </div>
      <div class="param">
        <span class="param-label">Price Fetch Interval <span class="tip" title="Prices are refreshed automatically every 300 seconds. Operations like borrowing and liquidation also trigger an on-demand refresh if the cached price is older than 30 seconds.">?</span></span>
        <span class="param-val">300 seconds (+ on-demand for operations)</span>
      </div>
      <div class="param">
        <span class="param-label">Read-Only Price Floor <span class="tip" title="If the oracle reports a price below this level, the protocol enters Read-Only mode and halts all operations as a safety measure.">?</span></span>
        <span class="param-val">$0.01</span>
      </div>
      <div class="param">
        <span class="param-label">Stuck Transfer Timeout <span class="tip" title="If a collateral or icUSD transfer fails and isn't successfully retried within this time, it may require manual intervention.">?</span></span>
        <span class="param-val">15 minutes</span>
      </div>
      <div class="param">
        <span class="param-label">Health Monitor Interval <span class="tip" title="A background process that checks for stuck transfers, under-collateralized vaults, and other health issues.">?</span></span>
        <span class="param-val">5 minutes</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Protocol Modes</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">General Availability <span class="tip" title="Normal operating mode. All operations are available. Liquidation only affects vaults below the Liquidation Ratio.">?</span></span>
        <span class="param-val">Total CR &ge; <span class="live">{crPct(recoveryModeThreshold)}</span></span>
      </div>
      <div class="param">
        <span class="param-label">Recovery <span class="tip" title="Triggered when the system is under-collateralized. The liquidation threshold rises to the Borrowing Threshold, borrowing fee drops to 0%, and vaults in the warning zone receive partial liquidation.">?</span></span>
        <span class="param-val">Total CR &lt; <span class="live">{crPct(recoveryModeThreshold)}</span></span>
      </div>
      <div class="param">
        <span class="param-label">Read-Only <span class="tip" title="Emergency mode where all state-changing operations are paused. Triggered by extreme under-collateralization or oracle failure.">?</span></span>
        <span class="param-val">Total CR &lt; 100% or oracle failure</span>
      </div>
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
  .param-val.live, .param-val .live { color: var(--rumi-action); }
  .live-indicator { color: var(--rumi-action); font-weight: 600; }
  .doc-loading {
    font-size: 0.875rem; color: var(--rumi-text-muted);
    text-align: center; padding: 2rem 0;
  }
  .tip {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1rem;
    height: 1rem;
    font-size: 0.625rem;
    font-weight: 700;
    color: var(--rumi-text-muted);
    border: 1px solid var(--rumi-border);
    border-radius: 50%;
    cursor: help;
    margin-left: 0.35rem;
    vertical-align: middle;
    position: relative;
  }
  .tip:hover {
    color: var(--rumi-text-primary);
    border-color: var(--rumi-text-secondary);
  }
</style>
