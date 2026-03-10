<script lang="ts" context="module">
  import type { PoolStatus } from '../../lib/services/threePoolService';
  let _poolStatus: PoolStatus | null = null;
  let _userLpBalance: bigint | null = null;
  let _cachedForPrincipal: string | null = null;
</script>

<script lang="ts">
  import { onMount } from 'svelte';
  import { walletStore } from '../../lib/stores/wallet';
  import { threePoolService } from '../../lib/services/threePoolService';
  import SwapInterface from '../../lib/components/swap/SwapInterface.svelte';
  import LiquidityInterface from '../../lib/components/swap/LiquidityInterface.svelte';
  import PoolInfoCard from '../../lib/components/swap/PoolInfoCard.svelte';
  import LoadingSpinner from '../../lib/components/common/LoadingSpinner.svelte';

  type PageTab = 'swap' | 'liquidity';
  let activePageTab: PageTab = 'swap';

  let hasCachedData = _poolStatus !== null;
  let loading = !hasCachedData;
  let error = '';
  let poolStatus: PoolStatus | null = _poolStatus;
  let userLpBalance: bigint = _userLpBalance ?? 0n;

  $: isConnected = $walletStore.isConnected;
  $: principal = $walletStore.principal;

  async function loadAllData() {
    try {
      if (!hasCachedData) loading = true;
      error = '';

      const status = await threePoolService.getPoolStatus();
      poolStatus = status;
      _poolStatus = status;

      // Load user LP balance if connected
      if (isConnected && principal) {
        const lp = await threePoolService.getLpBalance(principal).catch(() => 0n);
        userLpBalance = lp;
        _userLpBalance = lp;
        _cachedForPrincipal = principal.toString();
      } else {
        userLpBalance = 0n;
        _userLpBalance = null;
        _cachedForPrincipal = null;
      }

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

  // Reload on wallet connection changes
  let previousConnected = false;
  $: if (isConnected !== previousConnected) {
    previousConnected = isConnected;
    if (!isConnected || principal?.toString() !== _cachedForPrincipal) {
      _userLpBalance = null;
      _cachedForPrincipal = null;
      userLpBalance = 0n;
    }
    loadAllData();
  }

  onMount(() => { loadAllData(); });

  function handleSuccess() {
    loadAllData();
    walletStore.refreshBalance();
  }
</script>

<svelte:head>
  <title>Swap — Stablecoin Exchange | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <div class="page-header">
    <h1 class="page-title">Stablecoin Exchange</h1>
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
      <button class="btn-primary" on:click={loadAllData}>Try Again</button>
    </div>
  {:else}
    <div class="page-layout">
      <!-- LEFT: Info card (280px, sticky) -->
      <div class="stats-column">
        <PoolInfoCard {poolStatus} {userLpBalance} />
      </div>

      <!-- RIGHT: Swap/Liquidity panel -->
      <div class="action-column">
        <div class="action-panel">
          <!-- Tab switcher (matches DepositInterface style) -->
          <div class="tab-bar">
            <button
              class="tab" class:active={activePageTab === 'swap'}
              on:click={() => { activePageTab = 'swap'; }}
            >Swap</button>
            <button
              class="tab" class:active={activePageTab === 'liquidity'}
              on:click={() => { activePageTab = 'liquidity'; }}
            >Liquidity</button>
            <div class="tab-indicator" class:right={activePageTab === 'liquidity'}></div>
          </div>

          {#if activePageTab === 'swap'}
            <SwapInterface on:success={handleSuccess} />
          {:else}
            <LiquidityInterface on:success={handleSuccess} />
          {/if}
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .page-container { max-width: 820px; margin: 0 auto; padding-bottom: 4rem; }

  /* ── Page header ── */
  .page-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 1.75rem;
    animation: fadeSlideIn 0.5s ease-out both;
    position: relative;
    z-index: 10;
  }

  @keyframes fadeSlideIn {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
  }

  /* ── Two-column layout (matches Earn page exactly) ── */
  .page-layout {
    display: grid;
    grid-template-columns: 280px 1fr;
    gap: 1.5rem;
    align-items: start;
    animation: fadeSlideIn 0.5s ease-out 0.05s both;
  }

  .stats-column { position: sticky; top: 5rem; }

  .action-column {
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 1rem;
  }

  .action-column > :global(*) { width: 100%; max-width: 420px; }

  /* ── Action panel (wraps tab-bar + swap/liquidity content) ── */
  .action-panel {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
    box-shadow:
      inset 0 1px 0 0 rgba(200, 210, 240, 0.03),
      0 2px 8px -2px rgba(8, 11, 22, 0.6);
  }

  /* ── Tab bar (matches DepositInterface exactly) ── */
  .tab-bar {
    position: relative;
    display: flex;
    background: var(--rumi-bg-surface2);
    border-radius: 0.5rem;
    padding: 0.1875rem;
    margin-bottom: 1.5rem;
  }

  .tab {
    flex: 1;
    padding: 0.5rem 1rem;
    background: none;
    border: none;
    border-radius: 0.375rem;
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    cursor: pointer;
    transition: color 0.2s ease;
    position: relative;
    z-index: 1;
  }

  .tab.active { color: var(--rumi-text-primary); }

  .tab-indicator {
    position: absolute;
    top: 0.1875rem;
    left: 0.1875rem;
    width: calc(50% - 0.1875rem);
    height: calc(100% - 0.375rem);
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border-hover);
    border-radius: 0.375rem;
    transition: transform 0.25s cubic-bezier(0.4, 0, 0.2, 1);
    z-index: 0;
  }

  .tab-indicator.right {
    transform: translateX(100%);
  }

  /* ── Loading & error states ── */
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

  /* ── Responsive ── */
  @media (max-width: 768px) {
    .page-layout { grid-template-columns: 1fr; }
    .stats-column { position: static; order: 2; }
    .action-column { order: 1; }
  }

  @media (max-width: 520px) {
    .page-container {
      padding-left: 0.5rem;
      padding-right: 0.5rem;
    }
  }
</style>
