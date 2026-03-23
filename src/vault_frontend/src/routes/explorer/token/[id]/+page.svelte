<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import SearchBar from '$lib/components/explorer/SearchBar.svelte';
  import EntityLink from '$lib/components/explorer/EntityLink.svelte';
  import TokenBadge from '$lib/components/explorer/TokenBadge.svelte';
  import DashboardCard from '$lib/components/explorer/DashboardCard.svelte';
  import DataTable from '$lib/components/explorer/DataTable.svelte';
  import { fetchAllVaults, fetchEvents, explorerEvents } from '$lib/stores/explorerStore';
  import { QueryOperations } from '$lib/services/protocol/queryOperations';
  import { resolveCollateralSymbol, getEventType, getEventBadgeColor, getEventSummary, getEventTimestamp, formatTimestamp } from '$lib/utils/eventFormatters';
  import { copyToClipboard } from '$lib/utils/principalHelpers';
  import { toastStore } from '$lib/stores/toast';
  import type { CollateralInfo } from '$lib/services/types';

  const E8S = 100_000_000;

  // ── State ──────────────────────────────────────────────────────────────────
  let config = $state<CollateralInfo | null>(null);
  let allVaults = $state<any[]>([]);
  let events = $state<Array<{ event: any; globalIndex: number }>>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let copied = $state(false);

  // ── Derived ────────────────────────────────────────────────────────────────
  const tokenId = $derived($page.params.id);

  const symbol = $derived(
    config ? resolveCollateralSymbol(config.principal) : resolveCollateralSymbol(tokenId)
  );

  const statusBadgeClass = $derived.by(() => {
    const s = config?.status ?? '';
    if (s === 'Active') return 'bg-green-500/20 text-green-400 border border-green-500/30';
    if (s === 'Paused') return 'bg-yellow-500/20 text-yellow-400 border border-yellow-500/30';
    if (s === 'Frozen') return 'bg-red-500/20 text-red-400 border border-red-500/30';
    return 'bg-gray-500/20 text-gray-400 border border-gray-500/30';
  });

  // Vaults that use this collateral type
  const tokenVaults = $derived(
    allVaults.filter((v: any) => {
      const ct = v.collateral_type?.toText?.() ?? v.collateral_type?.toString?.() ?? String(v.collateral_type);
      return ct === tokenId;
    })
  );

  const openVaults = $derived(
    tokenVaults.filter((v: any) => {
      if (!v.status) return true;
      const key = Object.keys(v.status)[0];
      return key !== 'Closed' && key !== 'closed' && key !== 'Liquidated' && key !== 'liquidated';
    })
  );

  const totalCollateralRaw = $derived(
    tokenVaults.reduce((sum: number, v: any) => sum + Number(v.collateral_amount), 0)
  );

  const totalDebtRaw = $derived(
    tokenVaults.reduce((sum: number, v: any) => sum + Number(v.borrowed_icusd_amount), 0)
  );

  const decimals = $derived(config?.decimals ?? 8);

  const totalCollateralHuman = $derived(totalCollateralRaw / Math.pow(10, decimals));
  const totalCollateralUsd = $derived(totalCollateralHuman * (config?.price ?? 0));
  const totalDebtHuman = $derived(totalDebtRaw / E8S);

  const debtCeilingHuman = $derived(config ? config.debtCeiling / E8S : 0);
  const debtUtilizationPct = $derived(
    debtCeilingHuman > 0 ? (totalDebtHuman / debtCeilingHuman) * 100 : 0
  );

  // Filter events by collateral type matching this token
  const tokenEvents = $derived(
    events.filter(({ event }) => {
      const key = Object.keys(event)[0];
      const data = event[key];
      if (!data) return false;
      // Check collateral_type in event data
      const ct = data.collateral_type ?? data.vault?.collateral_type;
      if (!ct) return false;
      const ctStr = ct?.toText?.() ?? ct?.toString?.() ?? String(ct);
      return ctStr === tokenId;
    })
  );

  const eventsSorted = $derived(
    [...tokenEvents].sort((a, b) => (b.globalIndex ?? 0) - (a.globalIndex ?? 0))
  );

  const vaultCollateralMap = $derived.by(() => {
    const map = new Map<number, any>();
    for (const v of tokenVaults) {
      map.set(Number(v.vault_id), v.collateral_type);
    }
    return map;
  });

  // Config table rows
  const configRows = $derived.by(() => {
    if (!config) return [];
    return [
      { key: 'Borrow Threshold (Min CR)', value: config.minimumCr > 0 ? `${(config.minimumCr * 100).toFixed(0)}%` : '—' },
      { key: 'Liquidation Ratio', value: config.liquidationCr > 0 ? `${(config.liquidationCr * 100).toFixed(0)}%` : '—' },
      { key: 'Liquidation Bonus', value: config.liquidationBonus > 0 ? `${(config.liquidationBonus * 100).toFixed(2)}%` : '—' },
      { key: 'Recovery Target CR', value: config.recoveryTargetCr > 0 ? `${(config.recoveryTargetCr * 100).toFixed(0)}%` : '—' },
      { key: 'Borrowing Fee (one-time)', value: config.borrowingFee > 0 ? `${(config.borrowingFee * 100).toFixed(3)}%` : '0%' },
      { key: 'Min Vault Debt', value: config.minVaultDebt > 0 ? `${(config.minVaultDebt / E8S).toLocaleString('en-US', { minimumFractionDigits: 2 })} icUSD` : '—' },
      { key: 'Debt Ceiling', value: debtCeilingHuman > 0 ? `${debtCeilingHuman.toLocaleString('en-US', { minimumFractionDigits: 2 })} icUSD` : '—' },
      { key: 'Ledger Fee', value: config.ledgerFee > 0 ? `${(config.ledgerFee / Math.pow(10, decimals)).toFixed(decimals > 8 ? 10 : 8)} ${symbol}` : '—' },
      { key: 'Decimals', value: String(decimals) },
      { key: 'Ledger Canister', value: config.ledgerCanisterId },
    ];
  });

  // DataTable columns
  const activityColumns = [
    { key: 'index', label: '#', align: 'right' as const, width: '3rem' },
    { key: 'time', label: 'Time', align: 'left' as const },
    { key: 'type', label: 'Type', align: 'left' as const },
    { key: 'summary', label: 'Summary', align: 'left' as const },
  ];

  // ── Helpers ────────────────────────────────────────────────────────────────
  async function handleCopy() {
    const success = await copyToClipboard(tokenId);
    if (success) {
      copied = true;
      toastStore.success('Copied!');
      setTimeout(() => { copied = false; }, 2000);
    }
  }

  // ── Load ───────────────────────────────────────────────────────────────────
  onMount(async () => {
    loading = true;
    error = null;
    try {
      const id = $page.params.id;
      const [cfg, vaults] = await Promise.all([
        QueryOperations.getCollateralConfig(id),
        fetchAllVaults(),
      ]);

      if (!cfg) {
        error = `Token ${id} is not a known collateral type.`;
        return;
      }
      config = cfg;
      allVaults = vaults;

      // Fetch a batch of recent events for client-side filtering
      await fetchEvents(0);
      // explorerEvents store is updated by fetchEvents
    } catch (e) {
      console.error('Failed to load token page:', e);
      error = 'Failed to load token data. Please try again.';
    } finally {
      loading = false;
    }
  });

  // Subscribe to explorerEvents store
  let unsubscribeEvents: (() => void) | null = null;
  onMount(() => {
    unsubscribeEvents = explorerEvents.subscribe((evts) => {
      events = evts;
    });
    return () => unsubscribeEvents?.();
  });
</script>

<div class="token-page">
  <a href="/explorer" class="back-link">← Back to Explorer</a>

  <div class="search-row"><SearchBar /></div>

  {#if loading}
    <div class="empty">Loading token data…</div>
  {:else if error}
    <div class="empty error-msg">{error}</div>
  {:else if config}
    <!-- ── Header ─────────────────────────────────────────────────────────── -->
    <div class="token-header">
      <div class="token-title-row">
        <TokenBadge {symbol} principalId={tokenId} size="md" linked={false} />
        <h1 class="page-title">{symbol}</h1>
        <span class="status-badge {statusBadgeClass}">{config.status}</span>
      </div>

      <div class="token-meta">
        <div class="meta-item">
          <span class="meta-label">Ledger Canister</span>
          <EntityLink type="canister" id={config.ledgerCanisterId} />
        </div>
        <div class="meta-item">
          <span class="meta-label">Decimals</span>
          <span class="meta-value">{config.decimals}</span>
        </div>
        {#if config.price > 0}
          <div class="meta-item">
            <span class="meta-label">Current Price</span>
            <span class="meta-value price">
              ${config.price.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 4 })}
            </span>
          </div>
        {/if}
        <div class="meta-item">
          <button class="copy-btn" onclick={handleCopy} title="Copy canister ID">
            <span class="font-mono text-xs text-gray-400">{tokenId.substring(0, 10)}…</span>
            <span class="copy-icon">{copied ? '✓' : '⧉'}</span>
          </button>
        </div>
      </div>
    </div>

    <!-- ── Collateral Stats ──────────────────────────────────────────────── -->
    <section class="section">
      <h2 class="section-title">Collateral Stats</h2>
      <div class="stats-grid">
        <DashboardCard
          label="Total Locked as Collateral"
          value="{totalCollateralHuman.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 4 })} {symbol}"
          subtitle={totalCollateralUsd > 0 ? `≈ $${totalCollateralUsd.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}` : undefined}
        />
        <DashboardCard
          label="Total Debt Minted"
          value="{totalDebtHuman.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })} icUSD"
        />
        <DashboardCard
          label="Active Vaults"
          value={String(openVaults.length)}
          subtitle={tokenVaults.length !== openVaults.length ? `${tokenVaults.length} total (incl. closed)` : undefined}
        />
        <DashboardCard
          label="Debt Ceiling Utilization"
          value={debtCeilingHuman > 0 ? `${debtUtilizationPct.toFixed(1)}%` : '—'}
          subtitle={debtCeilingHuman > 0 ? `${totalDebtHuman.toLocaleString('en-US', { maximumFractionDigits: 0 })} / ${debtCeilingHuman.toLocaleString('en-US', { maximumFractionDigits: 0 })} icUSD` : undefined}
          trend={debtUtilizationPct > 90 ? 'down' : debtUtilizationPct > 70 ? 'neutral' : 'up'}
        />
        <DashboardCard
          label="Borrowing Fee"
          value={config.borrowingFee > 0 ? `${(config.borrowingFee * 100).toFixed(3)}%` : '0%'}
          subtitle="One-time at mint"
        />
        {#if (config as any).interestRateApr !== undefined && (config as any).interestRateApr > 0}
          <DashboardCard
            label="Interest Rate (APR)"
            value={`${((config as any).interestRateApr * 100).toFixed(2)}%`}
          />
        {/if}
      </div>
    </section>

    <!-- ── Configuration ────────────────────────────────────────────────── -->
    <section class="section">
      <h2 class="section-title">Configuration</h2>
      <div class="glass-card config-table-wrap">
        <table class="config-table">
          <tbody>
            {#each configRows as row}
              <tr class="config-row">
                <td class="config-key">{row.key}</td>
                <td class="config-val">
                  {#if row.key === 'Ledger Canister'}
                    <EntityLink type="canister" id={row.value} />
                  {:else}
                    <span class="font-mono">{row.value}</span>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </section>

    <!-- ── Token Activity ───────────────────────────────────────────────── -->
    <section class="section">
      <h2 class="section-title">
        Recent Activity
        {#if eventsSorted.length > 0}
          <span class="count-badge">{eventsSorted.length} events found</span>
        {/if}
      </h2>
      <div class="glass-card overflow-hidden">
        <DataTable
          columns={activityColumns}
          rows={eventsSorted}
          emptyMessage="No recent events found for this token. Events are filtered from the latest batch."
          loading={false}
        >
          {#snippet row(item: any, i: number)}
            {@const evt = item.event ?? item}
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
      <p class="note">Showing events from the latest batch of 100. Only events that include collateral type are matched.</p>
    </section>
  {/if}
</div>

<style>
  .token-page { max-width: 960px; margin: 0 auto; padding: 2rem 1rem; }

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
  .token-header { margin-bottom: 2rem; }
  .token-title-row { display: flex; align-items: center; gap: 0.75rem; margin-bottom: 0.75rem; flex-wrap: wrap; }
  .page-title { font-size: 1.75rem; font-weight: 700; color: var(--rumi-text-primary); margin: 0; }

  .status-badge {
    font-size: 0.75rem;
    font-weight: 500;
    padding: 0.25rem 0.625rem;
    border-radius: 9999px;
  }

  .token-meta { display: flex; align-items: center; gap: 1.5rem; flex-wrap: wrap; }
  .meta-item { display: flex; align-items: center; gap: 0.5rem; }
  .meta-label { font-size: 0.75rem; color: var(--rumi-text-muted); }
  .meta-value { font-size: 0.875rem; color: var(--rumi-text-secondary); }
  .meta-value.price { color: var(--rumi-safe, #22c55e); font-weight: 500; }

  .copy-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.375rem;
    background: none;
    border: none;
    cursor: pointer;
    padding: 0.25rem 0.5rem;
    border-radius: 0.375rem;
    transition: background 0.15s;
  }
  .copy-btn:hover { background: rgba(255,255,255,0.06); }
  .copy-icon { font-size: 0.75rem; color: var(--rumi-text-muted); }

  /* Sections */
  .section { margin-bottom: 2rem; }
  .section-title {
    font-size: 1rem;
    font-weight: 600;
    color: var(--rumi-text-secondary);
    margin-bottom: 0.75rem;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .count-badge {
    font-size: 0.7rem;
    font-weight: 400;
    padding: 0.125rem 0.5rem;
    border-radius: 9999px;
    background: rgba(255,255,255,0.06);
    color: var(--rumi-text-muted);
  }

  .stats-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 1rem;
  }

  /* Config table */
  .config-table-wrap { overflow: hidden; }
  .config-table { width: 100%; border-collapse: collapse; }
  .config-row { border-bottom: 1px solid rgba(255,255,255,0.05); }
  .config-row:last-child { border-bottom: none; }
  .config-key {
    padding: 0.625rem 1rem;
    font-size: 0.8rem;
    color: var(--rumi-text-muted);
    width: 45%;
    vertical-align: middle;
  }
  .config-val {
    padding: 0.625rem 1rem;
    font-size: 0.875rem;
    color: var(--rumi-text-primary);
    vertical-align: middle;
  }

  /* Activity table rows */
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

  .note {
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
    margin-top: 0.5rem;
    text-align: center;
  }

  .empty { text-align: center; padding: 3rem; color: var(--rumi-text-muted); }
  .error-msg { color: var(--rumi-danger, #ef4444); }
</style>
