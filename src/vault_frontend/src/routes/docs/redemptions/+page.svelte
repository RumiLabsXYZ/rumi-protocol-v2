<script lang="ts">
  import { onMount } from 'svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { protocolService } from '$lib/services/protocol';
  import { fetchProtocolConfig } from '$services/explorer/explorerService';
  import { getTokenSymbol } from '$utils/explorerHelpers';
  import type { InterestSplitArg } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did';

  let reserveRedemptionFee = 0;
  let redemptionFeeFloor = 0;
  let redemptionFeeCeiling = 0;
  let reserveRedemptionsEnabled = false;
  let rmrFloor = 0.96;
  let rmrCeiling = 1.0;
  let rmrFloorCr = 2.25;
  let rmrCeilingCr = 1.5;
  let systemCR = 2.0;
  let loaded = false;

  // Collateral priority tiers: Map<tierNumber, symbolList>
  let collateralTiers: Map<number, string[]> = new Map();

  $: rmrFloorPct = (rmrFloor * 100).toFixed(0);
  $: rmrCeilingPct = (rmrCeiling * 100).toFixed(0);
  $: currentRmr = systemCR >= rmrFloorCr
    ? rmrFloor
    : systemCR <= rmrCeilingCr
      ? rmrCeiling
      : rmrCeiling - ((systemCR - rmrCeilingCr) / (rmrFloorCr - rmrCeilingCr)) * (rmrCeiling - rmrFloor);

  // RMR curve computed values
  $: rmrFloorPctVal = rmrFloor * 100;
  $: rmrCeilingPctVal = rmrCeiling * 100;
  $: rmrFloorCrPct = rmrFloorCr * 100;
  $: rmrCeilingCrPct = rmrCeilingCr * 100;
  $: rmrCrPadding = (rmrFloorCrPct - rmrCeilingCrPct) * 0.15;
  $: rmrCrMin = rmrCeilingCrPct - rmrCrPadding;
  $: rmrCrMax = rmrFloorCrPct + rmrCrPadding;
  $: rmrRateMin = rmrFloorPctVal - (rmrCeilingPctVal - rmrFloorPctVal) * 0.3;
  $: rmrRateMax = rmrCeilingPctVal + (rmrCeilingPctVal - rmrFloorPctVal) * 0.3;
  $: currentRmrPct = currentRmr * 100;
  $: currentCrPct = Math.min(Math.max(systemCR * 100, rmrCeilingCrPct), rmrFloorCrPct);

  function rmrCurveX(cr: number): number {
    return 55 + ((cr - rmrCrMin) / (rmrCrMax - rmrCrMin || 1)) * 385;
  }
  function rmrCurveY(rate: number): number {
    return 120 - ((rate - rmrRateMin) / (rmrRateMax - rmrRateMin || 1)) * 95;
  }

  let interestSplit: InterestSplitArg[] = [];

  function splitPct(dest: string): string {
    const entry = interestSplit.find(s => s.destination === dest);
    return entry ? (Number(entry.bps) / 100).toFixed(0) + '%' : '—';
  }

  function pctRaw(val: number, decimals = 1): string {
    if (val === undefined || val === null) return '—';
    return (val * 100).toFixed(decimals) + '%';
  }

  function tierSymbols(tier: number): string {
    const symbols = collateralTiers.get(tier);
    return symbols?.length ? symbols.join(', ') : 'None';
  }

  onMount(async () => {
    try {
      const [rrFee, rfFloor, rfCeil, rrEnabled, rFloor, rCeil, rFloorCr, rCeilCr, split, status, config] = await Promise.all([
        publicActor.get_reserve_redemption_fee() as Promise<number>,
        publicActor.get_redemption_fee_floor() as Promise<number>,
        publicActor.get_redemption_fee_ceiling() as Promise<number>,
        publicActor.get_reserve_redemptions_enabled() as Promise<boolean>,
        publicActor.get_rmr_floor() as Promise<number>,
        publicActor.get_rmr_ceiling() as Promise<number>,
        publicActor.get_rmr_floor_cr() as Promise<number>,
        publicActor.get_rmr_ceiling_cr() as Promise<number>,
        publicActor.get_interest_split(),
        protocolService.getProtocolStatus(),
        fetchProtocolConfig(),
      ]);
      reserveRedemptionFee = Number(rrFee);
      redemptionFeeFloor = Number(rfFloor);
      redemptionFeeCeiling = Number(rfCeil);
      reserveRedemptionsEnabled = rrEnabled;
      rmrFloor = Number(rFloor);
      rmrCeiling = Number(rCeil);
      rmrFloorCr = Number(rFloorCr);
      rmrCeilingCr = Number(rCeilCr);
      systemCR = status.totalCollateralRatio ?? 2.0;
      interestSplit = split;

      // Build collateral tier map from protocol config
      if (config?.collateral_configs) {
        const tierMap = new Map<number, string[]>();
        for (const [principal, cfg] of config.collateral_configs) {
          const pid = principal?.toText?.() ?? String(principal);
          const tier = cfg.redemption_tier?.[0] ?? 1;
          const symbol = getTokenSymbol(pid);
          if (!tierMap.has(tier)) tierMap.set(tier, []);
          tierMap.get(tier)!.push(symbol);
        }
        collateralTiers = tierMap;
      }
    } catch (e) {
      console.error('Failed to fetch redemption params:', e);
    }
    loaded = true;
  });
</script>

<svelte:head><title>Redemptions | Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Redemptions</h1>

  <section class="doc-section">
    <h2 class="doc-heading">What Is Redemption</h2>
    <p>Redemption lets any icUSD holder exchange their icUSD for value at close to face value ($1 per icUSD), minus a fee and a Redemption Margin Ratio (RMR) adjustment. This creates a price floor for icUSD: if icUSD trades below $1 on the open market, arbitrageurs can buy it cheaply and redeem it for near-$1 worth of assets, driving the price back up.</p>
    <p>Redemption is a core peg-maintenance mechanism. It protects icUSD holders by ensuring their tokens are always backed by redeemable value.</p>
    <div class="fee-box">
      <span class="fee-label">Redemption Margin Ratio (RMR)</span>
      <span class="fee-value live">{rmrFloorPct}% (healthy) → {rmrCeilingPct}% (at recovery)</span>
    </div>

    <!-- RMR Curve -->
    {#if loaded}
      <div class="rmr-curve-wrap">
        <svg viewBox="0 0 480 190" class="rmr-curve-svg" preserveAspectRatio="xMidYMid meet">
          <!-- Grid lines -->
          {#each [rmrFloorPctVal, (rmrFloorPctVal + rmrCeilingPctVal) / 2, rmrCeilingPctVal] as gridRate}
            <line x1="55" y1={rmrCurveY(gridRate)} x2="440" y2={rmrCurveY(gridRate)} stroke="var(--rumi-border, #333)" stroke-width="0.5" stroke-dasharray="3,3" />
            <text x="50" y={rmrCurveY(gridRate) + 3} text-anchor="end" fill="var(--rumi-text-muted, #888)" font-size="9" font-family="Inter, sans-serif">{gridRate.toFixed(0)}%</text>
          {/each}

          <!-- Area fill -->
          <path d="M {rmrCurveX(rmrCrMin)},{rmrCurveY(rmrCeilingPctVal)} L {rmrCurveX(rmrCeilingCrPct)},{rmrCurveY(rmrCeilingPctVal)} L {rmrCurveX(rmrFloorCrPct)},{rmrCurveY(rmrFloorPctVal)} L {rmrCurveX(rmrCrMax)},{rmrCurveY(rmrFloorPctVal)} L {rmrCurveX(rmrCrMax)},120 L {rmrCurveX(rmrCrMin)},120 Z" fill="var(--rumi-action, #34d399)" opacity="0.07" />

          <!-- RMR line -->
          <polyline
            points="{rmrCurveX(rmrCrMin)},{rmrCurveY(rmrCeilingPctVal)} {rmrCurveX(rmrCeilingCrPct)},{rmrCurveY(rmrCeilingPctVal)} {rmrCurveX(rmrFloorCrPct)},{rmrCurveY(rmrFloorPctVal)} {rmrCurveX(rmrCrMax)},{rmrCurveY(rmrFloorPctVal)}"
            fill="none" stroke="var(--rumi-action, #34d399)" stroke-width="2" stroke-linejoin="round"
          />

          <!-- Endpoint circles -->
          <circle cx={rmrCurveX(rmrCeilingCrPct)} cy={rmrCurveY(rmrCeilingPctVal)} r="4.5" fill="var(--rumi-action, #34d399)" />
          <circle cx={rmrCurveX(rmrCeilingCrPct)} cy={rmrCurveY(rmrCeilingPctVal)} r="2" fill="var(--rumi-bg-surface, #0d0d1a)" />
          <text x={rmrCurveX(rmrCeilingCrPct)} y={rmrCurveY(rmrCeilingPctVal) - 10} text-anchor="middle" fill="var(--rumi-text-primary, #eee)" font-size="10" font-weight="600" font-family="Inter, sans-serif">{rmrCeilingPctVal.toFixed(0)}%</text>

          <circle cx={rmrCurveX(rmrFloorCrPct)} cy={rmrCurveY(rmrFloorPctVal)} r="4.5" fill="var(--rumi-action, #34d399)" />
          <circle cx={rmrCurveX(rmrFloorCrPct)} cy={rmrCurveY(rmrFloorPctVal)} r="2" fill="var(--rumi-bg-surface, #0d0d1a)" />
          <text x={rmrCurveX(rmrFloorCrPct)} y={rmrCurveY(rmrFloorPctVal) - 10} text-anchor="middle" fill="var(--rumi-text-primary, #eee)" font-size="10" font-weight="600" font-family="Inter, sans-serif">{rmrFloorPctVal.toFixed(0)}%</text>

          <!-- Current position marker -->
          <line x1={rmrCurveX(currentCrPct)} y1={rmrCurveY(currentRmrPct) + 6} x2={rmrCurveX(currentCrPct)} y2="133" stroke="#d176e8" stroke-width="1.5" stroke-dasharray="3,2" />
          <circle cx={rmrCurveX(currentCrPct)} cy={rmrCurveY(currentRmrPct)} r="5.5" fill="#d176e8" />
          <circle cx={rmrCurveX(currentCrPct)} cy={rmrCurveY(currentRmrPct)} r="2.5" fill="var(--rumi-bg-surface, #0d0d1a)" />
          <text x={rmrCurveX(currentCrPct)} y={rmrCurveY(currentRmrPct) - 12} text-anchor="middle" fill="#d176e8" font-size="10" font-weight="700" font-family="Inter, sans-serif">Now: {currentRmrPct.toFixed(0)}%</text>

          <!-- Meter bar -->
          <defs>
            <linearGradient id="meter-rmr-docs" x1="0" y1="0" x2="1" y2="0">
              <stop offset="0%" stop-color="#e06b9f" stop-opacity="0.75" />
              <stop offset="40%" stop-color="#a78bfa" stop-opacity="0.5" />
              <stop offset="100%" stop-color="#2DD4BF" stop-opacity="0.5" />
            </linearGradient>
          </defs>
          <rect x="55" y="135" width="385" height="12" rx="6" fill="url(#meter-rmr-docs)" />

          {#each [[rmrCeilingCrPct, 'Stressed'], [rmrFloorCrPct, 'Healthy']] as [crVal, _label]}
            <line x1={rmrCurveX(crVal)} y1="133" x2={rmrCurveX(crVal)} y2="149" stroke="var(--rumi-text-primary, #eee)" stroke-width="1.5" opacity="0.6" />
            <circle cx={rmrCurveX(crVal)} cy="141" r="2.5" fill="var(--rumi-text-primary, #eee)" opacity="0.8" />
            <text x={rmrCurveX(crVal)} y="165" text-anchor="middle" fill="var(--rumi-text-secondary, #b0b0c0)" font-size="10" font-family="Inter, sans-serif">{crVal.toFixed(0)}%</text>
          {/each}

          <text x="247" y="182" text-anchor="middle" fill="var(--rumi-text-muted, #888)" font-size="9" font-family="Inter, sans-serif">System Collateral Ratio</text>
        </svg>
      </div>
    {/if}

    <p>The RMR determines what fraction of face value you receive. When the system is healthy (total CR well above recovery threshold), you receive {rmrFloorPct}% of face value. As the system approaches recovery, the RMR scales linearly up to {rmrCeilingPct}%. This prevents mint-and-redeem arbitrage under normal conditions while ensuring full redemption value when the system most needs peg support.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Reserve Redemptions</h2>
    <p>The protocol holds reserves of <strong>ckUSDT</strong> and <strong>ckUSDC</strong>, real stablecoins that accumulate when users repay vault debt with ckStables. When you redeem icUSD, the protocol first tries to fill your redemption from these reserves.</p>
    {#if loaded}
      <div class="fee-box">
        <span class="fee-label">Reserve Redemption Fee</span>
        <span class="fee-value" class:live={reserveRedemptionFee > 0}>{pctRaw(reserveRedemptionFee)}</span>
      </div>
      <div class="fee-box">
        <span class="fee-label">Reserve Redemptions</span>
        <span class="fee-value" class:enabled={reserveRedemptionsEnabled} class:disabled={!reserveRedemptionsEnabled}>{reserveRedemptionsEnabled ? 'Enabled' : 'Disabled'}</span>
      </div>
    {/if}
    <p>Reserve redemptions are the cleanest outcome: you burn icUSD and receive ckStables in return. No vaults are affected.</p>
    <p>Reserves grow when users repay vault debt with ckUSDT or ckUSDC. Note that interest revenue from stablecoin repayments is split according to the protocol's interest split: currently <span class="live">{splitPct('stability_pool')}</span> to the stability pool, <span class="live">{splitPct('three_pool')}</span> to the 3pool, and <span class="live">{splitPct('treasury')}</span> to treasury.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Vault Redemptions</h2>
    <p>If the protocol's reserves don't have enough ckStables to fill the full redemption amount, the remainder "spills over" into vault redemptions. The protocol first targets vaults holding <strong>Tier 1 collateral</strong>, then Tier 2, then Tier 3. Within each tier, vaults with the <strong>lowest collateral ratios</strong> are redeemed against first, reducing their debt but also taking an equivalent value of their collateral.</p>
    {#if loaded}
      <div class="fee-box">
        <span class="fee-label">Vault Redemption Fee Floor</span>
        <span class="fee-value live">{pctRaw(redemptionFeeFloor)}</span>
      </div>
      <div class="fee-box">
        <span class="fee-label">Vault Redemption Fee Ceiling</span>
        <span class="fee-value live">{pctRaw(redemptionFeeCeiling)}</span>
      </div>
    {/if}
    <p>The vault redemption fee is dynamic. It is calculated using a base rate that increases with each redemption and decays over time:</p>
    <p class="doc-formula">fee = base_rate &times; 0.94<sup>hours_since_last_redemption</sup> + (redeemed / total_borrowed) &times; 0.5</p>
    <p>The base rate starts at zero and increases with each redemption. The 0.94 decay factor means the rate halves roughly every 11 hours of inactivity. The result is clamped between the floor and ceiling shown above. After each redemption, the base rate is updated to the newly computed fee.</p>
    <p>You receive the vault's collateral asset (not ckStables) from vault redemptions. The collateral is sent directly to your account.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Collateral Priority Tiers</h2>
    <p>When vault redemptions occur, not all collateral types are treated equally. Each collateral asset is assigned to a <strong>priority tier</strong> that determines the order in which vaults are redeemed against. Tier 1 collateral is redeemed first, then Tier 2, then Tier 3. Within a tier, vaults are selected by lowest collateral ratio.</p>
    {#if loaded && collateralTiers.size > 0}
      <div class="tier-table">
        {#each [1, 2, 3] as tier}
          {#if collateralTiers.has(tier)}
            <div class="tier-row">
              <span class="tier-label">Tier {tier}{tier === 1 ? ' (redeemed first)' : tier === 3 ? ' (redeemed last)' : ''}</span>
              <span class="tier-assets live">{tierSymbols(tier)}</span>
            </div>
          {/if}
        {/each}
      </div>
    {/if}
    <p>This tiering protects certain asset classes from being redeemed against unless necessary. Tier 1 assets (typically native or high-liquidity tokens) absorb redemptions first, shielding Tier 2 and Tier 3 assets (wrapped or bridged tokens) from unnecessary exposure.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Token Preference</h2>
    <p>When redeeming through reserves, you can choose whether to receive <strong>ckUSDT</strong> or <strong>ckUSDC</strong>. If the protocol doesn't have enough of your preferred token, it falls back to the other. If neither reserve is sufficient, the remainder spills over to vault redemptions.</p>
    <p>By default, the protocol fills ckUSDT first. You can override this by specifying your preferred token ledger when calling the redemption function.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Impact on Vault Owners</h2>
    <p><strong>Reserve redemptions:</strong> Zero impact on vaults. The protocol draws from its own stablecoin reserves. Your vault is completely unaffected.</p>
    <p><strong>Vault redemptions:</strong> Your vault's debt is reduced, but collateral is also taken proportionally. The protocol uses a <strong>water-filling algorithm</strong> rather than simply draining the single lowest-CR vault. Instead, it identifies the band of vaults with the lowest CRs and distributes the redemption proportionally by debt across the band, raising them all equally. If the redemption amount would raise the entire band above the next band, it merges upward and the process repeats. This means redemptions affect multiple low-CR vaults simultaneously rather than wiping out one vault at a time.</p>
    <p>Vault redemption is not liquidation. Your vault remains open and you retain any remaining collateral and debt. The collateral taken is valued at the RMR-adjusted rate ({rmrFloorPct}–{rmrCeilingPct}% of face value depending on system health), minus the dynamic vault redemption fee. Keeping your vault well-collateralized reduces the chance of being redeemed against.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">How Redemption Flows</h2>
    <ol class="flow-list">
      <li>You submit icUSD for redemption.</li>
      <li>The icUSD is burned (removed from circulation).</li>
      <li><strong>Reserves first:</strong> The protocol checks its ckStable reserves and sends you stablecoins up to the available balance.</li>
      <li><strong>Vault spillover:</strong> Any remaining amount after reserves is filled by taking collateral from the lowest-CR vaults, prioritized by collateral tier (see below).</li>
      <li>The Redemption Margin Ratio (RMR) is applied. You receive {rmrFloorPct}–{rmrCeilingPct}% of face value depending on system health.</li>
      <li>Fees are deducted from the amount you receive: flat fee for reserve redemptions, dynamic fee for vault redemptions.</li>
      <li>Reserve fees are sent to the protocol treasury. Vault redemption fees are deducted from the collateral released.</li>
    </ol>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Safety: Failed Transfer Handling</h2>
    <p>On the Internet Computer, cross-canister calls are not atomic, so a transfer can fail after your icUSD has already been burned. If a reserve redemption's ckStable transfer fails, the protocol automatically <strong>refunds your icUSD</strong> by minting it back to your account. You will see an error message, but your funds are safe.</p>
    <p>If both the ckStable transfer and the refund fail (an extremely unlikely scenario), the incident is logged for manual intervention by the protocol admin using the <a href="/transparency" class="doc-link">admin mint</a> function.</p>
  </section>
</article>

<style>
  .fee-box {
    display: flex; justify-content: space-between; align-items: baseline;
    padding: 0.5rem 0.75rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    margin: 0.5rem 0;
  }
  .fee-label { font-size: 0.8125rem; color: var(--rumi-text-secondary); }
  .fee-value {
    font-family: 'Inter', sans-serif; font-size: 0.875rem;
    font-weight: 600; color: var(--rumi-text-primary);
    font-variant-numeric: tabular-nums;
  }
  .fee-value.live { color: var(--rumi-action); }
  .live { color: var(--rumi-action); font-weight: 600; }
  .fee-value.enabled { color: var(--rumi-safe); }
  .fee-value.disabled { color: var(--rumi-danger); }

  .rmr-curve-wrap {
    margin: 0.75rem 0;
  }
  .rmr-curve-svg {
    width: 100%;
    max-width: 520px;
    height: auto;
  }

  .doc-formula {
    font-family: 'Inter', monospace;
    font-size: 0.8125rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    padding: 0.75rem 1rem;
    text-align: center;
    color: var(--rumi-text-primary);
    font-weight: 500;
    overflow-x: auto;
  }

  .flow-list {
    padding-left: 1.25rem;
    display: flex; flex-direction: column; gap: 0.5rem;
  }
  .flow-list li {
    font-size: 0.875rem; color: var(--rumi-text-secondary); line-height: 1.5;
  }

  .tier-table {
    display: flex; flex-direction: column; gap: 0.375rem;
    margin: 0.5rem 0;
  }
  .tier-row {
    display: flex; justify-content: space-between; align-items: baseline;
    padding: 0.5rem 0.75rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
  }
  .tier-label {
    font-size: 0.8125rem; color: var(--rumi-text-secondary);
  }
  .tier-assets {
    font-family: 'Inter', sans-serif; font-size: 0.875rem;
    font-weight: 600;
  }
</style>
