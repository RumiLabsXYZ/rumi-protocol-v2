<script lang="ts">
  import { onMount } from 'svelte';
  import { walletStore } from '../../lib/stores/wallet';
  import SwapInterface from '../../lib/components/swap/SwapInterface.svelte';
  import SwapLiquidityToggle from '../../lib/components/swap/SwapLiquidityToggle.svelte';
  import PoolListView from '../../lib/components/swap/PoolListView.svelte';
  import AmmLiquidityPanel from '../../lib/components/swap/AmmLiquidityPanel.svelte';
  import LiquidityInterface from '../../lib/components/swap/LiquidityInterface.svelte';
  import { getAmm1Apy, AMM1_THREEUSD_VALUE_SHARE } from '../../lib/services/amm1ApyService';
  import { getThreePoolApy } from '../../lib/services/threePoolApyService';

  let mode: 'swap' | 'liquidity' = 'swap';
  let liquidityView: 'list' | 'threepool' | 'amm' = 'list';

  // APY state for the combined hero. Each null while loading.
  let threePoolApyPct: number | null = null;
  let ammApyPct: number | null = null;

  // "Earn up to" is the best per-dollar yield available, not a weighted
  // blend of the two pools. Capital can only be in one position at a time,
  // and the AMM1 LP path stacks 3pool yield on its 3USD half:
  //   amm1Effective = amm1Apy + 0.5 * threePoolApy
  // The advertised number is whichever of the two paths wins per dollar.
  $: combinedApy =
    threePoolApyPct !== null && ammApyPct !== null
      ? Math.max(
          threePoolApyPct,
          ammApyPct + AMM1_THREEUSD_VALUE_SHARE * threePoolApyPct,
        )
      : null;

  onMount(() => {
    // Fire both APY loaders in parallel; never block the page render
    loadThreePoolApy().catch(e => console.warn('3pool APY (swap hero) failed:', e));
    loadAmmApy().catch(e => console.warn('AMM1 APY (swap hero) failed:', e));
  });

  async function loadThreePoolApy() {
    const r = await getThreePoolApy();
    threePoolApyPct = r.total_apy_pct;
  }

  async function loadAmmApy() {
    const r = await getAmm1Apy();
    ammApyPct = r.total_apy_pct;
  }

  function handleSuccess() {
    walletStore.refreshBalance();
  }

  function handlePoolSelect(e: CustomEvent<{ pool: 'threepool' | 'amm' }>) {
    liquidityView = e.detail.pool;
  }

  function handleBack() {
    liquidityView = 'list';
  }

  function handleModeChange() {
    liquidityView = 'list';
  }

  function switchToLiquidityTab() {
    mode = 'liquidity';
    liquidityView = 'list';
  }
</script>

<svelte:head>
  <title>{mode === 'swap' ? 'Swap' : 'Liquidity'} | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <div class="page-header">
    <h1 class="page-title">{mode === 'swap' ? 'Swap' : 'Liquidity'}</h1>
  </div>

  {#if combinedApy !== null && mode === 'swap'}
    <div class="earn-banner">
      <span class="earn-label">Earn up to {combinedApy.toFixed(2)}% APY</span>
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
      {:else if liquidityView === 'threepool'}
        <div>
          <button class="back-link" on:click={handleBack}>← All pools</button>
          <p class="explainer">Deposit stablecoins to mint 3USD</p>
          <LiquidityInterface on:success={handleSuccess} />
        </div>
      {:else if liquidityView === 'amm'}
        <AmmLiquidityPanel on:success={handleSuccess} on:back={handleBack} />
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

  .back-link {
    background: none;
    border: none;
    color: var(--rumi-teal);
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    padding: 0;
    margin-bottom: 1rem;
  }

  .back-link:hover { text-decoration: underline; }

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

  .explainer {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    margin: 0 0 1.25rem;
    line-height: 1.5;
  }

  @media (max-width: 520px) {
    .page-container {
      padding-left: 0.5rem;
      padding-right: 0.5rem;
    }
  }
</style>
