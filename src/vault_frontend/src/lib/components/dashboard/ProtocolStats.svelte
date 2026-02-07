<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { protocolService } from '$lib/services/protocol';
  import { formatNumber } from '$lib/utils/format';
  
  let protocolStatus = {
    mode: 'GeneralAvailability',
    totalIcpMargin: 0,
    totalIcusdBorrowed: 0,
    lastIcpRate: 0,
    lastIcpTimestamp: 0,
    totalCollateralRatio: 0
  };
  
  let isLoading = true;
  let refreshInterval: NodeJS.Timeout;
  
  async function fetchProtocolStatus() {
    isLoading = true;
    try {
      const status = await protocolService.getProtocolStatus();
      protocolStatus = {
        mode: status.mode || 'GeneralAvailability',
        totalIcpMargin: Number(status.totalIcpMargin || 0),
        totalIcusdBorrowed: Number(status.totalIcusdBorrowed || 0),
        lastIcpRate: Number(status.lastIcpRate || 0),
        lastIcpTimestamp: Number(status.lastIcpTimestamp || 0),
        totalCollateralRatio: Number(status.totalCollateralRatio || 0)
      };
      if (protocolStatus.lastIcpRate === 0) {
        protocolStatus.lastIcpRate = 10.0;
        protocolStatus.lastIcpTimestamp = Date.now() * 1000000;
      }
    } catch (error) {
      console.error('Error fetching protocol status:', error);
      protocolStatus = {
        mode: 'GeneralAvailability',
        totalIcpMargin: 0,
        totalIcusdBorrowed: 0,
        lastIcpRate: 10.0,
        lastIcpTimestamp: Date.now() * 1000000,
        totalCollateralRatio: 0
      };
    } finally {
      isLoading = false;
    }
  }
  
  onMount(() => {
    fetchProtocolStatus();
    refreshInterval = setInterval(fetchProtocolStatus, 15000);
    return () => { if (refreshInterval) clearInterval(refreshInterval); };
  });
  
  onDestroy(() => {
    if (refreshInterval) clearInterval(refreshInterval);
  });
  
  $: icpValueInUsd = protocolStatus.totalIcpMargin * protocolStatus.lastIcpRate;
  $: collateralPercent = protocolStatus.totalIcusdBorrowed > 0
    ? protocolStatus.totalCollateralRatio * 100
    : protocolStatus.totalIcpMargin > 0 ? Infinity : 0;
  $: formattedCollateralPercent = collateralPercent === Infinity 
    ? '∞' : collateralPercent > 1000000 ? '>1M' : formatNumber(collateralPercent);
</script>

<div class="protocol-stats">
  <h3 class="stats-heading">Protocol</h3>
  <div class="stats-stack">
    <div class="stat-row">
      <span class="stat-label">Total Collateral</span>
      <span class="stat-value">{formatNumber(protocolStatus.totalIcpMargin)} ICP</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Total Borrowed</span>
      <span class="stat-value">{formatNumber(protocolStatus.totalIcusdBorrowed)} icUSD</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Collateral Ratio</span>
      <span class="stat-value">{formattedCollateralPercent}%</span>
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
  .stats-heading {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.6875rem;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--rumi-text-muted);
    margin-bottom: 1rem;
  }
  .stats-stack {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .stat-row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }
  .stat-label {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
  }
  .stat-value {
    font-family: 'Inter', sans-serif;
    font-size: 0.875rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
  }
</style>
