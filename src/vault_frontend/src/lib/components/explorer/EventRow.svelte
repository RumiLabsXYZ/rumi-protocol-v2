<script lang="ts">
  import { getEventType, getEventCategory, getEventBadgeColor, getEventSummary, formatTimestamp } from '$lib/utils/eventFormatters';

  export let event: any;
  export let index: number | null = null;

  $: type = getEventType(event);
  $: category = getEventCategory(event);
  $: badgeColor = getEventBadgeColor(event);
  $: summary = getEventSummary(event);
</script>

<a class="event-row" href={index !== null ? `/explorer/event/${index}` : undefined}>
  <span class="event-badge" style="background:{badgeColor}20; color:{badgeColor}; border:1px solid {badgeColor}40;">
    {type}
  </span>
  <span class="event-summary">{summary}</span>
  {#if event[Object.keys(event)[0]]?.vault?.last_accrual_time}
    <span class="event-time">{formatTimestamp(event[Object.keys(event)[0]].vault.last_accrual_time)}</span>
  {/if}
</a>

<style>
  .event-row {
    display:grid; grid-template-columns:auto 1fr auto; gap:0.75rem; align-items:center;
    padding:0.625rem 0.875rem; border-bottom:1px solid var(--rumi-border);
    text-decoration:none; color:var(--rumi-text-primary);
    transition:background 0.15s;
  }
  .event-row:hover { background:var(--rumi-bg-surface-2); }
  .event-badge {
    font-size:0.75rem; font-weight:500; padding:0.125rem 0.5rem;
    border-radius:9999px; white-space:nowrap;
  }
  .event-summary { font-size:0.875rem; color:var(--rumi-text-secondary); overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .event-time { font-size:0.75rem; color:var(--rumi-text-muted); white-space:nowrap; font-variant-numeric:tabular-nums; }
</style>
