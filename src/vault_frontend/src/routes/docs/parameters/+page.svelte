<script lang="ts">
  import { onMount } from 'svelte';
  import { protocolService } from '$lib/services/protocol';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { collateralStore } from '$lib/stores/collateralStore';
  import type { CollateralInfo } from '$lib/services/types';
  import { get } from 'svelte/store';
  import { threePoolService } from '$lib/services/threePoolService';

  let liquidationBonus = 0;
  let recoveryTargetCr = 0;
  let recoveryModeThreshold = 0;
  let recoveryCrMultiplier = 0;
  let borrowingFee = 0;
  let redemptionFeeFloor = 0;
  let redemptionFeeCeiling = 0;
  let ckstableRepayFee = 0;
  let reserveRedemptionFee = 0;
  let liquidationProtocolShare = 0.03;
  let rmrFloor = 0.96;
  let rmrCeiling = 1.0;
  let rmrFloorCr = 2.25;
  let rmrCeilingCr = 1.5;
  let borrowingFeeCurve: [number, number][] = [];

  type InterestSplitEntry = { destination: string; bps: bigint };
  let interestSplit: InterestSplitEntry[] = [];

  function splitPct(dest: string): string {
    const entry = interestSplit.find(s => s.destination === dest);
    return entry ? (Number(entry.bps) / 100).toFixed(0) + '%' : '—';
  }

  function destLabel(dest: string): string {
    switch (dest) {
      case 'stability_pool': return 'Stability Pool';
      case 'treasury': return 'Treasury';
      case 'three_pool': return '3pool';
      default: return dest;
    }
  }

  // 3pool state
  let poolSwapFeeBps = 0n;
  let poolAdminFeeBps = 0n;
  let poolCurrentA = 0n;
  let poolTokenSymbols: string[] = [];

  // All supported collateral types (populated dynamically)
  let collaterals: CollateralInfo[] = [];

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
  function fmtLedgerFee(c: CollateralInfo): string {
    const fee = c.ledgerFee / Math.pow(10, c.decimals);
    return fee > 0 ? `${fee} ${c.symbol}` : `0.${'0'.repeat(c.decimals - 1)}1 ${c.symbol}`;
  }
  function fmtDebtCeiling(c: CollateralInfo): string {
    if (c.debtCeiling > 0 && c.debtCeiling < Number.MAX_SAFE_INTEGER) {
      return `${(c.debtCeiling / 1e8).toLocaleString()} icUSD`;
    }
    return 'Unlimited';
  }

  onMount(async () => {
    try {
      // Fetch global parameters and per-collateral config in parallel
      const [status, bFee, rfFloor, rfCeil, ckFee, rrFee, split, lpShare, rFloor, rCeil, rFloorCr, rCeilCr, poolStatus] = await Promise.all([
        protocolService.getProtocolStatus(),
        publicActor.get_borrowing_fee() as Promise<number>,
        publicActor.get_redemption_fee_floor() as Promise<number>,
        publicActor.get_redemption_fee_ceiling() as Promise<number>,
        publicActor.get_ckstable_repay_fee() as Promise<number>,
        publicActor.get_reserve_redemption_fee() as Promise<number>,
        publicActor.get_interest_split() as Promise<InterestSplitEntry[]>,
        publicActor.get_liquidation_protocol_share() as Promise<number>,
        publicActor.get_rmr_floor() as Promise<number>,
        publicActor.get_rmr_ceiling() as Promise<number>,
        publicActor.get_rmr_floor_cr() as Promise<number>,
        publicActor.get_rmr_ceiling_cr() as Promise<number>,
        threePoolService.getPoolStatus(),
      ]);

      liquidationBonus = status.liquidationBonus;
      recoveryTargetCr = status.recoveryTargetCr;
      recoveryModeThreshold = status.recoveryModeThreshold;
      recoveryCrMultiplier = status.recoveryCrMultiplier;
      borrowingFee = Number(bFee);
      redemptionFeeFloor = Number(rfFloor);
      redemptionFeeCeiling = Number(rfCeil);
      ckstableRepayFee = Number(ckFee);
      reserveRedemptionFee = Number(rrFee);
      interestSplit = split;
      liquidationProtocolShare = Number(lpShare);
      rmrFloor = Number(rFloor);
      rmrCeiling = Number(rCeil);
      rmrFloorCr = Number(rFloorCr);
      rmrCeilingCr = Number(rCeilCr);
      borrowingFeeCurve = status.borrowingFeeCurveResolved ?? [];

      // 3pool parameters
      poolSwapFeeBps = poolStatus.swap_fee_bps;
      poolAdminFeeBps = poolStatus.admin_fee_bps;
      poolCurrentA = poolStatus.current_a;
      poolTokenSymbols = poolStatus.tokens.map(t => t.symbol);

      // Load ALL supported collateral types
      await collateralStore.fetchSupportedCollateral();
      const state = get(collateralStore);
      collaterals = state.collaterals;

    } catch (e) {
      console.error('Failed to fetch protocol parameters:', e);
    }
    loaded = true;
  });
</script>

<svelte:head><title>Protocol Parameters | Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Protocol Parameters</h1>
  <p class="doc-intro">Live values read directly from the Rumi Protocol canister. Parameters shown in <span class="live-indicator">teal</span> are configurable by the protocol admin and always reflect the current on-chain state.</p>

  {#if !loaded}
    <p class="doc-loading">Loading parameters from canister...</p>
  {:else}

  <section class="doc-section">
    <h2 class="doc-heading">Collateral Parameters</h2>
    <div class="collateral-table-wrap">
      <table class="collateral-table">
        <thead>
          <tr>
            <th class="ct-label-col"></th>
            {#each collaterals as c (c.principal)}
              <th class="ct-val-col"><span class="ct-symbol"><span class="ct-dot" style="background:{c.color}"></span>{c.symbol}</span></th>
            {/each}
          </tr>
        </thead>
        <tbody>
          <tr>
            <td class="ct-label">Borrowing Threshold <span class="tip" data-tip="The minimum collateral ratio required to open a vault or borrow more icUSD.">?</span></td>
            {#each collaterals as c}<td class="ct-val live">{crPct(c.minimumCr)}</td>{/each}
          </tr>
          <tr>
            <td class="ct-label">Liquidation Ratio <span class="tip" data-tip="If your vault's collateral ratio drops below this level, it becomes eligible for liquidation.">?</span></td>
            {#each collaterals as c}<td class="ct-val live">{crPct(c.liquidationCr)}</td>{/each}
          </tr>
          <tr>
            <td class="ct-label">Recovery Target CR <span class="tip" data-tip="During Recovery Mode, partially liquidated vaults are restored to this CR. Equal to Borrowing Threshold × Recovery CR Multiplier ({(recoveryCrMultiplier * 100).toFixed(1)}%).">?</span></td>
            {#each collaterals as c}<td class="ct-val live">{crPct(c.recoveryTargetCr)}</td>{/each}
          </tr>
          <tr>
            <td class="ct-label">Liquidation Penalty <span class="tip" data-tip="The extra collateral seized from a liquidated vault above the debt repaid.">?</span></td>
            {#each collaterals as c}<td class="ct-val live">{pct(c.liquidationBonus)}</td>{/each}
          </tr>
          {#if borrowingFeeCurve.length > 0}
            <tr class="curve-row">
              <td class="ct-label">Borrowing Fee <span class="tip" data-tip="One-time fee deducted from minted icUSD. Scales with the system's Total Collateral Ratio (TCR) — as TCR drops toward recovery, a multiplier increases the effective fee for all assets.">?</span></td>
              {#each collaterals as c}<td class="ct-val"></td>{/each}
            </tr>
            {@const sortedCurve = [...borrowingFeeCurve].sort((a, b) => b[0] - a[0])}
            {#each sortedCurve as [cr, mult], i}
              <tr class="curve-row {i === sortedCurve.length - 1 ? 'curve-row-last' : ''}">
                <td class="ct-label ct-label-indent">{mult.toFixed(2)}× at {(cr * 100).toFixed(0)}%</td>
                {#each collaterals as c}<td class="ct-val live ct-val-sm">{(c.borrowingFee * mult * 100).toFixed(2)}%</td>{/each}
              </tr>
            {/each}
          {/if}
          <tr>
            <td class="ct-label">Interest Rate <span class="tip" data-tip="Annual interest charged on outstanding vault debt (APR).">?</span></td>
            {#each collaterals as c}<td class="ct-val live">{pctRaw(c.interestRateApr)}</td>{/each}
          </tr>
          <tr>
            <td class="ct-label">Min Borrow <span class="tip" data-tip="The smallest amount of icUSD you can borrow in a vault.">?</span></td>
            {#each collaterals as c}<td class="ct-val live">{c.minVaultDebt > 0 ? `${c.minVaultDebt / 1e8}` : '—'}</td>{/each}
          </tr>
          <tr>
            <td class="ct-label">Debt Ceiling <span class="tip" data-tip="Maximum total icUSD that can be borrowed against this collateral across all vaults.">?</span></td>
            {#each collaterals as c}<td class="ct-val live">{fmtDebtCeiling(c)}</td>{/each}
          </tr>
          <tr>
            <td class="ct-label">Ledger Fee <span class="tip" data-tip="Network fee charged by the token's ledger for each transfer. This is an Internet Computer system fee, not a Rumi fee.">?</span></td>
            {#each collaterals as c}<td class="ct-val">{fmtLedgerFee(c)}</td>{/each}
          </tr>
          <tr>
            <td class="ct-label">Status</td>
            {#each collaterals as c}<td class="ct-val" class:live={c.status === 'Active'}>{c.status}</td>{/each}
          </tr>
        </tbody>
      </table>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Global Liquidation Parameters</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Recovery Mode Threshold <span class="tip" data-tip="The system-wide collateral ratio that triggers Recovery Mode. Calculated as a debt-weighted average of all collateral types' borrowing thresholds. When the total system CR falls below this, the protocol enters Recovery Mode and the liquidation threshold rises.">?</span></span>
        <span class="param-val live">{crPct(recoveryModeThreshold)} (system-wide, debt-weighted)</span>
      </div>
      <div class="param">
        <span class="param-label">Recovery CR Multiplier <span class="tip" data-tip="Multiplied by each collateral type's Borrowing Threshold to determine its per-asset Recovery Target CR. For example, {(recoveryCrMultiplier * 100).toFixed(1)}% × 150% threshold = {(recoveryCrMultiplier * 1.5 * 100).toFixed(0)}% recovery target.">?</span></span>
        <span class="param-val live">{(recoveryCrMultiplier * 100).toFixed(1)}%</span>
      </div>
      <div class="param">
        <span class="param-label">Partial Liquidation <span class="tip" data-tip="In Recovery Mode, vaults between the Liquidation Ratio and Borrowing Threshold are not fully liquidated. Instead, only enough debt is repaid to restore the vault to its per-asset Recovery Target CR.">?</span></span>
        <span class="param-val">Restores vault CR to Recovery Target</span>
      </div>
      <div class="param">
        <span class="param-label">Liquidation Protocol Fee <span class="tip" data-tip="A percentage of the liquidation bonus (penalty) that goes to the protocol treasury. For example, if the bonus is 15% and the protocol fee is 3%, the liquidator receives 97% of the bonus and the protocol keeps 3%.">?</span></span>
        <span class="param-val live">{pctRaw(liquidationProtocolShare)} of liquidation bonus</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Fees</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Redemption Fee Floor <span class="tip" data-tip="The minimum fee charged when redeeming icUSD for collateral. The actual fee starts here and increases with redemption volume, then decays back over time.">?</span></span>
        <span class="param-val live">{pctRaw(redemptionFeeFloor)}</span>
      </div>
      <div class="param">
        <span class="param-label">Redemption Fee Ceiling <span class="tip" data-tip="The maximum the redemption fee can reach, no matter how much redemption activity occurs.">?</span></span>
        <span class="param-val live">{pctRaw(redemptionFeeCeiling)}</span>
      </div>
      <div class="param">
        <span class="param-label">Redemption Fee Decay <span class="tip" data-tip="The vault redemption fee decays by this factor each hour of inactivity. 0.94 means the rate roughly halves every 11 hours. This is hardcoded, not admin-configurable.">?</span></span>
        <span class="param-val">0.94 per hour</span>
      </div>
      <div class="param">
        <span class="param-label">Reserve Redemption Fee <span class="tip" data-tip="A flat fee applied when redeeming icUSD through the protocol's ckStable reserves (Tier 1). Unlike vault redemption fees, this does not vary with volume.">?</span></span>
        <span class="param-val live">{pctRaw(reserveRedemptionFee)}</span>
      </div>
      <div class="param">
        <span class="param-label">ckUSDT / ckUSDC Repay Fee <span class="tip" data-tip="A small fee applied when repaying vault debt or liquidating with ckUSDT or ckUSDC instead of icUSD. Compensates for potential stablecoin price variance.">?</span></span>
        <span class="param-val live">{pctRaw(ckstableRepayFee, 2)}</span>
      </div>
      <div class="param">
        <span class="param-label">icUSD Ledger Fee <span class="tip" data-tip="The network fee charged by the icUSD ledger for each transfer.">?</span></span>
        <span class="param-val">0.001 icUSD</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Interest & Revenue</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Interest Accrual <span class="tip" data-tip="Interest is applied to vault debt before every mutation (borrow, repay, withdraw, liquidation) and ticked forward every 5 minutes by a background timer.">?</span></span>
        <span class="param-val">Continuous (5-min tick + on-demand)</span>
      </div>
      <div class="param">
        <span class="param-label">Interest Rate Layers <span class="tip" data-tip="Layer 1: per-vault multiplier based on how close the vault's CR is to liquidation (higher rate for riskier vaults). Layer 2: system-wide multiplier active during Recovery Mode.">?</span></span>
        <span class="param-val">Per-vault CR curve + Recovery multiplier</span>
      </div>
      <div class="param">
        <span class="param-label">Interest Revenue Split <span class="tip" data-tip="Interest revenue is split between the 3pool (donated as icUSD, boosting LP token value), stability pool depositors (minted as icUSD), and the protocol treasury. This split is admin-configurable.">?</span></span>
        <span class="param-val live">{#each interestSplit as entry, i}{#if i > 0} / {/if}{(Number(entry.bps) / 100).toFixed(0)}% {destLabel(entry.destination)}{/each}</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Redemption Mechanics</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Redemption Margin Ratio (RMR) <span class="tip" data-tip="Redeemers receive this percentage of face value. {pctRaw(rmrFloor)} when system CR ≥ {crPct(rmrFloorCr)}, scaling linearly up to {pctRaw(rmrCeiling)} when system CR ≤ {crPct(rmrCeilingCr)}. Prevents mint-and-redeem arbitrage while protecting redeemers near recovery.">?</span></span>
        <span class="param-val live">{pctRaw(rmrFloor)} (healthy, CR ≥ {crPct(rmrFloorCr)}) → {pctRaw(rmrCeiling)} (stressed, CR ≤ {crPct(rmrCeilingCr)})</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">3pool (Stablecoin AMM)</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Pool Tokens <span class="tip" data-tip="The three stablecoins that can be swapped in the 3pool.">?</span></span>
        <span class="param-val live">{poolTokenSymbols.join(', ') || '—'}</span>
      </div>
      <div class="param">
        <span class="param-label">LP Token</span>
        <span class="param-val">3USD (ICRC-1)</span>
      </div>
      <div class="param">
        <span class="param-label">Swap Fee <span class="tip" data-tip="Fee charged on each swap, in basis points. For example, 4 bps = 0.04%. Admin-configurable via set_swap_fee.">?</span></span>
        <span class="param-val live">{(Number(poolSwapFeeBps) / 100).toFixed(2)}%</span>
      </div>
      <div class="param">
        <span class="param-label">Admin Fee <span class="tip" data-tip="The fraction of the swap fee retained by the protocol. For example, 50% means half the swap fee goes to protocol admin fees. Admin-configurable via set_admin_fee.">?</span></span>
        <span class="param-val live">{(Number(poolAdminFeeBps) / 100).toFixed(0)}% of swap fee</span>
      </div>
      <div class="param">
        <span class="param-label">Amplification Coefficient (A) <span class="tip" data-tip="Controls the curvature of the StableSwap invariant. Higher A means tighter peg (lower slippage near 1:1). Admin-configurable via ramp_a (gradual change over time).">?</span></span>
        <span class="param-val live">{poolCurrentA.toString()}</span>
      </div>
      <div class="param">
        <span class="param-label">Interest Donation Share <span class="tip" data-tip="The percentage of all vault interest revenue donated to the 3pool. This increases the virtual price of LP tokens, generating yield for liquidity providers. Admin-configurable via set_interest_split.">?</span></span>
        <span class="param-val live">{splitPct('three_pool')}</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Minimums</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Minimum Partial Repayment <span class="tip" data-tip="The smallest repayment amount accepted by the protocol.">?</span></span>
        <span class="param-val">0.1 icUSD</span>
      </div>
      <div class="param">
        <span class="param-label">Dust Threshold (auto-forgiven) <span class="tip" data-tip="Debt amounts smaller than this are automatically forgiven when closing a vault, to avoid rounding issues.">?</span></span>
        <span class="param-val">0.0005 icUSD</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Supported Tokens</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Collateral</span>
        <span class="param-val live">{collaterals.map(c => c.symbol).join(', ') || '—'}</span>
      </div>
      <div class="param"><span class="param-label">Stablecoin (minted)</span><span class="param-val">icUSD</span></div>
      <div class="param"><span class="param-label">Repayment & Liquidation</span><span class="param-val">icUSD, ckUSDT, ckUSDC</span></div>
      <div class="param">
        <span class="param-label">ckStable Depeg Rejection <span class="tip" data-tip="If ckUSDT or ckUSDC is trading outside this range, the protocol rejects it for repayment or liquidation to protect against depeg events. Stablecoin prices are cached for up to 60 seconds (vs 30s for collateral).">?</span></span>
        <span class="param-val">Outside $0.95 – $1.05</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Precision & Rounding</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Token Arithmetic <span class="tip" data-tip="All internal token division operations (e.g., converting icUSD to ICP at a given price) round down (floor). This means the protocol never overpays — rounding always favors the protocol by a fraction of a unit.">?</span></span>
        <span class="param-val">Floor rounding (truncation)</span>
      </div>
      <div class="param">
        <span class="param-label">icUSD Precision <span class="tip" data-tip="icUSD uses 8 decimal places (e8s). 1 icUSD = 100,000,000 e8s.">?</span></span>
        <span class="param-val">8 decimals (e8s)</span>
      </div>
      <div class="param">
        <span class="param-label">ckStable Precision <span class="tip" data-tip="ckUSDT and ckUSDC use 6 decimal places (e6s). When converting between icUSD (8 decimals) and ckStables (6 decimals), amounts are truncated to the nearest 100 e8s. Up to 0.00000099 icUSD may be lost per conversion.">?</span></span>
        <span class="param-val">6 decimals (e6s) — 100:1 conversion from e8s</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Oracle & Timing</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Price Source <span class="tip" data-tip="The Internet Computer's built-in Exchange Rate Canister, which aggregates prices from multiple exchanges.">?</span></span>
        <span class="param-val">IC Exchange Rate Canister (XRC)</span>
      </div>
      <div class="param">
        <span class="param-label">Background Price Polling <span class="tip" data-tip="Prices are refreshed automatically on this interval via a canister timer.">?</span></span>
        <span class="param-val">300 seconds</span>
      </div>
      <div class="param">
        <span class="param-label">On-Demand Freshness <span class="tip" data-tip="Operations like borrowing and liquidation trigger a fresh price fetch if the cached price is older than this. Ensures operations use recent data.">?</span></span>
        <span class="param-val">30 seconds</span>
      </div>
      <div class="param">
        <span class="param-label">Stale Price Rejection <span class="tip" data-tip="If the latest price is older than this, the protocol rejects all state-changing operations until a fresh price is obtained. This prevents operations based on dangerously outdated prices.">?</span></span>
        <span class="param-val">10 minutes</span>
      </div>
      <div class="param">
        <span class="param-label">Read-Only Price Floor <span class="tip" data-tip="If the oracle reports a price below this level, the protocol enters Read-Only mode and halts all operations as a safety measure.">?</span></span>
        <span class="param-val">$0.01</span>
      </div>
      <div class="param">
        <span class="param-label">Stuck Transfer Timeout <span class="tip" data-tip="If a collateral or icUSD transfer fails and isn't successfully retried within this time, it may require manual intervention.">?</span></span>
        <span class="param-val">15 minutes</span>
      </div>
      <div class="param">
        <span class="param-label">Health Monitor Interval <span class="tip" data-tip="A background process that checks for stuck transfers, under-collateralized vaults, and other health issues.">?</span></span>
        <span class="param-val">5 minutes</span>
      </div>
      <div class="param">
        <span class="param-label">Operation Concurrency <span class="tip" data-tip="Each user can only have one vault operation in-flight at a time. If you submit a second operation before the first completes, it will fail with 'AlreadyProcessing'. Guards auto-release after 2.5 minutes for the same user, with a hard 5-minute expiry. The system supports up to 100 concurrent operations across all users.">?</span></span>
        <span class="param-val">1 per user / 100 global (5-min guard timeout)</span>
      </div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Protocol Modes</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">General Availability <span class="tip" data-tip="Normal operating mode. All operations are available. Liquidation only affects vaults below the Liquidation Ratio.">?</span></span>
        <span class="param-val">Total CR &ge; <span class="live">{crPct(recoveryModeThreshold)}</span></span>
      </div>
      <div class="param">
        <span class="param-label">Recovery <span class="tip" data-tip="Triggered when the system is under-collateralized. The liquidation threshold rises to the Borrowing Threshold, minimum CR for borrows is raised to the recovery target, and vaults in the warning zone receive partial liquidation.">?</span></span>
        <span class="param-val">Total CR &lt; <span class="live">{crPct(recoveryModeThreshold)}</span></span>
      </div>
      <div class="param">
        <span class="param-label">Read-Only <span class="tip" data-tip="Emergency mode where all state-changing operations are paused. Triggered by extreme under-collateralization or oracle failure.">?</span></span>
        <span class="param-val">Total CR &lt; 100% or oracle failure</span>
      </div>
      <div class="param">
        <span class="param-label">Frozen <span class="tip" data-tip="Emergency kill-switch activated manually by the protocol admin. All state-changing operations are paused until the admin unfreezes the protocol.">?</span></span>
        <span class="param-val">Admin-triggered emergency halt</span>
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
  .live { color: var(--rumi-action); font-weight: 600; }
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
  .tip::after {
    content: attr(data-tip);
    position: absolute;
    bottom: calc(100% + 0.5rem);
    left: 50%;
    transform: translateX(-50%);
    background: var(--rumi-bg-surface1, #1e1e2e);
    color: var(--rumi-text-secondary, #b0b0c0);
    border: 1px solid var(--rumi-border, #333);
    border-radius: 0.5rem;
    padding: 0.5rem 0.75rem;
    font-size: 0.75rem;
    font-weight: 400;
    line-height: 1.45;
    white-space: normal;
    width: max-content;
    max-width: 280px;
    z-index: 100;
    pointer-events: none;
    opacity: 0;
    transition: opacity 0.15s ease;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
  }
  .tip:hover::after {
    opacity: 1;
  }
  /* Collateral comparison table */
  .collateral-table-wrap {
    overflow: visible;
  }
  .collateral-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.8125rem;
  }
  .collateral-table th,
  .collateral-table td {
    padding: 0.5rem 0.75rem;
    border-bottom: 1px solid var(--rumi-border);
    white-space: nowrap;
  }
  .collateral-table thead th {
    border-bottom: 2px solid var(--rumi-border);
    padding-bottom: 0.625rem;
  }
  .collateral-table tbody tr:last-child td { border-bottom: none; }
  .ct-label-col { text-align: left; }
  .ct-val-col { text-align: right; }
  .ct-symbol {
    display: inline-flex;
    align-items: center;
    gap: 0.375rem;
    font-size: 0.9375rem;
    font-weight: 700;
    letter-spacing: 0.02em;
    color: var(--rumi-text-primary);
  }
  .ct-dot {
    display: inline-block;
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 50%;
    flex-shrink: 0;
  }
  .ct-label {
    color: var(--rumi-text-secondary);
    text-align: left;
    font-weight: 400;
  }
  .ct-label-indent {
    padding-left: 1.5rem;
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
  }
  .curve-label {
    color: var(--rumi-text-secondary);
    font-size: 0.6875rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin-right: 0.25rem;
  }
  .curve-row td { padding-top: 0.25rem; padding-bottom: 0.25rem; border-bottom-color: transparent; }
  .curve-row-last td { border-bottom-color: var(--rumi-border); }
  .ct-val {
    text-align: right;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
  }
  .ct-val.live { color: var(--rumi-action); }
  .ct-val-sm { font-size: 0.75rem; }
</style>
