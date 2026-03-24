<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import CopyButton from '$components/explorer/CopyButton.svelte';
  import TimeAgo from '$components/explorer/TimeAgo.svelte';
  import { fetchEvents } from '$services/explorer/explorerService';
  import { formatEvent } from '$utils/explorerFormatters';
  import type { EventField } from '$utils/explorerFormatters';
  import { formatTimestamp } from '$utils/explorerHelpers';

  let event: any = $state(null);
  let globalIndex: bigint | null = $state(null);
  let loading = $state(true);
  let error: string | null = $state(null);

  const eventIndex = $derived(Number($page.params.index));

  const formatted = $derived(event ? formatEvent(event) : null);

  // Extract unique related entities from fields for the sidebar
  const relatedEntities = $derived.by(() => {
    if (!formatted) return { vaults: [] as string[], addresses: [] as string[], tokens: [] as string[] };
    const vaults = new Set<string>();
    const addresses = new Set<string>();
    const tokens = new Set<string>();
    for (const field of formatted.fields) {
      if (field.type === 'vault' && field.linkTarget) vaults.add(field.linkTarget);
      if ((field.type === 'address' || field.type === 'canister') && field.linkTarget) addresses.add(field.linkTarget);
      if (field.type === 'token' && field.linkTarget) tokens.add(field.linkTarget);
    }
    return { vaults: [...vaults], addresses: [...addresses], tokens: [...tokens] };
  });

  const hasRelated = $derived(
    relatedEntities.vaults.length > 0 ||
    relatedEntities.addresses.length > 0 ||
    relatedEntities.tokens.length > 0
  );

  // Get raw timestamp from the event for TimeAgo
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
      const results = await fetchEvents(BigInt(eventIndex), BigInt(1));
      if (results.length > 0) {
        const [idx, evt] = results[0];
        globalIndex = idx;
        event = evt;
      } else {
        error = `Event #${eventIndex} not found.`;
      }
    } catch (e) {
      console.error('Failed to load event:', e);
      error = 'Failed to load event. Please try again.';
    } finally {
      loading = false;
    }
  });
</script>

<svelte:head>
  <title>Event #{eventIndex} | Rumi Explorer</title>
</svelte:head>

<div class="max-w-[960px] mx-auto px-4 py-8">
  <!-- Back link -->
  <a href="/explorer/events" class="text-sm text-blue-400 hover:text-blue-300 hover:underline inline-flex items-center gap-1 mb-6">
    &larr; Back to Events
  </a>

  {#if loading}
    <div class="flex flex-col items-center justify-center py-24 gap-3">
      <div class="w-8 h-8 border-2 border-gray-600 border-t-blue-400 rounded-full animate-spin"></div>
      <p class="text-gray-500 text-sm">Loading event #{eventIndex}...</p>
    </div>
  {:else if error || !formatted}
    <div class="text-center py-24">
      <p class="text-gray-400 text-lg mb-2">{error ?? `Event #${eventIndex} not found.`}</p>
      <a href="/explorer/events" class="text-sm text-blue-400 hover:underline">Return to Events</a>
    </div>
  {:else}
    <!-- Header -->
    <div class="mb-8">
      <div class="flex items-center gap-3 mb-3 flex-wrap">
        <h1 class="text-2xl font-bold text-white">Event #{eventIndex}</h1>
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
              {#if field.type === 'vault' && field.linkTarget}
                <EntityLink type="vault" value={field.linkTarget} />
              {:else if (field.type === 'address' || field.type === 'canister') && field.linkTarget}
                <span class="inline-flex items-center gap-1.5">
                  <EntityLink type="address" value={field.linkTarget} />
                  <CopyButton text={field.linkTarget} />
                </span>
              {:else if field.type === 'token' && field.linkTarget}
                <EntityLink type="token" value={field.linkTarget} />
              {:else if field.type === 'event' && field.linkTarget}
                <EntityLink type="event" value={field.linkTarget} />
              {:else if field.type === 'amount'}
                <span class="text-white font-mono">{field.value}</span>
              {:else if field.type === 'usd'}
                <span class="text-green-400 font-mono">{field.value}</span>
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
              {:else if field.type === 'block_index'}
                <span class="text-gray-300 font-mono">Block #{field.value}</span>
              {:else if field.type === 'json'}
                <details class="text-left">
                  <summary class="cursor-pointer text-xs text-gray-500 hover:text-gray-400 transition-colors">
                    Expand
                  </summary>
                  <pre class="mt-2 text-xs overflow-x-auto bg-gray-900/50 border border-gray-700/30 rounded-lg p-3 text-gray-400 font-mono whitespace-pre-wrap">{field.value}</pre>
                </details>
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
          {#each relatedEntities.vaults as vaultId}
            <div class="flex items-center gap-2 text-sm">
              <span class="text-gray-500 min-w-[60px]">Vault</span>
              <EntityLink type="vault" value={vaultId} />
            </div>
          {/each}
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
      {#if eventIndex > 0}
        <a
          href="/explorer/event/{eventIndex - 1}"
          class="text-sm text-blue-400 hover:text-blue-300 hover:underline inline-flex items-center gap-1"
        >
          &larr; Previous Event
        </a>
      {:else}
        <span></span>
      {/if}
      <a
        href="/explorer/event/{eventIndex + 1}"
        class="text-sm text-blue-400 hover:text-blue-300 hover:underline inline-flex items-center gap-1"
      >
        Next Event &rarr;
      </a>
    </div>
  {/if}
</div>
