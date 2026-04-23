<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import EntityShell from '$components/explorer/entity/EntityShell.svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import CopyButton from '$components/explorer/CopyButton.svelte';
  import TimeAgo from '$components/explorer/TimeAgo.svelte';
  import MixedEventRow from '$components/explorer/MixedEventRow.svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { formatTimestamp, shortenPrincipal, getCanisterName } from '$utils/explorerHelpers';
  import {
    fetchAllVaults, fetchEventCount, fetchEvents, fetchDexEvent, fetchDexEventCount,
    fetchAllDexEvents,
  } from '$services/explorer/explorerService';
  import type { DexEventSource } from '$services/explorer/explorerService';
  import {
    displayEvent, wrapBackendEvent, extractEventTimestamp, DEX_SOURCE_LABEL,
  } from '$utils/displayEvent';
  import type { DisplayEvent, DisplayEventSource } from '$utils/displayEvent';
  import { CANISTER_IDS } from '$lib/config';

  const rawId = $derived($page.params.id);

  // Parse "dex:source:id" prefix or fall back to plain backend index.
  const parsed = $derived.by(() => {
    const m = rawId.match(/^dex:([a-z_0-9]+):(\d+)$/i);
    if (m) return { source: m[1] as DexEventSource, id: Number(m[2]) };
    const n = Number(rawId);
    if (Number.isFinite(n)) return { source: 'backend' as const, id: n };
    return null;
  });

  const isBackend = $derived(parsed?.source === 'backend');
  const sourceCanister = $derived.by(() => {
    if (!parsed) return null;
    switch (parsed.source) {
      case 'backend': return CANISTER_IDS.PROTOCOL;
      case '3pool_swap':
      case '3pool_liquidity':
      case '3pool_admin': return CANISTER_IDS.THREEPOOL;
      case 'amm_swap':
      case 'amm_liquidity':
      case 'amm_admin': return CANISTER_IDS.RUMI_AMM;
      case 'stability_pool': return CANISTER_IDS.STABILITY_POOL;
      default: return null;
    }
  });

  let event = $state<any>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let vaultCollateralMap = $state<Map<number, string>>(new Map());
  let totalCount = $state(0);
  let contextEvents = $state<DisplayEvent[]>([]);

  const display = $derived.by(() => {
    if (!event || !parsed) return null;
    const de: DisplayEvent = {
      event,
      globalIndex: BigInt(parsed.id),
      source: parsed.source as DisplayEventSource,
      timestamp: parsed.source === 'backend' ? extractEventTimestamp(event) : Number(event.timestamp ?? 0),
    };
    return displayEvent(de, { vaultCollateralMap });
  });

  // ── Relationships: Entities touched by this event ──────────────────────────
  const touched = $derived.by(() => {
    const vaults = new Set<string>();
    const addresses = new Set<string>();
    const tokens = new Set<string>();
    const canisters = new Set<string>();
    if (display) {
      for (const f of display.formatted.fields) {
        const target = f.linkTarget;
        if (!target) continue;
        if (f.type === 'vault') vaults.add(target);
        else if (f.type === 'token') tokens.add(target);
        else if (f.type === 'canister') canisters.add(target);
        else if (f.type === 'address') {
          if (getCanisterName(target)) canisters.add(target);
          else addresses.add(target);
        }
      }
    }
    if (display?.principal && !addresses.has(display.principal) && !canisters.has(display.principal)) {
      if (getCanisterName(display.principal)) canisters.add(display.principal);
      else addresses.add(display.principal);
    }
    return {
      vaults: [...vaults],
      addresses: [...addresses],
      tokens: [...tokens],
      canisters: [...canisters],
    };
  });

  onMount(async () => {
    loading = true;
    error = null;
    if (!parsed) {
      error = `Invalid event id: "${rawId}"`;
      loading = false;
      return;
    }

    try {
      if (parsed.source === 'backend') {
        const [results, vaults, count] = await Promise.all([
          publicActor.get_events({ start: BigInt(parsed.id), length: 1n, types: [], principal: [], collateral_token: [], time_range: [], min_size_e8s: [] }),
          fetchAllVaults().catch(() => []),
          fetchEventCount().catch(() => 0n),
        ]);
        totalCount = Number(count);
        const map = new Map<number, string>();
        for (const v of vaults as any[]) {
          const vid = Number(v.vault_id);
          const ct = v.collateral_type?.toText?.() ?? String(v.collateral_type ?? '');
          if (ct) map.set(vid, ct);
        }
        vaultCollateralMap = map;

        if (results.length > 0) {
          event = results[0];
        } else {
          error = `Event #${parsed.id} not found.`;
        }
      } else {
        const [result, count] = await Promise.all([
          fetchDexEvent(parsed.source as DexEventSource, parsed.id),
          fetchDexEventCount(parsed.source as DexEventSource).catch(() => 0n),
        ]);
        totalCount = Number(count);
        if (result) event = result;
        else error = `${DEX_SOURCE_LABEL[parsed.source as Exclude<DisplayEventSource, 'backend'>]} event #${parsed.id} not found.`;
      }
    } catch (e) {
      console.error('[event] load failed:', e);
      error = 'Failed to load event.';
    } finally {
      loading = false;
    }

    if (!event || !parsed) return;
    // ── Context strip: adjacent events from same source within ±5 minutes ──
    try {
      const anchorNs = display?.timestamp ?? 0;
      if (!anchorNs) return;
      const windowNs = 5n * 60n * 1_000_000_000n;
      const contextList: DisplayEvent[] = [];

      if (parsed.source === 'backend') {
        const neighborhood = 25;
        const start = BigInt(Math.max(0, parsed.id - neighborhood));
        const len = BigInt(neighborhood * 2 + 1);
        const results = await publicActor.get_events({ start, length: len, types: [], principal: [], collateral_token: [], time_range: [], min_size_e8s: [] });
        for (let i = 0; i < results.length; i++) {
          const idx = Number(start) + i;
          if (idx === parsed.id) continue;
          const ts = extractEventTimestamp(results[i]);
          if (!ts) continue;
          if (BigInt(Math.abs(ts - anchorNs)) > windowNs) continue;
          contextList.push({
            event: results[i],
            globalIndex: BigInt(idx),
            source: 'backend',
            timestamp: ts,
          });
        }
      } else {
        const all = await fetchAllDexEvents(parsed.source as DexEventSource);
        for (const e of all) {
          const id = Number(e.id ?? 0);
          if (id === parsed.id) continue;
          const ts = Number(e.timestamp ?? 0);
          if (!ts) continue;
          if (BigInt(Math.abs(ts - anchorNs)) > windowNs) continue;
          contextList.push({
            event: e,
            globalIndex: BigInt(id),
            source: parsed.source as DisplayEventSource,
            timestamp: ts,
          });
        }
      }
      contextList.sort((a, b) => b.timestamp - a.timestamp);
      contextEvents = contextList.slice(0, 15);
    } catch (err) {
      console.error('[event] context load failed:', err);
    }
  });

  const timestampNs = $derived(display?.timestamp ?? 0);
  const blockIndex = $derived.by(() => {
    if (!display) return null;
    const f = display.formatted.fields.find((x) => x.type === 'block_index');
    return f?.value ?? null;
  });
</script>

<svelte:head>
  <title>Event {rawId} | Rumi Explorer</title>
</svelte:head>

<EntityShell
  title={display ? `${display.formatted.typeName}${isBackend ? ` #${parsed?.id}` : ` ${display.sourceLabel ?? ''} #${parsed?.id}`}` : `Event ${rawId}`}
  loading={loading}
  error={error}
>
  {#snippet identity()}
    {#if display}
      <div class="flex flex-wrap items-center gap-3">
        <span class="inline-block text-xs font-semibold px-3 py-1 rounded-full {display.formatted.badgeColor}">
          {display.formatted.typeName}
        </span>
        {#if display.sourceLabel}
          <span class="text-[10px] uppercase tracking-wider px-2 py-0.5 rounded bg-gray-800 border border-gray-700/50 text-gray-400">
            {display.sourceLabel}
          </span>
        {/if}
        {#if sourceCanister}
          <span class="text-xs text-gray-500">Source</span>
          <EntityLink type="canister" value={sourceCanister} />
        {/if}
        {#if blockIndex}
          <span class="text-xs text-gray-500">Block #{blockIndex}</span>
        {/if}
      </div>

      {#if timestampNs}
        <div class="text-sm text-gray-400">
          <TimeAgo timestamp={timestampNs} />
          <span class="text-gray-600 mx-1">·</span>
          <span class="text-gray-500 font-mono text-xs">{formatTimestamp(timestampNs)}</span>
        </div>
      {/if}

      <p class="text-gray-300 text-sm leading-relaxed">{display.formatted.summary}</p>

      <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
        <h3 class="text-[10px] uppercase tracking-wider text-gray-500 mb-3">Details</h3>
        <div class="flex flex-col">
          {#each display.formatted.fields as field (field.label)}
            <div class="flex justify-between items-baseline gap-4 py-2 border-b border-gray-700/30 last:border-b-0">
              <span class="text-xs text-gray-400 shrink-0 min-w-[120px]">{field.label}</span>
              <span class="text-sm text-right break-all">
                {#if field.type === 'vault' && field.linkTarget}
                  <EntityLink type="vault" value={field.linkTarget} />
                {:else if (field.type === 'address' || field.type === 'canister') && field.linkTarget}
                  <span class="inline-flex items-center gap-1.5">
                    <EntityLink type={field.type === 'canister' ? 'canister' : 'address'} value={field.linkTarget} />
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
                {:else if field.type === 'timestamp' && field.linkTarget}
                  <span class="inline-flex items-center gap-2">
                    <TimeAgo timestamp={BigInt(field.linkTarget)} />
                    <span class="text-gray-500 text-xs">({formatTimestamp(BigInt(field.linkTarget))})</span>
                  </span>
                {:else if field.type === 'json'}
                  <details class="text-left">
                    <summary class="cursor-pointer text-xs text-gray-500 hover:text-gray-400">Expand</summary>
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
    {/if}
  {/snippet}

  {#snippet relationships()}
    <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
      <div class="text-[10px] uppercase tracking-wider text-gray-500 mb-3">Entities Touched</div>
      {#if touched.vaults.length + touched.addresses.length + touched.tokens.length + touched.canisters.length === 0}
        <div class="text-xs text-gray-500">No entities to surface for this event.</div>
      {:else}
        <div class="space-y-3">
          {#if touched.vaults.length > 0}
            <div>
              <div class="text-[10px] text-gray-500 mb-1">Vaults</div>
              <div class="flex flex-wrap gap-2">
                {#each touched.vaults as v (v)}
                  <EntityLink type="vault" value={v} />
                {/each}
              </div>
            </div>
          {/if}
          {#if touched.tokens.length > 0}
            <div>
              <div class="text-[10px] text-gray-500 mb-1">Tokens</div>
              <div class="flex flex-wrap gap-2">
                {#each touched.tokens as t (t)}
                  <EntityLink type="token" value={t} />
                {/each}
              </div>
            </div>
          {/if}
          {#if touched.canisters.length > 0}
            <div>
              <div class="text-[10px] text-gray-500 mb-1">Canisters</div>
              <div class="flex flex-wrap gap-2 items-center">
                {#each touched.canisters as c (c)}
                  <span class="inline-flex items-center gap-1">
                    <EntityLink type="canister" value={c} />
                    <CopyButton text={c} />
                  </span>
                {/each}
              </div>
            </div>
          {/if}
          {#if touched.addresses.length > 0}
            <div>
              <div class="text-[10px] text-gray-500 mb-1">Principals</div>
              <div class="flex flex-wrap gap-2 items-center">
                {#each touched.addresses as a (a)}
                  <span class="inline-flex items-center gap-1">
                    <EntityLink type="address" value={a} />
                    <CopyButton text={a} />
                  </span>
                {/each}
              </div>
            </div>
          {/if}
        </div>
      {/if}
    </div>
  {/snippet}

  {#snippet activity()}
    <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl overflow-hidden">
      <div class="px-5 py-3 border-b border-gray-700/50 text-[10px] uppercase tracking-wider text-gray-500">
        Context · adjacent events from {display?.sourceLabel ?? 'this source'} within ±5 minutes
      </div>
      {#if contextEvents.length === 0}
        <div class="px-5 py-6 text-center text-gray-500 text-sm">No adjacent events in the window.</div>
      {:else}
        <table class="w-full">
          <thead class="bg-gray-900/40">
            <tr class="text-[10px] uppercase tracking-wider text-gray-500">
              <th class="px-4 py-2 text-left">Event</th>
              <th class="px-4 py-2 text-left">When</th>
              <th class="px-4 py-2 text-left">By</th>
              <th class="px-4 py-2 text-left">Type</th>
              <th class="px-4 py-2 text-left">Summary</th>
              <th class="px-4 py-2 text-right"></th>
            </tr>
          </thead>
          <tbody>
            {#each contextEvents as ctx (ctx.source + ':' + ctx.globalIndex)}
              <MixedEventRow event={ctx} vaultOwnerMap={undefined} />
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  {/snippet}
</EntityShell>
