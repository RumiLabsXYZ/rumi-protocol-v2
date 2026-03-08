<script lang="ts" context="module">
  // Module-level cache survives component unmount/remount (SPA navigation).
  // This gives us stale-while-revalidate: show cached data instantly, refresh in background.
  import type { PoolStatus, UserPosition, LiquidationRecord } from '../../lib/services/stabilityPoolService';

  let _poolStatus: PoolStatus | null = null;
  let _protocolStatus: any = null;
  let _userPosition: UserPosition | null = null;
  let _liquidationHistory: LiquidationRecord[] = [];
  let _cachedForPrincipal: string | null = null;
</script>

<script lang="ts">
  import { onMount } from 'svelte';
  import { walletStore } from '../../lib/stores/wallet';
  import { stabilityPoolService } from '../../lib/services/stabilityPoolService';
  import { QueryOperations } from '../../lib/services/protocol/queryOperations';
  import DepositInterface from '../../lib/components/stability-pool/DepositInterface.svelte';
  import EarnInfoCard from '../../lib/components/stability-pool/EarnInfoCard.svelte';
  import LiquidationHistoryCard from '../../lib/components/stability-pool/LiquidationHistoryCard.svelte';
  import LoadingSpinner from '../../lib/components/common/LoadingSpinner.svelte';

  // Initialize from cache — if we have data, show it immediately (no spinner)
  let hasCachedData = _poolStatus !== null;
  let loading = !hasCachedData;
  let error = '';
  let poolStatus: PoolStatus | null = _poolStatus;
  let protocolStatus: any = _protocolStatus;
  let userPosition: UserPosition | null = _userPosition;
  let liquidationHistory: LiquidationRecord[] = _liquidationHistory;

  $: isConnected = $walletStore.isConnected;
  $: principal = $walletStore.principal;

  // APR calculation for header badge
  $: poolApr = (() => {
    if (!protocolStatus || !poolStatus || poolStatus.total_deposits_e8s === 0n) return null;
    const weightedRate = protocolStatus.weightedAverageInterestRate;
    const poolShare = protocolStatus.interestPoolShare;
    const totalDebt = protocolStatus.totalIcusdBorrowed;
    const poolTvl = Number(poolStatus.total_deposits_e8s) / 1e8;
    if (poolTvl === 0 || totalDebt === 0 || weightedRate === 0) return null;
    const apr = (weightedRate * poolShare * totalDebt) / poolTvl;
    return (apr * 100).toFixed(2);
  })();

  let showAprTooltip = false;

  async function loadAllData() {
    try {
      // Only show spinner if we have no cached data at all
      if (!hasCachedData) {
        loading = true;
      }
      error = '';

      // Always load pool status and protocol status (public queries)
      const [pool, proto] = await Promise.all([
        stabilityPoolService.getPoolStatus(),
        QueryOperations.getProtocolStatus().catch(() => null),
      ]);
      poolStatus = pool;
      protocolStatus = proto;

      // Update module-level cache
      _poolStatus = pool;
      _protocolStatus = proto;

      // Load user-specific data if connected
      if (isConnected && principal) {
        const [position, history] = await Promise.all([
          stabilityPoolService.getUserPosition(principal).catch(() => null),
          stabilityPoolService.getLiquidationHistory(50).catch(() => []),
        ]);
        userPosition = position;
        liquidationHistory = history;

        // Update module-level cache
        _userPosition = position;
        _liquidationHistory = history;
        _cachedForPrincipal = principal.toString();
      } else {
        userPosition = null;
        liquidationHistory = [];
        _userPosition = null;
        _liquidationHistory = [];
        _cachedForPrincipal = null;
      }

      hasCachedData = true;
    } catch (err: any) {
      console.error('Failed to load stability pool data:', err);
      // Only show error if we have no cached data to fall back on
      if (!hasCachedData) {
        error = err.message || 'Failed to load stability pool data';
      }
    } finally {
      loading = false;
    }
  }

  // Reload on wallet connection changes
  let previousConnected = false;
  $: if (isConnected !== previousConnected) {
    previousConnected = isConnected;
    // Clear user-specific cache on wallet change
    if (!isConnected || principal?.toString() !== _cachedForPrincipal) {
      _userPosition = null;
      _liquidationHistory = [];
      _cachedForPrincipal = null;
      userPosition = null;
      liquidationHistory = [];
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
  <title>Earn — Stability Pool | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <!-- Page header with APR badge -->
  <div class="page-header">
    <h1 class="page-title">Stability Pool</h1>
    {#if poolApr !== null}
      <!-- svelte-ignore a11y-mouse-events-have-key-events -->
      <div
        class="apr-badge"
        on:mouseover={() => { showAprTooltip = true; }}
        on:mouseleave={() => { showAprTooltip = false; }}
      >
        <svg class="apr-arrow" width="10" height="10" viewBox="0 0 10 10" fill="none">
          <path d="M5 8V2M5 2L2 5M5 2L8 5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
        {poolApr}% APR<span class="apr-asterisk">*</span>

        {#if showAprTooltip}
          <div class="apr-tooltip">
            <div class="apr-tooltip-caret"></div>
            <p><strong>Interest APR</strong> applies to <strong>icUSD</strong> deposits. icUSD depositors earn a share of all borrowing interest paid by vault owners.</p>
            <div class="apr-tooltip-divider"></div>
            <p><strong>ckUSDC</strong> and <strong>ckUSDT</strong> deposits don't earn interest but are used <em>first</em> for liquidations, giving priority access to discounted collateral.</p>
          </div>
        {/if}
      </div>
    {/if}
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
        <EarnInfoCard {poolStatus} {userPosition} {protocolStatus} {isConnected} on:success={handleSuccess} />
      </div>

      <!-- RIGHT: Deposit tool + Liquidation history -->
      <div class="action-column">
        <DepositInterface {poolStatus} {userPosition} on:success={handleSuccess} />
        <div class="liq-section">
          <LiquidationHistoryCard {poolStatus} {liquidationHistory} />
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

  /* ── APR badge ── */
  .apr-badge {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 0.3125rem;
    padding: 0.25rem 0.75rem;
    background: rgba(74, 222, 128, 0.1);
    border: 1px solid rgba(74, 222, 128, 0.3);
    border-radius: 1.25rem;
    font-size: 0.8125rem;
    font-weight: 600;
    color: #4ade80;
    cursor: default;
    white-space: nowrap;
  }

  .apr-arrow { color: #4ade80; flex-shrink: 0; }

  .apr-asterisk {
    font-size: 0.625rem;
    opacity: 0.6;
    margin-left: -0.125rem;
    vertical-align: super;
  }

  /* ── APR tooltip ── */
  .apr-tooltip {
    position: absolute;
    top: calc(100% + 0.625rem);
    left: 50%;
    transform: translateX(-50%);
    z-index: 50;
    width: 17rem;
    padding: 0.75rem 0.875rem;
    background: #1e293b;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 0.5rem;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
    font-size: 0.6875rem;
    font-weight: 400;
    line-height: 1.5;
    color: #94a3b8;
    cursor: default;
    white-space: normal;
    word-wrap: break-word;
    overflow-wrap: break-word;
    animation: tooltipFade 0.15s ease-out;
  }

  @keyframes tooltipFade {
    from { opacity: 0; transform: translateX(-50%) translateY(4px); }
    to { opacity: 1; transform: translateX(-50%) translateY(0); }
  }

  .apr-tooltip-caret {
    position: absolute;
    top: -5px;
    left: 50%;
    transform: translateX(-50%) rotate(45deg);
    width: 10px;
    height: 10px;
    background: #1e293b;
    border-top: 1px solid rgba(255, 255, 255, 0.08);
    border-left: 1px solid rgba(255, 255, 255, 0.08);
  }

  .apr-tooltip p { margin: 0; }
  .apr-tooltip strong { color: #cbd5e1; font-weight: 600; }
  .apr-tooltip em { font-style: italic; }

  .apr-tooltip-divider {
    height: 1px;
    background: rgba(255, 255, 255, 0.06);
    margin: 0.5rem 0;
  }

  /* ── Two-column layout (matches Borrow page exactly) ── */
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

  /* Constrain cards in right column to match Borrow page */
  .action-column > :global(*) { width: 100%; max-width: 420px; }
  .liq-section { width: 100%; max-width: 420px; }

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

    .page-header {
      flex-wrap: wrap;
    }

    .apr-tooltip {
      left: 0;
      transform: none;
      width: calc(100vw - 2rem);
    }

    .apr-tooltip-caret {
      left: 2rem;
      transform: rotate(45deg);
    }
  }
</style>
