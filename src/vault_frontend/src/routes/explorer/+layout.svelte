<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import SearchBar from '$lib/components/explorer/SearchBar.svelte';
  import {
    isVaultId, isEventIndex, parseEventIndex, isPrincipal, resolveTokenAlias
  } from '$lib/utils/explorerHelpers';
  import type { Snippet } from 'svelte';

  let { children }: { children: Snippet } = $props();

  let mobileNavOpen = $state(false);
  let mobileSearchOpen = $state(false);

  let currentPath = $derived($page.url.pathname);

  // Three-section IA: Protocol (lens-scoped dashboard), Activity (query layer),
  // Entities (vault / token / address / pool / canister / event — reached via
  // search + links, no dedicated nav tab).
  const NAV_ITEMS = [
    {
      href: '/explorer',
      label: 'Protocol',
      match: (p: string) =>
        p === '/explorer'
        || p.startsWith('/explorer/markets')
        || p.startsWith('/explorer/pools')
        || p.startsWith('/explorer/revenue')
        || p.startsWith('/explorer/risk')
        || p.startsWith('/explorer/stats')
        || p.startsWith('/explorer/holders')
        || p.startsWith('/explorer/token/')
    },
    {
      href: '/explorer/activity',
      label: 'Activity',
      match: (p: string) =>
        p.startsWith('/explorer/activity')
        || p.startsWith('/explorer/events')
        || p.startsWith('/explorer/event/')
        || p.startsWith('/explorer/e/event/')
        || p.startsWith('/explorer/dex/')
        || p.startsWith('/explorer/liquidations')
    },
  ];

  function handleSearch(query: string) {
    const q = query.trim();
    if (!q) return;
    if (isVaultId(q)) goto(`/explorer/e/vault/${q}`);
    else if (isEventIndex(q)) goto(`/explorer/e/event/${parseEventIndex(q)}`);
    else if (resolveTokenAlias(q)) goto(`/explorer/token/${resolveTokenAlias(q)}`);
    else if (isPrincipal(q)) goto(`/explorer/e/address/${q}`);
    mobileSearchOpen = false;
    mobileNavOpen = false;
  }

  function handleNavClick() {
    mobileNavOpen = false;
  }
</script>

<!-- Explorer Header Bar -->
<header class="border-b" style="border-color: var(--rumi-border); background: rgba(14, 18, 34, 0.85); backdrop-filter: blur(8px);">
  <div class="mx-auto flex h-14 max-w-[1200px] items-center justify-between px-4">
    <!-- Left: Title -->
    <a href="/explorer" class="flex items-center gap-2 text-base font-semibold hover:opacity-80 transition-opacity" style="color: var(--rumi-text-primary);">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" class="h-5 w-5" style="color: var(--rumi-teal);">
        <path d="M3.75 3v11.25A2.25 2.25 0 006 16.5h2.25M3.75 3h-1.5m1.5 0h16.5m0 0h1.5m-1.5 0v11.25A2.25 2.25 0 0118 16.5h-2.25m-7.5 0h7.5m-7.5 0l-1 3m8.5-3l1 3m0 0l.5 1.5m-.5-1.5h-9.5m0 0l-.5 1.5" />
      </svg>
      <span class="hidden sm:inline">Explorer</span>
    </a>

    <!-- Center: Nav tabs (desktop) -->
    <nav class="hidden md:flex items-center gap-0.5">
      {#each NAV_ITEMS as item}
        <a
          href={item.href}
          class="relative px-3 py-1.5 text-sm font-medium rounded-md transition-colors"
          style="{item.match(currentPath)
            ? `color: var(--rumi-teal); background: var(--rumi-teal-dim);`
            : `color: var(--rumi-text-secondary);`}"
          onmouseenter={(e) => { if (!item.match(currentPath)) { const t = e.currentTarget as HTMLElement; t.style.color = 'var(--rumi-text-primary)'; t.style.background = 'var(--rumi-bg-surface2)'; } }}
          onmouseleave={(e) => { if (!item.match(currentPath)) { const t = e.currentTarget as HTMLElement; t.style.color = 'var(--rumi-text-secondary)'; t.style.background = 'transparent'; } }}
        >
          {item.label}
          {#if item.match(currentPath)}
            <span class="absolute -bottom-[0.875rem] left-3 right-3 h-0.5 rounded-t" style="background: var(--rumi-teal);"></span>
          {/if}
        </a>
      {/each}
    </nav>

    <!-- Right: Search + mobile toggles -->
    <div class="flex items-center gap-2">
      <!-- Search bar (desktop) -->
      <div class="hidden md:block w-64">
        <SearchBar onSearch={handleSearch} />
      </div>

      <!-- Search toggle (mobile) -->
      <button
        onclick={() => { mobileSearchOpen = !mobileSearchOpen; mobileNavOpen = false; }}
        class="md:hidden flex items-center justify-center w-9 h-9 rounded-md transition-colors"
        style="color: var(--rumi-text-secondary);"
        aria-label="Toggle search"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="w-5 h-5">
          <circle cx="11" cy="11" r="8" /><path d="M21 21l-4.35-4.35" />
        </svg>
      </button>

      <!-- Hamburger (mobile) -->
      <button
        onclick={() => { mobileNavOpen = !mobileNavOpen; mobileSearchOpen = false; }}
        class="md:hidden flex items-center justify-center w-9 h-9 rounded-md transition-colors"
        style="color: var(--rumi-text-secondary);"
        aria-label="Toggle navigation"
      >
        {#if mobileNavOpen}
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="w-5 h-5">
            <path d="M6 18L18 6M6 6l12 12" />
          </svg>
        {:else}
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="w-5 h-5">
            <path d="M4 6h16M4 12h16M4 18h16" />
          </svg>
        {/if}
      </button>
    </div>
  </div>

  <!-- Mobile search dropdown -->
  {#if mobileSearchOpen}
    <div class="md:hidden px-4 py-3" style="border-top: 1px solid var(--rumi-border); background: var(--rumi-bg-surface1);">
      <SearchBar onSearch={handleSearch} />
    </div>
  {/if}

  <!-- Mobile nav dropdown -->
  {#if mobileNavOpen}
    <nav class="md:hidden px-4 py-2" style="border-top: 1px solid var(--rumi-border); background: var(--rumi-bg-surface1);">
      {#each NAV_ITEMS as item}
        <a
          href={item.href}
          onclick={handleNavClick}
          class="block rounded-md px-3 py-2 text-sm font-medium transition-colors"
          style="{item.match(currentPath)
            ? `color: var(--rumi-teal); background: var(--rumi-teal-dim);`
            : `color: var(--rumi-text-secondary);`}"
        >
          {item.label}
        </a>
      {/each}
    </nav>
  {/if}
</header>

<!-- Content area -->
<main class="mx-auto max-w-[1200px] px-4 py-6">
  {@render children()}
</main>
