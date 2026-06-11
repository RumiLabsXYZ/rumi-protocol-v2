<script lang="ts">
  import { onMount } from 'svelte';
  import {
    fetchBotLiquidations,
    fetchBotLiquidationCount,
    fetchStuckBotLiquidations,
    fetchActiveBotClaimVaultIds,
  } from '$services/explorer/explorerService';
  import type { LiquidationRecordV1, LiquidationStatus } from '$declarations/liquidation_bot/liquidation_bot.did';
  import LoadingSpinner from '../common/LoadingSpinner.svelte';

  export let pageSize: number = 20;
  /** Optional pre-filter: only show records with this status. */
  export let initialStatusFilter: 'all' | 'completed' | 'failed' = 'all';
  /** Hide records whose error_message contains "test_force_liquidate". */
  export let hideTestRecords: boolean = true;

  let loading = true;
  let error = '';
  let records: LiquidationRecordV1[] = [];
  let stuckRecords: LiquidationRecordV1[] = [];
  let total = 0n;
  let loadedPages = 0;
  let loadingMore = false;

  let statusFilter: 'all' | 'completed' | 'failed' = initialStatusFilter;
  let hideTests = hideTestRecords;
  // `ClaimFailed` records are no-op probes: the claim never succeeded, so no
  // funds moved. They arrive in bursts (3 retries × N borderline vaults per
  // price dip) and bury the records where money actually moved, so they're
  // hidden by default behind this toggle.
  let showClaimAttempts = false;

  function isClaimAttempt(r: LiquidationRecordV1): boolean {
    return 'ClaimFailed' in r.status;
  }

  const E8S = 100_000_000;
  const E6 = 1_000_000;

  function statusName(status: LiquidationStatus): string {
    return Object.keys(status)[0] ?? 'Unknown';
  }

  function isCompleted(status: LiquidationStatus): boolean {
    return 'Completed' in status || 'AdminResolved' in status;
  }

  function isFailed(status: LiquidationStatus): boolean {
    return 'SwapFailed' in status
      || 'ClaimFailed' in status
      || 'TransferFailed' in status
      || 'ConfirmFailed' in status;
  }

  function statusClass(status: LiquidationStatus): string {
    if ('Completed' in status) return 'ok';
    if ('AdminResolved' in status) return 'admin';
    if (isFailed(status)) return 'fail';
    return '';
  }

  function statusLabel(status: LiquidationStatus): string {
    const name = statusName(status);
    if (name === 'SwapFailed') return 'Swap Failed';
    if (name === 'ClaimFailed') return 'Claim Failed';
    if (name === 'TransferFailed') return 'Transfer Failed';
    if (name === 'ConfirmFailed') return 'Confirm Failed';
    if (name === 'AdminResolved') return 'Admin Resolved';
    return name;
  }

  function isTestRecord(r: LiquidationRecordV1): boolean {
    const msg = r.error_message[0] ?? '';
    return msg.includes('test_force_liquidate');
  }

  $: hiddenClaimAttempts = records.filter((r) => isClaimAttempt(r)).length;

  $: filtered = records.filter((r) => {
    if (hideTests && isTestRecord(r)) return false;
    if (!showClaimAttempts && isClaimAttempt(r)) return false;
    if (statusFilter === 'completed' && !isCompleted(r.status)) return false;
    if (statusFilter === 'failed' && !isFailed(r.status)) return false;
    return true;
  });

  $: canLoadMore = BigInt(records.length) < total;

  // With claim attempts hidden, a fetched page can be mostly invisible rows.
  // Keep pulling pages (bounded) until the visible table has some substance,
  // so the default view isn't a near-empty page with a "Load more" button.
  let autoFillRounds = 0;
  $: if (!loading && !loadingMore && !showClaimAttempts
         && filtered.length < Math.min(10, Number(total))
         && canLoadMore && autoFillRounds < 6) {
    autoFillRounds += 1;
    loadMore();
  }

  function fmtE8s(v: bigint | number, decimals = 4): string {
    return (Number(v) / E8S).toLocaleString(undefined, {
      minimumFractionDigits: decimals,
      maximumFractionDigits: decimals,
    });
  }

  function fmtE6(v: bigint | number, decimals = 2): string {
    return (Number(v) / E6).toLocaleString(undefined, {
      minimumFractionDigits: decimals,
      maximumFractionDigits: decimals,
    });
  }

  function fmtRelativeTime(ns: bigint): string {
    const ms = Number(ns) / 1_000_000;
    const date = new Date(ms);
    const diffMs = Date.now() - ms;
    const diffMin = Math.floor(diffMs / 60_000);
    const diffHr = Math.floor(diffMs / 3_600_000);
    const diffDay = Math.floor(diffMs / 86_400_000);
    if (diffMin < 1) return 'just now';
    if (diffMin < 60) return `${diffMin}m ago`;
    if (diffHr < 24) return `${diffHr}h ago`;
    if (diffDay < 30) return `${diffDay}d ago`;
    return date.toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' });
  }

  function fmtAbsoluteTime(ns: bigint): string {
    const ms = Number(ns) / 1_000_000;
    return new Date(ms).toLocaleString();
  }

  function fmtBps(bps: number): string {
    if (!bps) return '—';
    return `${(bps / 100).toFixed(2)}%`;
  }

  async function load() {
    try {
      loading = true;
      error = '';
      const [first, stuck, activeClaims] = await Promise.all([
        fetchBotLiquidations(0, pageSize),
        fetchStuckBotLiquidations(),
        fetchActiveBotClaimVaultIds(),
      ]);
      records = first.records;
      total = first.total;
      // The bot's `get_stuck_liquidations` is its permanent history of
      // failed records, but the protocol-side claim may have since been
      // resolved (Wave-11 BOT-001 auto-cancel, or `admin_resolve_stuck_claim`).
      // Only surface records whose vault is still in the protocol's
      // active claim set so the banner reflects actionable items.
      stuckRecords = stuck.filter((r) => activeClaims.has(BigInt(r.vault_id)));
      loadedPages = 1;
    } catch (err: any) {
      console.error('[BotLiquidationsTable] load failed:', err);
      error = err?.message ?? 'Failed to load bot liquidation history';
    } finally {
      loading = false;
    }
  }

  async function loadMore() {
    if (loadingMore || !canLoadMore) return;
    try {
      loadingMore = true;
      const next = await fetchBotLiquidations(loadedPages, pageSize);
      records = [...records, ...next.records];
      loadedPages += 1;
    } catch (err: any) {
      console.error('[BotLiquidationsTable] loadMore failed:', err);
      error = err?.message ?? 'Failed to load more records';
    } finally {
      loadingMore = false;
    }
  }

  onMount(() => { load(); });
</script>

{#if loading}
  <div class="state">
    <LoadingSpinner />
    <p class="state-text">Loading liquidation history…</p>
  </div>
{:else if error}
  <div class="state">
    <p class="state-text error">{error}</p>
    <button class="btn-secondary" on:click={load}>Retry</button>
  </div>
{:else}
  {#if stuckRecords.length > 0}
    <div class="stuck-banner">
      <div class="stuck-icon">!</div>
      <div class="stuck-body">
        <div class="stuck-title">{stuckRecords.length} stuck claim{stuckRecords.length === 1 ? '' : 's'} awaiting admin resolution</div>
        <div class="stuck-detail">
          {#each stuckRecords as r, i (r.id)}
            <span>#{r.vault_id} ({statusLabel(r.status)}){i < stuckRecords.length - 1 ? ', ' : ''}</span>
          {/each}
        </div>
      </div>
    </div>
  {/if}

  <div class="filters">
    <div class="filter-group">
      <button
        class="filter-btn"
        class:active={statusFilter === 'all'}
        on:click={() => (statusFilter = 'all')}
      >All</button>
      <button
        class="filter-btn"
        class:active={statusFilter === 'completed'}
        on:click={() => (statusFilter = 'completed')}
      >Completed</button>
      <button
        class="filter-btn"
        class:active={statusFilter === 'failed'}
        on:click={() => (statusFilter = 'failed')}
      >Failed</button>
    </div>
    <label class="hide-tests">
      <input type="checkbox" bind:checked={hideTests} />
      <span>Hide test entries</span>
    </label>
    <label class="hide-tests" title="Failed claim attempts are no-op probes: the vault's CR recovered above the liquidation threshold before the bot could claim it, so no funds moved.">
      <input type="checkbox" bind:checked={showClaimAttempts} />
      <span>Show claim attempts{hiddenClaimAttempts > 0 && !showClaimAttempts ? ` (${hiddenClaimAttempts} hidden)` : ''}</span>
    </label>
    <span class="count">{filtered.length} shown · {records.length} of {Number(total)} loaded</span>
  </div>

  {#if filtered.length === 0}
    <div class="state">
      <p class="state-text">No records match the current filter.</p>
    </div>
  {:else}
    <div class="table-wrap">
      <div class="table">
        <div class="row head">
          <span>Date</span>
          <span>Status</span>
          <span>Vault</span>
          <span class="num">Debt</span>
          <span class="num">Collateral</span>
          <span class="num">→ Treasury</span>
          <span class="num">Swapped</span>
          <span class="num">ckUSDC</span>
          <span class="num">Slip</span>
        </div>
        {#each filtered as r (r.id)}
          {@const isTest = isTestRecord(r)}
          <div class="row" class:is-test={isTest}>
            <span class="date" title={fmtAbsoluteTime(r.timestamp)}>
              {fmtRelativeTime(r.timestamp)}
            </span>
            <span class="status-cell">
              <span class="status-pill {statusClass(r.status)}">{statusLabel(r.status)}</span>
              {#if isTest}<span class="test-tag">test</span>{/if}
            </span>
            <span class="vault">#{r.vault_id.toString()}</span>
            <span class="num">{fmtE8s(r.debt_to_cover_e8s, 4)}</span>
            <span class="num">{fmtE8s(r.collateral_claimed_e8s, 4)}</span>
            <span class="num">{fmtE8s(r.icp_to_treasury_e8s, 4)}</span>
            <span class="num">{fmtE8s(r.icp_swapped_e8s, 4)}</span>
            <span class="num">{r.ckusdc_received_e6 > 0n ? fmtE6(r.ckusdc_received_e6, 2) : '—'}</span>
            <span class="num">{fmtBps(r.slippage_bps)}</span>
          </div>
          {#if r.error_message[0]}
            <div class="row-error" class:on-success={isCompleted(r.status)}>
              {isCompleted(r.status) ? 'Note: ' : 'Error: '}{r.error_message[0]}
            </div>
          {/if}
        {/each}
      </div>
    </div>

    {#if canLoadMore}
      <div class="footer">
        <button class="btn-secondary" on:click={loadMore} disabled={loadingMore}>
          {loadingMore ? 'Loading…' : 'Load more'}
        </button>
      </div>
    {/if}
  {/if}
{/if}

<style>
  .state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.75rem;
    padding: 3rem 1rem;
  }
  .state-text { color: var(--rumi-text-muted, #9ca3af); font-size: 0.875rem; }
  .state-text.error { color: var(--rumi-error, #ef4444); }

  .btn-secondary {
    background: transparent;
    color: var(--rumi-text-primary, #fff);
    border: 1px solid var(--rumi-border, rgba(255, 255, 255, 0.12));
    padding: 0.45rem 0.9rem;
    border-radius: 6px;
    cursor: pointer;
    font-size: 0.8125rem;
  }
  .btn-secondary:hover { border-color: var(--rumi-action, #3b82f6); }
  .btn-secondary:disabled { opacity: 0.5; cursor: not-allowed; }

  .stuck-banner {
    display: flex;
    gap: 0.75rem;
    align-items: flex-start;
    background: rgba(239, 68, 68, 0.06);
    border: 1px solid rgba(239, 68, 68, 0.3);
    border-radius: 8px;
    padding: 0.75rem 1rem;
    margin-bottom: 1rem;
  }
  .stuck-icon {
    flex: 0 0 1.5rem;
    width: 1.5rem;
    height: 1.5rem;
    border-radius: 50%;
    background: var(--rumi-error, #ef4444);
    color: white;
    display: flex;
    align-items: center;
    justify-content: center;
    font-weight: 700;
    font-size: 0.875rem;
  }
  .stuck-title { font-weight: 600; font-size: 0.875rem; color: var(--rumi-error, #ef4444); }
  .stuck-detail { font-size: 0.75rem; color: var(--rumi-text-muted, #9ca3af); margin-top: 0.25rem; }

  .filters {
    display: flex;
    align-items: center;
    gap: 1rem;
    margin-bottom: 0.75rem;
    flex-wrap: wrap;
    font-size: 0.8125rem;
  }
  .filter-group {
    display: inline-flex;
    border: 1px solid var(--rumi-border, rgba(255, 255, 255, 0.1));
    border-radius: 6px;
    overflow: hidden;
  }
  .filter-btn {
    background: transparent;
    color: var(--rumi-text-muted, #9ca3af);
    border: none;
    padding: 0.35rem 0.85rem;
    cursor: pointer;
    font-size: 0.8125rem;
    border-right: 1px solid var(--rumi-border, rgba(255, 255, 255, 0.1));
  }
  .filter-btn:last-child { border-right: none; }
  .filter-btn.active {
    background: var(--rumi-action, #3b82f6);
    color: white;
  }
  .hide-tests {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--rumi-text-muted, #9ca3af);
    cursor: pointer;
  }
  .count { margin-left: auto; color: var(--rumi-text-muted, #6b7280); font-size: 0.75rem; }

  .table-wrap {
    border: 1px solid var(--rumi-border, rgba(255, 255, 255, 0.06));
    border-radius: 8px;
    overflow-x: auto;
  }
  .table {
    display: flex;
    flex-direction: column;
    min-width: 760px;
  }
  .row {
    display: grid;
    grid-template-columns: 100px 130px 70px 1fr 1fr 1fr 1fr 1fr 60px;
    gap: 0.5rem;
    padding: 0.6rem 0.9rem;
    align-items: center;
    font-size: 0.8125rem;
    border-bottom: 1px solid var(--rumi-border, rgba(255, 255, 255, 0.04));
    color: var(--rumi-text-primary, #fff);
  }
  .row:last-child { border-bottom: none; }
  .row.head {
    font-size: 0.6875rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--rumi-text-muted, #6b7280);
    font-weight: 600;
    background: var(--rumi-bg-surface1, rgba(255, 255, 255, 0.02));
  }
  .row.is-test { opacity: 0.55; }
  .num { text-align: right; font-variant-numeric: tabular-nums; }
  .date { color: var(--rumi-text-secondary, #d1d5db); cursor: default; }
  .vault { font-weight: 500; }

  .status-cell {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
  }
  .status-pill {
    display: inline-block;
    padding: 0.15rem 0.55rem;
    border-radius: 999px;
    font-size: 0.6875rem;
    font-weight: 600;
    white-space: nowrap;
  }
  .status-pill.ok {
    background: rgba(34, 197, 94, 0.12);
    color: var(--rumi-success, #22c55e);
  }
  .status-pill.admin {
    background: rgba(59, 130, 246, 0.12);
    color: var(--rumi-action, #60a5fa);
  }
  .status-pill.fail {
    background: rgba(239, 68, 68, 0.12);
    color: var(--rumi-error, #f87171);
  }
  .test-tag {
    font-size: 0.625rem;
    text-transform: uppercase;
    color: var(--rumi-text-muted, #6b7280);
    border: 1px solid var(--rumi-border, rgba(255, 255, 255, 0.15));
    border-radius: 4px;
    padding: 0 0.3rem;
  }

  .row-error {
    grid-column: 1 / -1;
    padding: 0.35rem 0.9rem 0.75rem;
    font-size: 0.75rem;
    color: var(--rumi-error, #f87171);
    font-family: ui-monospace, SFMono-Regular, monospace;
    border-bottom: 1px solid var(--rumi-border, rgba(255, 255, 255, 0.04));
  }
  .row-error.on-success { color: var(--rumi-text-muted, #9ca3af); }

  .footer {
    display: flex;
    justify-content: center;
    padding: 1rem 0;
  }
</style>
