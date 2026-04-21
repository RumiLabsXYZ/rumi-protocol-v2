<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import CopyButton from '$components/explorer/CopyButton.svelte';
  import TimeAgo from '$components/explorer/TimeAgo.svelte';
  import { fetchDexEvent, fetchDexEventCount } from '$services/explorer/explorerService';
  import type { DexEventSource } from '$services/explorer/explorerService';
  import {
    formatSwapEvent, formatAmmSwapEvent, formatAmmLiquidityEvent,
    formatAmmAdminEvent, format3PoolLiquidityEvent, format3PoolAdminEvent,
    formatStabilityPoolEvent,
  } from '$utils/explorerFormatters';
  import type { FormattedEvent, EventField } from '$utils/explorerFormatters';
  import { formatTimestamp } from '$utils/explorerHelpers';

  const SOURCE_LABELS: Record<string, string> = {
    '3pool_swap': '3Pool Swap',
    'amm_swap': 'AMM Swap',
    'amm_liquidity': 'AMM Liquidity',
    'amm_admin': 'AMM Admin',
    '3pool_liquidity': '3Pool Liquidity',
    '3pool_admin': '3Pool Admin',
    'stability_pool': 'Stability Pool',
  };

  let event: any = $state(null);
  let loading = $state(true);
  let error: string | null = $state(null);
  let totalCount: number = $state(0);

  const source = $derived($page.params.source as DexEventSource);
  const eventId = $derived(Number($page.params.id));
  const sourceLabel = $derived(SOURCE_LABELS[source] ?? source);

  const formatted = $derived.by((): FormattedEvent | null => {
    if (!event) return null;
    switch (source) {
      case '3pool_swap': return formatSwapEvent(event);
      case 'amm_swap': return formatAmmSwapEvent(event);
      case 'amm_liquidity': return formatAmmLiquidityEvent(event);
      case 'amm_admin': return formatAmmAdminEvent(event);
      case '3pool_liquidity': return format3PoolLiquidityEvent(event);
      case '3pool_admin': return format3PoolAdminEvent(event);
      case 'stability_pool': return formatStabilityPoolEvent(event);
      default: return null;
    }
  });

  const relatedEntities = $derived.by(() => {
    if (!formatted) return { addresses: [] as string[], tokens: [] as string[] };
    const addresses = new Set<string>();
    const tokens = new Set<string>();
    for (const field of formatted.fields) {
      if ((field.type === 'address' || field.type === 'canister') && field.linkTarget) addresses.add(field.linkTarget);
      if (field.type === 'token' && field.linkTarget) tokens.add(field.linkTarget);
    }
    return { addresses: [...addresses], tokens: [...tokens] };
  });

  const hasRelated = $derived(relatedEntities.addresses.length > 0 || relatedEntities.tokens.length > 0);

  const rawTimestamp = $derived.by(() => {
    if (!formatted) return null;
    const tsField = formatted.fields.find(f => f.type === 'timestamp');
    if (tsField?.linkTarget) return BigInt(tsField.linkTarget);
    return null;
  });

  onMount(async () => {
    loading = true;
    error = null;
    try {
      const [result, count] = await Promise.all([
        fetchDexEvent(source, eventId),
        fetchDexEventCount(source),
      ]);
      totalCount = Number(count);
      if (result) {
        event = result;
      } else {
        error = `Event not found: ${sourceLabel} #${eventId}`;
      }
    } catch (e) {
      console.error('Failed to load DEX event:', e);
      error = 'Failed to load event. Please try again.';
    } finally {
      loading = false;
    }
  });
</script>

<svelte:head>
  <title>{sourceLabel} #{eventId} | Rumi Explorer</title>
</svelte:head>

<div class="max-w-[960px] mx-auto px-4 py-8">
  <a href="/explorer/activity?filter=dex" class="text-sm text-blue-400 hover:text-blue-300 hover:underline inline-flex items-center gap-1 mb-6">
    &larr; Back to Activity
  </a>

  {#if loading}
    <div class="flex flex-col items-center justify-center py-24 gap-3">
      <div class="w-8 h-8 border-2 border-gray-600 border-t-blue-400 rounded-full animate-spin"></div>
      <p class="text-gray-500 text-sm">Loading {sourceLabel} #{eventId}...</p>
    </div>
  {:else if error || !formatted}
    <div class="text-center py-24">
      <p class="text-gray-400 text-lg mb-2">{error ?? `Event not found.`}</p>
      <a href="/explorer/activity" class="text-sm text-blue-400 hover:underline">Return to Activity</a>
    </div>
  {:else}
    <!-- Header -->
    <div class="mb-8">
      <div class="flex items-center gap-3 mb-3 flex-wrap">
        <h1 class="text-2xl font-bold text-white">{sourceLabel} #{eventId}</h1>
        <span class="inline-block text-sm font-semibold px-3 py-1 rounded-full {formatted.badgeColor}">
          {formatted.typeName}
        </span>
      </div>

      {#if rawTimestamp}
        <p class="text-sm text-gray-400 mb-2">
          <TimeAgo timestamp={rawTimestamp} />
          <span class="text-gray-600 mx-1">&middot;</span>
          <span class="text-gray-500">{formatTimestamp(rawTimestamp)}</span>
        </p>
      {/if}

      <p class="text-gray-300 text-sm leading-relaxed">{formatted.summary}</p>
    </div>

    <!-- Structured Fields -->
    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-5 mb-6">
      <h2 class="text-sm font-semibold text-gray-300 mb-4 uppercase tracking-wider">Details</h2>
      <div class="flex flex-col">
        {#each formatted.fields as field}
          <div class="flex justify-between items-baseline gap-4 py-2.5 border-b border-gray-700/30 last:border-b-0">
            <span class="text-xs text-gray-400 capitalize shrink-0 min-w-[120px]">{field.label}</span>
            <span class="text-sm text-right break-all">
              {#if (field.type === 'address' || field.type === 'canister') && field.linkTarget}
                <span class="inline-flex items-center gap-1.5">
                  <EntityLink type="address" value={field.linkTarget} />
                  <CopyButton text={field.linkTarget} />
                </span>
              {:else if field.type === 'token' && field.linkTarget}
                <EntityLink type="token" value={field.linkTarget} />
              {:else if field.type === 'amount'}
                <span class="text-white font-mono">{field.value}</span>
              {:else if field.type === 'timestamp'}
                {#if field.linkTarget}
                  <span class="inline-flex items-center gap-2">
                    <TimeAgo timestamp={BigInt(field.linkTarget)} />
                    <span class="text-gray-500 text-xs">({formatTimestamp(BigInt(field.linkTarget))})</span>
                  </span>
                {:else}
                  <span class="text-gray-400">{field.value}</span>
                {/if}
              {:else if field.type === 'percentage' || field.type === 'ratio'}
                <span class="text-gray-300 font-mono">{field.value}</span>
              {:else}
                <span class="text-gray-300">{field.value}</span>
              {/if}
            </span>
          </div>
        {/each}
      </div>
    </div>

    <!-- Related Entities -->
    {#if hasRelated}
      <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl p-5 mb-6">
        <h2 class="text-sm font-semibold text-gray-300 mb-3 uppercase tracking-wider">Related Entities</h2>
        <div class="flex flex-col gap-2.5">
          {#each relatedEntities.addresses as address}
            <div class="flex items-center gap-2 text-sm">
              <span class="text-gray-500 min-w-[60px]">Address</span>
              <EntityLink type="address" value={address} />
              <CopyButton text={address} />
            </div>
          {/each}
          {#each relatedEntities.tokens as token}
            <div class="flex items-center gap-2 text-sm">
              <span class="text-gray-500 min-w-[60px]">Token</span>
              <EntityLink type="token" value={token} />
            </div>
          {/each}
        </div>
      </div>
    {/if}

    <!-- Raw Event Data -->
    <details class="group mb-8">
      <summary class="cursor-pointer text-xs text-gray-500 hover:text-gray-400 transition-colors py-2 select-none">
        Raw Event Data
      </summary>
      <pre class="mt-2 text-xs overflow-x-auto bg-gray-900/50 border border-gray-700/30 rounded-lg p-4 text-gray-400 font-mono whitespace-pre-wrap">{JSON.stringify(event, (_, v) => typeof v === 'bigint' ? v.toString() : v, 2)}</pre>
    </details>

    <!-- Navigation -->
    <div class="flex items-center justify-between pt-4 border-t border-gray-700/30">
      {#if eventId > 0}
        <a
          href="/explorer/dex/{source}/{eventId - 1}"
          class="text-sm text-blue-400 hover:text-blue-300 hover:underline inline-flex items-center gap-1"
        >
          &larr; Previous
        </a>
      {:else}
        <span></span>
      {/if}
      {#if eventId < totalCount - 1}
        <a
          href="/explorer/dex/{source}/{eventId + 1}"
          class="text-sm text-blue-400 hover:text-blue-300 hover:underline inline-flex items-center gap-1"
        >
          Next &rarr;
        </a>
      {:else}
        <span></span>
      {/if}
    </div>
  {/if}
</div>
