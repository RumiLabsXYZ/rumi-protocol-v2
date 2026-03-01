<script lang="ts">
  import { onMount } from 'svelte';
  import { appDataStore, userVaults, isLoadingVaults } from '$lib/stores/appDataStore';
  import { walletStore, isConnected, principal } from '$lib/stores/wallet';
  import { permissionStore } from '$lib/stores/permissionStore';
  import VaultCard from '$lib/components/vault/VaultCard.svelte';
  import { isDevelopment, CANISTER_IDS } from '$lib/config';
  import { developerAccess } from '$lib/stores/developer';
  import { collateralStore } from '$lib/stores/collateralStore';
  import { getMinimumCR, getLiquidationCR } from '$lib/protocol';

  let icpPrice = 0;
  let expandedVaultId: number | null = null;

  function handleToggle(e: CustomEvent<{ vaultId: number }>) {
    expandedVaultId = expandedVaultId === e.detail.vaultId ? null : e.detail.vaultId;
  }

  $: canViewVaults = isDevelopment || $developerAccess || $isConnected
    || ($permissionStore.initialized && $permissionStore.canViewVaults);

  // Sort vaults by risk level group (red → purple → white), then CR ascending within each group.
  // Uses per-collateral price + per-collateral liquidation/minimum ratios.
  function vaultRiskBucket(cr: number, ct: string): number {
    const minCR = getMinimumCR(ct);
    const liqCR = getLiquidationCR(ct);
    const comfortCR = minCR * 1.234;
    if (cr === Infinity || cr >= comfortCR) return 2; // safe (white)
    if (cr >= minCR) return 1;                        // caution (purple)
    return 0;                                          // danger/warning (red)
  }

  $: sortedVaults = [...$userVaults].sort((a, b) => {
    const ctA = a.collateralType || CANISTER_IDS.ICP_LEDGER;
    const ctB = b.collateralType || CANISTER_IDS.ICP_LEDGER;
    const priceA = collateralStore.getCollateralPrice(ctA) || (ctA === CANISTER_IDS.ICP_LEDGER ? icpPrice : 0);
    const priceB = collateralStore.getCollateralPrice(ctB) || (ctB === CANISTER_IDS.ICP_LEDGER ? icpPrice : 0);
    const amountA = a.collateralAmount ?? a.icpMargin;
    const amountB = b.collateralAmount ?? b.icpMargin;
    const crA = a.borrowedIcusd > 0 && priceA > 0
      ? (amountA * priceA) / a.borrowedIcusd : Infinity;
    const crB = b.borrowedIcusd > 0 && priceB > 0
      ? (amountB * priceB) / b.borrowedIcusd : Infinity;
    const bucketA = vaultRiskBucket(crA, ctA);
    const bucketB = vaultRiskBucket(crB, ctB);
    if (bucketA !== bucketB) return bucketA - bucketB;
    if (crA !== crB) return crA - crB;
    return a.vaultId - b.vaultId;
  });

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
      {#each sortedVaults as vault (vault.vaultId)}
        <VaultCard {vault} {icpPrice} {expandedVaultId} on:updated={loadUserVaults} on:toggle={handleToggle} />
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
