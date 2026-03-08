<script lang="ts">
  import type { PoolStatus } from '../../services/stabilityPoolService';
  import type { ProtocolStatusDTO } from '../../services/types';
  import { formatE8s, formatTokenAmount, symbolForLedger, decimalsForLedger } from '../../services/stabilityPoolService';
  import { formatStableTokenDisplay } from '../../utils/format';
  import { CANISTER_IDS } from '../../config';

  export let poolStatus: PoolStatus | null = null;
  export let protocolStatus: ProtocolStatusDTO | null = null;

  $: totalDepositsUsd = poolStatus ? formatStableTokenDisplay(poolStatus.total_deposits_e8s, 8) : '0.0000';
  $: depositorCount = poolStatus ? Number(poolStatus.total_depositors) : 0;
  $: liquidationCount = poolStatus ? Number(poolStatus.total_liquidations_executed) : 0;

  $: stablecoinBreakdown = poolStatus?.stablecoin_balances ?? [];
  $: collateralGains = poolStatus?.collateral_gains ?? [];
  $: stablecoinRegistry = poolStatus?.stablecoin_registry ?? [];
  $: collateralRegistry = poolStatus?.collateral_registry ?? [];

  $: poolApr = (() => {
    if (!protocolStatus || !poolStatus) return null;
    const weightedRate = protocolStatus.weightedAverageInterestRate;
    const poolShare = protocolStatus.interestPoolShare;
    const totalDebt = protocolStatus.totalIcusdBorrowed;
    const icusdEntry = poolStatus.stablecoin_balances.find(([l]: [any, bigint]) => l.toText() === CANISTER_IDS.ICUSD_LEDGER);
    const icusdTvl = icusdEntry ? Number(icusdEntry[1]) / 1e8 : 0;
    if (icusdTvl === 0 || totalDebt === 0 || weightedRate === 0) return null;
    const apr = (weightedRate * poolShare * totalDebt) / icusdTvl;
    return (apr * 100).toFixed(2);
  })();

  function getRegistries() {
    return { stablecoins: stablecoinRegistry, collateral: collateralRegistry };
  }

  function getCollateralAmount(ledgerId: any): bigint {
    const found = collateralGains.find(([l]) => l.toText() === ledgerId.toText());
    return found ? found[1] : 0n;
  }
</script>

<div class="stats-card">
  <!-- Primary stats row -->
  <div class="stats-row">
    <div class="stat">
      <span class="stat-label">TVL</span>
      <span class="stat-value"><span class="dollar">$</span>{totalDepositsUsd}</span>
    </div>
    <div class="stat-divider"></div>
    <div class="stat">
      <span class="stat-label">Depositors</span>
      <span class="stat-value">{depositorCount}</span>
    </div>
    <div class="stat-divider"></div>
    <div class="stat">
      <span class="stat-label">Liquidations</span>
      <span class="stat-value">{liquidationCount}</span>
    </div>
    {#if poolApr !== null}
      <div class="stat-divider"></div>
      <div class="stat">
        <span class="stat-label">Interest APR</span>
        <span class="stat-value">{poolApr}%</span>
      </div>
    {/if}
  </div>

  <!-- Deposits grid -->
  {#if stablecoinBreakdown.length > 0}
    <div class="grid-section">
      <div class="grid-header-row">
        <span class="grid-row-label">Deposits</span>
        {#each stablecoinBreakdown as [ledger]}
          <span class="grid-col-header">{symbolForLedger(ledger, getRegistries())}</span>
        {/each}
      </div>
      <div class="grid-data-row">
        <span class="grid-row-label"></span>
        {#each stablecoinBreakdown as [ledger, amount]}
          <span class="grid-cell">{formatStableTokenDisplay(amount, decimalsForLedger(ledger, getRegistries()))}</span>
        {/each}
      </div>
    </div>
  {/if}

  <!-- Collateral liquidated grid -->
  {#if collateralRegistry.length > 0}
    <div class="grid-section">
      <div class="grid-header-row">
        <span class="grid-row-label">Liquidated</span>
        {#each collateralRegistry as col}
          <span class="grid-col-header">{symbolForLedger(col.ledger_id, getRegistries())}</span>
        {/each}
      </div>
      <div class="grid-data-row">
        <span class="grid-row-label"></span>
        {#each collateralRegistry as col}
          {@const amount = getCollateralAmount(col.ledger_id)}
          {@const dec = decimalsForLedger(col.ledger_id, getRegistries())}
          <span class="grid-cell {amount > 0n ? 'teal' : ''}">{formatTokenAmount(amount, dec)}</span>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .stats-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.25rem 1.5rem;
    box-shadow:
      inset 0 1px 0 0 rgba(200, 210, 240, 0.03),
      0 2px 8px -2px rgba(8, 11, 22, 0.6);
  }

  .stats-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .stat { display: flex; flex-direction: column; gap: 0.25rem; flex: 1; }

  .stat-label {
    font-size: 0.625rem; font-weight: 500; text-transform: uppercase;
    letter-spacing: 0.08em; color: var(--rumi-text-muted); white-space: nowrap;
  }

  .stat-value {
    font-family: 'Inter', sans-serif; font-weight: 700; font-size: 1.125rem;
    font-variant-numeric: tabular-nums; color: var(--rumi-text-primary);
    line-height: 1.2; white-space: nowrap;
  }

  .dollar { color: var(--rumi-text-secondary); font-weight: 400; font-size: 0.875rem; margin-right: 0.0625rem; }
  .stat-divider { width: 1px; height: 2rem; background: var(--rumi-border); flex-shrink: 0; margin: 0 1rem; }
  .teal { color: var(--rumi-teal) !important; }

  .grid-section {
    margin-top: 1rem; padding-top: 1rem;
    border-top: 1px solid var(--rumi-border);
  }

  .grid-header-row, .grid-data-row { display: flex; align-items: center; }

  .grid-row-label {
    font-size: 0.625rem; font-weight: 500; text-transform: uppercase;
    letter-spacing: 0.06em; color: var(--rumi-text-muted);
    white-space: nowrap; min-width: 4.5rem; flex-shrink: 0;
  }

  .grid-col-header {
    flex: 1; text-align: center;
    font-size: 0.75rem; font-weight: 600; color: var(--rumi-text-secondary);
    padding: 0.25rem 0;
  }

  .grid-data-row {
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    padding: 0.5rem 0; margin-top: 0.25rem;
  }

  .grid-data-row .grid-row-label { padding-left: 0.75rem; }

  .grid-cell {
    flex: 1; text-align: center;
    font-family: 'Inter', sans-serif; font-size: 0.8125rem;
    font-weight: 600; font-variant-numeric: tabular-nums;
    color: var(--rumi-text-muted);
  }

  @media (max-width: 520px) {
    .stats-card { padding: 1rem 1.25rem; }
    .stat-divider { display: none; }
    .stats-row { flex-wrap: wrap; gap: 1rem; }
    .stat-value { font-size: 1rem; }
    .grid-row-label { min-width: 3.5rem; font-size: 0.5625rem; }
    .grid-col-header { font-size: 0.6875rem; }
    .grid-cell { font-size: 0.75rem; }
  }
</style>
