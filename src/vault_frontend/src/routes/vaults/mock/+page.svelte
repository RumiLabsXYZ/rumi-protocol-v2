<script lang="ts">
  import VaultCard from '$lib/components/vault/VaultCard.svelte';

  // Mock data — various risk levels
  const mockVaults = [
    { vaultId: 12, icpMargin: 5.0,    borrowedIcusd: 10.0,  owner: 'mock' },
    { vaultId: 28, icpMargin: 0.1,    borrowedIcusd: 0.17,  owner: 'mock' },
    { vaultId: 7,  icpMargin: 100.0,  borrowedIcusd: 200.0, owner: 'mock' },
    { vaultId: 41, icpMargin: 2.5,    borrowedIcusd: 0,     owner: 'mock' },
    { vaultId: 3,  icpMargin: 1.0,    borrowedIcusd: 4.5,   owner: 'mock' },
  ];

  const icpPrice = 7.50;
  let expandedVaultId: number | null = null;

  function handleToggle(e: CustomEvent<{ vaultId: number }>) {
    expandedVaultId = expandedVaultId === e.detail.vaultId ? null : e.detail.vaultId;
  }

  // Sort by CR ascending (riskiest first)
  $: sortedVaults = [...mockVaults].sort((a, b) => {
    const crA = a.borrowedIcusd > 0 ? (a.icpMargin * icpPrice) / a.borrowedIcusd : Infinity;
    const crB = b.borrowedIcusd > 0 ? (b.icpMargin * icpPrice) / b.borrowedIcusd : Infinity;
    if (crA !== crB) return crA - crB;
    return a.vaultId - b.vaultId;
  });
</script>

<div class="page-container">
  <div class="page-header">
    <h1 class="page-title">My Vaults <span style="font-size:0.75rem; color:var(--rumi-text-muted); font-weight:400;">(mock data — ICP $7.50)</span></h1>
  </div>

  <div class="vault-list">
    {#each sortedVaults as vault (vault.vaultId)}
      <VaultCard {vault} {icpPrice} {expandedVaultId} on:toggle={handleToggle} />
    {/each}
  </div>
</div>

<style>
  .page-container { max-width: 800px; margin: 0 auto; }
  .page-header {
    display: flex; justify-content: space-between;
    align-items: center; margin-bottom: 1.25rem;
  }
  .vault-list {
    display: flex; flex-direction: column; gap: 0.5rem;
  }
</style>
