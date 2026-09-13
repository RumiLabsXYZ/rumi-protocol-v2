<script lang="ts">
  import { onMount } from 'svelte';
  import {
    loadThreeUsdRate,
    loadSpRate,
    withTimeout,
    EARN_RATE_TIMEOUT_MS,
    UNAVAILABLE_RATE,
    type RateState,
  } from '../../services/earnRates';

  // `null` while `/earn` hasn't picked a destination yet (its brief transient
  // view). Real links either way, not fake `role="tab"` buttons, so native
  // keyboard/tab behavior, Back, and bookmarks all keep working.
  export let active: '3usd' | 'stability-pool' | null;

  let threeUsdRate: RateState = { status: 'loading', pct: null };
  let spRate: RateState = { status: 'loading', pct: null };

  onMount(() => {
    let cancelled = false;
    // Bounded, same as `/earn`'s redirect decision: a stalled query settles
    // into "unavailable" rather than leaving a badge stuck on "Loading…"
    // forever, on direct pool-route visits too.
    withTimeout(loadThreeUsdRate(), EARN_RATE_TIMEOUT_MS, () => UNAVAILABLE_RATE).then(
      (r) => { if (!cancelled) threeUsdRate = r; },
    );
    withTimeout(loadSpRate(), EARN_RATE_TIMEOUT_MS, () => UNAVAILABLE_RATE).then(
      (r) => { if (!cancelled) spRate = r; },
    );
    return () => { cancelled = true; };
  });
</script>

<nav class="earn-tabbar" aria-label="Earn sections">
  <a
    href="/3usd"
    class="earn-tab earn-tab-3usd"
    class:selected={active === '3usd'}
    aria-current={active === '3usd' ? 'page' : undefined}
  >
    <span class="tab-label">3USD</span>
    {#if threeUsdRate.status === 'loading'}
      <span class="tab-badge tab-badge-loading">Loading…</span>
    {:else if threeUsdRate.status === 'ready'}
      <span class="tab-badge">{threeUsdRate.pct.toFixed(2)}% APY</span>
    {:else}
      <span class="tab-badge tab-badge-unavailable">Rate unavailable</span>
    {/if}
  </a>
  <a
    href="/stability-pool"
    class="earn-tab earn-tab-sp"
    class:selected={active === 'stability-pool'}
    aria-current={active === 'stability-pool' ? 'page' : undefined}
  >
    <span class="tab-label">Stability Pool</span>
    {#if spRate.status === 'loading'}
      <span class="tab-badge tab-badge-loading">Loading…</span>
    {:else if spRate.status === 'ready'}
      <span class="tab-badge">{spRate.pct.toFixed(2)}% APY</span>
    {:else}
      <span class="tab-badge tab-badge-unavailable">Rate unavailable</span>
    {/if}
  </a>
</nav>

<style>
  .earn-tabbar {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    margin-bottom: 1.5rem;
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    overflow: hidden;
    animation: fadeSlideIn 0.5s ease-out both;
  }

  @keyframes fadeSlideIn {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .earn-tab {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.625rem;
    min-width: 0;
    min-height: 76px;
    padding: 0.75rem 1rem;
    background: var(--rumi-bg-surface1);
    text-decoration: none;
    text-align: center;
    transition: background 0.15s ease;
  }

  .earn-tab-3usd { border-right: 1px solid var(--rumi-border); }

  .earn-tab:hover { background: var(--rumi-bg-surface2); }

  .earn-tab:focus-visible {
    outline: 2px solid var(--rumi-teal);
    outline-offset: -2px;
    z-index: 1;
  }

  .earn-tab.selected {
    background: var(--rumi-bg-surface2);
  }

  .earn-tab.selected::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 3px;
    background: var(--rumi-teal);
  }

  .tab-label {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.25rem;
    font-weight: 600;
    color: var(--rumi-text-secondary);
    white-space: nowrap;
  }

  .earn-tab-sp.selected .tab-label { color: var(--rumi-text-primary); }
  /* 3USD keeps its purple branding, and a larger label, whether or not it's the selected tab. */
  .earn-tab-3usd .tab-label { color: var(--rumi-purple-accent); font-size: 1.5rem; }

  .tab-badge {
    display: inline-flex;
    align-items: center;
    padding: 0.25rem 0.6875rem;
    background: rgba(74, 222, 128, 0.1);
    border: 1px solid rgba(74, 222, 128, 0.3);
    border-radius: 1.25rem;
    font-size: 0.75rem;
    font-weight: 600;
    color: #4ade80;
    white-space: nowrap;
  }

  .tab-badge.tab-badge-loading,
  .tab-badge.tab-badge-unavailable {
    background: var(--rumi-bg-surface3);
    border-color: var(--rumi-border);
    color: var(--rumi-text-muted);
  }

  /* Two tabs must still fit down to 320px: stack label above badge instead of
     shrinking either past legibility. 600px (not just the 320-390px phone
     range) so a wide label plus badge side by side never gets clipped. */
  @media (max-width: 600px) {
    .earn-tab {
      flex-direction: column;
      gap: 0.375rem;
      padding: 0.625rem 0.375rem;
    }

    .tab-label,
    .earn-tab-3usd .tab-label { font-size: 0.9375rem; }

    .tab-badge { font-size: 0.6875rem; padding: 0.1875rem 0.5rem; }
  }
</style>
