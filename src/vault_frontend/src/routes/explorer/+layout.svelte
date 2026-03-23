<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import type { Snippet } from 'svelte';

  let { children }: { children: Snippet } = $props();
  let searchQuery = $state('');

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

  function handleSearch() {
    const q = searchQuery.trim();
    if (!q) return;

    // Pure numeric -> vault ID
    if (/^\d+$/.test(q)) {
      goto(`/explorer/vault/${q}`);
      searchQuery = '';
      return;
    }

    // Event index prefixed with #
    if (/^#\d+$/.test(q)) {
      goto(`/explorer/event/${q.slice(1)}`);
      searchQuery = '';
      return;
    }

    // Principal-like (contains dashes, at least 10 chars)
    if (q.includes('-') && q.length >= 10) {
      goto(`/explorer/address/${q}`);
      searchQuery = '';
      return;
    }

    // Fallback: treat as address
    goto(`/explorer/address/${q}`);
    searchQuery = '';
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') handleSearch();
  }
</script>

<div class="explorer-subnav">
  <div class="subnav-inner">
    <nav class="subnav-links">
      {#each navLinks as link}
        <a
          href={link.href}
          class="subnav-link"
          class:active={isActive(link)}
        >
          {link.label}
        </a>
      {/each}
    </nav>
    <div class="subnav-search">
      <input
        type="text"
        bind:value={searchQuery}
        onkeydown={handleKeydown}
        placeholder="Search vault, address, event..."
        class="subnav-search-input"
      />
      <button onclick={handleSearch} class="subnav-search-btn" aria-label="Search">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="16" height="16">
          <circle cx="11" cy="11" r="8"/><path d="M21 21l-4.35-4.35"/>
        </svg>
      </button>
    </div>
  </div>
</div>

{@render children()}

<style>
  .explorer-subnav {
    border-bottom: 1px solid var(--rumi-border);
    background: var(--rumi-bg-surface-1);
  }

  .subnav-inner {
    max-width: 1200px;
    margin: 0 auto;
    padding: 0 1rem;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    height: 2.75rem;
  }

  .subnav-links {
    display: flex;
    align-items: center;
    gap: 0.125rem;
  }

  .subnav-link {
    position: relative;
    display: flex;
    align-items: center;
    padding: 0.5rem 0.75rem;
    font-size: 0.8125rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    text-decoration: none;
    border-radius: 0.375rem;
    transition: color 0.15s ease, background 0.15s ease;
  }

  .subnav-link:hover {
    color: var(--rumi-text-secondary);
    background: rgba(255, 255, 255, 0.03);
  }

  .subnav-link.active {
    color: var(--rumi-text-primary);
  }

  .subnav-link.active::after {
    content: '';
    position: absolute;
    bottom: -0.6875rem;
    left: 0.75rem;
    right: 0.75rem;
    height: 2px;
    background: var(--rumi-action);
    border-radius: 1px 1px 0 0;
  }

  .subnav-search {
    display: flex;
    align-items: center;
    gap: 0.25rem;
  }

  .subnav-search-input {
    width: 14rem;
    padding: 0.375rem 0.625rem;
    font-size: 0.75rem;
    border-radius: 0.375rem;
    border: 1px solid var(--rumi-border);
    background: var(--rumi-bg-surface-2);
    color: var(--rumi-text-primary);
    outline: none;
    transition: border-color 0.15s;
  }

  .subnav-search-input::placeholder {
    color: var(--rumi-text-muted);
  }

  .subnav-search-input:focus {
    border-color: var(--rumi-border-hover);
  }

  .subnav-search-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 1.75rem;
    height: 1.75rem;
    border-radius: 0.375rem;
    border: 1px solid var(--rumi-border);
    background: var(--rumi-bg-surface-2);
    color: var(--rumi-text-muted);
    cursor: pointer;
    transition: color 0.15s, border-color 0.15s;
  }

  .subnav-search-btn:hover {
    color: var(--rumi-text-primary);
    border-color: var(--rumi-border-hover);
  }

  @media (max-width: 640px) {
    .subnav-inner {
      flex-direction: column;
      height: auto;
      padding: 0.5rem 1rem;
      gap: 0.5rem;
    }

    .subnav-links {
      width: 100%;
      overflow-x: auto;
      -webkit-overflow-scrolling: touch;
    }

    .subnav-search {
      width: 100%;
    }

    .subnav-search-input {
      flex: 1;
      width: auto;
    }

    .subnav-link.active::after {
      bottom: -0.5rem;
    }
  }
</style>
