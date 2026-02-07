<script lang="ts">
  import { onMount } from 'svelte';
  import { appDataStore, userVaults, isLoadingVaults } from '$lib/stores/appDataStore';
  import { walletStore, isConnected, principal } from '$lib/stores/wallet';
  import { permissionStore } from '$lib/stores/permissionStore';
  import VaultCard from '$lib/components/vault/VaultCard.svelte';
  import { isDevelopment } from '$lib/config';
  import { developerAccess } from '$lib/stores/developer';

  let icpPrice = 0;

  $: canViewVaults = isDevelopment || $developerAccess || $isConnected
    || ($permissionStore.initialized && $permissionStore.canViewVaults);

  // Auto-load on wallet connect
  $: if ($isConnected && $principal && !$isLoadingVaults) {
    loadUserVaults();
  }

  async function loadUserVaults() {
    if (!$principal) return;
    try {
      const protocolStatus = await appDataStore.fetchProtocolStatus();
      if (protocolStatus) icpPrice = protocolStatus.lastIcpRate;
      await appDataStore.fetchUserVaults($principal);
    } catch (error) {
      console.error('Error loading vaults:', error);
    }
  }

  onMount(() => {
    console.log('Vaults page mounted');
  });
</script>

<div class="page-container">
  <div class="page-header">
    <h1 class="page-title">My Vaults</h1>
    {#if $isConnected}
      <button class="btn-secondary btn-compact" on:click={loadUserVaults}
        disabled={$isLoadingVaults}>
        {$isLoadingVaults ? 'Refreshing…' : 'Refresh'}
      </button>
    {/if}
  </div>

  {#if !canViewVaults}
    <div class="empty-state glass-card">
      <p class="empty-text">Connect your wallet to view vaults.</p>
    </div>
  {:else if !$isConnected}
    <div class="empty-state glass-card">
      <p class="empty-text">Connect your wallet to view your vaults.</p>
      <p class="empty-sub">Use the wallet button in the top right corner.</p>
    </div>
  {:else if $isLoadingVaults && $userVaults.length === 0}
    <div class="empty-state">
      <div class="spinner"></div>
    </div>
  {:else if $userVaults.length === 0}
    <div class="empty-state glass-card">
      <p class="empty-text">No vaults yet.</p>
      <a href="/" class="btn-primary">Create Your First Vault</a>
    </div>
  {:else}
    <div class="vault-list">
      {#each $userVaults as vault (vault.vaultId)}
        <VaultCard {vault} {icpPrice} on:updated={loadUserVaults} />
      {/each}
    </div>
  {/if}
</div>

<style>
  .page-container { max-width: 800px; margin: 0 auto; }
  .page-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 1.25rem;
  }
  .btn-compact {
    padding: 0.375rem 0.875rem;
    font-size: 0.75rem;
  }

  .vault-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .empty-state {
    text-align: center;
    padding: 3rem 1.5rem;
  }
  .empty-text {
    font-size: 0.9375rem;
    color: var(--rumi-text-secondary);
    margin-bottom: 0.75rem;
  }
  .empty-sub {
    font-size: 0.8125rem;
    color: var(--rumi-text-muted);
  }

  .spinner {
    width: 1.5rem;
    height: 1.5rem;
    border: 2px solid var(--rumi-border-hover);
    border-top-color: var(--rumi-action);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    margin: 0 auto;
  }
  @keyframes spin { to { transform: rotate(360deg); } }
</style>
