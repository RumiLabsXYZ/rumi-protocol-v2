<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { formatNumber } from '$lib/utils/format';
  import { protocolService } from '$lib/services/protocol';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { collateralStore } from '$lib/stores/collateralStore';
  import { get } from 'svelte/store';
  import { TokenService } from '$lib/services/tokenService';
  import { CANISTER_IDS } from '$lib/config';
  import { Principal } from '@dfinity/principal';

  export let protocolStatus: {
    mode: any;
    totalIcpMargin: number;
    totalIcusdBorrowed: number;
    lastIcpRate: number;
    lastIcpTimestamp: number;
    totalCollateralRatio: number;
    liquidationBonus: number;
    recoveryTargetCr: number;
    recoveryModeThreshold: number;
    recoveryLiquidationBuffer: number;
    reserveRedemptionsEnabled?: boolean;
    reserveRedemptionFee?: number;
  } | undefined = undefined;

  // Self-fetch fallback when no prop is provided
  let selfFetchedStatus: typeof protocolStatus;
  let selfFetchedBorrowFee = 0;
  let ckusdtReserve = 0;
  let ckusdcReserve = 0;
  let refreshInterval: ReturnType<typeof setInterval>;

  const protocolPrincipal = Principal.fromText(CANISTER_IDS.PROTOCOL);

  async function fetchStatus() {
    try {
      const [s, bFee] = await Promise.all([
        protocolService.getProtocolStatus(),
        publicActor.get_borrowing_fee() as Promise<number>,
      ]);
      selfFetchedBorrowFee = Number(bFee);
      selfFetchedStatus = {
        mode: s.mode || 'GeneralAvailability',
        totalIcpMargin: Number(s.totalIcpMargin || 0),
        totalIcusdBorrowed: Number(s.totalIcusdBorrowed || 0),
        lastIcpRate: Number(s.lastIcpRate || 0),
        lastIcpTimestamp: Number(s.lastIcpTimestamp || 0),
        totalCollateralRatio: Number(s.totalCollateralRatio || 0),
        liquidationBonus: Number(s.liquidationBonus || 0),
        recoveryTargetCr: Number(s.recoveryTargetCr || 0),
        recoveryModeThreshold: Number(s.recoveryModeThreshold || 0),
        recoveryLiquidationBuffer: Number(s.recoveryLiquidationBuffer || 0),
        reserveRedemptionsEnabled: Boolean(s.reserveRedemptionsEnabled),
        reserveRedemptionFee: Number(s.reserveRedemptionFee || 0),
      };
      // Also fetch per-collateral config for ICP-specific values
      await collateralStore.fetchSupportedCollateral();
      // Fetch ckStable reserves held by the protocol canister
      fetchCkStableReserves();
    } catch (e) { console.error('ProtocolStats fetch error:', e); }
  }

  async function fetchCkStableReserves() {
    try {
      const [usdt, usdc] = await Promise.all([
        TokenService.getTokenBalance(CANISTER_IDS.CKUSDT_LEDGER, protocolPrincipal),
        TokenService.getTokenBalance(CANISTER_IDS.CKUSDC_LEDGER, protocolPrincipal),
      ]);
      ckusdtReserve = Number(usdt) / 1e6; // ckUSDT = 6 decimals
      ckusdcReserve = Number(usdc) / 1e6; // ckUSDC = 6 decimals
    } catch (e) { console.error('ckStable reserve fetch error:', e); }
  }

  onMount(() => {
    if (!protocolStatus) {
      fetchStatus();
      refreshInterval = setInterval(fetchStatus, 15000);
    }
    return () => { if (refreshInterval) clearInterval(refreshInterval); };
  });
  onDestroy(() => { if (refreshInterval) clearInterval(refreshInterval); });

  $: status = protocolStatus || selfFetchedStatus;
  $: icpPrice = status?.lastIcpRate || 0;
  $: collateralValueUsd = (status?.totalIcpMargin || 0) * icpPrice;
  $: collateralPercent = (status?.totalIcusdBorrowed || 0) > 0
    ? (status?.totalCollateralRatio || 0) * 100
    : (status?.totalIcpMargin || 0) > 0 ? Infinity : 0;
  $: formattedCR = collateralPercent === Infinity
    ? '∞' : collateralPercent > 1000000 ? '>1M' : formatNumber(collateralPercent);
  $: modeLabel = (() => {
    const m = status?.mode;
    if (!m) return 'Unknown';
    if (typeof m === 'string') return m === 'GeneralAvailability' ? 'Normal' : m;
    if (m.GeneralAvailability !== undefined) return 'Normal';
    if (m.Recovery !== undefined) return 'Recovery';
    if (m.ReadOnly !== undefined) return 'Read Only';
    return 'Unknown';
  })();
  $: modeClass = modeLabel === 'Normal' ? 'mode-normal' : modeLabel === 'Recovery' ? 'mode-recovery' : 'mode-other';
  $: liqBonus = status?.liquidationBonus ? (status.liquidationBonus - 1) * 100 : 0;
  $: borrowFee = selfFetchedBorrowFee * 100;
  // Per-collateral ICP values from collateral store (live, not hardcoded)
  $: icpConfig = $collateralStore.collaterals.find(c => c.symbol === 'ICP');
  $: minCR = icpConfig?.minimumCr ?? 1.5;
  $: liqCR = icpConfig?.liquidationCr ?? 1.33;
  $: recoveryThreshold = status?.recoveryModeThreshold ?? 1.5;
  $: interestApr = icpConfig?.interestRateApr ?? 0;
</script>

<div class="protocol-stats">
  <!-- System -->
  <h4 class="group-heading">System</h4>
  <div class="stats-stack">
    <div class="stat-row">
      <span class="stat-label">Protocol CR</span>
      <span class="stat-value">{formattedCR}%</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Total Collateral</span>
      <span class="stat-value-stack">
        <span>{formatNumber(status?.totalIcpMargin || 0)} ICP</span>
      </span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Collateral Value</span>
      <span class="stat-value">${formatNumber(collateralValueUsd)}</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Total Borrowed</span>
      <span class="stat-value">{formatNumber(status?.totalIcusdBorrowed || 0)} icUSD</span>
    </div>
    {#if ckusdtReserve > 0 || ckusdcReserve > 0}
      <div class="stat-row">
        <span class="stat-label">Reserves</span>
        <span class="stat-value-stack">
          {#if ckusdtReserve > 0}
            <span>{formatNumber(ckusdtReserve, 2)} ckUSDT</span>
          {/if}
          {#if ckusdcReserve > 0}
            <span>{formatNumber(ckusdcReserve, 2)} ckUSDC</span>
          {/if}
        </span>
      </div>
    {/if}
  </div>

  <div class="group-divider"></div>

  <!-- Parameters -->
  <h4 class="group-heading">Parameters</h4>
  <div class="stats-stack">
    <div class="stat-row">
      <span class="stat-label">Min CR (ICP)</span>
      <span class="stat-value">{(minCR * 100).toFixed(0)}%</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Liquidation CR (ICP)</span>
      <span class="stat-value">{(liqCR * 100).toFixed(0)}%</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Recovery Threshold</span>
      <span class="stat-value">{(recoveryThreshold * 100).toFixed(0)}%</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Liq. Bonus</span>
      <span class="stat-value">{liqBonus > 0 ? `${formatNumber(liqBonus)}%` : '—'}</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Borrowing Fee</span>
      <span class="stat-value">{formatNumber(borrowFee)}%</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Interest APR</span>
      <span class="stat-value">{interestApr > 0 ? `${formatNumber(interestApr * 100)}%` : '0%'}</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Mode</span>
      <span class="stat-value"><span class="mode-badge {modeClass}">{modeLabel}</span></span>
    </div>
  </div>
</div>

<style>
  .protocol-stats {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.25rem;
  }
  .group-heading {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.625rem;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--rumi-text-muted);
    margin-bottom: 0.625rem;
  }
  .group-divider {
    height: 1px;
    background: var(--rumi-border);
    margin: 0.875rem 0;
    opacity: 0.5;
  }
  .stats-stack {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .stat-row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }
  .stat-label {
    font-size: 0.75rem;
    color: var(--rumi-text-secondary);
  }
  .stat-value {
    font-family: 'Inter', sans-serif;
    font-size: 0.8125rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
  }
  .stat-value-stack {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 0.125rem;
    font-family: 'Inter', sans-serif;
    font-size: 0.8125rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
  }
  .mode-badge {
    display: inline-block;
    font-size: 0.6875rem;
    font-weight: 600;
    padding: 0.125rem 0.5rem;
    border-radius: 9999px;
    letter-spacing: 0.02em;
  }
  .mode-normal {
    background: rgba(16, 185, 129, 0.15);
    color: #6ee7b7;
  }
  .mode-recovery {
    background: rgba(245, 158, 11, 0.15);
    color: #fbbf24;
  }
  .mode-other {
    background: rgba(107, 114, 128, 0.15);
    color: #9ca3af;
  }
</style>
