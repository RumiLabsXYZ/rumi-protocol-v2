<script lang="ts">
  import { walletStore } from '../../lib/stores/wallet';
  import SwapInterface from '../../lib/components/swap/SwapInterface.svelte';

  function handleSuccess() {
    walletStore.refreshBalance();
  }
</script>

<svelte:head>
  <title>Swap | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <div class="page-header">
    <h1 class="page-title">Swap</h1>
  </div>

  <div class="action-column">
    <div class="action-panel">
      <SwapInterface on:success={handleSuccess} />
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

  @media (max-width: 520px) {
    .page-container {
      padding-left: 0.5rem;
      padding-right: 0.5rem;
    }
  }
</style>
