<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import SearchBar from '$lib/components/explorer/SearchBar.svelte';
  import EntityLink from '$lib/components/explorer/EntityLink.svelte';
  import TokenBadge from '$lib/components/explorer/TokenBadge.svelte';
  import VaultHealthBar from '$lib/components/explorer/VaultHealthBar.svelte';
  import DashboardCard from '$lib/components/explorer/DashboardCard.svelte';
  import DataTable from '$lib/components/explorer/DataTable.svelte';
  import { fetchVaultHistory, fetchAllVaults } from '$lib/stores/explorerStore';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { formatAmount, resolveCollateralSymbol, getEventType, getEventBadgeColor, getEventSummary, getEventTimestamp, formatTimestamp } from '$lib/utils/eventFormatters';

  // ── State ──────────────────────────────────────────────────────────────────
  let vault = $state<any>(null);
  let history = $state<any[]>([]);
  let loading = $state(true);
  let collateralConfig = $state<any>(null);

  // ── Derived ────────────────────────────────────────────────────────────────
  const vaultId = $derived(Number($page.params.id));

  const ownerStr = $derived(vault?.owner?.toString?.() ?? '');

  const collateralType = $derived(vault?.collateral_type ?? null);

  const collateralSymbol = $derived(
    collateralType ? resolveCollateralSymbol(collateralType) : 'tokens'
  );

  const collateralPrincipalStr = $derived(
    collateralType ? (collateralType?.toString?.() ?? collateralType?.toText?.() ?? String(collateralType)) : ''
  );

  const decimals = $derived(collateralConfig?.decimals ? Number(collateralConfig.decimals) : 8);

  const price = $derived(collateralConfig?.last_price?.[0] ?? 0);

  const collateralAmount = $derived(vault ? Number(vault.collateral_amount) : 0);

  const collateralHuman = $derived(collateralAmount / Math.pow(10, decimals));

  const collateralValueUsd = $derived(collateralHuman * price);

  const debtRaw = $derived(vault ? Number(vault.borrowed_icusd_amount) : 0);
  const interestRaw = $derived(vault ? Number(vault.accrued_interest ?? 0n) : 0);
  const totalDebtRaw = $derived(debtRaw + interestRaw);

  const debtHuman = $derived(debtRaw / 1e8);
  const interestHuman = $derived(interestRaw / 1e8);
  const totalDebtHuman = $derived(totalDebtRaw / 1e8);

  // CR in percent
  const cr = $derived(totalDebtHuman > 0 ? (collateralValueUsd / totalDebtHuman) * 100 : Infinity);

  // Liquidation ratio from config (typically 1.1 = 110%)
  const liquidationRatioPct = $derived(
    collateralConfig?.liquidation_threshold
      ? Number(collateralConfig.liquidation_threshold) * 100
      : 110
  );

  // Liquidation price = total debt / collateral * liquidation_ratio
  const liquidationPrice = $derived(
    collateralHuman > 0 && totalDebtHuman > 0
      ? (totalDebtHuman * (liquidationRatioPct / 100)) / collateralHuman
      : 0
  );

  // Distance to liquidation as %: (price - liqPrice) / price
  const distanceToLiqPct = $derived(
    price > 0 && liquidationPrice > 0
      ? ((price - liquidationPrice) / price) * 100
      : null
  );

  // Vault status
  const vaultStatus = $derived.by<'Open' | 'Liquidated' | 'Closed'>(() => {
    if (!vault) return 'Open';
    const key = vault.status ? Object.keys(vault.status)[0] : null;
    if (key === 'Closed' || key === 'closed') return 'Closed';
    if (key === 'Liquidated' || key === 'liquidated') return 'Liquidated';
    return 'Open';
  });

  const statusBadgeClass = $derived(
    vaultStatus === 'Open'
      ? 'bg-green-500/20 text-green-400 border border-green-500/30'
      : vaultStatus === 'Liquidated'
        ? 'bg-red-500/20 text-red-400 border border-red-500/30'
        : 'bg-gray-500/20 text-gray-400 border border-gray-500/30'
  );

  // Quick stats from history
  const totalBorrowed = $derived(
    history.reduce((sum: number, item: any) => {
      const evt = item.event ?? item;
      const key = Object.keys(evt)[0];
      if (key === 'borrow_from_vault') {
        const amt = evt[key]?.borrowed_amount;
        return sum + (amt ? Number(amt) : 0);
      }
      return sum;
    }, 0)
  );

  const totalRepaid = $derived(
    history.reduce((sum: number, item: any) => {
      const evt = item.event ?? item;
      const key = Object.keys(evt)[0];
      if (key === 'repay_to_vault') {
        const amt = evt[key]?.repayed_amount;
        return sum + (amt ? Number(amt) : 0);
      }
      return sum;
    }, 0)
  );

  // Vault collateral map for event summaries
  const vaultCollateralMap = $derived(
    vault ? new Map([[Number(vault.vault_id), vault.collateral_type]]) : new Map<number, any>()
  );

  // History sorted newest-first (history from API is already in event order; reverse for newest-first)
  const historySorted = $derived([...history].reverse());

  // DataTable columns
  const historyColumns = [
    { key: 'index', label: '#', align: 'right' as const, width: '3rem' },
    { key: 'time', label: 'Time', align: 'left' as const },
    { key: 'type', label: 'Type', align: 'left' as const },
    { key: 'summary', label: 'Summary', align: 'left' as const },
  ];

  // ── Load ───────────────────────────────────────────────────────────────────
  onMount(async () => {
    loading = true;
    try {
      const id = Number($page.params.id);
      const [allVaults, vaultHistory] = await Promise.all([
        fetchAllVaults(),
        fetchVaultHistory(id)
      ]);
      vault = allVaults.find((v: any) => Number(v.vault_id) === id) ?? null;
      history = Array.isArray(vaultHistory) ? vaultHistory : [];

      if (vault) {
        const config = await publicActor.get_collateral_config(vault.collateral_type);
        collateralConfig = config[0] ?? null;
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
    <div class="empty">Loading vault #{$page.params.id}…</div>
  {:else if !vault}
    <div class="empty">Vault #{$page.params.id} not found.</div>
  {:else}
    <!-- ── Header ───────────────────────────────────────────────────────────── -->
    <div class="vault-header">
      <div class="vault-title-row">
        <h1 class="page-title">Vault #{vaultId}</h1>
        <span class="status-badge {statusBadgeClass}">{vaultStatus}</span>
      </div>
      <div class="vault-meta">
        <div class="meta-item">
          <span class="meta-label">Owner</span>
          <EntityLink type="address" id={ownerStr} />
        </div>
        <div class="meta-item">
          <span class="meta-label">Collateral Type</span>
          <TokenBadge symbol={collateralSymbol} principalId={collateralPrincipalStr} size="sm" linked={true} />
        </div>
      </div>
    </div>

    <!-- ── Health Section ─────────────────────────────────────────────────── -->
    <section class="section">
      <h2 class="section-title">Health</h2>
      <div class="health-bar-wrap glass-card">
        {#if cr !== Infinity}
          <VaultHealthBar collateralRatio={cr} liquidationRatio={liquidationRatioPct} />
        {:else}
          <span class="text-gray-400 text-sm">No debt — vault is fully collateralised</span>
        {/if}
      </div>

      <div class="stats-grid">
        <DashboardCard
          label="Collateral"
          value="{formatAmount(BigInt(collateralAmount), decimals)} {collateralSymbol}"
          subtitle={collateralValueUsd > 0 ? `≈ $${collateralValueUsd.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}` : undefined}
        />
        <DashboardCard
          label="Debt (icUSD)"
          value={formatAmount(BigInt(debtRaw))}
          subtitle={interestHuman > 0 ? `+ ${formatAmount(BigInt(interestRaw))} accrued interest` : undefined}
        />
        <DashboardCard
          label="Collateral Ratio"
          value={cr === Infinity ? '∞' : `${cr.toFixed(1)}%`}
          subtitle={`Liquidation at ${liquidationRatioPct.toFixed(0)}%`}
        />
        {#if liquidationPrice > 0}
          <DashboardCard
            label="Liquidation Price"
            value={`$${liquidationPrice.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 4 })}`}
            subtitle={distanceToLiqPct !== null
              ? distanceToLiqPct >= 0
                ? `${distanceToLiqPct.toFixed(1)}% above current price`
                : `${Math.abs(distanceToLiqPct).toFixed(1)}% below current price`
              : undefined}
            trend={distanceToLiqPct !== null && distanceToLiqPct < 20 ? 'down' : 'neutral'}
          />
        {/if}
      </div>
    </section>

    <!-- ── Quick Stats ─────────────────────────────────────────────────────── -->
    {#if history.length > 0}
      <section class="section">
        <h2 class="section-title">Lifetime Stats</h2>
        <div class="stats-grid stats-grid--3">
          <DashboardCard label="Total Borrowed" value="{formatAmount(BigInt(totalBorrowed))} icUSD" />
          <DashboardCard label="Total Repaid" value="{formatAmount(BigInt(totalRepaid))} icUSD" />
          <DashboardCard label="Operations" value={String(history.length)} />
        </div>
      </section>
    {/if}

    <!-- ── Vault History ───────────────────────────────────────────────────── -->
    <section class="section">
      <h2 class="section-title">Event History ({history.length})</h2>
      <div class="glass-card overflow-hidden">
        <DataTable
          columns={historyColumns}
          rows={historySorted}
          emptyMessage="No events found for this vault."
          loading={false}
        >
          {#snippet row(item: any, i: number)}
            {@const evt = item.event ?? item}
            {@const evtKey = Object.keys(evt)[0]}
            {@const ts = getEventTimestamp(evt)}
            {@const badgeColor = getEventBadgeColor(evt)}
            {@const summary = getEventSummary(evt, vaultCollateralMap)}
            {@const globalIdx = item.globalIndex ?? null}
            <tr class="history-row">
              <td class="px-4 py-3 text-right text-gray-500 text-xs font-mono">
                {#if globalIdx !== null}
                  <a href="/explorer/event/{globalIdx}" class="hover:text-blue-400 transition-colors">#{globalIdx}</a>
                {:else}
                  {i + 1}
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
              </td>
            </tr>
          {/snippet}
        </DataTable>
      </div>
    </section>
  {/if}
</div>

<style>
  .vault-page { max-width: 960px; margin: 0 auto; padding: 2rem 1rem; }

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
  .vault-header { margin-bottom: 2rem; }
  .vault-title-row { display: flex; align-items: center; gap: 0.75rem; margin-bottom: 0.75rem; }
  .page-title { font-size: 1.75rem; font-weight: 700; color: var(--rumi-text-primary); margin: 0; }
  .status-badge {
    font-size: 0.75rem;
    font-weight: 500;
    padding: 0.25rem 0.625rem;
    border-radius: 9999px;
  }
  .vault-meta { display: flex; align-items: center; gap: 1.5rem; flex-wrap: wrap; }
  .meta-item { display: flex; align-items: center; gap: 0.5rem; }
  .meta-label { font-size: 0.75rem; color: var(--rumi-text-muted); }

  /* Sections */
  .section { margin-bottom: 2rem; }
  .section-title { font-size: 1rem; font-weight: 600; color: var(--rumi-text-secondary); margin-bottom: 0.75rem; }

  .health-bar-wrap { padding: 1rem 1.25rem; margin-bottom: 1rem; }

  .stats-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 1rem;
  }
  .stats-grid--3 {
    grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
  }

  /* History table rows */
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
