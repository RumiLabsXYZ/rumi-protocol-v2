<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import LiquidationBotTab from '$lib/components/liquidations/LiquidationBotTab.svelte';
  import StabilityPoolTab from '$lib/components/liquidations/StabilityPoolTab.svelte';
  import ManualLiquidations from '$lib/components/liquidations/ManualLiquidations.svelte';

  type Tab = 'bot' | 'pool' | 'manual';

  // Read initial tab from URL query param, default to 'pool'
  $: tabParam = $page.url.searchParams.get('tab');
  $: activeTab = (tabParam === 'bot' ? 'bot' : tabParam === 'manual' ? 'manual' : 'pool') as Tab;

  function switchTab(tab: Tab) {
    const url = new URL($page.url);
    if (tab === 'pool') {
      url.searchParams.delete('tab');
    } else {
      url.searchParams.set('tab', tab);
    }
    goto(url.toString(), { replaceState: true, noScroll: true });
  }
</script>

<svelte:head><title>Liquidate | Rumi Protocol</title></svelte:head>

<div class="liquidate-page">
  <div class="page-header">
    <h1 class="page-title">Liquidate</h1>
  </div>

  <div class="tab-bar">
    <button
      class="tab-btn"
      class:active={activeTab === 'bot'}
      on:click={() => switchTab('bot')}
    >
      Liquidation Bot
    </button>
    <button
      class="tab-btn"
      class:active={activeTab === 'pool'}
      on:click={() => switchTab('pool')}
    >
      Stability Pool
    </button>
    <button
      class="tab-btn"
      class:active={activeTab === 'manual'}
      on:click={() => switchTab('manual')}
    >
      Manual Liquidations
    </button>
  </div>

  <div class="tab-content">
    {#if activeTab === 'bot'}
      <LiquidationBotTab />
    {:else if activeTab === 'pool'}
      <StabilityPoolTab />
    {:else}
      <ManualLiquidations />
    {/if}
  </div>
</div>

<style>
  .liquidate-page {
    max-width: 860px;
    margin: 0 auto;
    padding-bottom: 4rem;
  }

  .page-header {
    margin-bottom: 1rem;
    animation: fadeSlideIn 0.5s ease-out both;
  }

  @keyframes fadeSlideIn {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .tab-bar {
    display: flex;
    gap: 0.125rem;
    margin-bottom: 1.5rem;
    border-bottom: 1px solid var(--rumi-border);
    animation: fadeSlideIn 0.5s ease-out 0.05s both;
  }

  .tab-btn {
    position: relative;
    background: none;
    border: none;
    padding: 0.625rem 1.25rem;
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    cursor: pointer;
    transition: color 0.15s ease;
    white-space: nowrap;
  }

  .tab-btn:hover {
    color: var(--rumi-text-secondary);
  }

  .tab-btn.active {
    color: var(--rumi-text-primary);
  }

  .tab-btn.active::after {
    content: '';
    position: absolute;
    bottom: -1px;
    left: 0.5rem;
    right: 0.5rem;
    height: 2px;
    background: var(--rumi-action);
    border-radius: 1px 1px 0 0;
  }

  .tab-content {
    animation: fadeSlideIn 0.5s ease-out 0.1s both;
  }
</style>
