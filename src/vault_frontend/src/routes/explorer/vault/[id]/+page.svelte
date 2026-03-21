<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import SearchBar from '$lib/components/explorer/SearchBar.svelte';
  import EventRow from '$lib/components/explorer/EventRow.svelte';
  import { fetchVaultHistory, fetchAllVaults } from '$lib/stores/explorerStore';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { truncatePrincipal } from '$lib/utils/principalHelpers';
  import { formatAmount, formatTimestamp, resolveCollateralSymbol } from '$lib/utils/eventFormatters';

  let vault: any = null;
  let history: any[] = [];
  let loading = true;
  let collateralConfig: any = null;

  $: vaultId = Number($page.params.id);

  $: ownerStr = vault?.owner?.toString?.() || '';
  $: collateralSymbol = vault?.collateral_type ? resolveCollateralSymbol(vault.collateral_type) : 'tokens';
  $: vaultCollateralMap = vault ? new Map([[Number(vault.vault_id), vault.collateral_type]]) : undefined;
  $: decimals = collateralConfig?.decimals ? Number(collateralConfig.decimals) : 8;
  $: price = collateralConfig?.last_price?.[0] ?? 0;
  $: collateralValue = vault ? Number(vault.collateral_amount) / Math.pow(10, decimals) * price : 0;
  $: debtValue = vault ? Number(vault.borrowed_icusd_amount) / 1e8 : 0;
  $: cr = debtValue > 0 ? (collateralValue / debtValue) * 100 : Infinity;
  $: crColor = cr >= 200 ? 'var(--rumi-safe)' : cr >= 150 ? 'var(--rumi-caution)' : 'var(--rumi-danger)';

  onMount(async () => {
    loading = true;
    try {
      const [allVaults, vaultHistory] = await Promise.all([
        fetchAllVaults(),
        fetchVaultHistory(vaultId)
      ]);
      vault = allVaults.find((v: any) => Number(v.vault_id) === vaultId) || null;
      history = vaultHistory;

      if (vault) {
        const config = await publicActor.get_collateral_config(vault.collateral_type);
        collateralConfig = config[0] || null;
      }
    } catch (e) {
      console.error('Failed to load vault:', e);
    } finally {
      loading = false;
    }
  });
</script>

<div class="vault-page">
  <a href="/explorer" class="back-link">← Back to Explorer</a>

  <div class="search-row"><SearchBar /></div>

  {#if loading}
    <div class="loading">Loading vault #{vaultId}…</div>
  {:else if !vault}
    <div class="empty">Vault #{vaultId} not found.</div>
  {:else}
    <h1 class="page-title">Vault #{vaultId}</h1>

    <div class="vault-state glass-card">
      <div class="state-grid">
        <div class="state-item">
          <span class="label">Owner</span>
          <a href="/explorer/address/{ownerStr}" class="value link">{truncatePrincipal(ownerStr)}</a>
        </div>
        <div class="state-item">
          <span class="label">Collateral</span>
          <span class="value key-number">{formatAmount(vault.collateral_amount, decimals)} {collateralSymbol}</span>
        </div>
        <div class="state-item">
          <span class="label">Debt</span>
          <span class="value key-number">{formatAmount(vault.borrowed_icusd_amount)} icUSD</span>
        </div>
        <div class="state-item">
          <span class="label">Collateral Ratio</span>
          <span class="value key-number" style="color:{crColor}">{cr === Infinity ? '∞' : cr.toFixed(1)}%</span>
        </div>
        <div class="state-item">
          <span class="label">Accrued Interest</span>
          <span class="value key-number">{formatAmount(vault.accrued_interest)} icUSD</span>
        </div>
      </div>
    </div>

    <h2 class="section-title">Event History</h2>
    {#if history.length === 0}
      <div class="empty">No events found for this vault.</div>
    {:else}
      <div class="events-list glass-card">
        {#each history as event}
          <EventRow {event} {vaultCollateralMap} />
        {/each}
      </div>
    {/if}
  {/if}
</div>

<style>
  .vault-page { max-width:900px; margin:0 auto; padding:2rem 1rem; }
  .back-link { color:var(--rumi-purple-accent); text-decoration:none; font-size:0.875rem; display:inline-block; margin-bottom:1rem; }
  .back-link:hover { text-decoration:underline; }
  .search-row { margin-bottom:1.5rem; display:flex; justify-content:center; }
  .vault-state { padding:1.25rem; margin-bottom:2rem; }
  .state-grid { display:grid; grid-template-columns:repeat(auto-fit, minmax(160px, 1fr)); gap:1rem; }
  .state-item { display:flex; flex-direction:column; gap:0.25rem; }
  .label { font-size:0.75rem; color:var(--rumi-text-muted); }
  .value { font-size:1rem; }
  .link { color:var(--rumi-purple-accent); text-decoration:none; }
  .link:hover { text-decoration:underline; }
  .section-title { margin-bottom:0.75rem; }
  .events-list { padding:0; overflow:hidden; }
  .loading, .empty { text-align:center; padding:3rem; color:var(--rumi-text-muted); }
</style>
