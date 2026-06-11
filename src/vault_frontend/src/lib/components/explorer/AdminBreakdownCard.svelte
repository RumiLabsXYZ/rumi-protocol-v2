<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchAdminEventBreakdown } from '$lib/services/explorer/analyticsService';
  import type { AdminEventLabelCount } from '$declarations/rumi_analytics/rumi_analytics.did';

  let labels: AdminEventLabelCount[] = $state([]);
  let loading = $state(true);

  onMount(async () => {
    try {
      const resp = await fetchAdminEventBreakdown();
      labels = resp.labels.slice().sort((a, b) => Number(b.count) - Number(a.count));
    } catch (err) {
      console.error('[AdminBreakdownCard] load failed:', err);
    } finally {
      loading = false;
    }
  });

  // Protocol-health incidents the analytics labeler tracks. These are
  // self-reported failures/safety events, not operator actions — surfaced in
  // their own section so 138 oracle rejections don't read as "admin actions".
  const HEALTH_LABELS: Record<string, string> = {
    OracleCircuitBreaker: 'oracle_incident',
    OracleSourceCountInsufficient: 'oracle_incident',
    BreakerTripped: 'breaker_event',
    BreakerCleared: 'breaker_event',
    BotClaimReconciliationNeeded: 'ops_incident',
    StabilityPoolCallFailed: 'ops_incident',
  };

  const healthRows = $derived(labels.filter((l) => l.label in HEALTH_LABELS));
  const adminRows = $derived(labels.filter((l) => !(l.label in HEALTH_LABELS)));

  function healthHref(label: string): string {
    return `/explorer/activity?type=${HEALTH_LABELS[label]}`;
  }

  // Categorize setter labels by domain. Surfaced in the row tooltip so
  // hovering tells you which area of the protocol a setter belongs to,
  // without crowding the visible label cells.
  function groupOf(label: string): string {
    if (label.startsWith('SetCollateral')) return 'Collateral';
    if (label.startsWith('SetRmr') || label.startsWith('SetRedemption') || label.startsWith('SetReserve')) return 'Redemption';
    if (label.startsWith('SetRecovery')) return 'Recovery';
    if (label.includes('Fee') || label.includes('InterestRate') || label.includes('InterestSplit') || label.includes('InterestPoolShare')) return 'Fees & interest';
    if (label === 'Init' || label === 'Upgrade') return 'Lifecycle';
    if (label.includes('Bot')) return 'Bot';
    return 'Other';
  }
</script>

{#if loading}
  <div class="explorer-card">
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  </div>
{:else}
  {#if healthRows.length > 0}
    <div class="explorer-card">
      <h3 class="text-sm font-medium text-orange-300 mb-1">Health incidents by type</h3>
      <p class="text-xs text-gray-500 mb-3">
        Self-reported safety events: oracle rejections and circuit-breaker trips, mass-liquidation
        breaker activity, and operational failures. High counts here are an oracle/ops signal, not admin activity.
      </p>
      <div class="grid grid-cols-2 md:grid-cols-3 gap-x-6 gap-y-2">
        {#each healthRows as l (l.label)}
          <a
            href={healthHref(l.label)}
            class="flex items-baseline justify-between text-sm py-1 border-b border-white/[0.03] hover:bg-white/[0.02]"
            title="click to filter the activity feed"
          >
            <span class="text-orange-300/90 font-mono text-xs truncate mr-2">{l.label}</span>
            <span class="tabular-nums text-gray-200 font-medium">{Number(l.count).toLocaleString()}</span>
          </a>
        {/each}
      </div>
    </div>
  {/if}

  <div class="explorer-card">
    <h3 class="text-sm font-medium text-gray-300 mb-1">Admin actions by label</h3>
    <p class="text-xs text-gray-500 mb-3">Counts of each setter / lifecycle event the analytics canister has tailed.</p>
    {#if adminRows.length === 0}
      <p class="text-sm text-gray-500 py-2">No admin actions recorded yet.</p>
    {:else}
      <div class="grid grid-cols-2 md:grid-cols-3 gap-x-6 gap-y-2">
        {#each adminRows as l (l.label)}
          <a
            href="/explorer/activity?type=admin&admin={encodeURIComponent(l.label)}"
            class="flex items-baseline justify-between text-sm py-1 border-b border-white/[0.03] hover:bg-white/[0.02]"
            title="{groupOf(l.label)} · click to filter"
          >
            <span class="text-gray-300 font-mono text-xs truncate mr-2">{l.label}</span>
            <span class="tabular-nums text-gray-200 font-medium">{Number(l.count).toLocaleString()}</span>
          </a>
        {/each}
      </div>
    {/if}
  </div>
{/if}
