<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { Principal } from '@dfinity/principal';
  import SearchBar from '$lib/components/explorer/SearchBar.svelte';
  import VaultSummaryCard from '$lib/components/explorer/VaultSummaryCard.svelte';
  import EventRow from '$lib/components/explorer/EventRow.svelte';
  import { fetchVaultsByOwner, fetchEventsByPrincipal, fetchVaultHistory } from '$lib/stores/explorerStore';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { truncatePrincipal, copyToClipboard } from '$lib/utils/principalHelpers';
  import { formatAmount } from '$lib/utils/eventFormatters';
  import { toastStore } from '$lib/stores/toast';

  let vaults: any[] = [];
  let allHistory: any[] = [];
  let loading = true;
  let collateralConfigs: Map<string, any> = new Map();
  let copied = false;

  $: principalStr = $page.params.principal;

  async function handleCopy() {
    const ok = await copyToClipboard(principalStr);
    if (ok) { copied = true; setTimeout(() => copied = false, 2000); }
  }

  $: totalDebt = vaults.reduce((sum, v) => sum + Number(v.borrowed_icusd_amount), 0);

  onMount(async () => {
    loading = true;
    try {
      const principal = Principal.fromText(principalStr);
      vaults = await fetchVaultsByOwner(principal);

      // Fetch collateral configs for all collateral types
      const types = [...new Set(vaults.map((v: any) => v.collateral_type.toString()))];
      for (const ct of types) {
        try {
          const config = await publicActor.get_collateral_config(Principal.fromText(ct));
          if (config[0]) collateralConfigs.set(ct, config[0]);
        } catch {}
      }
      collateralConfigs = collateralConfigs; // trigger reactivity

      // Fetch events by principal (new endpoint) + vault history (catches old events without caller)
      const [principalEvents, vaultHistories] = await Promise.all([
        fetchEventsByPrincipal(principalStr),
        Promise.all(vaults.map((v: any) => fetchVaultHistory(Number(v.vault_id))))
      ]);
      // Merge and deduplicate by globalIndex
      const vaultEvents = vaultHistories.flat().map((e: any, i: number) => ({
        event: e, globalIndex: i
      }));
      const seen = new Set(principalEvents.map((e: any) => e.globalIndex));
      const merged = [...principalEvents];
      for (const ve of vaultEvents) {
        if (!seen.has(ve.globalIndex)) merged.push(ve);
      }
      allHistory = merged;
    } catch (e) {
      console.error('Failed to load address:', e);
      toastStore.error('Invalid principal or failed to load data');
    } finally {
      loading = false;
    }
  });
</script>

<div class="address-page">
  <a href="/explorer" class="back-link">← Back to Explorer</a>

  <div class="search-row"><SearchBar /></div>

  {#if loading}
    <div class="loading">Loading address…</div>
  {:else}
    <h1 class="page-title">Address</h1>
    <div class="principal-row">
      <code class="principal-full">{principalStr}</code>
      <button class="copy-btn" on:click={handleCopy}>{copied ? 'Copied!' : 'Copy'}</button>
    </div>

    <div class="stats-row">
      <div class="stat glass-card">
        <span class="stat-label">Vaults</span>
        <span class="stat-value key-number">{vaults.length}</span>
      </div>
      <div class="stat glass-card">
        <span class="stat-label">Total Debt</span>
        <span class="stat-value key-number">{formatAmount(BigInt(totalDebt))} icUSD</span>
      </div>
    </div>

    {#if vaults.length > 0}
      <h2 class="section-title">Vaults</h2>
      <div class="vault-grid">
        {#each vaults as vault}
          {@const ct = vault.collateral_type.toString()}
          {@const config = collateralConfigs.get(ct)}
          <VaultSummaryCard
            {vault}
            collateralSymbol={ct.startsWith('ryjl3') ? 'ICP' : 'tokens'}
            collateralDecimals={config?.decimals ? Number(config.decimals) : 8}
            collateralPrice={config?.last_price?.[0] ?? 0}
          />
        {/each}
      </div>
    {:else}
      <div class="empty">No vaults found for this address.</div>
    {/if}

    {#if allHistory.length > 0}
      <h2 class="section-title">Activity ({allHistory.length} events)</h2>
      <div class="events-list glass-card">
        {#each allHistory as item}
          <EventRow event={item.event ?? item} index={item.globalIndex} />
        {/each}
      </div>
    {/if}
  {/if}
</div>

<style>
  .address-page { max-width:900px; margin:0 auto; padding:2rem 1rem; }
  .back-link { color:var(--rumi-purple-accent); text-decoration:none; font-size:0.875rem; display:inline-block; margin-bottom:1rem; }
  .back-link:hover { text-decoration:underline; }
  .search-row { margin-bottom:1.5rem; display:flex; justify-content:center; }
  .principal-row { display:flex; align-items:center; gap:0.75rem; margin-bottom:1.5rem; flex-wrap:wrap; }
  .principal-full { font-size:0.8125rem; color:var(--rumi-text-secondary); word-break:break-all; background:var(--rumi-bg-surface-2); padding:0.5rem 0.75rem; border-radius:0.375rem; }
  .copy-btn { padding:0.375rem 0.75rem; font-size:0.75rem; border:1px solid var(--rumi-border); border-radius:0.375rem; background:transparent; color:var(--rumi-text-secondary); cursor:pointer; }
  .copy-btn:hover { border-color:var(--rumi-border-hover); }
  .stats-row { display:flex; gap:1rem; margin-bottom:1.5rem; }
  .stat { padding:0.75rem 1rem; text-align:center; flex:1; }
  .stat-label { display:block; font-size:0.75rem; color:var(--rumi-text-muted); margin-bottom:0.25rem; }
  .stat-value { font-size:1.25rem; font-weight:600; }
  .vault-grid { display:grid; grid-template-columns:repeat(auto-fill, minmax(280px, 1fr)); gap:1rem; margin-bottom:2rem; }
  .section-title { margin-bottom:0.75rem; }
  .events-list { padding:0; overflow:hidden; }
  .loading, .empty { text-align:center; padding:3rem; color:var(--rumi-text-muted); }
</style>
