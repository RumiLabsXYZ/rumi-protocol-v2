<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import EntityLink from '$lib/components/explorer/EntityLink.svelte';
  import TokenBadge from '$lib/components/explorer/TokenBadge.svelte';
  import DataTable from '$lib/components/explorer/DataTable.svelte';
  import ErrorState from '$lib/components/explorer/ErrorState.svelte';
  import { fetchEventsByPrincipal } from '$lib/stores/explorerStore';
  import { CANISTER_IDS } from '$lib/config';
  import { copyToClipboard } from '$lib/utils/principalHelpers';
  import { displayEvent, wrapBackendEvent } from '$lib/utils/displayEvent';
  import { timeAgo, formatTimestamp } from '$lib/utils/explorerHelpers';
  import { toastStore } from '$lib/stores/toast';

  // ── Canister role map ─────────────────────────────────────────────────────
  // Maps known canister IDs to human-readable role labels.
  // Ledger canisters additionally get a { isLedger: true, symbol } entry.
  interface CanisterRole {
    name: string;
    isLedger?: boolean;
    symbol?: string;
    category: 'protocol' | 'ledger' | 'pool' | 'external';
  }

  const KNOWN_CANISTERS: Record<string, CanisterRole> = {
    // Protocol
    [CANISTER_IDS.PROTOCOL]: { name: 'Protocol Backend', category: 'protocol' },
    [CANISTER_IDS.STABILITY_POOL]: { name: 'Stability Pool', category: 'pool' },
    [CANISTER_IDS.THREEPOOL]: { name: '3Pool AMM (icUSD/ckUSDT/ckUSDC)', category: 'pool' },
    [CANISTER_IDS.TREASURY]: { name: 'Treasury', category: 'protocol' },
    // Ledgers
    [CANISTER_IDS.ICP_LEDGER]: { name: 'ICP Ledger', isLedger: true, symbol: 'ICP', category: 'ledger' },
    [CANISTER_IDS.ICUSD_LEDGER]: { name: 'icUSD Ledger', isLedger: true, symbol: 'icUSD', category: 'ledger' },
    [CANISTER_IDS.CKUSDT_LEDGER]: { name: 'ckUSDT Ledger', isLedger: true, symbol: 'ckUSDT', category: 'ledger' },
    [CANISTER_IDS.CKUSDC_LEDGER]: { name: 'ckUSDC Ledger', isLedger: true, symbol: 'ckUSDC', category: 'ledger' },
    // Well-known collateral ledgers (not in CANISTER_IDS but commonly referenced)
    'mxzaz-hqaaa-aaaar-qaada-cai': { name: 'ckBTC Ledger', isLedger: true, symbol: 'ckBTC', category: 'ledger' },
    'ss2fx-dyaaa-aaaar-qacoq-cai': { name: 'ckETH Ledger', isLedger: true, symbol: 'ckETH', category: 'ledger' },
    'o7oak-6yaaa-aaaap-qhgbq-cai': { name: 'ckXAUT Ledger', isLedger: true, symbol: 'ckXAUT', category: 'ledger' },
  };

  // ── State ──────────────────────────────────────────────────────────────────
  let events = $state<Array<{ event: any; globalIndex: number }>>([]);
  let eventsLoading = $state(true);
  let eventsError = $state<string | null>(null);
  let copied = $state(false);

  // ── Derived ────────────────────────────────────────────────────────────────
  const canisterId = $derived($page.params.id);

  const role = $derived<CanisterRole>(
    KNOWN_CANISTERS[canisterId] ?? { name: 'External Canister', category: 'external' }
  );

  const roleBadgeClass = $derived.by(() => {
    switch (role.category) {
      case 'protocol': return 'bg-purple-500/20 text-purple-400 border border-purple-500/30';
      case 'pool':     return 'bg-blue-500/20 text-blue-400 border border-blue-500/30';
      case 'ledger':   return 'bg-green-500/20 text-green-400 border border-green-500/30';
      default:         return 'bg-gray-500/20 text-gray-400 border border-gray-500/30';
    }
  });

  const dashboardUrl = $derived(
    `https://dashboard.internetcomputer.org/canister/${canisterId}`
  );

  const eventsSorted = $derived(
    [...events].sort((a, b) => (b.globalIndex ?? 0) - (a.globalIndex ?? 0))
  );

  // DataTable columns
  const activityColumns = [
    { key: 'index', label: '#', align: 'right' as const, width: '3rem' },
    { key: 'time', label: 'Time', align: 'left' as const },
    { key: 'type', label: 'Type', align: 'left' as const },
    { key: 'summary', label: 'Summary', align: 'left' as const },
  ];

  // ── Helpers ────────────────────────────────────────────────────────────────
  async function handleCopy() {
    const success = await copyToClipboard(canisterId);
    if (success) {
      copied = true;
      toastStore.success('Copied!');
      setTimeout(() => { copied = false; }, 2000);
    }
  }

  // ── Load ───────────────────────────────────────────────────────────────────
  async function loadEvents() {
    eventsLoading = true;
    eventsError = null;
    try {
      const id = $page.params.id;
      const result = await fetchEventsByPrincipal(id);
      events = Array.isArray(result) ? result : [];
    } catch (e) {
      console.error('Failed to fetch canister events:', e);
      events = [];
      eventsError = 'Could not fetch events for this canister. The backend may be briefly unavailable.';
    } finally {
      eventsLoading = false;
    }
  }

  onMount(loadEvents);
</script>

<div class="canister-page">
  <a href="/explorer" class="back-link">← Back to Explorer</a>

  <!-- ── Header ───────────────────────────────────────────────────────────── -->
  <div class="canister-header">
    <div class="canister-title-row">
      {#if role.isLedger && role.symbol}
        <TokenBadge symbol={role.symbol} principalId={canisterId} size="md" linked={false} />
      {/if}
      <h1 class="page-title">Canister</h1>
      <span class="role-badge {roleBadgeClass}">{role.name}</span>
    </div>

    <div class="id-row">
      <span class="canister-id-mono">{canisterId}</span>
      <button class="copy-btn" onclick={handleCopy} title="Copy canister ID">
        <span class="copy-icon">{copied ? '✓ Copied' : '⧉ Copy'}</span>
      </button>
    </div>

    <div class="canister-links">
      <a
        href={dashboardUrl}
        target="_blank"
        rel="noopener noreferrer"
        class="ic-dashboard-link"
      >
        View on IC Dashboard ↗
      </a>

      {#if role.isLedger && role.symbol}
        <a href="/explorer/e/token/{canisterId}" class="token-link">
          View token details →
        </a>
      {/if}
    </div>
  </div>

  <!-- ── Info Section ──────────────────────────────────────────────────────── -->
  {#if role.category !== 'external'}
    <section class="section">
      <h2 class="section-title">About This Canister</h2>
      <div class="glass-card info-card">
        <div class="info-row">
          <span class="info-label">Role</span>
          <span class="info-val">{role.name}</span>
        </div>
        <div class="info-row">
          <span class="info-label">Canister ID</span>
          <span class="info-val font-mono text-xs break-all">{canisterId}</span>
        </div>
        {#if role.isLedger && role.symbol}
          <div class="info-row">
            <span class="info-label">Token</span>
            <div class="info-val">
              <TokenBadge symbol={role.symbol} principalId={canisterId} size="sm" linked={true} />
            </div>
          </div>
        {/if}
        <div class="info-row">
          <span class="info-label">IC Dashboard</span>
          <a
            href={dashboardUrl}
            target="_blank"
            rel="noopener noreferrer"
            class="info-link"
          >
            {dashboardUrl.replace('https://', '')}
          </a>
        </div>
      </div>
    </section>
  {:else}
    <section class="section">
      <div class="glass-card info-card">
        <div class="info-row">
          <span class="info-label">Canister ID</span>
          <span class="info-val font-mono text-xs break-all">{canisterId}</span>
        </div>
        <div class="info-row">
          <span class="info-label">IC Dashboard</span>
          <a
            href={dashboardUrl}
            target="_blank"
            rel="noopener noreferrer"
            class="info-link"
          >
            {dashboardUrl.replace('https://', '')}
          </a>
        </div>
      </div>
    </section>
  {/if}

  <!-- ── Activity ─────────────────────────────────────────────────────────── -->
  <section class="section">
    <h2 class="section-title">
      Protocol Activity
      {#if eventsSorted.length > 0 && !eventsLoading}
        <span class="count-badge">{eventsSorted.length} events</span>
      {/if}
    </h2>
    {#if eventsError}
      <div class="glass-card overflow-hidden">
        <ErrorState message={eventsError} onRetry={loadEvents} />
      </div>
    {:else}
    <div class="glass-card overflow-hidden">
      <DataTable
        columns={activityColumns}
        data={eventsSorted}
        emptyMessage="No protocol events found involving this canister."
        loading={eventsLoading}
      >
        {#snippet row(item: any, i: number)}
          {@const evt = item.event ?? item}
          {@const globalIdx = item.globalIndex ?? null}
          {@const display = displayEvent(wrapBackendEvent(evt, globalIdx ?? 0))}
          <tr class="history-row">
            <td class="px-4 py-3 text-right text-gray-500 text-xs font-mono">
              {#if globalIdx !== null}
                <a href={display.detailHref} class="hover:text-blue-400 transition-colors">#{globalIdx}</a>
              {:else}
                {i + 1}
              {/if}
            </td>
            <td class="px-4 py-3 text-gray-400 text-xs whitespace-nowrap">
              {#if display.timestamp}
                <span title={formatTimestamp(display.timestamp)}>{timeAgo(display.timestamp)}</span>
              {:else}
                —
              {/if}
            </td>
            <td class="px-4 py-3">
              <span class="event-badge {display.formatted.badgeColor}">
                {display.formatted.typeName}
              </span>
            </td>
            <td class="px-4 py-3 text-gray-300 text-sm">
              {display.formatted.summary}
            </td>
          </tr>
        {/snippet}
      </DataTable>
    </div>
    {/if}
    <p class="note">Events are matched by principal — only events where this canister appears as caller or owner are shown.</p>
  </section>
</div>

<style>
  .canister-page { max-width: 960px; margin: 0 auto; padding: 2rem 1rem; }

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
  .canister-header { margin-bottom: 2rem; }

  .canister-title-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.75rem;
    flex-wrap: wrap;
  }

  .page-title {
    font-size: 1.75rem;
    font-weight: 700;
    color: var(--rumi-text-primary);
    margin: 0;
  }

  .role-badge {
    font-size: 0.75rem;
    font-weight: 500;
    padding: 0.25rem 0.625rem;
    border-radius: 9999px;
  }

  .id-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.75rem;
    flex-wrap: wrap;
  }

  .canister-id-mono {
    font-family: monospace;
    font-size: 0.85rem;
    color: var(--rumi-text-secondary);
    word-break: break-all;
  }

  .copy-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.375rem;
    background: rgba(255,255,255,0.06);
    border: 1px solid rgba(255,255,255,0.1);
    border-radius: 0.375rem;
    padding: 0.25rem 0.625rem;
    cursor: pointer;
    transition: background 0.15s;
  }
  .copy-btn:hover { background: rgba(255,255,255,0.1); }
  .copy-icon { font-size: 0.75rem; color: var(--rumi-text-muted); }

  .canister-links {
    display: flex;
    align-items: center;
    gap: 1.5rem;
    flex-wrap: wrap;
  }

  .ic-dashboard-link {
    font-size: 0.875rem;
    color: var(--rumi-purple-accent);
    text-decoration: none;
    transition: color 0.15s;
  }
  .ic-dashboard-link:hover { text-decoration: underline; color: #a78bfa; }

  .token-link {
    font-size: 0.875rem;
    color: var(--rumi-safe, #22c55e);
    text-decoration: none;
    transition: color 0.15s;
  }
  .token-link:hover { text-decoration: underline; }

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

  /* Info card */
  .info-card { padding: 0; overflow: hidden; }
  .info-row {
    display: flex;
    align-items: flex-start;
    gap: 1rem;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid rgba(255,255,255,0.05);
  }
  .info-row:last-child { border-bottom: none; }
  .info-label {
    font-size: 0.8rem;
    color: var(--rumi-text-muted);
    min-width: 120px;
    flex-shrink: 0;
    padding-top: 0.125rem;
  }
  .info-val {
    font-size: 0.875rem;
    color: var(--rumi-text-primary);
  }
  .info-link {
    font-size: 0.875rem;
    color: var(--rumi-purple-accent);
    text-decoration: none;
    word-break: break-all;
  }
  .info-link:hover { text-decoration: underline; }

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
</style>
