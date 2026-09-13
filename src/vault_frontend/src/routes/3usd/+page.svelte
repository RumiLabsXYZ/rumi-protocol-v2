<script lang="ts" context="module">
  import type { PoolStatus, VirtualPriceSnapshot } from '../../lib/services/threePoolService';
  let _poolStatus: PoolStatus | null = null;
  let _userLpBalance: bigint | null = null;
  let _cachedForPrincipal: string | null = null;
  let _vpSnapshots: VirtualPriceSnapshot[] | null = null;
</script>

<script lang="ts">
  import { onMount } from 'svelte';
  import { walletStore } from '../../lib/stores/wallet';
  import { threePoolService, calculateApy, calculateTotalApy, POOL_TOKENS } from '../../lib/services/threePoolService';
  import { ProtocolService } from '../../lib/services/protocol';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import LiquidityInterface from '../../lib/components/swap/LiquidityInterface.svelte';
  import PoolInfoCard from '../../lib/components/swap/PoolInfoCard.svelte';
  import LoadingSpinner from '../../lib/components/common/LoadingSpinner.svelte';
  import EarnSubNav from '../../lib/components/layout/EarnSubNav.svelte';

  let hasCachedData = _poolStatus !== null;
  let loading = !hasCachedData;
  let error = '';
  let poolStatus: PoolStatus | null = _poolStatus;
  let userLpBalance: bigint = _userLpBalance ?? 0n;
  let apy: number | null = null;

  $: isConnected = $walletStore.isConnected;
  $: principal = $walletStore.principal;

  async function loadAllData() {
    try {
      if (!hasCachedData) loading = true;
      error = '';

      const [status, snapshots, protocolStatus, interestSplit, swapFees7d] = await Promise.all([
        threePoolService.getPoolStatus(),
        threePoolService.getVpSnapshots().catch(() => [] as VirtualPriceSnapshot[]),
        ProtocolService.getProtocolStatus().catch(() => null),
        (publicActor.get_interest_split() as Promise<{ destination: string; bps: bigint }[]>).catch(() => null),
        threePoolService.getSwapFeesOverWindow(7).catch(() => 0n),
      ]);
      poolStatus = status;
      _poolStatus = status;
      _vpSnapshots = snapshots;

      // Compute theoretical APY from protocol borrowing interest data.
      // Falls back to VP-based APY if protocol data is unavailable.
      let theoreticalApy: number | null = null;
      if (protocolStatus && status) {
        let poolTvlE8s = 0;
        for (let i = 0; i < status.balances.length; i++) {
          const token = POOL_TOKENS[i];
          if (token) {
            const normalized = token.decimals === 8
              ? Number(status.balances[i])
              : Number(status.balances[i]) * 100;
            poolTvlE8s += normalized;
          }
        }

        const threePoolEntry = interestSplit?.find(e => e.destination === 'three_pool');
        const threePoolShareBps = threePoolEntry ? Number(threePoolEntry.bps) : 5000;

        theoreticalApy = calculateTotalApy(
          threePoolShareBps,
          protocolStatus.perCollateralInterest,
          poolTvlE8s / 1e8,
          swapFees7d,
        );
      }

      const vpApy = calculateApy(status.virtual_price, snapshots, 7);
      apy = theoreticalApy ?? vpApy;

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
    walletStore.refreshBalance({ skipCache: true });
  }
</script>

<svelte:head>
  <title>3USD | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <EarnSubNav active="3usd" />

  <p class="pool-description">
    3USD represents your share of the pool. Deposit icUSD, ckUSDT, or ckUSDC, in any
    combination, and 3USD earns swap fees plus a share of the protocol's borrowing
    interest, based on current protocol rates and pool TVL.
  </p>

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
      <!-- LEFT: 3USD Stats -->
      <div class="stats-column">
        <PoolInfoCard {poolStatus} {userLpBalance} {apy} />
      </div>

      <!-- RIGHT: Mint/Redeem panel -->
      <div class="action-column">
        <div class="action-panel">
          <LiquidityInterface on:success={handleSuccess} />
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .page-container { max-width: 820px; margin: 0 auto; padding-bottom: 4rem; }

  @keyframes fadeSlideIn {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
  }

  /* ── Pool description ── */
  .pool-description {
    max-width: 68ch;
    margin: 0 0 1.5rem;
    font-size: 0.8125rem;
    line-height: 1.6;
    color: var(--rumi-text-secondary);
    animation: fadeSlideIn 0.5s ease-out both;
  }

  /* ── Two-column layout ── */
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

  .action-panel {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
    box-shadow:
      inset 0 1px 0 0 rgba(200, 210, 240, 0.03),
      0 2px 8px -2px rgba(8, 11, 22, 0.6);
  }

  /* ── Loading & error states ── */
  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 5rem;
    color: var(--rumi-text-secondary);
  }

  .loading-text { margin-top: 1rem; font-size: 0.875rem; }

  .error-state { text-align: center; padding: 4rem 1rem; }

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
