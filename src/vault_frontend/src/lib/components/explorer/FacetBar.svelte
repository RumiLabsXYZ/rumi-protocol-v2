<script lang="ts">
  import { getTokenSymbol } from '$utils/explorerHelpers';
  import {
    TYPE_FACET_GROUPS, typeFacetLabel,
    TIME_PRESETS, SIZE_PRESETS,
    type Facets, type TypeFacetKey, type TimePresetKey,
  } from '$utils/eventFacets';

  interface TokenOption {
    principal: string;
    label: string;
  }

  interface PoolOption {
    id: string;
    label: string;
  }

  interface Props {
    facets: Facets;
    tokenOptions: TokenOption[];
    poolOptions: PoolOption[];
    onChange: (next: Facets) => void;
  }

  let { facets, tokenOptions, poolOptions, onChange }: Props = $props();

  type DropdownKey = 'type' | 'token' | 'pool' | 'entity' | 'size' | 'time';
  let openDropdown: DropdownKey | null = $state(null);
  let entityQuery = $state('');
  let customSize = $state('');
  let customFrom = $state('');
  let customTo = $state('');

  function toggle(k: DropdownKey) {
    openDropdown = openDropdown === k ? null : k;
  }

  function close() {
    openDropdown = null;
  }

  function handleDocClick(ev: MouseEvent) {
    const target = ev.target as HTMLElement | null;
    if (!target) return;
    if (target.closest('[data-facet-root]')) return;
    close();
  }

  $effect(() => {
    if (typeof document === 'undefined') return;
    document.addEventListener('click', handleDocClick);
    return () => document.removeEventListener('click', handleDocClick);
  });

  // Type facet
  function toggleType(k: TypeFacetKey) {
    const types = facets.types.includes(k)
      ? facets.types.filter((t) => t !== k)
      : [...facets.types, k];
    onChange({ ...facets, types });
  }

  function toggleAllInGroup(keys: TypeFacetKey[]) {
    const allSelected = keys.every((k) => facets.types.includes(k));
    const types = allSelected
      ? facets.types.filter((t) => !keys.includes(t))
      : [...new Set([...facets.types, ...keys])];
    onChange({ ...facets, types });
  }

  // Token facet
  function toggleToken(principal: string) {
    const tokens = facets.tokens.includes(principal)
      ? facets.tokens.filter((t) => t !== principal)
      : [...facets.tokens, principal];
    onChange({ ...facets, tokens });
  }

  // Pool facet
  function togglePool(id: string) {
    const pools = facets.pools.includes(id)
      ? facets.pools.filter((p) => p !== id)
      : [...facets.pools, id];
    onChange({ ...facets, pools });
  }

  // Entity facet — free-text resolves to vault id or principal
  function submitEntity() {
    const raw = entityQuery.trim();
    if (!raw) return;
    if (/^\d+$/.test(raw) || /^#\d+$/.test(raw)) {
      const n = Number(raw.replace(/^#/, ''));
      if (Number.isFinite(n) && !facets.vaultIds.includes(n)) {
        onChange({ ...facets, vaultIds: [...facets.vaultIds, n] });
      }
    } else if (raw.includes('-') && raw.length > 10) {
      if (!facets.principals.includes(raw)) {
        onChange({ ...facets, principals: [...facets.principals, raw] });
      }
    }
    entityQuery = '';
    close();
  }

  function handleEntityKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      submitEntity();
    } else if (e.key === 'Escape') {
      close();
    }
  }

  // Size facet
  function setSize(min: number | null) {
    onChange({ ...facets, minSizeUsd: min });
  }

  function applyCustomSize() {
    const n = Number(customSize);
    if (Number.isFinite(n) && n > 0) {
      setSize(n);
      customSize = '';
      close();
    }
  }

  // Time facet
  function setTime(preset: TimePresetKey) {
    onChange({ ...facets, time: { preset } });
    close();
  }

  function applyCustomTime() {
    const fromMs = customFrom ? Date.parse(customFrom) : undefined;
    const toMs = customTo ? Date.parse(customTo) : undefined;
    if (fromMs == null && toMs == null) return;
    onChange({
      ...facets,
      time: {
        preset: 'custom',
        fromMs: Number.isFinite(fromMs) ? (fromMs as number) : undefined,
        toMs: Number.isFinite(toMs) ? (toMs as number) : undefined,
      },
    });
    close();
  }

  // Button label helpers
  const typeLabel = $derived(
    facets.types.length === 0 ? 'Type' : `Type · ${facets.types.length}`,
  );
  const tokenLabel = $derived(
    facets.tokens.length === 0 ? 'Token' : `Token · ${facets.tokens.length}`,
  );
  const poolLabel = $derived(
    facets.pools.length === 0 ? 'Pool' : `Pool · ${facets.pools.length}`,
  );
  const entityCount = $derived(facets.vaultIds.length + facets.principals.length);
  const entityLabel = $derived(entityCount === 0 ? 'Entity' : `Entity · ${entityCount}`);
  const sizeLabel = $derived(
    facets.minSizeUsd == null ? 'Size' : `> $${facets.minSizeUsd.toLocaleString()}`,
  );
  const timeLabel = $derived(
    facets.time.preset === 'all'
      ? 'Time'
      : facets.time.preset === 'custom'
        ? 'Custom'
        : facets.time.preset,
  );

  function isActive(k: DropdownKey): boolean {
    switch (k) {
      case 'type': return facets.types.length > 0;
      case 'token': return facets.tokens.length > 0;
      case 'pool': return facets.pools.length > 0;
      case 'entity': return entityCount > 0;
      case 'size': return facets.minSizeUsd != null;
      case 'time': return facets.time.preset !== 'all';
    }
  }

  function btnClass(k: DropdownKey): string {
    const active = isActive(k);
    const open = openDropdown === k;
    if (active || open) {
      return 'px-3 py-1.5 text-sm rounded-md bg-teal-500/20 text-teal-200 border border-teal-500/40';
    }
    return 'px-3 py-1.5 text-sm rounded-md bg-gray-800/60 text-gray-300 border border-gray-700 hover:border-gray-500 hover:text-gray-100';
  }
</script>

<div class="flex flex-wrap gap-2" data-facet-root>
  <!-- Type -->
  <div class="relative">
    <button type="button" class={btnClass('type')} onclick={(e) => { e.stopPropagation(); toggle('type'); }}>
      {typeLabel} <span class="text-xs opacity-70">▾</span>
    </button>
    {#if openDropdown === 'type'}
      <div class="absolute left-0 top-full mt-1 z-30 min-w-[280px] max-h-[420px] overflow-y-auto bg-gray-900 border border-gray-700 rounded-lg shadow-xl p-2">
        {#each TYPE_FACET_GROUPS as g (g.group)}
          {@const allInGroup = g.keys.every((k) => facets.types.includes(k))}
          <div class="flex items-center justify-between px-2 py-1 text-[10px] uppercase tracking-wider text-gray-500">
            <span>{g.group}</span>
            <button
              type="button"
              class="text-teal-400 hover:text-teal-300"
              onclick={() => toggleAllInGroup(g.keys)}
            >
              {allInGroup ? 'Clear' : 'All'}
            </button>
          </div>
          {#each g.keys as k (k)}
            <label class="flex items-center gap-2 px-2 py-1 rounded hover:bg-gray-800 cursor-pointer text-sm text-gray-200">
              <input
                type="checkbox"
                checked={facets.types.includes(k)}
                onchange={() => toggleType(k)}
                class="accent-teal-500"
              />
              {typeFacetLabel(k)}
            </label>
          {/each}
        {/each}
      </div>
    {/if}
  </div>

  <!-- Token -->
  <div class="relative">
    <button type="button" class={btnClass('token')} onclick={(e) => { e.stopPropagation(); toggle('token'); }}>
      {tokenLabel} <span class="text-xs opacity-70">▾</span>
    </button>
    {#if openDropdown === 'token'}
      <div class="absolute left-0 top-full mt-1 z-30 min-w-[220px] max-h-[360px] overflow-y-auto bg-gray-900 border border-gray-700 rounded-lg shadow-xl p-2">
        {#if tokenOptions.length === 0}
          <div class="px-2 py-1 text-sm text-gray-500">No tokens available.</div>
        {:else}
          {#each tokenOptions as opt (opt.principal)}
            <label class="flex items-center gap-2 px-2 py-1 rounded hover:bg-gray-800 cursor-pointer text-sm text-gray-200">
              <input
                type="checkbox"
                checked={facets.tokens.includes(opt.principal)}
                onchange={() => toggleToken(opt.principal)}
                class="accent-teal-500"
              />
              {opt.label}
            </label>
          {/each}
        {/if}
      </div>
    {/if}
  </div>

  <!-- Pool -->
  <div class="relative">
    <button type="button" class={btnClass('pool')} onclick={(e) => { e.stopPropagation(); toggle('pool'); }}>
      {poolLabel} <span class="text-xs opacity-70">▾</span>
    </button>
    {#if openDropdown === 'pool'}
      <div class="absolute left-0 top-full mt-1 z-30 min-w-[220px] max-h-[360px] overflow-y-auto bg-gray-900 border border-gray-700 rounded-lg shadow-xl p-2">
        {#if poolOptions.length === 0}
          <div class="px-2 py-1 text-sm text-gray-500">No pools available.</div>
        {:else}
          {#each poolOptions as opt (opt.id)}
            <label class="flex items-center gap-2 px-2 py-1 rounded hover:bg-gray-800 cursor-pointer text-sm text-gray-200">
              <input
                type="checkbox"
                checked={facets.pools.includes(opt.id)}
                onchange={() => togglePool(opt.id)}
                class="accent-teal-500"
              />
              {opt.label}
            </label>
          {/each}
        {/if}
      </div>
    {/if}
  </div>

  <!-- Entity -->
  <div class="relative">
    <button type="button" class={btnClass('entity')} onclick={(e) => { e.stopPropagation(); toggle('entity'); }}>
      {entityLabel} <span class="text-xs opacity-70">▾</span>
    </button>
    {#if openDropdown === 'entity'}
      <div class="absolute left-0 top-full mt-1 z-30 min-w-[320px] bg-gray-900 border border-gray-700 rounded-lg shadow-xl p-3">
        <div class="text-[10px] uppercase tracking-wider text-gray-500 mb-1">
          Vault ID or principal
        </div>
        <input
          type="text"
          placeholder="e.g. 42 or hg7sz-..."
          bind:value={entityQuery}
          onkeydown={handleEntityKeydown}
          class="w-full px-2 py-1.5 text-sm bg-gray-800 border border-gray-600 rounded text-gray-100 placeholder:text-gray-500 focus:border-teal-500 focus:outline-none"
        />
        <div class="mt-2 flex justify-end gap-2">
          <button type="button" class="text-xs text-gray-400 hover:text-gray-200 px-2 py-1" onclick={close}>
            Cancel
          </button>
          <button type="button" class="text-xs text-teal-400 hover:text-teal-300 px-2 py-1" onclick={submitEntity}>
            Add
          </button>
        </div>
      </div>
    {/if}
  </div>

  <!-- Size -->
  <div class="relative">
    <button type="button" class={btnClass('size')} onclick={(e) => { e.stopPropagation(); toggle('size'); }}>
      {sizeLabel} <span class="text-xs opacity-70">▾</span>
    </button>
    {#if openDropdown === 'size'}
      <div class="absolute left-0 top-full mt-1 z-30 min-w-[220px] bg-gray-900 border border-gray-700 rounded-lg shadow-xl p-3">
        <div class="flex flex-col gap-1">
          {#each SIZE_PRESETS as p (p.key)}
            <button
              type="button"
              class="text-left px-2 py-1 text-sm rounded hover:bg-gray-800 {facets.minSizeUsd === p.min ? 'text-teal-300' : 'text-gray-200'}"
              onclick={() => { setSize(p.min); close(); }}
            >
              {p.label}
            </button>
          {/each}
          <button
            type="button"
            class="text-left px-2 py-1 text-sm rounded hover:bg-gray-800 text-gray-400"
            onclick={() => { setSize(null); close(); }}
          >
            Any size
          </button>
        </div>
        <div class="mt-2 pt-2 border-t border-gray-700">
          <div class="text-[10px] uppercase tracking-wider text-gray-500 mb-1">Custom minimum ($)</div>
          <div class="flex gap-2">
            <input
              type="number"
              min="1"
              placeholder="e.g. 5000"
              bind:value={customSize}
              class="w-full px-2 py-1 text-sm bg-gray-800 border border-gray-600 rounded text-gray-100 placeholder:text-gray-500 focus:border-teal-500 focus:outline-none"
            />
            <button type="button" class="text-xs text-teal-400 hover:text-teal-300 px-2" onclick={applyCustomSize}>
              Apply
            </button>
          </div>
        </div>
      </div>
    {/if}
  </div>

  <!-- Time -->
  <div class="relative">
    <button type="button" class={btnClass('time')} onclick={(e) => { e.stopPropagation(); toggle('time'); }}>
      {timeLabel} <span class="text-xs opacity-70">▾</span>
    </button>
    {#if openDropdown === 'time'}
      <div class="absolute left-0 top-full mt-1 z-30 min-w-[280px] bg-gray-900 border border-gray-700 rounded-lg shadow-xl p-3">
        <div class="flex flex-wrap gap-1">
          {#each TIME_PRESETS as p (p.key)}
            <button
              type="button"
              class="px-2 py-1 text-xs rounded {facets.time.preset === p.key ? 'bg-teal-500/20 text-teal-200 border border-teal-500/40' : 'bg-gray-800 text-gray-300 border border-gray-700 hover:border-gray-500'}"
              onclick={() => setTime(p.key)}
            >
              {p.label}
            </button>
          {/each}
        </div>
        <div class="mt-3 pt-2 border-t border-gray-700">
          <div class="text-[10px] uppercase tracking-wider text-gray-500 mb-1">Custom range</div>
          <div class="grid grid-cols-2 gap-2 text-xs">
            <label class="flex flex-col text-gray-500">From
              <input
                type="datetime-local"
                bind:value={customFrom}
                class="mt-1 px-2 py-1 bg-gray-800 border border-gray-600 rounded text-gray-100 focus:border-teal-500 focus:outline-none"
              />
            </label>
            <label class="flex flex-col text-gray-500">To
              <input
                type="datetime-local"
                bind:value={customTo}
                class="mt-1 px-2 py-1 bg-gray-800 border border-gray-600 rounded text-gray-100 focus:border-teal-500 focus:outline-none"
              />
            </label>
          </div>
          <div class="mt-2 flex justify-end">
            <button type="button" class="text-xs text-teal-400 hover:text-teal-300 px-2 py-1" onclick={applyCustomTime}>
              Apply range
            </button>
          </div>
        </div>
      </div>
    {/if}
  </div>
</div>
