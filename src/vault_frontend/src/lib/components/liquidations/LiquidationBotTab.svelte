<script lang="ts">
  import { onMount } from 'svelte';
  import { ApiClient } from '../../services/protocol/apiClient';
  import LoadingSpinner from '../common/LoadingSpinner.svelte';
  import BotLiquidationsTable from './BotLiquidationsTable.svelte';

  interface BotStats {
    liquidation_bot_principal: string | null;
    budget_total_e8s: bigint;
    budget_remaining_e8s: bigint;
    budget_start_timestamp: bigint;
    total_debt_covered_e8s: bigint;
  }

  let loading = true;
  let error = '';
  let stats: BotStats | null = null;

  const E8S = 100_000_000;

  function formatE8s(val: bigint | number, decimals = 2): string {
    const n = Number(val) / E8S;
    return n.toLocaleString(undefined, { minimumFractionDigits: decimals, maximumFractionDigits: decimals });
  }

  function daysLeftInBudget(startNanos: bigint): number {
    const startMs = Number(startNanos) / 1_000_000;
    const endMs = startMs + 30 * 24 * 60 * 60 * 1000; // 30 days
    const remainingMs = endMs - Date.now();
    return Math.max(0, Math.ceil(remainingMs / (24 * 60 * 60 * 1000)));
  }

  function budgetPercent(remaining: bigint, total: bigint): number {
    if (Number(total) === 0) return 0;
    return Math.round(Number(remaining) * 100 / Number(total));
  }

  async function loadData() {
    try {
      loading = true;
      error = '';
      const raw = await ApiClient.getPublicData<any>('get_bot_stats');
      stats = {
        liquidation_bot_principal: raw.liquidation_bot_principal?.[0]?.toText() ?? null,
        budget_total_e8s: raw.budget_total_e8s,
        budget_remaining_e8s: raw.budget_remaining_e8s,
        budget_start_timestamp: raw.budget_start_timestamp,
        total_debt_covered_e8s: raw.total_debt_covered_e8s,
      };
    } catch (err: any) {
      console.error('Failed to load bot stats:', err);
      error = err.message || 'Failed to load bot stats';
    } finally {
      loading = false;
    }
  }

  onMount(() => { loadData(); });
</script>

<div class="bot-container">
  {#if loading}
    <div class="loading-state">
      <LoadingSpinner />
      <p class="loading-text">Loading bot data…</p>
    </div>
  {:else if error}
    <div class="error-state">
      <div class="error-icon">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="10"/>
          <line x1="12" y1="8" x2="12" y2="12"/>
          <line x1="12" y1="16" x2="12.01" y2="16"/>
        </svg>
      </div>
      <p class="error-text">{error}</p>
      <button class="btn-primary" on:click={loadData}>Try Again</button>
    </div>
  {:else if stats}
    <!-- Budget Status Card -->
    <div class="card budget-card">
      <h3 class="card-title">Monthly Budget</h3>
      <div class="budget-bar-container">
        <div class="budget-bar" style="width: {budgetPercent(stats.budget_remaining_e8s, stats.budget_total_e8s)}%"></div>
      </div>
      <div class="budget-details">
        <div class="budget-item">
          <span class="label">Remaining</span>
          <span class="value">{formatE8s(stats.budget_remaining_e8s)} icUSD</span>
        </div>
        <div class="budget-item">
          <span class="label">Total Budget</span>
          <span class="value">{formatE8s(stats.budget_total_e8s)} icUSD</span>
        </div>
        <div class="budget-item">
          <span class="label">Days Left</span>
          <span class="value">{daysLeftInBudget(stats.budget_start_timestamp)}</span>
        </div>
      </div>
    </div>

    <!-- All-time Stats -->
    <div class="card stats-card">
      <h3 class="card-title">All-Time Stats</h3>
      <div class="stats-grid">
        <div class="stat">
          <span class="stat-label">Total Debt Covered</span>
          <span class="stat-value">{formatE8s(stats.total_debt_covered_e8s)} icUSD</span>
        </div>
        <div class="stat">
          <span class="stat-label">Bot Canister</span>
          <span class="stat-value principal">{stats.liquidation_bot_principal ?? 'Not configured'}</span>
        </div>
      </div>
    </div>

    <!-- Event Log -->
    <div class="card events-card">
      <h3 class="card-title">Liquidation Events</h3>
      <BotLiquidationsTable />
    </div>
  {/if}
</div>

<style>
  .bot-container { max-width: 1100px; margin: 0 auto; }

  .loading-state, .error-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1rem;
    padding: 3rem;
  }
  .loading-text, .error-text { color: var(--rumi-text-muted); font-size: 0.875rem; }
  .error-icon { width: 2rem; height: 2rem; color: var(--rumi-error, #ef4444); }
  .btn-primary {
    background: var(--rumi-action);
    color: white;
    border: none;
    padding: 0.5rem 1rem;
    border-radius: 6px;
    cursor: pointer;
    font-size: 0.875rem;
  }

  .card {
    background: var(--rumi-card-bg, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--rumi-border, rgba(255, 255, 255, 0.06));
    border-radius: 12px;
    padding: 1.25rem;
    margin-bottom: 1rem;
  }
  .card-title {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-secondary, #9ca3af);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin: 0 0 1rem;
  }

  .budget-bar-container {
    width: 100%;
    height: 8px;
    background: var(--rumi-border, rgba(255, 255, 255, 0.06));
    border-radius: 4px;
    overflow: hidden;
    margin-bottom: 1rem;
  }
  .budget-bar {
    height: 100%;
    background: var(--rumi-action, #3b82f6);
    border-radius: 4px;
    transition: width 0.3s ease;
  }
  .budget-details {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 0.75rem;
  }
  .budget-item {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .label {
    font-size: 0.75rem;
    color: var(--rumi-text-muted, #6b7280);
  }
  .value {
    font-size: 0.9375rem;
    font-weight: 500;
    color: var(--rumi-text-primary, #fff);
  }

  .stats-grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 1rem;
  }
  .stat { display: flex; flex-direction: column; gap: 0.25rem; }
  .stat-label {
    font-size: 0.75rem;
    color: var(--rumi-text-muted, #6b7280);
  }
  .stat-value {
    font-size: 0.9375rem;
    font-weight: 500;
    color: var(--rumi-text-primary, #fff);
  }
  .stat-value.principal {
    font-size: 0.75rem;
    font-family: monospace;
    word-break: break-all;
  }
</style>
