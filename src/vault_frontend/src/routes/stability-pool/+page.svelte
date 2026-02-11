<script lang="ts">
  import { onMount } from 'svelte';
  import { walletStore } from '../../lib/stores/wallet';
  import { stabilityPoolService } from '../../lib/services/stabilityPoolService';
  import PoolStats from '../../lib/components/stability-pool/PoolStats.svelte';
  import DepositInterface from '../../lib/components/stability-pool/DepositInterface.svelte';
  import UserAccount from '../../lib/components/stability-pool/UserAccount.svelte';
  import LiquidationMonitor from '../../lib/components/stability-pool/LiquidationMonitor.svelte';
  import RewardsDashboard from '../../lib/components/stability-pool/RewardsDashboard.svelte';
  import LoadingSpinner from '../../lib/components/common/LoadingSpinner.svelte';

  let loading = true;
  let error = '';
  let poolData: any = null;
  let userDeposit: any = null;
  let liquidationHistory: any[] = [];

  $: isConnected = $walletStore.isConnected;

  async function loadPoolData() {
    try {
      loading = true;
      error = '';
      poolData = await stabilityPoolService.getPoolInfo();
      if (isConnected) {
        userDeposit = await stabilityPoolService.getUserDeposit();
        liquidationHistory = await stabilityPoolService.getLiquidationHistory();
      }
    } catch (err) {
      console.error('Failed to load stability pool data:', err);
      error = 'Failed to load stability pool data. Please try again.';
    } finally {
      loading = false;
    }
  }

  onMount(() => { loadPoolData(); });
  $: if (isConnected !== undefined) { loadPoolData(); }

  function handleDepositSuccess() { loadPoolData(); }
  function handleWithdrawSuccess() { loadPoolData(); }
</script>

<svelte:head>
  <title>Stability Pool - Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <h1 class="page-title">Stability Pool</h1>

  {#if loading}
    <div class="loading-state">
      <LoadingSpinner />
      <p class="loading-text">Loading stability pool data…</p>
    </div>
  {:else if error}
    <div class="error-state">
      <p class="error-text">{error}</p>
      <button class="btn-primary" on:click={loadPoolData}>Try Again</button>
    </div>
  {:else}
    <!-- Stats row -->
    <div class="stats-row">
      <PoolStats {poolData} />
    </div>

    <!-- Main action area: two-column -->
    <div class="page-layout">
      <div class="action-column">
        <div class="action-card">
          <DepositInterface 
            {poolData} 
            {userDeposit}
            on:depositSuccess={handleDepositSuccess}
            on:withdrawSuccess={handleWithdrawSuccess}
          />
        </div>

        {#if isConnected && userDeposit}
          <div class="action-card" style="margin-top: 1.5rem;">
            <UserAccount {userDeposit} {poolData} />
          </div>
        {/if}
      </div>

      <div class="context-column">
        {#if isConnected && userDeposit}
          <div class="action-card">
            <RewardsDashboard {userDeposit} {liquidationHistory} />
          </div>
        {/if}
      </div>
    </div>

    <!-- Liquidation monitor -->
    <div class="monitor-section">
      <LiquidationMonitor {liquidationHistory} {poolData} />
    </div>
  {/if}
</div>

<style>
  .page-container { max-width: 1100px; margin: 0 auto; }
  .stats-row { margin-bottom: 1.5rem; }
  .page-layout { display: grid; grid-template-columns: 1fr 340px; gap: 1.5rem; align-items: start; }
  .action-column { min-width: 0; }
  .context-column { position: sticky; top: 5rem; }
  .action-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
  }
  .monitor-section { margin-top: 1.5rem; }

  .loading-state { display: flex; flex-direction: column; align-items: center; padding: 4rem; color: var(--rumi-text-secondary); }
  .loading-text { margin-top: 1rem; font-size: 0.875rem; }
  .error-state { text-align: center; padding: 3rem; }
  .error-text { font-size: 0.875rem; color: var(--rumi-danger); margin-bottom: 1rem; }

  @media (max-width: 768px) {
    .page-layout { grid-template-columns: 1fr; }
    .context-column { position: static; }
  }
</style>
