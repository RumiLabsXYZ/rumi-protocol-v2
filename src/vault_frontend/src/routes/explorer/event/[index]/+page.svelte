<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import EntityLink from '$lib/components/explorer/EntityLink.svelte';
  import TokenBadge from '$lib/components/explorer/TokenBadge.svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import {
    getEventType, getEventBadgeColor, getEventSummary, getEventKey,
    getEventVaultId, getEventCaller, getEventTimestamp, formatTimestamp,
    resolveCollateralSymbol
  } from '$lib/utils/eventFormatters';
  import { extractEventFields, getEventTypeDescription, type EventField } from '$lib/utils/explorerFormatters';
  import { truncatePrincipal } from '$lib/utils/principalHelpers';

  let event: any = $state(null);
  let loading = $state(true);

  const eventIndex = $derived(Number($page.params.index));
  const key = $derived(event ? getEventKey(event) : '');
  const type = $derived(event ? getEventType(event) : '');
  const badgeColor = $derived(event ? getEventBadgeColor(event) : '');
  const summary = $derived(event ? getEventSummary(event) : '');
  const vaultId = $derived(event ? getEventVaultId(event) : null);
  const caller = $derived(event ? getEventCaller(event) : null);
  const timestamp = $derived(event ? getEventTimestamp(event) : null);
  const description = $derived(key ? getEventTypeDescription(key) : '');
  const fields = $derived(event ? extractEventFields(event) : []);

  // Collateral info for context section
  const collateralInfo = $derived((() => {
    if (!event) return null;
    const data = event[key];
    if (!data) return null;
    const ct = data.collateral_type ?? data.vault?.collateral_type;
    if (!ct) return null;
    const principalId = ct?.toString?.() ?? ct?.toText?.() ?? String(ct);
    const symbol = resolveCollateralSymbol(ct);
    return { principalId, symbol };
  })());

  // Relative time for display
  function relativeTime(nanos: bigint | number): string {
    const ms = Number(BigInt(nanos) / BigInt(1_000_000));
    const diff = Date.now() - ms;
    if (diff < 60_000) return 'just now';
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
    if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
    if (diff < 2_592_000_000) return `${Math.floor(diff / 86_400_000)}d ago`;
    return '';
  }

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
</script>

<div class="max-w-[900px] mx-auto px-4 py-8">
  <a href="/explorer/events" class="text-sm text-blue-400 hover:text-blue-300 hover:underline inline-flex items-center gap-1 mb-6">
    &larr; Back to Events
  </a>

  {#if loading}
    <div class="text-center py-16 text-gray-500">Loading event #{eventIndex}...</div>
  {:else if !event}
    <div class="text-center py-16 text-gray-500">Event #{eventIndex} not found.</div>
  {:else}
    <!-- Header -->
    <div class="mb-8">
      <div class="flex items-center gap-3 mb-3 flex-wrap">
        <span
          class="inline-block text-sm font-semibold px-4 py-1.5 rounded-full"
          style="background:{badgeColor}20; color:{badgeColor}; border:1px solid {badgeColor}40;"
        >
          {type}
        </span>
        <span class="text-gray-500 text-sm font-mono">Event #{eventIndex}</span>
      </div>
      <p class="text-gray-400 text-sm mb-2">{description}</p>
      {#if timestamp}
        <p class="text-xs text-gray-500">
          {formatTimestamp(timestamp)}
          {#if relativeTime(timestamp)}
            <span class="text-gray-600 ml-1">({relativeTime(timestamp)})</span>
          {/if}
        </p>
      {/if}
    </div>

    <!-- Structured Fields -->
    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-5 mb-6">
      <h3 class="text-sm font-semibold text-gray-300 mb-4">Details</h3>
      <div class="flex flex-col gap-0">
        {#each fields as field}
          <div class="flex justify-between items-baseline gap-4 py-2.5 border-b border-gray-700/30 last:border-b-0">
            <span class="text-xs text-gray-500 capitalize shrink-0">{field.label}</span>
            <span class="text-sm text-right break-all">
              {#if field.type === 'vault' && field.linkId !== undefined}
                <EntityLink type="vault" id={field.linkId} label={field.value} />
              {:else if field.type === 'address' && field.linkId}
                <EntityLink type="address" id={String(field.linkId)} />
              {:else if field.type === 'token' && field.linkId}
                <TokenBadge symbol={field.value} principalId={String(field.linkId)} size="sm" linked={true} />
              {:else if field.type === 'amount'}
                <span class="text-white font-mono">{field.value}</span>
              {:else if field.type === 'percentage'}
                <span class="text-gray-300">{field.value}</span>
              {:else if field.type === 'timestamp'}
                <span class="text-gray-400">{field.value}</span>
              {:else}
                <span class="text-gray-300">{field.value}</span>
              {/if}
            </span>
          </div>
        {/each}
      </div>
    </div>

    <!-- Context Links -->
    {#if vaultId !== null || caller || collateralInfo}
      <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl p-5 mb-6">
        <h3 class="text-sm font-semibold text-gray-300 mb-3">Related</h3>
        <div class="flex flex-col gap-2.5">
          {#if vaultId !== null}
            <div class="flex items-center gap-2 text-sm">
              <span class="text-gray-500">Vault:</span>
              <EntityLink type="vault" id={vaultId} label="View Vault #{vaultId}" />
            </div>
          {/if}
          {#if caller}
            <div class="flex items-center gap-2 text-sm">
              <span class="text-gray-500">Address:</span>
              <EntityLink type="address" id={caller} label="View all activity for {truncatePrincipal(caller)}" />
            </div>
          {/if}
          {#if collateralInfo}
            <div class="flex items-center gap-2 text-sm">
              <span class="text-gray-500">Token:</span>
              <TokenBadge symbol={collateralInfo.symbol} principalId={collateralInfo.principalId} size="sm" linked={true} />
            </div>
          {/if}
        </div>
      </div>
    {/if}

    <!-- Raw Data -->
    <details class="group">
      <summary class="cursor-pointer text-xs text-gray-500 hover:text-gray-400 transition-colors py-2">
        Raw Event Data
      </summary>
      <pre class="mt-2 text-xs overflow-x-auto bg-gray-900/50 border border-gray-700/30 rounded-lg p-4 text-gray-400 font-mono">{JSON.stringify(event, (_, v) => typeof v === 'bigint' ? v.toString() : v, 2)}</pre>
    </details>
  {/if}
</div>
