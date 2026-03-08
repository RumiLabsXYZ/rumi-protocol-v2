<script lang="ts">
  import {
    formatTokenAmount,
    symbolForLedger,
    decimalsForLedger,
  } from '../../services/stabilityPoolService';
  import { formatStableTokenTx } from '../../utils/format';
  import type { PoolStatus, LiquidationRecord } from '../../services/stabilityPoolService';

  export let poolStatus: PoolStatus | null = null;
  export let liquidationHistory: LiquidationRecord[] = [];

  $: stablecoinRegistry = poolStatus?.stablecoin_registry ?? [];
  $: collateralRegistry = poolStatus?.collateral_registry ?? [];
  $: registries = { stablecoins: stablecoinRegistry, collateral: collateralRegistry };

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

<div class="liquidation-feed">
  <div class="feed-header">
    <h3 class="feed-title">Liquidation History</h3>
    <span class="feed-count">{liquidationHistory.length} events</span>
  </div>

  {#if displayRecords.length === 0}
    <div class="empty-state">
      <div class="empty-icon">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="10"/>
          <path d="M8 12h8"/>
        </svg>
      </div>
      <p class="empty-text">No liquidations have been processed yet</p>
      <p class="empty-sub">Liquidation events will appear here when vault positions are absorbed by the pool</p>
    </div>
  {:else}
    <div class="feed-list">
      {#each displayRecords as record, i}
        {@const collSym = symbolForLedger(record.collateral_type, registries)}
        {@const collDec = decimalsForLedger(record.collateral_type, registries)}
        <div class="feed-item" style="animation-delay: {i * 40}ms">
          <div class="item-left">
            <div class="item-badge">{collSym}</div>
            <div class="item-details">
              <div class="item-primary">
                Vault #{Number(record.vault_id)}
                <span class="item-gained">+{formatTokenAmount(record.collateral_gained, collDec)} {collSym}</span>
              </div>
              <div class="item-secondary">
                {#each record.stables_consumed as [ledger, amount]}
                  {@const stableSym = symbolForLedger(ledger, registries)}
                  {@const stableDec = decimalsForLedger(ledger, registries)}
                  <span class="consumed-chip">
                    −{formatStableTokenTx(amount, stableDec)} {stableSym}
                  </span>
                {/each}
                <span class="item-depositors">{Number(record.depositors_count)} depositors</span>
              </div>
            </div>
          </div>
          <div class="item-time">{formatTimestamp(record.timestamp)}</div>
        </div>
      {/each}
    </div>

    {#if liquidationHistory.length > 20}
      <div class="feed-footer">
        Showing 20 of {liquidationHistory.length} events
      </div>
    {/if}
  {/if}
</div>

<style>
  .liquidation-feed {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
    box-shadow: inset 0 1px 0 0 rgba(200, 210, 240, 0.03), 0 2px 8px -2px rgba(8, 11, 22, 0.6);
  }

  .feed-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 1.25rem;
  }

  .feed-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
  }

  .feed-count {
    font-size: 0.6875rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    padding: 0.125rem 0.5rem;
    background: var(--rumi-bg-surface2);
    border-radius: 1rem;
  }

  /* ── Empty state ── */
  .empty-state {
    text-align: center;
    padding: 2.5rem 1rem;
  }

  .empty-icon {
    width: 2.5rem;
    height: 2.5rem;
    color: var(--rumi-text-muted);
    margin: 0 auto 0.75rem;
    opacity: 0.5;
  }

  .empty-text {
    font-size: 0.875rem;
    color: var(--rumi-text-secondary);
    margin-bottom: 0.25rem;
  }

  .empty-sub {
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
    max-width: 320px;
    margin: 0 auto;
    line-height: 1.5;
  }

  /* ── Feed list ── */
  .feed-list {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
  }

  .feed-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.75rem;
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

  .feed-item:hover {
    border-color: var(--rumi-border);
  }

  .item-left {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    min-width: 0;
    flex: 1;
  }

  .item-badge {
    width: 2.25rem;
    height: 2.25rem;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(45, 212, 191, 0.08);
    border: 1px solid rgba(45, 212, 191, 0.15);
    border-radius: 0.5rem;
    font-size: 0.625rem;
    font-weight: 700;
    color: var(--rumi-teal);
    flex-shrink: 0;
    letter-spacing: -0.02em;
  }

  .item-details {
    min-width: 0;
    flex: 1;
  }

  .item-primary {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    margin-bottom: 0.25rem;
  }

  .item-gained {
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--rumi-teal);
    font-variant-numeric: tabular-nums;
  }

  .item-secondary {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.375rem;
  }

  .consumed-chip {
    font-size: 0.6875rem;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-muted);
    padding: 0.0625rem 0.375rem;
    background: var(--rumi-bg-surface3);
    border-radius: 0.25rem;
  }

  .item-depositors {
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
  }

  .item-time {
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
    white-space: nowrap;
    margin-left: 0.75rem;
    flex-shrink: 0;
  }

  /* ── Footer ── */
  .feed-footer {
    margin-top: 0.75rem;
    text-align: center;
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
    padding-top: 0.75rem;
    border-top: 1px solid var(--rumi-border);
  }

  @media (max-width: 520px) {
    .item-primary { flex-direction: column; align-items: flex-start; gap: 0.125rem; }
    .feed-item { flex-direction: column; align-items: flex-start; gap: 0.5rem; }
    .item-time { margin-left: 0; }
  }
</style>
