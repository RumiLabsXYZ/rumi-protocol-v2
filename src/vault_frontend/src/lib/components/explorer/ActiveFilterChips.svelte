<script lang="ts">
  import { shortenPrincipal, getTokenSymbol } from '$utils/explorerHelpers';
  import { typeFacetLabel, hasAnyFacet, type Facets } from '$utils/eventFacets';
  import { ammPoolLabel } from '$utils/ammNaming';
  import { CANISTER_IDS } from '$lib/config';

  interface Props {
    facets: Facets;
    onChange: (next: Facets) => void;
    onClear: () => void;
    onSaveView: () => void;
  }

  let { facets, onChange, onClear, onSaveView }: Props = $props();

  // Friendly label for a pool facet value: "Rumi 3Pool" for the literal
  // 3pool token and "AMM1 · 3USD/ICP" for AMM pools (resolved through the
  // shared registry that DexsLens seeds at load time). Falls back to a
  // shortened principal pair for anything still unknown.
  function poolChipLabel(id: string): string {
    if (id === '3pool' || id === CANISTER_IDS.THREEPOOL) return 'Rumi 3Pool';
    const friendly = ammPoolLabel(id);
    if (friendly && friendly !== 'AMM' && friendly !== id) return friendly;
    // Last-resort: split principal pair on `_` and shorten each side
    if (id.includes('_')) {
      return id.split('_').map(shortenPrincipal).join(' / ');
    }
    return shortenPrincipal(id);
  }

  const anyActive = $derived(hasAnyFacet(facets));

  function removeType(k: string) {
    onChange({ ...facets, types: facets.types.filter((t) => t !== k) as typeof facets.types });
  }

  function removeToken(p: string) {
    onChange({ ...facets, tokens: facets.tokens.filter((t) => t !== p) });
  }

  function removePool(id: string) {
    onChange({ ...facets, pools: facets.pools.filter((p) => p !== id) });
  }

  function removeVault(id: number) {
    onChange({ ...facets, vaultIds: facets.vaultIds.filter((v) => v !== id) });
  }

  function removePrincipal(p: string) {
    onChange({ ...facets, principals: facets.principals.filter((x) => x !== p) });
  }

  function removeAdminLabel(label: string) {
    onChange({ ...facets, adminLabels: facets.adminLabels.filter((x) => x !== label) });
  }

  function removeSize() {
    onChange({ ...facets, minSizeUsd: null });
  }

  function removeTime() {
    onChange({ ...facets, time: { preset: 'all' } });
  }

  function formatTimeChip(): string {
    if (facets.time.preset === 'all') return '';
    if (facets.time.preset === 'custom') {
      const from = facets.time.fromMs ? new Date(facets.time.fromMs).toLocaleDateString() : '…';
      const to = facets.time.toMs ? new Date(facets.time.toMs).toLocaleDateString() : 'now';
      return `${from} → ${to}`;
    }
    return facets.time.preset;
  }
</script>

{#if anyActive}
  <div class="flex flex-wrap items-center gap-2">
    {#each facets.types as t (t)}
      <span class="inline-flex items-center gap-1.5 px-2 py-0.5 text-xs rounded-full bg-teal-500/15 text-teal-300 border border-teal-500/30">
        <span>type:{typeFacetLabel(t)}</span>
        <button type="button" aria-label="Remove type" class="text-teal-200 hover:text-white" onclick={() => removeType(t)}>×</button>
      </span>
    {/each}

    {#each facets.tokens as p (p)}
      <span class="inline-flex items-center gap-1.5 px-2 py-0.5 text-xs rounded-full bg-emerald-500/15 text-emerald-300 border border-emerald-500/30">
        <span>token:{getTokenSymbol(p)}</span>
        <button type="button" aria-label="Remove token" class="text-emerald-200 hover:text-white" onclick={() => removeToken(p)}>×</button>
      </span>
    {/each}

    {#each facets.pools as id (id)}
      <span class="inline-flex items-center gap-1.5 px-2 py-0.5 text-xs rounded-full bg-cyan-500/15 text-cyan-300 border border-cyan-500/30" title={`pool:${id}`}>
        <span>pool:{poolChipLabel(id)}</span>
        <button type="button" aria-label="Remove pool" class="text-cyan-200 hover:text-white" onclick={() => removePool(id)}>×</button>
      </span>
    {/each}

    {#each facets.vaultIds as v (v)}
      <span class="inline-flex items-center gap-1.5 px-2 py-0.5 text-xs rounded-full bg-blue-500/15 text-blue-300 border border-blue-500/30">
        <span>vault:#{v}</span>
        <button type="button" aria-label="Remove vault" class="text-blue-200 hover:text-white" onclick={() => removeVault(v)}>×</button>
      </span>
    {/each}

    {#each facets.principals as p (p)}
      <span class="inline-flex items-center gap-1.5 px-2 py-0.5 text-xs rounded-full bg-violet-500/15 text-violet-300 border border-violet-500/30 font-mono">
        <span>addr:{shortenPrincipal(p)}</span>
        <button type="button" aria-label="Remove principal" class="text-violet-200 hover:text-white" onclick={() => removePrincipal(p)}>×</button>
      </span>
    {/each}

    {#each facets.adminLabels as label (label)}
      <span class="inline-flex items-center gap-1.5 px-2 py-0.5 text-xs rounded-full bg-blue-500/15 text-blue-300 border border-blue-500/30 font-mono">
        <span>admin:{label}</span>
        <button type="button" aria-label="Remove admin label" class="text-blue-200 hover:text-white" onclick={() => removeAdminLabel(label)}>×</button>
      </span>
    {/each}

    {#if facets.minSizeUsd != null}
      <span class="inline-flex items-center gap-1.5 px-2 py-0.5 text-xs rounded-full bg-amber-500/15 text-amber-300 border border-amber-500/30">
        <span>&gt; ${facets.minSizeUsd.toLocaleString()}</span>
        <button type="button" aria-label="Remove size filter" class="text-amber-200 hover:text-white" onclick={removeSize}>×</button>
      </span>
    {/if}

    {#if facets.time.preset !== 'all'}
      <span class="inline-flex items-center gap-1.5 px-2 py-0.5 text-xs rounded-full bg-gray-600/30 text-gray-200 border border-gray-500/40">
        <span>time:{formatTimeChip()}</span>
        <button type="button" aria-label="Remove time filter" class="text-gray-200 hover:text-white" onclick={removeTime}>×</button>
      </span>
    {/if}

    <button
      type="button"
      class="text-xs text-gray-400 hover:text-gray-200 underline underline-offset-2"
      onclick={onClear}
    >
      Clear all
    </button>
    <button
      type="button"
      class="text-xs text-teal-400 hover:text-teal-300 underline underline-offset-2"
      onclick={onSaveView}
    >
      Save view…
    </button>
  </div>
{/if}
