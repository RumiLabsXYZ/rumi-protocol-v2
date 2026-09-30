<script lang="ts">
  import { tick } from 'svelte';
  import type { CkErc20TokenConfig } from '$lib/services/ckerc20Minter';
  import { ckErc20Logo } from '$lib/utils/ckerc20Logos';

  export let tokens: CkErc20TokenConfig[] = [];
  export let selectedLedgerId = '';
  export let disabled = false;
  export let label = 'Choose token';
  export let id: string;
  export let onSelect: (ledgerId: string) => void;

  let open = false;
  let openAbove = false;
  let menuMaxHeight = 300;
  let root: HTMLDivElement;
  let trigger: HTMLButtonElement;
  let menu: HTMLDivElement;
  let listbox: HTMLDivElement;
  $: selected = tokens.find((token) => token.ledgerId === selectedLedgerId);
  $: if (disabled && open) open = false;

  function options() {
    return Array.from(listbox?.querySelectorAll<HTMLButtonElement>('[role="option"]') ?? []);
  }

  function focusOption(index: number) {
    const option = options()[index];
    if (!option) return;

    // Keep keyboard navigation inside a short mobile menu without scrolling the page.
    option.focus({ preventScroll: true });
    if (menu) {
      const optionBounds = option.getBoundingClientRect();
      const menuBounds = menu.getBoundingClientRect();
      const optionTop = menu.scrollTop + optionBounds.top - menuBounds.top - menu.clientTop;
      const optionBottom = optionTop + optionBounds.height;
      if (optionTop < menu.scrollTop) menu.scrollTop = optionTop;
      else if (optionBottom > menu.scrollTop + menu.clientHeight) {
        menu.scrollTop = optionBottom - menu.clientHeight;
      }
    }
  }

  async function openMenu() {
    if (disabled) return;
    open = true;
    await tick();
    if (!open || disabled || !trigger || !menu) return;

    const triggerBounds = trigger.getBoundingClientRect();
    const roomAbove = Math.max(0, triggerBounds.top - 12);
    const roomBelow = Math.max(0, window.innerHeight - triggerBounds.bottom - 12);
    const desiredHeight = Math.min(300, Math.max(88, tokens.length * 44 + 48));
    openAbove = roomBelow < desiredHeight && roomAbove > roomBelow;
    menuMaxHeight = Math.max(1, Math.min(300, openAbove ? roomAbove : roomBelow));
    await tick();
    if (!open || disabled || !trigger || !menu) return;

    const selectedIndex = tokens.findIndex((token) => token.ledgerId === selectedLedgerId);
    focusOption(selectedIndex >= 0 ? selectedIndex : 0);
  }

  async function toggle() {
    if (open) {
      open = false;
      return;
    }
    await openMenu();
  }

  function choose(ledgerId: string) {
    if (disabled) return;
    open = false;
    onSelect(ledgerId);
    trigger.focus({ preventScroll: true });
  }

  function handleKeys(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      open = false;
      trigger.focus({ preventScroll: true });
      return;
    }
    if (event.key === 'Tab') {
      // Let the browser move focus naturally; the listbox is a single tab stop.
      open = false;
      return;
    }

    const currentOptions = options();
    if (currentOptions.length === 0) return;
    const current = currentOptions.indexOf(document.activeElement as HTMLButtonElement);
    let next = current;
    if (event.key === 'ArrowDown') next = (current + 1 + currentOptions.length) % currentOptions.length;
    else if (event.key === 'ArrowUp') next = (current - 1 + currentOptions.length) % currentOptions.length;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = currentOptions.length - 1;
    else return;

    event.preventDefault();
    focusOption(next);
  }

  function handleTriggerKeys(event: KeyboardEvent) {
    if (!open && (event.key === 'ArrowDown' || event.key === 'ArrowUp')) {
      event.preventDefault();
      void openMenu();
    } else if (open && event.key === 'Escape') {
      event.preventDefault();
      open = false;
      trigger.focus({ preventScroll: true });
    } else if (open && event.key === 'Tab') {
      open = false;
    }
  }
</script>

<svelte:window on:click={(event) => { if (open && root && !root.contains(event.target as Node)) open = false; }} />

<div class="token-selector" bind:this={root}>
  <button
    class="token-trigger"
    type="button"
    bind:this={trigger}
    {disabled}
    aria-label={`${label}: ${selected?.symbol ?? 'Token'}`}
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-controls={id}
    on:click={toggle}
    on:keydown={handleTriggerKeys}
  >
    {#if selected && ckErc20Logo(selected.symbol)}<img src={ckErc20Logo(selected.symbol)} alt="" width="26" height="26" />{/if}
    <span>{selected?.symbol ?? 'Token'}</span>
    <svg class:open width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="m4 6 4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" /></svg>
  </button>
  {#if open}
    <div
      class="token-menu"
      class:above={openAbove}
      style={`--menu-max-height: ${menuMaxHeight}px`}
      bind:this={menu}
    >
      <p class="menu-label">SUPPORTED ON ETHEREUM</p>
      {#if tokens.length > 0}
        <div id={id} role="listbox" aria-label={label} tabindex="-1" bind:this={listbox} on:keydown={handleKeys}>
          {#each tokens as token (token.ledgerId)}
            <button type="button" role="option" aria-selected={token.ledgerId === selectedLedgerId} class:selected={token.ledgerId === selectedLedgerId} tabindex="-1" on:click={() => choose(token.ledgerId)}>
              {#if ckErc20Logo(token.symbol)}<img src={ckErc20Logo(token.symbol)} alt="" width="28" height="28" />{/if}
              <span>{token.symbol}</span>
              <small aria-hidden="true">{token.symbol.replace(/^ck/, '')}</small>
              {#if token.ledgerId === selectedLedgerId}<svg width="16" height="16" viewBox="0 0 20 20" fill="none" aria-hidden="true"><path d="m4 10 4 4 8-8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>{/if}
            </button>
          {/each}
        </div>
      {:else}
        <p class="empty-message" role="status">No tokens available</p>
      {/if}
    </div>
  {/if}
</div>

<style>
  .token-selector { position: relative; flex-shrink: 0; }
  .token-trigger { display: flex; align-items: center; gap: 8px; height: 42px; padding: 0 10px; border: 1px solid var(--rumi-border-hover); border-radius: 24px; background: var(--rumi-bg-surface2); color: var(--rumi-text-primary); font: inherit; font-size: 13px; font-weight: 600; cursor: pointer; }
  img { flex-shrink: 0; border-radius: 50%; object-fit: contain; }
  .token-trigger:hover:not(:disabled) { background: var(--rumi-bg-surface3); }
  .token-trigger:focus-visible, .token-menu button:focus-visible { outline: 2px solid var(--rumi-action); outline-offset: 2px; }
  .token-trigger:disabled { opacity: .5; cursor: not-allowed; }
  .token-trigger svg { color: var(--rumi-text-secondary); transition: transform .15s; }
  .token-trigger svg.open { transform: rotate(180deg); }
  .token-menu { position: absolute; top: calc(100% + 8px); right: 0; z-index: 20; box-sizing: border-box; width: 248px; max-height: var(--menu-max-height, 300px); overflow-y: auto; overscroll-behavior: contain; padding: 8px; background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border-hover); border-radius: 12px; box-shadow: 0 16px 40px #0004; }
  .token-menu.above { top: auto; bottom: calc(100% + 8px); }
  .menu-label { margin: 4px 8px 8px; font-size: 10px; font-weight: 600; letter-spacing: .08em; color: var(--rumi-text-secondary); }
  .token-menu [role="option"] { width: 100%; display: flex; align-items: center; gap: 10px; padding: 8px; border: 0; border-radius: 8px; color: var(--rumi-text-primary); background: transparent; font: inherit; font-size: 13px; cursor: pointer; text-align: left; }
  .token-menu [role="option"]:hover, .token-menu [role="option"]:focus-visible { background: var(--rumi-bg-surface3); }
  .token-menu [role="option"].selected { background: var(--rumi-action-dim); }
  .token-menu small { margin-left: auto; font-size: 11px; color: var(--rumi-text-secondary); }
  .token-menu [role="option"] svg { color: var(--rumi-action); }
  .empty-message { margin: 8px; color: var(--rumi-text-secondary); font-size: 13px; }
</style>
