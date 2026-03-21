<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import SearchBar from '$lib/components/explorer/SearchBar.svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { getEventType, getEventCategory, getEventBadgeColor, getEventSummary, getEventKey, getEventVaultId, formatTimestamp, formatAmount, resolveCollateralSymbol } from '$lib/utils/eventFormatters';

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

  // Check if a value looks like a Principal (canister ID)
  function isPrincipalLike(val: any): boolean {
    if (!val) return false;
    if (val._isPrincipal || val.toText) return true;
    if (typeof val === 'string' && /^[a-z0-9]{5}-[a-z0-9]{5}-[a-z0-9]{5}-[a-z0-9]{5}-[a-z0-9]{3}$/.test(val)) return true;
    return false;
  }

  // Human-readable labels for CollateralConfig fields
  const CONFIG_LABELS: Record<string, string> = {
    ledger_canister_id: 'Ledger Canister',
    decimals: 'Decimals',
    liquidation_ratio: 'Liquidation Ratio',
    borrow_threshold_ratio: 'Borrow Threshold (Recovery CR)',
    liquidation_bonus: 'Liquidation Bonus',
    borrowing_fee: 'Borrowing Fee',
    interest_rate_apr: 'Interest Rate (APR)',
    debt_ceiling: 'Debt Ceiling',
    min_vault_debt: 'Min Vault Debt',
    ledger_fee: 'Ledger Fee',
    price_source: 'Price Source',
    status: 'Status',
    last_price: 'Last Price',
    last_price_timestamp: 'Last Price Update',
    redemption_fee_floor: 'Redemption Fee Floor',
    redemption_fee_ceiling: 'Redemption Fee Ceiling',
    current_base_rate: 'Current Base Rate',
    last_redemption_time: 'Last Redemption Time',
    recovery_target_cr: 'Recovery Target CR',
    min_collateral_amount: 'Min Collateral Amount',
    recovery_borrowing_fee: 'Recovery Borrowing Fee',
    recovery_interest_rate: 'Recovery Interest Rate',
  };

  // Format a config field value for display
  function formatConfigValue(field: string, val: any): string {
    if (val === null || val === undefined) return '—';
    // Candid optionals
    if (Array.isArray(val)) {
      if (val.length === 0) return '—';
      if (val.length === 1) return formatConfigValue(field, val[0]);
    }
    // Principals
    if (isPrincipalLike(val)) {
      const text = val?.toString?.() ?? val?.toText?.() ?? String(val);
      const symbol = resolveCollateralSymbol(val);
      return symbol !== text.substring(0, 5) + '…' ? `${symbol} (${text})` : text;
    }
    // Ratios come as objects with a single numeric value or as numbers
    if (typeof val === 'object' && !Array.isArray(val)) {
      // Ratio type might serialize as { "0": "0.005" } or similar
      const keys = Object.keys(val);
      if (keys.length === 1 && keys[0] === '0') return val['0'];
      // Status enum: { Active: null } or { Frozen: null }
      const statusKey = keys.find(k => val[k] === null);
      if (statusKey) return statusKey;
      // PriceSource: { XRC: { base_asset: ..., quote_asset: ... } }
      const sourceKey = keys[0];
      if (sourceKey && typeof val[sourceKey] === 'object') {
        return `${sourceKey}: ${JSON.stringify(val[sourceKey], (_, v) => typeof v === 'bigint' ? v.toString() : v)}`;
      }
      return JSON.stringify(val, (_, v) => typeof v === 'bigint' ? v.toString() : v);
    }
    // Ratio-like strings (percentages)
    if (typeof val === 'string' && !isNaN(Number(val))) {
      const num = Number(val);
      if (field.includes('ratio') || field.includes('bonus') || field === 'recovery_target_cr') {
        return `${(num * 100).toFixed(1)}%`;
      }
      if (field.includes('fee') || field.includes('rate') || field === 'current_base_rate') {
        return `${(num * 100).toFixed(2)}%`;
      }
    }
    if (typeof val === 'bigint') return val.toLocaleString();
    if (typeof val === 'number') {
      if (field === 'last_price') return `$${val.toFixed(4)}`;
      return val.toLocaleString();
    }
    return String(val);
  }

  // Check if this event has a config object that should be expanded
  function hasExpandableConfig(eventKey: string): boolean {
    return ['update_collateral_config', 'add_collateral_type'].includes(eventKey);
  }

  // Fields that should be shown with collateral symbol resolution
  const COLLATERAL_TYPE_FIELDS = ['collateral_type', 'treasury'];

  // Check if a field name represents a collateral type / principal that should be resolved
  function isCollateralTypeField(field: string): boolean {
    return COLLATERAL_TYPE_FIELDS.includes(field);
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
            {#if field === 'config' && hasExpandableConfig(key)}
              <!-- Expand CollateralConfig into individual fields -->
              <div class="detail-item config-header">
                <span class="label config-label">Configuration</span>
                <span class="value"></span>
              </div>
              {#if typeof val === 'object' && val !== null}
                {#each Object.entries(val) as [configField, configVal]}
                  <div class="detail-item config-item">
                    <span class="label">{CONFIG_LABELS[configField] || configField.replace(/_/g, ' ')}</span>
                    <span class="value">{formatConfigValue(configField, configVal)}</span>
                  </div>
                {/each}
              {/if}
            {:else}
              <div class="detail-item">
                <span class="label">{field.replace(/_/g, ' ')}</span>
                {#if field === 'owner' || field === 'liquidator' || field === 'caller'}
                  <a href="/explorer/address/{formatValue(val)}" class="value link">{formatValue(val)}</a>
                {:else if field === 'vault' && typeof val === 'object'}
                  <span class="value">Vault #{val.vault_id?.toString()}</span>
                {:else if field === 'timestamp' && (typeof val === 'bigint' || typeof val === 'number' || (Array.isArray(val) && val.length > 0))}
                  <span class="value">{formatTimestamp(Array.isArray(val) ? val[0] : val)}</span>
                {:else if isCollateralTypeField(field) && isPrincipalLike(val)}
                  <span class="value">{resolveCollateralSymbol(val)} <span class="principal-hint">({formatValue(val)})</span></span>
                {:else if (field.includes('amount') || field.includes('margin') || field.includes('payment') || field === 'fee_amount') && (typeof val === 'bigint' || typeof val === 'number')}
                  <span class="value key-number">{formatAmount(val as any)}</span>
                {:else if field === 'status' && typeof val === 'object' && val !== null}
                  <span class="value">{Object.keys(val).find(k => val[k] === null) || formatValue(val)}</span>
                {:else if field === 'description' && (typeof val === 'string' || (Array.isArray(val) && val.length > 0))}
                  <span class="value">{Array.isArray(val) ? val[0] : val}</span>
                {:else}
                  <span class="value">{formatValue(val)}</span>
                {/if}
              </div>
            {/if}
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
  .config-header { border-bottom:none; padding-bottom:0; }
  .config-label { font-weight:600; font-size:0.875rem; color:var(--rumi-text-secondary); }
  .config-item { padding-left:1rem; }
  .config-item .label { font-size:0.75rem; }
  .principal-hint { font-size:0.6875rem; color:var(--rumi-text-muted); }
  .raw-data { margin-top:1rem; }
  .raw-data summary { cursor:pointer; color:var(--rumi-text-muted); font-size:0.8125rem; padding:0.5rem; }
  .raw-data pre { font-size:0.75rem; overflow-x:auto; background:var(--rumi-bg-surface-2); padding:1rem; border-radius:0.5rem; color:var(--rumi-text-secondary); }
  .loading, .empty { text-align:center; padding:3rem; color:var(--rumi-text-muted); }
</style>
