<script lang="ts">
  import {
    formatTokenAmount,
    formatE8s,
    symbolForLedger,
    decimalsForLedger,
  } from '../../services/stabilityPoolService';
  import type { PoolStatus, LiquidationRecord } from '../../services/stabilityPoolService';

  export let poolStatus: PoolStatus | null = null;
  export let liquidationHistory: LiquidationRecord[] = [];

  $: stablecoinRegistry = poolStatus?.stablecoin_registry ?? [];
  $: collateralRegistry = poolStatus?.collateral_registry ?? [];
  $: registries = { stablecoins: stablecoinRegistry, collateral: collateralRegistry };
  $: liquidationCount = poolStatus ? Number(poolStatus.total_liquidations_executed) : 0;
  $: hasLiquidations = liquidationHistory.length > 0;
  $: displayRecords = liquidationHistory.slice(0, 20);

  function formatTimestamp(ts: bigint): string {
    const date = new Date(Number(ts) / 1_000_000);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMin = Math.floor(diffMs / 60_000);
    const diffHr = Math.floor(diffMs / 3_600_000);
    const diffDay = Math.floor(diffMs / 86_400_000);
    if (diffMin < 1) return 'just now';
    if (diffMin < 60) return `${diffMin}m ago`;
    if (diffHr < 24) return `${diffHr}h ago`;
    if (diffDay < 7) return `${diffDay}d ago`;
    return date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
  }
</script>

<div class="liq-card">
  {#if !hasLiquidations}
    <!-- Compact single-line empty state -->
    <div class="liq-empty">
      <span class="liq-title">Liquidation History</span>
      <span class="liq-none">No liquidations processed yet</span>
    </div>
  {:else}
    <!-- Expanded state with liquidation entries -->
    <div class="liq-header">
      <span class="liq-title">Liquidation History</span>
      <span class="liq-count">{liquidationCount}</span>
    </div>

    <div class="liq-list">
      {#each displayRecords as record, i}
        {@const collSym = symbolForLedger(record.collateral_type, registries)}
        {@const collDec = decimalsForLedger(record.collateral_type, registries)}
        <div class="liq-item" style="animation-delay: {i * 40}ms">
          <div class="liq-item-left">
            <div class="liq-badge">{collSym}</div>
            <div class="liq-details">
              <div class="liq-primary">
                Vault #{Number(record.vault_id)}
                <span class="liq-gained">+{formatTokenAmount(record.collateral_gained, collDec)} {collSym}</span>
              </div>
              <div class="liq-secondary">
                {#each record.stables_consumed as [ledger, amount]}
                  {@const stableSym = symbolForLedger(ledger, registries)}
                  {@const stableDec = decimalsForLedger(ledger, registries)}
                  <span class="consumed-chip">
                    −{formatTokenAmount(amount, stableDec)} {stableSym}
                  </span>
                {/each}
                <span class="liq-depositors">{Number(record.depositors_count)} depositors</span>
              </div>
            </div>
          </div>
          <div class="liq-time">{formatTimestamp(record.timestamp)}</div>
        </div>
      {/each}
    </div>

    {#if liquidationHistory.length > 20}
      <div class="liq-footer">
        Showing 20 of {liquidationHistory.length} events
      </div>
    {/if}
  {/if}
</div>

<style>
  .liq-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1rem 1.25rem;
  }

  /* ── Empty state (compact single-line) ── */
  .liq-empty {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .liq-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
  }

  .liq-none {
    font-size: 0.75rem;
    font-style: italic;
    color: var(--rumi-text-muted);
  }

  /* ── Expanded state ── */
  .liq-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 0.75rem;
  }

  .liq-count {
    font-size: 0.6875rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    padding: 0.125rem 0.5rem;
    background: var(--rumi-bg-surface2);
    border-radius: 1rem;
  }

  /* ── Feed items ── */
  .liq-list {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
  }

  .liq-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.625rem 0.75rem;
    background: var(--rumi-bg-surface2);
    border-radius: 0.5rem;
    border: 1px solid transparent;
    transition: border-color 0.15s ease;
    animation: fadeSlide 0.3s ease-out both;
  }

  @keyframes fadeSlide {
    from { opacity: 0; transform: translateY(4px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .liq-item:hover { border-color: var(--rumi-border); }

  .liq-item-left {
    display: flex;
    align-items: center;
    gap: 0.625rem;
    min-width: 0;
    flex: 1;
  }

  .liq-badge {
    width: 2rem;
    height: 2rem;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(45, 212, 191, 0.08);
    border: 1px solid rgba(45, 212, 191, 0.15);
    border-radius: 0.375rem;
    font-size: 0.5625rem;
    font-weight: 700;
    color: var(--rumi-teal);
    flex-shrink: 0;
    letter-spacing: -0.02em;
  }

  .liq-details { min-width: 0; flex: 1; }

  .liq-primary {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    margin-bottom: 0.125rem;
  }

  .liq-gained {
    font-size: 0.6875rem;
    font-weight: 600;
    color: var(--rumi-teal);
    font-variant-numeric: tabular-nums;
  }

  .liq-secondary {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.25rem;
  }

  .consumed-chip {
    font-size: 0.625rem;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-muted);
    padding: 0.0625rem 0.3125rem;
    background: var(--rumi-bg-surface3);
    border-radius: 0.25rem;
  }

  .liq-depositors {
    font-size: 0.625rem;
    color: var(--rumi-text-muted);
  }

  .liq-time {
    font-size: 0.625rem;
    color: var(--rumi-text-muted);
    white-space: nowrap;
    margin-left: 0.5rem;
    flex-shrink: 0;
  }

  .liq-footer {
    margin-top: 0.625rem;
    text-align: center;
    font-size: 0.625rem;
    color: var(--rumi-text-muted);
    padding-top: 0.625rem;
    border-top: 1px solid var(--rumi-border);
  }

  @media (max-width: 520px) {
    .liq-primary { flex-direction: column; align-items: flex-start; gap: 0.125rem; }
    .liq-item { flex-direction: column; align-items: flex-start; gap: 0.375rem; }
    .liq-time { margin-left: 0; }
  }
</style>
