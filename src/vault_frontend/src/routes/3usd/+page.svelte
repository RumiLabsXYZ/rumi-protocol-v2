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
  import { threePoolService, calculateApy, calculateTheoreticalApy, POOL_TOKENS } from '../../lib/services/threePoolService';
  import { ProtocolService } from '../../lib/services/protocol';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import LiquidityInterface from '../../lib/components/swap/LiquidityInterface.svelte';
  import PoolInfoCard from '../../lib/components/swap/PoolInfoCard.svelte';
  import LoadingSpinner from '../../lib/components/common/LoadingSpinner.svelte';

  let hasCachedData = _poolStatus !== null;
  let loading = !hasCachedData;
  let error = '';
  let poolStatus: PoolStatus | null = _poolStatus;
  let userLpBalance: bigint = _userLpBalance ?? 0n;
  let apy: number | null = null;
  let showApyTooltip = false;

  $: apyFormatted = apy !== null ? (apy * 100).toFixed(2) : null;
  $: isConnected = $walletStore.isConnected;
  $: principal = $walletStore.principal;

  async function loadAllData() {
    try {
      if (!hasCachedData) loading = true;
      error = '';

      const [status, snapshots, protocolStatus, interestSplit] = await Promise.all([
        threePoolService.getPoolStatus(),
        threePoolService.getVpSnapshots().catch(() => [] as VirtualPriceSnapshot[]),
        ProtocolService.getProtocolStatus().catch(() => null),
        (publicActor.get_interest_split() as Promise<{ destination: string; bps: bigint }[]>).catch(() => null),
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

        theoreticalApy = calculateTheoreticalApy(
          threePoolShareBps,
          protocolStatus.perCollateralInterest,
          poolTvlE8s / 1e8,
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
    walletStore.refreshBalance();
  }
</script>

<svelte:head>
  <title>3USD | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <div class="page-header">
    <h1 class="page-title">3USD</h1>
    {#if apyFormatted !== null}
      <!-- svelte-ignore a11y-mouse-events-have-key-events -->
      <div
        class="apy-badge"
        on:mouseover={() => { showApyTooltip = true; }}
        on:mouseleave={() => { showApyTooltip = false; }}
      >
        <svg class="apy-arrow" width="10" height="10" viewBox="0 0 10 10" fill="none">
          <path d="M5 8V2M5 2L2 5M5 2L8 5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
        {apyFormatted}% APY

        {#if showApyTooltip}
          <div class="apy-tooltip">
            <div class="apy-tooltip-caret"></div>
            <p><strong>3USD APY</strong> is earned from <strong>borrowing interest</strong> donations plus <strong>swap fees</strong>, based on current protocol borrowing rates and pool TVL.</p>
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
      <!-- LEFT: 3USD Stats -->
      <div class="stats-column">
        <PoolInfoCard {poolStatus} {userLpBalance} {apy} />
      </div>

      <!-- RIGHT: Mint/Redeem panel -->
      <div class="action-column">
        <div class="action-panel">
          <p class="explainer">Deposit any stablecoin or combination of the three to get 3USD</p>
          <LiquidityInterface on:success={handleSuccess} />
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

  /* ── APY badge ── */
  .apy-badge {
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

  .apy-arrow { color: #4ade80; flex-shrink: 0; }

  /* ── APY tooltip ── */
  .apy-tooltip {
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

  .apy-tooltip-caret {
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

  .apy-tooltip p { margin: 0; }
  .apy-tooltip strong { color: #cbd5e1; font-weight: 600; }

  /* ── Explainer ── */
  .explainer {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    margin: 0 0 1.25rem;
    line-height: 1.5;
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

    .page-header { flex-wrap: wrap; }

    .apy-tooltip {
      left: 0;
      transform: none;
      width: calc(100vw - 2rem);
    }

    .apy-tooltip-caret {
      left: 2rem;
      transform: rotate(45deg);
    }
  }
</style>
