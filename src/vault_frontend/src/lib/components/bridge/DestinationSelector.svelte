<script lang="ts">
  import {
    isEnabled,
    isSelectorKey,
    nextEnabledDestination,
    type DestinationView,
  } from '$lib/services/baseBridge/destinations';

  interface Props {
    destinations: readonly DestinationView[];
    /** The destination the page is on. May be disabled (it was chosen, then the bridge turned it off). */
    selectedId: string | null;
    /** Called only for an enabled destination. */
    onSelect: (id: string) => void;
  }

  let { destinations, selectedId, onSelect }: Props = $props();

  const optionId = (id: string) => `destination-${id}`;

  // One tab stop for the whole group: the selected entry, or the first enabled one when the selected one has been turned off.
  const tabStopId = $derived(
    (destinations.find((destination) => destination.id === selectedId && isEnabled(destination)) ?? destinations.find(isEnabled))?.id ?? null,
  );

  function onKeydown(event: KeyboardEvent) {
    if (!isSelectorKey(event.key)) return;
    event.preventDefault();
    // Disabled entries are skipped, so the keyboard can never select or focus one.
    const next = nextEnabledDestination(destinations, selectedId, event.key);
    if (!next) return;
    onSelect(next.id);
    document.getElementById(optionId(next.id))?.focus();
  }

  const networkLabel = (destination: DestinationView) => (destination.network === 'testnet' ? 'Testnet' : 'Mainnet');
</script>

{#snippet details(destination: DestinationView)}
  <span class="top">
    <span class="name">{destination.name}</span>
    {#if destination.chainId}
      <span class="net" class:testnet={destination.network === 'testnet'}>{networkLabel(destination)}</span>
    {/if}
    {#if !isEnabled(destination)}<span class="flag">Unavailable</span>{/if}
  </span>
  {#if destination.chainId}<span class="chain">Chain {destination.chainId}</span>{/if}
  {#if !isEnabled(destination) && destination.reason}<span class="reason">{destination.reason}</span>{/if}
{/snippet}

<div class="options" role="radiogroup" aria-label="Destination network" tabindex="-1" onkeydown={onKeydown}>
  {#each destinations as destination (destination.id)}
    {#if isEnabled(destination)}
      <button
        type="button"
        role="radio"
        id={optionId(destination.id)}
        class="option"
        class:selected={destination.id === selectedId}
        aria-checked={destination.id === selectedId}
        tabindex={destination.id === tabStopId ? 0 : -1}
        onclick={() => onSelect(destination.id)}>
        {@render details(destination)}
      </button>
    {:else}
      <!-- A disabled entry is not a control: no handler, no tab stop, never focusable, never reached by an arrow key. -->
      <div role="radio" id={optionId(destination.id)} class="option off" aria-checked={destination.id === selectedId} aria-disabled="true">
        {@render details(destination)}
      </div>
    {/if}
  {/each}
</div>

<style>
  .options { display:grid; grid-template-columns:repeat(auto-fill, minmax(11.5rem, 1fr)); gap:0.5rem; }
  .option { display:flex; flex-direction:column; gap:0.2rem; min-width:0; padding:0.6rem 0.75rem; border:1px solid var(--rumi-border); border-radius:0.5rem; background:var(--rumi-bg-primary); color:var(--rumi-text-primary); font:inherit; text-align:left; }
  button.option { cursor:pointer; }
  button.option:hover { border-color:var(--rumi-border-hover); }
  button.option:focus-visible { outline:2px solid var(--rumi-action); outline-offset:2px; }
  .option.selected { background:var(--rumi-bg-surface2); border-color:var(--rumi-border-accent); }
  .option.off { opacity:0.62; cursor:not-allowed; }
  .top { display:flex; flex-wrap:wrap; align-items:center; gap:0.35rem; }
  .name { font-size:0.85rem; font-weight:600; }
  .net, .flag { padding:0.05rem 0.4rem; border:1px solid var(--rumi-border-hover); border-radius:999px; font-size:0.62rem; letter-spacing:0.03em; color:var(--rumi-text-secondary); }
  .net.testnet { border-color:rgba(229,168,90,0.55); color:#ffd9a8; }
  .flag { color:var(--rumi-text-muted); }
  .chain { font-size:0.7rem; color:var(--rumi-text-muted); font-variant-numeric:tabular-nums; }
  .reason { font-size:0.7rem; line-height:1.4; color:var(--rumi-text-secondary); }
</style>
