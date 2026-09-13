<script lang="ts">
  // Shared secondary navigation for the Earn overview and its two canonical
  // opportunity pages. Plain links (not buttons) so state (URL, back button,
  // bookmarks) stays normal, and `aria-current` marks the active page for
  // assistive tech independent of the visual underline.
  export let active: 'overview' | '3usd' | 'stability-pool';

  // `shortLabel` keeps all three links on one line at narrow widths (390px);
  // the full label still renders at wider breakpoints via CSS below.
  const tabs: { key: typeof active; label: string; shortLabel?: string; href: string }[] = [
    { key: 'overview', label: 'Overview', href: '/earn' },
    { key: '3usd', label: 'Stablecoin Liquidity · 3USD', shortLabel: '3USD Liquidity', href: '/3usd' },
    { key: 'stability-pool', label: 'Stability Pool', href: '/stability-pool' },
  ];
</script>

<nav class="earn-subnav" aria-label="Earn sections">
  {#each tabs as tab}
    <a
      href={tab.href}
      class="earn-subnav-link"
      class:active={active === tab.key}
      aria-current={active === tab.key ? 'page' : undefined}
    >
      {#if tab.shortLabel}
        <span class="label-full">{tab.label}</span>
        <span class="label-short">{tab.shortLabel}</span>
      {:else}
        {tab.label}
      {/if}
    </a>
  {/each}
</nav>

<style>
  .earn-subnav {
    display: flex;
    gap: 1.25rem;
    margin-bottom: 1.5rem;
    border-bottom: 1px solid var(--rumi-border);
    overflow-x: auto;
    -webkit-overflow-scrolling: touch;
    animation: fadeSlideIn 0.5s ease-out both;
  }

  @keyframes fadeSlideIn {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .earn-subnav-link {
    position: relative;
    flex-shrink: 0;
    padding: 0.625rem 0.125rem;
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    text-decoration: none;
    white-space: nowrap;
    transition: color 0.15s ease;
  }

  .earn-subnav-link:hover { color: var(--rumi-text-secondary); }
  .earn-subnav-link.active { color: var(--rumi-text-primary); }

  .earn-subnav-link.active::after {
    content: '';
    position: absolute;
    bottom: -1px;
    left: 0;
    right: 0;
    height: 2px;
    background: var(--rumi-action);
    border-radius: 1px 1px 0 0;
  }

  .label-short { display: none; }

  @media (max-width: 520px) {
    .earn-subnav { gap: 1rem; }
    .label-full { display: none; }
    .label-short { display: inline; }
  }
</style>
