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

  const navLinks = [
    { href: '/explorer', label: 'Dashboard', exact: true },
    { href: '/explorer/events', label: 'Events', exact: false },
    { href: '/explorer/liquidations', label: 'Liquidations', exact: false },
    { href: '/explorer/stats', label: 'Stats', exact: false },
  ];

  function isActive(link: { href: string; exact: boolean }): boolean {
    if (link.exact) return currentPath === link.href;
    return currentPath.startsWith(link.href);
  }

  function handleSearch(query: string) {
    const q = query.trim();
    if (!q) return;
    if (isVaultId(q)) goto(`/explorer/vault/${q}`);
    else if (isEventIndex(q)) goto(`/explorer/event/${parseEventIndex(q)}`);
    else if (resolveTokenAlias(q)) goto(`/explorer/token/${resolveTokenAlias(q)}`);
    else if (isPrincipal(q)) goto(`/explorer/address/${q}`);
    mobileSearchOpen = false;
    mobileNavOpen = false;
  }

  function handleNavClick() {
    mobileNavOpen = false;
  }
</script>

<!-- Explorer Header Bar -->
<header class="border-b border-white/10 bg-gray-950/80">
  <div class="mx-auto flex h-14 max-w-7xl items-center justify-between px-4">
    <!-- Left: Title -->
    <a href="/explorer" class="flex items-center gap-2 text-lg font-semibold text-white hover:text-indigo-400 transition-colors">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" class="h-5 w-5 text-indigo-400">
        <path d="M3.75 3v11.25A2.25 2.25 0 006 16.5h2.25M3.75 3h-1.5m1.5 0h16.5m0 0h1.5m-1.5 0v11.25A2.25 2.25 0 0118 16.5h-2.25m-7.5 0h7.5m-7.5 0l-1 3m8.5-3l1 3m0 0l.5 1.5m-.5-1.5h-9.5m0 0l-.5 1.5" />
      </svg>
      <span class="hidden sm:inline">Rumi Explorer</span>
    </a>

    <!-- Center: Nav tabs (desktop) -->
    <nav class="hidden md:flex items-center gap-1">
      {#each navLinks as link}
        <a
          href={link.href}
          class="relative px-3 py-1.5 text-sm font-medium rounded-md transition-colors
                 {isActive(link)
                   ? 'text-white bg-white/10'
                   : 'text-white/50 hover:text-white/80 hover:bg-white/5'}"
        >
          {link.label}
          {#if isActive(link)}
            <span class="absolute -bottom-[1.125rem] left-3 right-3 h-0.5 rounded-t bg-indigo-500"></span>
          {/if}
        </a>
      {/each}
    </nav>

    <!-- Right: Search + mobile toggles -->
    <div class="flex items-center gap-2">
      <!-- Search bar (desktop) -->
      <div class="hidden md:block w-72">
        <SearchBar onSearch={handleSearch} />
      </div>

      <!-- Search toggle (mobile) -->
      <button
        onclick={() => { mobileSearchOpen = !mobileSearchOpen; mobileNavOpen = false; }}
        class="md:hidden flex items-center justify-center w-9 h-9 rounded-md text-white/60 hover:text-white hover:bg-white/10 transition-colors"
        aria-label="Toggle search"
      >
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="w-5 h-5">
          <circle cx="11" cy="11" r="8" /><path d="M21 21l-4.35-4.35" />
        </svg>
      </button>

      <!-- Hamburger (mobile) -->
      <button
        onclick={() => { mobileNavOpen = !mobileNavOpen; mobileSearchOpen = false; }}
        class="md:hidden flex items-center justify-center w-9 h-9 rounded-md text-white/60 hover:text-white hover:bg-white/10 transition-colors"
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
    <div class="md:hidden border-t border-white/10 bg-gray-950/95 px-4 py-3">
      <SearchBar onSearch={handleSearch} />
    </div>
  {/if}

  <!-- Mobile nav dropdown -->
  {#if mobileNavOpen}
    <nav class="md:hidden border-t border-white/10 bg-gray-950/95 px-4 py-2">
      {#each navLinks as link}
        <a
          href={link.href}
          onclick={handleNavClick}
          class="block rounded-md px-3 py-2 text-sm font-medium transition-colors
                 {isActive(link)
                   ? 'text-white bg-indigo-500/20'
                   : 'text-white/60 hover:text-white hover:bg-white/5'}"
        >
          {link.label}
        </a>
      {/each}
    </nav>
  {/if}
</header>

<!-- Content area -->
<main class="mx-auto max-w-7xl px-4 py-6">
  {@render children()}
</main>
