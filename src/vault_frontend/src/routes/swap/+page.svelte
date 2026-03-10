<script lang="ts" context="module">
  import type { PoolStatus } from '../../lib/services/threePoolService';
  let _poolStatus: PoolStatus | null = null;
</script>

<script lang="ts">
  import { onMount } from 'svelte';
  import { walletStore } from '../../lib/stores/wallet';
  import { threePoolService, POOL_TOKENS, formatTokenAmount } from '../../lib/services/threePoolService';
  import SwapInterface from '../../lib/components/swap/SwapInterface.svelte';
  import LiquidityInterface from '../../lib/components/swap/LiquidityInterface.svelte';
  import LoadingSpinner from '../../lib/components/common/LoadingSpinner.svelte';

  type PageTab = 'swap' | 'liquidity';
  let activePageTab: PageTab = 'swap';

  let hasCachedData = _poolStatus !== null;
  let loading = !hasCachedData;
  let error = '';
  let poolStatus: PoolStatus | null = _poolStatus;

  $: isConnected = $walletStore.isConnected;

  $: pageTitle = activePageTab === 'swap' ? 'Swap' : 'Liquidity';
  $: pageSubtitle = activePageTab === 'swap'
    ? 'Exchange stablecoins at low slippage'
    : 'Add or remove pool liquidity';

  async function loadPoolData() {
    try {
      if (!hasCachedData) loading = true;
      error = '';
      const status = await threePoolService.getPoolStatus();
      poolStatus = status;
      _poolStatus = status;
      hasCachedData = true;
    } catch (err: any) {
      console.error('Failed to load 3pool data:', err);
      if (!hasCachedData) {
        error = err.message || 'Failed to load pool data';
      }
    } finally {
      loading = false;
    }
  }

  onMount(() => { loadPoolData(); });

  function handleSuccess() {
    loadPoolData();
    walletStore.refreshBalance();
  }
</script>

<svelte:head>
  <title>{pageTitle} — Stablecoin Exchange | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <div class="page-header">
    <h1 class="page-title">{pageTitle}</h1>
    <span class="page-subtitle">{pageSubtitle}</span>
  </div>

  <!-- Top-level tab switcher -->
  <div class="page-tabs">
    <button class="page-tab" class:active={activePageTab === 'swap'} on:click={() => { activePageTab = 'swap'; }}>
      Swap
    </button>
    <button class="page-tab" class:active={activePageTab === 'liquidity'} on:click={() => { activePageTab = 'liquidity'; }}>
      Liquidity
    </button>
  </div>

  {#if loading}
    <div class="loading-state">
      <LoadingSpinner />
      <p class="loading-text">Loading pool data…</p>
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
      <button class="btn-primary" on:click={loadPoolData}>Try Again</button>
    </div>
  {:else}
    <!-- Pool balance summary (shown on Liquidity tab) -->
    {#if activePageTab === 'liquidity' && poolStatus}
      <div class="pool-summary">
        <div class="pool-summary-title">Pool Balances</div>
        <div class="pool-summary-grid">
          {#each POOL_TOKENS as token, i}
            <div class="pool-balance-item">
              <span class="token-dot" style="background:{token.color}"></span>
              <span class="pool-balance-symbol">{token.symbol}</span>
              <span class="pool-balance-amount">{formatTokenAmount(poolStatus.balances[i], token.decimals)}</span>
            </div>
          {/each}
        </div>
      </div>
    {/if}

    <div class="swap-layout">
      {#if activePageTab === 'swap'}
        <SwapInterface on:success={handleSuccess} />
      {:else}
        <LiquidityInterface on:success={handleSuccess} />
      {/if}
    </div>
  {/if}
</div>

<style>
  .page-container { max-width: 820px; margin: 0 auto; padding-bottom: 4rem; }

  .page-header {
    display: flex;
    align-items: baseline;
    gap: 0.75rem;
    margin-bottom: 1rem;
    animation: fadeSlideIn 0.5s ease-out both;
  }

  .page-subtitle {
    font-size: 0.8125rem;
    color: var(--rumi-text-muted);
  }

  @keyframes fadeSlideIn {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
  }

  /* ── Page-level tabs ── */
  .page-tabs {
    display: flex;
    justify-content: center;
    gap: 0;
    margin-bottom: 1.5rem;
    animation: fadeSlideIn 0.5s ease-out 0.02s both;
  }

  .page-tab {
    padding: 0.5rem 1.25rem;
    background: transparent;
    border: 1px solid var(--rumi-border);
    font-size: 0.8125rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .page-tab:first-child {
    border-radius: 0.5rem 0 0 0.5rem;
    border-right: none;
  }

  .page-tab:last-child {
    border-radius: 0 0.5rem 0.5rem 0;
  }

  .page-tab.active {
    background: var(--rumi-bg-surface1);
    color: var(--rumi-teal);
    border-color: var(--rumi-teal);
    font-weight: 600;
  }

  .page-tab:hover:not(.active) {
    color: var(--rumi-text-secondary);
    border-color: var(--rumi-border-hover);
  }

  /* ── Pool summary ── */
  .pool-summary {
    max-width: 420px;
    margin: 0 auto 1rem;
    padding: 0.75rem 1rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    animation: fadeSlideIn 0.5s ease-out 0.04s both;
  }

  .pool-summary-title {
    font-size: 0.6875rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    margin-bottom: 0.5rem;
  }

  .pool-summary-grid {
    display: flex;
    gap: 1rem;
    flex-wrap: wrap;
  }

  .pool-balance-item {
    display: flex;
    align-items: center;
    gap: 0.375rem;
  }

  .token-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .pool-balance-symbol {
    font-size: 0.75rem;
    color: var(--rumi-text-secondary);
    font-weight: 500;
  }

  .pool-balance-amount {
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    font-variant-numeric: tabular-nums;
  }

  .swap-layout {
    display: flex;
    justify-content: center;
    animation: fadeSlideIn 0.5s ease-out 0.05s both;
  }

  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 5rem;
    color: var(--rumi-text-secondary);
  }

  .loading-text {
    margin-top: 1rem;
    font-size: 0.875rem;
  }

  .error-state {
    text-align: center;
    padding: 4rem 1rem;
  }

  .error-icon {
    width: 2.5rem;
    height: 2.5rem;
    color: var(--rumi-danger);
    margin: 0 auto 1rem;
  }

  .error-text {
    font-size: 0.875rem;
    color: var(--rumi-danger);
    margin-bottom: 1.5rem;
  }
</style>
