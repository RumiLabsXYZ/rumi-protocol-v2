<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import SearchBar from '$lib/components/explorer/SearchBar.svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { getEventType, getEventCategory, getEventBadgeColor, getEventSummary, getEventKey, getEventVaultId, formatTimestamp, formatAmount } from '$lib/utils/eventFormatters';

  let event: any = null;
  let loading = true;

  $: eventIndex = Number($page.params.index);
  $: key = event ? getEventKey(event) : '';
  $: data = event ? event[key] : null;
  $: type = event ? getEventType(event) : '';
  $: badgeColor = event ? getEventBadgeColor(event) : '';
  $: summary = event ? getEventSummary(event) : '';
  $: vaultId = event ? getEventVaultId(event) : null;

  onMount(async () => {
    loading = true;
    try {
      const events = await publicActor.get_events({
        start: BigInt(eventIndex),
        length: BigInt(1)
      });
      event = events[0] || null;
    } catch (e) {
      console.error('Failed to load event:', e);
    } finally {
      loading = false;
    }
  });

  function formatValue(val: any): string {
    if (val === null || val === undefined) return '—';
    // Candid optionals: [] = None, [value] = Some(value)
    if (Array.isArray(val)) {
      if (val.length === 0) return '—';
      if (val.length === 1) return formatValue(val[0]);
    }
    if (typeof val === 'bigint') return val.toLocaleString();
    if (typeof val === 'number') return val.toLocaleString();
    if (typeof val === 'object' && (val._isPrincipal || val.toText)) return val.toString();
    if (typeof val === 'object' && val.toString && val.toString !== Object.prototype.toString) return val.toString();
    return String(val);
  }
</script>

<div class="event-page">
  <a href="/explorer" class="back-link">← Back to Explorer</a>

  <div class="search-row"><SearchBar /></div>

  {#if loading}
    <div class="loading">Loading event #{eventIndex}…</div>
  {:else if !event}
    <div class="empty">Event #{eventIndex} not found.</div>
  {:else}
    <h1 class="page-title">Event #{eventIndex}</h1>

    <div class="event-header">
      <span class="event-badge" style="background:{badgeColor}20; color:{badgeColor}; border:1px solid {badgeColor}40;">
        {type}
      </span>
      <span class="event-summary-text">{summary}</span>
    </div>

    <div class="event-detail glass-card">
      <h3 class="detail-title">Details</h3>
      <div class="detail-grid">
        <div class="detail-item">
          <span class="label">Type</span>
          <span class="value">{key}</span>
        </div>
        {#if vaultId !== null}
          <div class="detail-item">
            <span class="label">Vault</span>
            <a href="/explorer/vault/{vaultId}" class="value link">#{vaultId}</a>
          </div>
        {/if}
        {#if data}
          {#each Object.entries(data) as [field, val]}
            <div class="detail-item">
              <span class="label">{field.replace(/_/g, ' ')}</span>
              {#if field === 'owner' || field === 'liquidator' || field === 'caller'}
                <a href="/explorer/address/{formatValue(val)}" class="value link">{formatValue(val)}</a>
              {:else if field === 'vault' && typeof val === 'object'}
                <span class="value">Vault #{val.vault_id?.toString()}</span>
              {:else if (field.includes('amount') || field.includes('margin') || field.includes('payment') || field === 'fee_amount') && (typeof val === 'bigint' || typeof val === 'number')}
                <span class="value key-number">{formatAmount(val as any)}</span>
              {:else}
                <span class="value">{formatValue(val)}</span>
              {/if}
            </div>
          {/each}
        {/if}
      </div>
    </div>

    <details class="raw-data">
      <summary>Raw Event Data</summary>
      <pre>{JSON.stringify(event, (_, v) => typeof v === 'bigint' ? v.toString() : v, 2)}</pre>
    </details>
  {/if}
</div>

<style>
  .event-page { max-width:900px; margin:0 auto; padding:2rem 1rem; }
  .back-link { color:var(--rumi-purple-accent); text-decoration:none; font-size:0.875rem; display:inline-block; margin-bottom:1rem; }
  .back-link:hover { text-decoration:underline; }
  .search-row { margin-bottom:1.5rem; display:flex; justify-content:center; }
  .event-header { display:flex; align-items:center; gap:0.75rem; margin-bottom:1.5rem; flex-wrap:wrap; }
  .event-badge { font-size:0.8125rem; font-weight:500; padding:0.25rem 0.75rem; border-radius:9999px; }
  .event-summary-text { font-size:1rem; color:var(--rumi-text-secondary); }
  .event-detail { padding:1.25rem; margin-bottom:1.5rem; }
  .detail-title { margin:0 0 0.75rem; font-size:0.9375rem; }
  .detail-grid { display:flex; flex-direction:column; gap:0.5rem; }
  .detail-item { display:flex; justify-content:space-between; align-items:baseline; gap:1rem; padding:0.375rem 0; border-bottom:1px solid var(--rumi-border); }
  .detail-item:last-child { border-bottom:none; }
  .label { font-size:0.8125rem; color:var(--rumi-text-muted); text-transform:capitalize; }
  .value { font-size:0.8125rem; color:var(--rumi-text-primary); text-align:right; word-break:break-all; }
  .link { color:var(--rumi-purple-accent); text-decoration:none; }
  .link:hover { text-decoration:underline; }
  .raw-data { margin-top:1rem; }
  .raw-data summary { cursor:pointer; color:var(--rumi-text-muted); font-size:0.8125rem; padding:0.5rem; }
  .raw-data pre { font-size:0.75rem; overflow-x:auto; background:var(--rumi-bg-surface-2); padding:1rem; border-radius:0.5rem; color:var(--rumi-text-secondary); }
  .loading, .empty { text-align:center; padding:3rem; color:var(--rumi-text-muted); }
</style>
