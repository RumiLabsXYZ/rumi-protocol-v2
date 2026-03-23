<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { Principal } from '@dfinity/principal';
  import SearchBar from '$lib/components/explorer/SearchBar.svelte';
  import EntityLink from '$lib/components/explorer/EntityLink.svelte';
  import TokenBadge from '$lib/components/explorer/TokenBadge.svelte';
  import VaultHealthBar from '$lib/components/explorer/VaultHealthBar.svelte';
  import DashboardCard from '$lib/components/explorer/DashboardCard.svelte';
  import DataTable from '$lib/components/explorer/DataTable.svelte';
  import { fetchVaultsByOwner, fetchEventsByPrincipal } from '$lib/stores/explorerStore';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { copyToClipboard } from '$lib/utils/principalHelpers';
  import { formatAmount, resolveCollateralSymbol, getEventType, getEventBadgeColor, getEventSummary, getEventTimestamp, formatTimestamp } from '$lib/utils/eventFormatters';
  import { stabilityPoolService } from '$lib/services/stabilityPoolService';
  import { threePoolService } from '$lib/services/threePoolService';
  import { toastStore } from '$lib/stores/toast';

  // ── State ──────────────────────────────────────────────────────────────────
  let vaults = $state<any[]>([]);
  let allHistory = $state<any[]>([]);
  let loading = $state(true);
  let collateralConfigs = $state<Map<string, any>>(new Map());
  let copied = $state(false);
  let spPosition = $state<any>(null);
  let lpBalance = $state<bigint>(0n);
  let poolStatus = $state<any>(null);

  // ── Derived ────────────────────────────────────────────────────────────────
  const principalStr = $derived($page.params.principal);

  const totalCollateralByType = $derived.by(() => {
    const map = new Map<string, number>();
    for (const v of vaults) {
      const ct = v.collateral_type?.toString?.() ?? '';
      const cfg = collateralConfigs.get(ct);
      const dec = cfg?.decimals ? Number(cfg.decimals) : 8;
      const human = Number(v.collateral_amount) / Math.pow(10, dec);
      map.set(ct, (map.get(ct) ?? 0) + human);
    }
    return map;
  });

  const totalDebtHuman = $derived(
    vaults.reduce((sum, v) => sum + Number(v.borrowed_icusd_amount) / 1e8, 0)
  );

  const openVaults = $derived(vaults.filter((v) => {
    if (!v.status) return true;
    const key = Object.keys(v.status)[0];
    return key !== 'Closed' && key !== 'closed' && key !== 'Liquidated' && key !== 'liquidated';
  }));

  // DataTable columns for vaults
  const vaultColumns = [
    { key: 'id', label: 'Vault', align: 'left' as const },
    { key: 'collateral', label: 'Collateral Type', align: 'left' as const },
    { key: 'amount', label: 'Collateral Amount', align: 'right' as const },
    { key: 'debt', label: 'Debt (icUSD)', align: 'right' as const },
    { key: 'cr', label: 'CR', align: 'left' as const, width: '12rem' },
    { key: 'status', label: 'Status', align: 'center' as const },
  ];

  // DataTable columns for activity
  const activityColumns = [
    { key: 'index', label: '#', align: 'right' as const, width: '3rem' },
    { key: 'time', label: 'Time', align: 'left' as const },
    { key: 'type', label: 'Type', align: 'left' as const },
    { key: 'summary', label: 'Summary', align: 'left' as const },
  ];

  // History sorted newest-first
  const historySorted = $derived([...allHistory].sort((a, b) => (b.globalIndex ?? 0) - (a.globalIndex ?? 0)));

  // ── Helpers ────────────────────────────────────────────────────────────────
  function getVaultCr(vault: any): number {
    const ct = vault.collateral_type?.toString?.() ?? '';
    const cfg = collateralConfigs.get(ct);
    const dec = cfg?.decimals ? Number(cfg.decimals) : 8;
    const price = cfg?.last_price?.[0] ?? 0;
    const collValue = (Number(vault.collateral_amount) / Math.pow(10, dec)) * price;
    const debt = (Number(vault.borrowed_icusd_amount) + Number(vault.accrued_interest ?? 0n)) / 1e8;
    return debt > 0 ? (collValue / debt) * 100 : Infinity;
  }

  function getVaultLiqRatio(vault: any): number {
    const ct = vault.collateral_type?.toString?.() ?? '';
    const cfg = collateralConfigs.get(ct);
    return cfg?.liquidation_threshold ? Number(cfg.liquidation_threshold) * 100 : 110;
  }

  function getVaultStatus(vault: any): 'Open' | 'Closed' | 'Liquidated' {
    if (!vault.status) return 'Open';
    const key = Object.keys(vault.status)[0];
    if (key === 'Closed' || key === 'closed') return 'Closed';
    if (key === 'Liquidated' || key === 'liquidated') return 'Liquidated';
    return 'Open';
  }

  async function handleCopy() {
    const ok = await copyToClipboard(principalStr);
    if (ok) { copied = true; setTimeout(() => (copied = false), 2000); }
  }

  // ── Load ───────────────────────────────────────────────────────────────────
  onMount(async () => {
    loading = true;
    try {
      const principal = Principal.fromText($page.params.principal);

      const [ownerVaults, events] = await Promise.all([
        fetchVaultsByOwner(principal),
        fetchEventsByPrincipal($page.params.principal),
      ]);

      vaults = ownerVaults;
      allHistory = events;

      // Fetch collateral configs for all vault collateral types
      const types = [...new Set(ownerVaults.map((v: any) => v.collateral_type?.toString?.() ?? ''))].filter(Boolean);
      const cfgMap = new Map<string, any>();
      await Promise.all(
        types.map(async (ct: string) => {
          try {
            const config = await publicActor.get_collateral_config(Principal.fromText(ct));
            if (config[0]) cfgMap.set(ct, config[0]);
          } catch {}
        })
      );
      collateralConfigs = cfgMap;

      // Stability Pool position
      try {
        spPosition = await stabilityPoolService.getUserPosition(principal);
      } catch (e) {
        console.error('Failed to fetch SP position:', e);
      }

      // 3Pool LP balance
      try {
        lpBalance = await threePoolService.getLpBalance(principal);
        if (lpBalance > 0n) {
          poolStatus = await threePoolService.getPoolStatus();
        }
      } catch (e) {
        console.error('Failed to fetch 3pool position:', e);
      }
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
    <div class="empty">Loading address…</div>
  {:else}
    <!-- ── Header ─────────────────────────────────────────────────────────── -->
    <div class="addr-header">
      <h1 class="page-title">Address</h1>
      <div class="principal-row">
        <code class="principal-full">{principalStr}</code>
        <button class="copy-btn" onclick={handleCopy}>{copied ? 'Copied!' : 'Copy'}</button>
      </div>
    </div>

    <!-- ── Summary Stats ─────────────────────────────────────────────────── -->
    <div class="stats-grid">
      <DashboardCard label="Total Vaults" value={String(vaults.length)} subtitle={openVaults.length > 0 ? `${openVaults.length} open` : undefined} />
      <DashboardCard label="Total Debt" value="{totalDebtHuman.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })} icUSD" />
      {#if spPosition && Number(spPosition.total_usd_value_e8s ?? 0n) > 0}
        <DashboardCard label="SP Deposit" value="{formatAmount(spPosition.total_usd_value_e8s ?? 0n)} icUSD" />
      {/if}
      {#if lpBalance > 0n}
        <DashboardCard label="3Pool LP Balance" value="{formatAmount(lpBalance)} 3USD" />
      {/if}
    </div>

    <!-- ── Vaults Section ─────────────────────────────────────────────────── -->
    <section class="section">
      <h2 class="section-title">Vaults ({vaults.length})</h2>
      <div class="glass-card overflow-hidden">
        <DataTable
          columns={vaultColumns}
          rows={vaults}
          emptyMessage="No vaults found for this address."
          loading={false}
        >
          {#snippet row(vault: any, _i: number)}
            {@const ct = vault.collateral_type?.toString?.() ?? ''}
            {@const cfg = collateralConfigs.get(ct)}
            {@const dec = cfg?.decimals ? Number(cfg.decimals) : 8}
            {@const symbol = resolveCollateralSymbol(ct)}
            {@const vaultCr = getVaultCr(vault)}
            {@const liqRatio = getVaultLiqRatio(vault)}
            {@const status = getVaultStatus(vault)}
            {@const statusClass = status === 'Open'
              ? 'bg-green-500/20 text-green-400 border border-green-500/30'
              : status === 'Liquidated'
                ? 'bg-red-500/20 text-red-400 border border-red-500/30'
                : 'bg-gray-500/20 text-gray-400 border border-gray-500/30'}
            <tr class="vault-row">
              <td class="px-4 py-3">
                <EntityLink type="vault" id={Number(vault.vault_id)} />
              </td>
              <td class="px-4 py-3">
                <TokenBadge symbol={symbol} principalId={ct} size="sm" linked={true} />
              </td>
              <td class="px-4 py-3 text-right text-gray-200 text-sm font-mono">
                {formatAmount(vault.collateral_amount, dec)} {symbol}
              </td>
              <td class="px-4 py-3 text-right text-gray-200 text-sm font-mono">
                {formatAmount(vault.borrowed_icusd_amount)} icUSD
              </td>
              <td class="px-4 py-3" style="min-width: 11rem;">
                {#if vaultCr === Infinity}
                  <span class="text-gray-400 text-xs">No debt</span>
                {:else}
                  <VaultHealthBar collateralRatio={vaultCr} liquidationRatio={liqRatio} />
                {/if}
              </td>
              <td class="px-4 py-3 text-center">
                <span class="status-badge {statusClass}">{status}</span>
              </td>
            </tr>
          {/snippet}
        </DataTable>
      </div>
    </section>

    <!-- ── Positions Section ─────────────────────────────────────────────── -->
    {#if (spPosition && Number(spPosition.total_usd_value_e8s ?? 0n) > 0) || lpBalance > 0n}
      <section class="section">
        <h2 class="section-title">Positions</h2>
        <div class="positions-grid">
          {#if spPosition && Number(spPosition.total_usd_value_e8s ?? 0n) > 0}
            <div class="glass-card position-card">
              <h3 class="position-title">Stability Pool</h3>
              <div class="position-row">
                <span class="position-label">Deposited</span>
                <span class="position-value">{formatAmount(spPosition.total_usd_value_e8s ?? 0n)} icUSD</span>
              </div>
              {#if spPosition.collateral_gains?.length > 0}
                {#each spPosition.collateral_gains as [ledger, amount]}
                  {#if Number(amount) > 0}
                    <div class="position-row">
                      <span class="position-label">Collateral Gain</span>
                      <span class="position-value">
                        {formatAmount(amount)}
                        <TokenBadge symbol={resolveCollateralSymbol(ledger)} principalId={ledger?.toString?.() ?? ''} size="sm" linked={true} />
                      </span>
                    </div>
                  {/if}
                {/each}
              {/if}
            </div>
          {/if}

          {#if lpBalance > 0n}
            <div class="glass-card position-card">
              <h3 class="position-title">3Pool</h3>
              <div class="position-row">
                <span class="position-label">LP Balance</span>
                <span class="position-value">{formatAmount(lpBalance)} 3USD</span>
              </div>
              {#if poolStatus}
                {@const share = poolStatus.lp_total_supply > 0n
                  ? Number(lpBalance) / Number(poolStatus.lp_total_supply)
                  : 0}
                <div class="position-row">
                  <span class="position-label">Pool Share</span>
                  <span class="position-value">{(share * 100).toFixed(4)}%</span>
                </div>
              {/if}
            </div>
          {/if}
        </div>
      </section>
    {/if}

    <!-- ── Activity History ───────────────────────────────────────────────── -->
    <section class="section">
      <h2 class="section-title">Activity ({allHistory.length} events)</h2>
      <div class="glass-card overflow-hidden">
        <DataTable
          columns={activityColumns}
          rows={historySorted}
          emptyMessage="No events found for this address."
          loading={false}
        >
          {#snippet row(item: any, _i: number)}
            {@const evt = item.event ?? item}
            {@const ts = getEventTimestamp(evt)}
            {@const badgeColor = getEventBadgeColor(evt)}
            {@const summary = getEventSummary(evt)}
            {@const globalIdx = item.globalIndex ?? null}
            <tr class="history-row">
              <td class="px-4 py-3 text-right text-gray-500 text-xs font-mono">
                {#if globalIdx !== null}
                  <a href="/explorer/event/{globalIdx}" class="hover:text-blue-400 transition-colors">#{globalIdx}</a>
                {:else}
                  —
                {/if}
              </td>
              <td class="px-4 py-3 text-gray-400 text-xs whitespace-nowrap">
                {ts ? formatTimestamp(ts) : '—'}
              </td>
              <td class="px-4 py-3">
                <span
                  class="event-badge"
                  style="background:{badgeColor}20; color:{badgeColor}; border:1px solid {badgeColor}40;"
                >
                  {getEventType(evt)}
                </span>
              </td>
              <td class="px-4 py-3 text-gray-300 text-sm">
                {summary}
                {#if item.vaultId !== undefined}
                  <EntityLink type="vault" id={item.vaultId} />
                {/if}
              </td>
            </tr>
          {/snippet}
        </DataTable>
      </div>
    </section>
  {/if}
</div>

<style>
  .address-page { max-width: 960px; margin: 0 auto; padding: 2rem 1rem; }

  .back-link {
    color: var(--rumi-purple-accent);
    text-decoration: none;
    font-size: 0.875rem;
    display: inline-block;
    margin-bottom: 1rem;
  }
  .back-link:hover { text-decoration: underline; }

  .search-row { margin-bottom: 1.5rem; display: flex; justify-content: center; }

  /* Header */
  .addr-header { margin-bottom: 1.5rem; }
  .page-title { font-size: 1.75rem; font-weight: 700; color: var(--rumi-text-primary); margin: 0 0 0.75rem; }

  .principal-row { display: flex; align-items: center; gap: 0.75rem; flex-wrap: wrap; }
  .principal-full {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    word-break: break-all;
    background: var(--rumi-bg-surface-2);
    padding: 0.5rem 0.75rem;
    border-radius: 0.375rem;
    font-family: monospace;
  }
  .copy-btn {
    padding: 0.375rem 0.75rem;
    font-size: 0.75rem;
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    background: transparent;
    color: var(--rumi-text-secondary);
    cursor: pointer;
    white-space: nowrap;
  }
  .copy-btn:hover { border-color: var(--rumi-border-hover); }

  /* Stats */
  .stats-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 1rem;
    margin-bottom: 2rem;
  }

  /* Sections */
  .section { margin-bottom: 2rem; }
  .section-title { font-size: 1rem; font-weight: 600; color: var(--rumi-text-secondary); margin-bottom: 0.75rem; }

  /* Vault table rows */
  .vault-row { border-bottom: 1px solid rgba(255,255,255,0.05); }
  .vault-row:last-child { border-bottom: none; }
  .vault-row:hover { background: var(--rumi-bg-surface-2, rgba(255,255,255,0.03)); }

  .status-badge {
    font-size: 0.7rem;
    font-weight: 500;
    padding: 0.2rem 0.5rem;
    border-radius: 9999px;
  }

  /* Positions */
  .positions-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: 1rem;
  }
  .position-card { padding: 1rem 1.25rem; }
  .position-title { font-size: 0.875rem; font-weight: 600; color: var(--rumi-text-secondary); margin: 0 0 0.75rem; }
  .position-row { display: flex; justify-content: space-between; align-items: center; padding: 0.375rem 0; border-top: 1px solid rgba(255,255,255,0.05); }
  .position-row:first-of-type { border-top: none; }
  .position-label { font-size: 0.8125rem; color: var(--rumi-text-muted); }
  .position-value { font-size: 0.875rem; font-weight: 500; display: flex; align-items: center; gap: 0.375rem; }

  /* History rows */
  .history-row { border-bottom: 1px solid rgba(255,255,255,0.05); }
  .history-row:last-child { border-bottom: none; }
  .history-row:hover { background: var(--rumi-bg-surface-2, rgba(255,255,255,0.03)); }

  .event-badge {
    font-size: 0.7rem;
    font-weight: 500;
    padding: 0.125rem 0.5rem;
    border-radius: 9999px;
    white-space: nowrap;
  }

  .empty { text-align: center; padding: 3rem; color: var(--rumi-text-muted); }
</style>
