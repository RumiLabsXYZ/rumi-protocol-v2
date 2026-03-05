<script lang="ts">
  import type { PoolStatus } from '../../services/stabilityPoolService';
  import { formatE8s, formatTokenAmount, symbolForLedger, decimalsForLedger } from '../../services/stabilityPoolService';

  export let poolStatus: PoolStatus | null = null;

  $: totalDepositsUsd = poolStatus ? formatE8s(poolStatus.total_deposits_e8s) : '0';
  $: depositorCount = poolStatus ? Number(poolStatus.total_depositors) : 0;
  $: liquidationCount = poolStatus ? Number(poolStatus.total_liquidations_executed) : 0;
  $: isPaused = poolStatus?.emergency_paused ?? false;

  $: collateralGains = poolStatus?.collateral_gains ?? [];
  $: stablecoinBreakdown = poolStatus?.stablecoin_balances ?? [];
  $: stablecoinRegistry = poolStatus?.stablecoin_registry ?? [];
  $: collateralRegistry = poolStatus?.collateral_registry ?? [];

  function getRegistries() {
    return { stablecoins: stablecoinRegistry, collateral: collateralRegistry };
  }
</script>

<div class="pool-overview">
  <div class="metrics-grid">
    <div class="metric-card hero">
      <div class="metric-eyebrow">Total Value Locked</div>
      <div class="metric-value">
        <span class="dollar-sign">$</span>{totalDepositsUsd}
      </div>
      <div class="metric-sub">
        {#each stablecoinBreakdown as [ledger, amount]}
          <span class="token-chip">
            {symbolForLedger(ledger, getRegistries())}
            <span class="chip-amount">{formatTokenAmount(amount, decimalsForLedger(ledger, getRegistries()))}</span>
          </span>
        {/each}
      </div>
    </div>

    <div class="metric-card">
      <div class="metric-eyebrow">Depositors</div>
      <div class="metric-value">{depositorCount}</div>
      <div class="metric-sub">{depositorCount === 1 ? 'active position' : 'active positions'}</div>
    </div>

    <div class="metric-card">
      <div class="metric-eyebrow">Liquidations</div>
      <div class="metric-value">{liquidationCount}</div>
      <div class="metric-sub">
        {#if liquidationCount > 0}
          collateral distributed
        {:else}
          no events yet
        {/if}
      </div>
    </div>

    <div class="metric-card" class:paused={isPaused}>
      <div class="metric-eyebrow">Pool Status</div>
      <div class="metric-value status-value">
        {#if isPaused}
          <span class="status-dot danger"></span> Paused
        {:else}
          <span class="status-dot active"></span> Active
        {/if}
      </div>
      <div class="metric-sub">
        {stablecoinRegistry.filter(s => s.is_active).length} stablecoins ·
        {collateralRegistry.filter(c => 'Active' in c.status).length} collateral types
      </div>
    </div>
  </div>

  {#if collateralGains.some(([_, a]) => a > 0n)}
    <div class="gains-ribbon">
      <div class="ribbon-label">Pool Collateral Gains</div>
      <div class="ribbon-items">
        {#each collateralGains as [ledger, amount]}
          {@const sym = symbolForLedger(ledger, getRegistries())}
          {@const dec = decimalsForLedger(ledger, getRegistries())}
          {#if amount > 0n}
            <div class="ribbon-item">
              <span class="ribbon-symbol">{sym}</span>
              <span class="ribbon-amount">{formatTokenAmount(amount, dec)}</span>
            </div>
          {/if}
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .pool-overview { width: 100%; }

  .metrics-grid {
    display: grid;
    grid-template-columns: 1.6fr repeat(3, 1fr);
    gap: 1rem;
    margin-bottom: 1rem;
  }

  .metric-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.25rem 1.5rem;
    transition: border-color 0.2s ease, box-shadow 0.2s ease;
    box-shadow:
      inset 0 1px 0 0 rgba(200, 210, 240, 0.03),
      0 2px 8px -2px rgba(8, 11, 22, 0.6);
  }

  .metric-card:hover { border-color: var(--rumi-border-hover); }

  .metric-card.hero {
    border-color: rgba(45, 212, 191, 0.15);
    background: linear-gradient(135deg, var(--rumi-bg-surface1) 0%, rgba(45, 212, 191, 0.03) 100%);
  }

  .metric-card.paused { border-color: rgba(224, 107, 159, 0.2); }

  .metric-eyebrow {
    font-size: 0.6875rem; font-weight: 500; text-transform: uppercase;
    letter-spacing: 0.08em; color: var(--rumi-text-secondary); margin-bottom: 0.5rem;
  }

  .metric-value {
    font-family: 'Inter', sans-serif; font-weight: 700; font-size: 1.75rem;
    font-variant-numeric: tabular-nums; color: var(--rumi-text-primary);
    line-height: 1.1; margin-bottom: 0.5rem;
  }

  .dollar-sign { color: var(--rumi-text-secondary); font-weight: 400; font-size: 1.25rem; margin-right: 0.125rem; }

  .status-value { display: flex; align-items: center; gap: 0.5rem; font-size: 1.375rem; }

  .status-dot { width: 0.5rem; height: 0.5rem; border-radius: 50%; flex-shrink: 0; }
  .status-dot.active { background: var(--rumi-teal); box-shadow: 0 0 8px rgba(45, 212, 191, 0.4); }
  .status-dot.danger { background: var(--rumi-danger); box-shadow: 0 0 8px rgba(224, 107, 159, 0.4); }

  .metric-sub {
    font-size: 0.75rem; color: var(--rumi-text-muted);
    display: flex; flex-wrap: wrap; gap: 0.375rem; align-items: center;
  }

  .token-chip {
    display: inline-flex; align-items: center; gap: 0.25rem;
    padding: 0.125rem 0.5rem; background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border); border-radius: 1rem;
    font-size: 0.6875rem; color: var(--rumi-text-secondary); white-space: nowrap;
  }

  .chip-amount { color: var(--rumi-text-primary); font-weight: 600; font-variant-numeric: tabular-nums; }

  .gains-ribbon {
    display: flex; align-items: center; gap: 1rem;
    padding: 0.75rem 1.25rem; background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border); border-radius: 0.75rem;
    box-shadow: inset 0 1px 0 0 rgba(200, 210, 240, 0.03);
  }

  .ribbon-label {
    font-size: 0.6875rem; font-weight: 500; text-transform: uppercase;
    letter-spacing: 0.06em; color: var(--rumi-text-muted); white-space: nowrap;
  }

  .ribbon-items { display: flex; flex-wrap: wrap; gap: 0.75rem; flex: 1; }
  .ribbon-item { display: flex; align-items: center; gap: 0.375rem; }
  .ribbon-symbol { font-size: 0.75rem; font-weight: 500; color: var(--rumi-text-secondary); }
  .ribbon-amount { font-size: 0.875rem; font-weight: 600; font-variant-numeric: tabular-nums; color: var(--rumi-teal); }

  @media (max-width: 900px) {
    .metrics-grid { grid-template-columns: 1fr 1fr; }
    .metric-card.hero { grid-column: 1 / -1; }
  }

  @media (max-width: 520px) {
    .metrics-grid { grid-template-columns: 1fr; }
    .metric-value { font-size: 1.5rem; }
    .gains-ribbon { flex-direction: column; align-items: flex-start; gap: 0.5rem; }
  }
</style>
