<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { walletStore } from '../../lib/stores/wallet';
  import SwapInterface from '../../lib/components/swap/SwapInterface.svelte';
  import SwapLiquidityToggle from '../../lib/components/swap/SwapLiquidityToggle.svelte';
  import PoolListView from '../../lib/components/swap/PoolListView.svelte';
  import AmmLiquidityPanel from '../../lib/components/swap/AmmLiquidityPanel.svelte';
  import { getThreePoolApy } from '../../lib/services/threePoolApyService';
  import { AMM1_LIQUIDITY_PAUSED } from '../../lib/config';

  let mode: 'swap' | 'liquidity' = 'swap';
  // '3pool' liquidity reaches the canonical /3usd deposit experience (see
  // handlePoolSelect) rather than duplicating that form here, so this view
  // only ever needs to render the list or the paused AMM management panel.
  let liquidityView: 'list' | 'amm' = 'list';

  // The 3USD/ICP AMM is temporarily paused, so the liquidity hero reflects
  // the active 3pool opportunity only. `threePoolApyPct` stays null (number
  // hidden) whenever the rate can't be trusted as live, but the banner and
  // its "Provide liquidity" entry point remain visible either way.
  let threePoolApyPct: number | null = null;

  onMount(() => {
    loadThreePoolApy().catch(e => console.warn('3pool APY (swap hero) failed:', e));
  });

  async function loadThreePoolApy() {
    const r = await getThreePoolApy();
    threePoolApyPct = r.complete && Number.isFinite(r.total_apy_pct) ? r.total_apy_pct : null;
  }

  function handleSuccess() {
    walletStore.refreshBalance({ skipCache: true });
  }

  function handlePoolSelect(e: CustomEvent<{ pool: 'threepool' | 'amm' }>) {
    if (e.detail.pool === 'threepool') {
      // Same canonical 3USD deposit experience as the /3usd page and the
      // Earn overview, not a duplicate form embedded in Swap.
      goto('/3usd');
      return;
    }
    liquidityView = e.detail.pool;
  }

  function handleBack() {
    liquidityView = 'list';
  }

  function handleModeChange() {
    liquidityView = 'list';
  }

  function switchToLiquidityTab() {
    goto('/3usd');
  }
</script>

<svelte:head>
  <title>{mode === 'swap' ? 'Swap' : 'Liquidity'} | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <div class="page-header">
    <h1 class="page-title">{mode === 'swap' ? 'Swap' : 'Liquidity'}</h1>
  </div>

  {#if mode === 'swap'}
    <div class="earn-banner">
      <span class="earn-label">
        {#if threePoolApyPct !== null}
          Earn {threePoolApyPct.toFixed(2)}% APY providing stablecoin liquidity
        {:else}
          Earn from stablecoin liquidity
        {/if}
      </span>
      <button on:click={switchToLiquidityTab} class="earn-cta">Provide liquidity →</button>
    </div>
  {/if}

  <div class="action-column">
    <div class="action-panel">
      <SwapLiquidityToggle bind:mode on:change={handleModeChange} />

      {#if mode === 'swap'}
        <SwapInterface on:success={handleSuccess} />
      {:else if liquidityView === 'list'}
        <PoolListView on:select={handlePoolSelect} />
      {:else if liquidityView === 'amm'}
        <AmmLiquidityPanel depositsPaused={AMM1_LIQUIDITY_PAUSED} on:success={handleSuccess} on:back={handleBack} />
      {/if}
    </div>
  </div>
</div>

<style>
  .page-container {
    max-width: 420px;
    margin: 0 auto;
    padding-bottom: 4rem;
  }

  .page-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 1.75rem;
    animation: fadeSlideIn 0.5s ease-out both;
  }

  @keyframes fadeSlideIn {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .action-column {
    display: flex;
    flex-direction: column;
    align-items: center;
  }

  .action-column > :global(*) { width: 100%; }

  .action-panel {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
    box-shadow:
      inset 0 1px 0 0 rgba(200, 210, 240, 0.03),
      0 2px 8px -2px rgba(8, 11, 22, 0.6);
  }

  .earn-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    margin-bottom: 1rem;
    padding: 0.625rem 0.9375rem;
    background: rgba(74, 222, 128, 0.08);
    border: 1px solid rgba(74, 222, 128, 0.25);
    border-radius: 0.625rem;
    animation: fadeSlideIn 0.4s ease-out 0.1s both;
  }

  .earn-label {
    font-size: 0.875rem;
    font-weight: 600;
    color: #4ade80;
    font-variant-numeric: tabular-nums;
  }

  .earn-cta {
    background: none;
    border: none;
    color: #4ade80;
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    padding: 0.25rem 0.5rem;
    border-radius: 0.375rem;
    transition: background-color 0.15s;
    font-family: inherit;
    white-space: nowrap;
  }

  .earn-cta:hover {
    background-color: rgba(74, 222, 128, 0.12);
  }

  @media (max-width: 520px) {
    .page-container {
      padding-left: 0.5rem;
      padding-right: 0.5rem;
    }
  }
</style>
