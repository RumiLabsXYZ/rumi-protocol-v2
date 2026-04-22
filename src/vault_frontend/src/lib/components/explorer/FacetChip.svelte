<script lang="ts">
  import {
    addFacetValue, buildFacetsQueryString, emptyFacets,
    type FacetKind, type Facets,
  } from '$utils/eventFacets';
  import type { Snippet } from 'svelte';

  interface Props {
    kind: FacetKind;
    value: string | number;
    label: string;
    title?: string;
    class?: string;
    /** When set, clicking the chip adds this facet to the provided active filter. */
    onFacetClick?: (next: Facets) => void;
    currentFacets?: Facets;
    children?: Snippet;
  }

  let {
    kind, value, label, title,
    class: extraClass = '',
    onFacetClick, currentFacets,
    children,
  }: Props = $props();

  const alreadyActive = $derived.by(() => {
    const f = currentFacets;
    if (!f) return false;
    switch (kind) {
      case 'type': return f.types.includes(value as any);
      case 'token': return f.tokens.includes(String(value));
      case 'pool': return f.pools.includes(String(value));
      case 'vault': return f.vaultIds.includes(Number(value));
      case 'principal': return f.principals.includes(String(value));
      default: return false;
    }
  });

  const prefilledHref = $derived.by(() => {
    const next = addFacetValue(currentFacets ?? emptyFacets(), kind, value as any);
    return `/explorer/activity${buildFacetsQueryString(next)}`;
  });

  function handleClick(ev: MouseEvent) {
    if (!onFacetClick) return;
    ev.preventDefault();
    ev.stopPropagation();
    if (alreadyActive) return;
    const base = currentFacets ?? emptyFacets();
    onFacetClick(addFacetValue(base, kind, value as any));
  }

  const baseClass = 'cursor-pointer hover:ring-1 hover:ring-teal-400/50 rounded transition';
</script>

{#if onFacetClick}
  <button
    type="button"
    class="{extraClass} {baseClass} text-left"
    class:opacity-60={alreadyActive}
    {title}
    onclick={handleClick}
  >
    {#if children}{@render children()}{:else}{label}{/if}
  </button>
{:else}
  <a
    href={prefilledHref}
    class="{extraClass} {baseClass}"
    {title}
  >
    {#if children}{@render children()}{:else}{label}{/if}
  </a>
{/if}
