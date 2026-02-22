<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { formatNumber } from '$lib/utils/format';
  import { MINIMUM_CR, LIQUIDATION_CR } from '$lib/protocol';
  import { protocolService } from '$lib/services/protocol';

  export let protocolStatus: {
    mode: any;
    totalIcpMargin: number;
    totalIcusdBorrowed: number;
    lastIcpRate: number;
    lastIcpTimestamp: number;
    totalCollateralRatio: number;
    liquidationBonus: number;
    recoveryTargetCr: number;
  } | undefined = undefined;

  // Self-fetch fallback when no prop is provided
  let selfFetchedStatus: typeof protocolStatus;
  let refreshInterval: ReturnType<typeof setInterval>;

  async function fetchStatus() {
    try {
      const s = await protocolService.getProtocolStatus();
      selfFetchedStatus = {
        mode: s.mode || 'GeneralAvailability',
        totalIcpMargin: Number(s.totalIcpMargin || 0),
        totalIcusdBorrowed: Number(s.totalIcusdBorrowed || 0),
        lastIcpRate: Number(s.lastIcpRate || 0),
        lastIcpTimestamp: Number(s.lastIcpTimestamp || 0),
        totalCollateralRatio: Number(s.totalCollateralRatio || 0),
        liquidationBonus: Number(s.liquidationBonus || 0),
        recoveryTargetCr: Number(s.recoveryTargetCr || 0),
      };
    } catch (e) { console.error('ProtocolStats fetch error:', e); }
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
  $: liqBonus = (status?.liquidationBonus || 0) * 100;
</script>

<div class="protocol-stats">
  <!-- Market -->
  <h4 class="group-heading">Market</h4>
  <div class="stats-stack">
    <div class="stat-row">
      <span class="stat-label">ICP Price</span>
      <span class="stat-value">{icpPrice > 0 ? `$${formatNumber(icpPrice)}` : '—'}</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Protocol CR</span>
      <span class="stat-value">{formattedCR}%</span>
    </div>
  </div>

  <div class="group-divider"></div>

  <!-- System -->
  <h4 class="group-heading">System</h4>
  <div class="stats-stack">
    <div class="stat-row">
      <span class="stat-label">Total Collateral</span>
      <span class="stat-value">{formatNumber(status?.totalIcpMargin || 0)} ICP</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Collateral Value</span>
      <span class="stat-value">${formatNumber(collateralValueUsd)}</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Total Borrowed</span>
      <span class="stat-value">{formatNumber(status?.totalIcusdBorrowed || 0)} icUSD</span>
    </div>
  </div>

  <div class="group-divider"></div>

  <!-- Parameters -->
  <h4 class="group-heading">Parameters</h4>
  <div class="stats-stack">
    <div class="stat-row">
      <span class="stat-label">Min CR</span>
      <span class="stat-value">{(MINIMUM_CR * 100).toFixed(0)}%</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Liquidation CR</span>
      <span class="stat-value">{(LIQUIDATION_CR * 100).toFixed(0)}%</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Liq. Bonus</span>
      <span class="stat-value">{liqBonus > 0 ? `${formatNumber(liqBonus)}%` : '—'}</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Borrowing Fee</span>
      <span class="stat-value">0.5%</span>
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
