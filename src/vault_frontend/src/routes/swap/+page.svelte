<script lang="ts">
  import { walletStore } from '../../lib/stores/wallet';
  import SwapInterface from '../../lib/components/swap/SwapInterface.svelte';
  import SwapLiquidityToggle from '../../lib/components/swap/SwapLiquidityToggle.svelte';
  import PoolListView from '../../lib/components/swap/PoolListView.svelte';
  import AmmLiquidityPanel from '../../lib/components/swap/AmmLiquidityPanel.svelte';
  import LiquidityInterface from '../../lib/components/swap/LiquidityInterface.svelte';

  let mode: 'swap' | 'liquidity' = 'swap';
  let liquidityView: 'list' | 'threepool' | 'amm' = 'list';

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
</script>

<svelte:head>
  <title>{mode === 'swap' ? 'Swap' : 'Liquidity'} | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <div class="page-header">
    <h1 class="page-title">{mode === 'swap' ? 'Swap' : 'Liquidity'}</h1>
  </div>

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
