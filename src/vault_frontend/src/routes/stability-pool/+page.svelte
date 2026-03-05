<script lang="ts">
  import { onMount } from 'svelte';
  import { walletStore } from '../../lib/stores/wallet';
  import { stabilityPoolService } from '../../lib/services/stabilityPoolService';
  import type { PoolStatus, UserPosition, LiquidationRecord } from '../../lib/services/stabilityPoolService';
  import PoolStats from '../../lib/components/stability-pool/PoolStats.svelte';
  import DepositInterface from '../../lib/components/stability-pool/DepositInterface.svelte';
  import UserAccount from '../../lib/components/stability-pool/UserAccount.svelte';
  import LiquidationMonitor from '../../lib/components/stability-pool/LiquidationMonitor.svelte';
  import LoadingSpinner from '../../lib/components/common/LoadingSpinner.svelte';

  let loading = true;
  let error = '';
  let poolStatus: PoolStatus | null = null;
  let userPosition: UserPosition | null = null;
  let liquidationHistory: LiquidationRecord[] = [];

  $: isConnected = $walletStore.isConnected;
  $: principal = $walletStore.principal;

  async function loadAllData() {
    try {
      loading = true;
      error = '';

      // Always load pool status (public query)
      poolStatus = await stabilityPoolService.getPoolStatus();

      // Load user-specific data if connected
      if (isConnected && principal) {
        const [position, history] = await Promise.all([
          stabilityPoolService.getUserPosition(principal).catch(() => null),
          stabilityPoolService.getLiquidationHistory(50).catch(() => []),
        ]);
        userPosition = position;
        liquidationHistory = history;
      } else {
        userPosition = null;
        liquidationHistory = [];
      }
    } catch (err: any) {
      console.error('Failed to load stability pool data:', err);
      error = err.message || 'Failed to load stability pool data';
    } finally {
      loading = false;
    }
  }

  // Reload on wallet connection changes
  let previousConnected = false;
  $: if (isConnected !== previousConnected) {
    previousConnected = isConnected;
    loadAllData();
  }

  onMount(() => { loadAllData(); });

  function handleSuccess() {
    // Reload all data and refresh wallet balance
    loadAllData();
    walletStore.refreshBalance();
  }
</script>

<svelte:head>
  <title>Earn — Stability Pool | Rumi Protocol</title>
</svelte:head>

<div class="earn-page">
  <!-- Page title -->
  <div class="page-header">
    <h1 class="page-title animate-title">Stability Pool</h1>
    <p class="page-subtitle">Deposit stablecoins · Absorb liquidations · Earn collateral</p>
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
    <!-- Pool overview stats -->
    <section class="section-stats" style="animation-delay: 0.05s">
      <PoolStats {poolStatus} />
    </section>

    <!-- Two-column layout: action panel + position -->
    <div class="main-layout">
      <section class="col-action" style="animation-delay: 0.1s">
        <DepositInterface {poolStatus} {userPosition} on:success={handleSuccess} />
      </section>

      <section class="col-position" style="animation-delay: 0.15s">
        {#if isConnected && userPosition}
          <UserAccount {poolStatus} {userPosition} on:success={handleSuccess} />
        {:else if isConnected && !userPosition}
          <div class="no-position-card">
            <div class="np-icon">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 2v20M2 12h20"/>
              </svg>
            </div>
            <h3 class="np-title">No Position Yet</h3>
            <p class="np-text">Deposit stablecoins to start earning collateral gains from liquidations</p>
          </div>
        {:else}
          <div class="no-position-card">
            <div class="np-icon">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <rect x="3" y="11" width="18" height="11" rx="2" ry="2"/>
                <path d="M7 11V7a5 5 0 0 1 10 0v4"/>
              </svg>
            </div>
            <h3 class="np-title">Connect Wallet</h3>
            <p class="np-text">Connect your wallet to view your position and manage deposits</p>
          </div>
        {/if}
      </section>
    </div>

    <!-- Liquidation history feed -->
    <section class="section-feed" style="animation-delay: 0.2s">
      <LiquidationMonitor {poolStatus} {liquidationHistory} />
    </section>

    <!-- How it works explainer -->
    <section class="how-it-works" style="animation-delay: 0.25s">
      <h3 class="hiw-title">How the Stability Pool Works</h3>
      <div class="hiw-grid">
        <div class="hiw-step">
          <div class="hiw-number">1</div>
          <div class="hiw-content">
            <h4>Deposit Stablecoins</h4>
            <p>Deposit icUSD, ckUSDT, or ckUSDC into the pool. Higher-priority tokens (ckstables) are consumed first during liquidations.</p>
          </div>
        </div>
        <div class="hiw-step">
          <div class="hiw-number">2</div>
          <div class="hiw-content">
            <h4>Absorb Liquidations</h4>
            <p>When undercollateralized vaults are liquidated, the pool's stablecoins are used to cover debt. Your balance is proportionally reduced.</p>
          </div>
        </div>
        <div class="hiw-step">
          <div class="hiw-number">3</div>
          <div class="hiw-content">
            <h4>Earn Collateral</h4>
            <p>In return, you receive the liquidated collateral (ICP, ckBTC, ckXAUT, ckETH) at a discount — your net position increases in value.</p>
          </div>
        </div>
      </div>
    </section>
  {/if}
</div>

<style>
  .earn-page {
    max-width: 1100px;
    margin: 0 auto;
    padding-bottom: 4rem;
  }

  /* ── Page header ── */
  .page-header {
    margin-bottom: 1.75rem;
    animation: fadeSlideIn 0.5s ease-out both;
  }

  .page-subtitle {
    font-size: 0.875rem;
    color: var(--rumi-text-secondary);
    margin-top: 0.25rem;
    letter-spacing: 0.01em;
  }

  /* ── Sections with staggered entrance ── */
  .section-stats,
  .col-action,
  .col-position,
  .section-feed,
  .how-it-works {
    animation: fadeSlideIn 0.5s ease-out both;
  }

  @keyframes fadeSlideIn {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .section-stats {
    margin-bottom: 1.5rem;
  }

  /* ── Two-column layout ── */
  .main-layout {
    display: grid;
    grid-template-columns: 1fr 380px;
    gap: 1.5rem;
    align-items: start;
    margin-bottom: 1.5rem;
  }

  /* ── No position placeholder ── */
  .no-position-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 2.5rem 1.5rem;
    text-align: center;
    box-shadow: inset 0 1px 0 0 rgba(200, 210, 240, 0.03), 0 2px 8px -2px rgba(8, 11, 22, 0.6);
  }

  .np-icon {
    width: 2.5rem;
    height: 2.5rem;
    margin: 0 auto 1rem;
    color: var(--rumi-text-muted);
    opacity: 0.5;
  }

  .np-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    margin-bottom: 0.5rem;
  }

  .np-text {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    line-height: 1.5;
    max-width: 260px;
    margin: 0 auto;
  }

  /* ── Liquidation feed section ── */
  .section-feed {
    margin-bottom: 1.5rem;
  }

  /* ── How it works ── */
  .how-it-works {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
    box-shadow: inset 0 1px 0 0 rgba(200, 210, 240, 0.03);
  }

  .hiw-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1rem;
    font-weight: 600;
    color: var(--rumi-purple-accent);
    margin-bottom: 1.25rem;
  }

  .hiw-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 1.5rem;
  }

  .hiw-step {
    display: flex;
    gap: 0.75rem;
  }

  .hiw-number {
    width: 1.75rem;
    height: 1.75rem;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--rumi-teal-dim);
    border: 1px solid var(--rumi-border-teal);
    border-radius: 50%;
    font-size: 0.75rem;
    font-weight: 700;
    color: var(--rumi-teal);
    flex-shrink: 0;
  }

  .hiw-content h4 {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    margin-bottom: 0.375rem;
  }

  .hiw-content p {
    font-size: 0.75rem;
    color: var(--rumi-text-secondary);
    line-height: 1.5;
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
  @media (max-width: 900px) {
    .main-layout {
      grid-template-columns: 1fr;
    }

    .hiw-grid {
      grid-template-columns: 1fr;
      gap: 1rem;
    }
  }

  @media (max-width: 520px) {
    .earn-page {
      padding-left: 0.5rem;
      padding-right: 0.5rem;
    }
  }
</style>
