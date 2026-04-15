<!-- src/vault_frontend/src/lib/components/layout/PositionStrip.svelte -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { userVaults, isLoadingVaults } from '$lib/stores/appDataStore';
  import { isConnected } from '$lib/stores/wallet';
  import { permissionStore } from '$lib/stores/permissionStore';
  import { collateralStore } from '$lib/stores/collateralStore';
  import { isDevelopment } from '$lib/config';
  import { developerAccess } from '$lib/stores/developer';
  import { aggregatePosition } from './positionSummary';

  const STORAGE_KEY = 'rumi:positionStrip:expanded';

  let expanded = false;

  onMount(() => {
    try {
      expanded = localStorage.getItem(STORAGE_KEY) === '1';
    } catch {
      // localStorage may be blocked; fall back to collapsed.
      expanded = false;
    }
    return () => {
      // Reset the CSS variable when the component unmounts.
      if (typeof document !== 'undefined') {
        document.documentElement.style.setProperty('--rumi-strip-height', '0px');
      }
    };
  });

  function toggle() {
    expanded = !expanded;
    try {
      localStorage.setItem(STORAGE_KEY, expanded ? '1' : '0');
    } catch {
      // Ignore storage errors.
    }
  }

  // Gate visibility by connection + view permission (same rules as /vaults).
  $: canView = isDevelopment || $developerAccess || $isConnected
    || ($permissionStore.initialized && $permissionStore.canViewVaults);

  $: summary = aggregatePosition($userVaults, $collateralStore.collaterals);
  $: hasPosition = $userVaults.length > 0;
  $: showSkeleton = $isConnected && $isLoadingVaults && $userVaults.length === 0;

  // Expose the rendered height as a CSS variable on <html> so main-content
  // padding can compensate. Bound to the outer wrapper so it stays 0 when
  // the component renders nothing (e.g., disconnected).
  let stripHeight = 0;
  $: if (typeof document !== 'undefined') {
    document.documentElement.style.setProperty('--rumi-strip-height', `${stripHeight}px`);
  }

  // Formatters
  const fmtUsd = (n: number) => n.toLocaleString('en-US', { style: 'currency', currency: 'USD', maximumFractionDigits: 2 });
  const fmtIcusd = (n: number) => n.toLocaleString('en-US', { maximumFractionDigits: 2 });
  const fmtAmount = (n: number, decimals: number) => n.toLocaleString('en-US', { maximumFractionDigits: decimals });
  const fmtCr = (cr: number) => Number.isFinite(cr) ? `${Math.round(cr * 100)}%` : '—';

  // Decimals for breakdown display: BTC-like assets deserve more precision.
  function displayDecimals(symbol: string): number {
    if (/btc/i.test(symbol)) return 4;
    if (/eth/i.test(symbol)) return 4;
    if (/xaut/i.test(symbol)) return 3;
    return 2;
  }

  function healthLabel(tier: string): string {
    switch (tier) {
      case 'safe': return 'Healthy';
      case 'caution': return 'Caution';
      case 'danger': return 'At risk';
      case 'no-debt': return 'No debt';
      default: return '';
    }
  }
</script>

<!-- Outer wrapper is always rendered (so stripHeight stays bound) but is
     empty when there's nothing to show. position:fixed places it flush
     below the fixed top bar (height 3.5rem). -->
<div class="strip-outer" bind:clientHeight={stripHeight}>
  {#if !$isConnected || !canView}
    <!-- Disconnected or no permission -> render nothing (height stays 0). -->
  {:else if showSkeleton}
    <div class="strip skeleton" aria-hidden="true">
      <div class="sk-cell"></div>
      <div class="sk-div"></div>
      <div class="sk-cell"></div>
      <div class="sk-div"></div>
      <div class="sk-cell"></div>
    </div>
  {:else if !hasPosition}
    <div class="strip cta">
      <span class="cta-headline">No active position.</span>
      <span class="cta-sub">Lock ICP, ckBTC, or ckXAUT as collateral to mint icUSD.</span>
      <a class="cta-link" href="/">Open your first vault →</a>
    </div>
  {:else}
    <section class="strip-wrapper" class:is-expanded={expanded}>
      <button
        type="button"
        class="strip interactive"
        class:is-expanded={expanded}
        aria-expanded={expanded}
        aria-controls="position-breakdown"
        on:click={toggle}
      >
        <span class="cell">
          <span class="cell-label">Collateral</span>
          <span class="cell-val">{summary.hasAnyMissingPrice ? '≈' : ''}{fmtUsd(summary.totalCollateralUsd)}</span>
        </span>
        <span class="divider" aria-hidden="true"></span>
        <span class="cell">
          <span class="cell-label">Borrowed</span>
          <span class="cell-val">{fmtIcusd(summary.totalBorrowed)}<span class="cell-unit"> icUSD</span></span>
        </span>
        <span class="divider" aria-hidden="true"></span>
        <span class="cell">
          <span class="cell-label">Overall CR</span>
          <span class="cell-val health-{summary.healthTier}">{fmtCr(summary.overallCr)}</span>
        </span>
        {#if summary.healthTier !== 'unknown'}
          <span class="health-label health-{summary.healthTier}">{healthLabel(summary.healthTier)}</span>
        {/if}
        <span class="caret" aria-hidden="true">{expanded ? 'Hide ▴' : 'Show breakdown ▾'}</span>
      </button>

      {#if expanded}
        <div class="breakdown" id="position-breakdown">
          {#each summary.perCollateral as asset (asset.principal)}
            <div class="pill">
              <span class="pill-symbol">{asset.symbol}</span>
              <span class="pill-amount">{fmtAmount(asset.nativeAmount, displayDecimals(asset.symbol))}</span>
              {#if asset.hasPrice}
                <span class="pill-usd">{fmtUsd(asset.usdValue)}</span>
              {:else}
                <span class="pill-usd pill-nopx">no price</span>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </section>
  {/if}
</div>

<style>
  /* The top bar is position:fixed; height 3.5rem; z-index 100. We place
     the strip as position:fixed right below it at top:3.5rem. Its height
     is bound reactively to the --rumi-strip-height CSS variable, and
     .main-content's padding-top compensates via calc() (see +layout.svelte). */
  .strip-outer {
    position: fixed;
    top: 3.5rem;
    left: 0;
    right: 0;
    z-index: 99;  /* below top-bar's 100, above page content */
  }
  /* Only draw border/background when something is actually rendered.
     :not(:empty) keeps the outer invisible when the disconnected branch
     renders no DOM. */
  .strip-outer:not(:empty) {
    background: var(--rumi-bg-surface1);
    border-bottom: 1px solid var(--rumi-border);
  }
  .strip-wrapper {
    background: linear-gradient(180deg, rgba(167,139,250,0.05), rgba(52,211,153,0.02));
  }
  .strip {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 1.25rem;
    padding: 0.5rem 1.5rem;
    font-size: 0.8125rem;
    min-height: 2.25rem;
    box-sizing: border-box;
  }
  .strip.interactive {
    width: 100%;
    background: none;
    border: none;
    color: inherit;
    cursor: pointer;
    font: inherit;
    text-align: left;
  }
  .strip.interactive:hover { background: rgba(167,139,250,0.04); }
  .strip.interactive:focus-visible {
    outline: 2px solid var(--rumi-action);
    outline-offset: -2px;
  }

  .cell { display: inline-flex; align-items: baseline; gap: 0.5rem; }
  .cell-label {
    color: var(--rumi-text-muted);
    font-size: 0.6875rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .cell-val {
    color: var(--rumi-text-primary);
    font-weight: 600;
    font-size: 0.9375rem;
  }
  .cell-unit { color: var(--rumi-text-muted); font-weight: 500; font-size: 0.75rem; }

  .divider { width: 1px; height: 1.125rem; background: var(--rumi-border); }

  .health-safe { color: var(--rumi-safe); }
  .health-caution { color: #d9a53c; }
  .health-danger { color: var(--rumi-danger); }
  .health-no-debt { color: var(--rumi-text-secondary); }
  .health-unknown { color: var(--rumi-text-muted); }

  .health-label {
    font-size: 0.6875rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    padding: 0.125rem 0.4375rem;
    border-radius: 999px;
    background: rgba(255,255,255,0.04);
  }

  .caret {
    color: var(--rumi-text-muted);
    font-size: 0.75rem;
  }

  /* Breakdown row */
  .breakdown {
    padding: 0 1.5rem 0.625rem;
    justify-content: center;
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
  .pill {
    display: inline-flex;
    align-items: baseline;
    gap: 0.375rem;
    padding: 0.25rem 0.625rem;
    background: rgba(20,26,46,0.6);
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    font-size: 0.75rem;
  }
  .pill-symbol { color: var(--rumi-text-secondary); font-weight: 500; }
  .pill-amount { color: var(--rumi-text-primary); font-weight: 600; }
  .pill-usd { color: var(--rumi-text-muted); font-size: 0.6875rem; }
  .pill-nopx { font-style: italic; }

  /* CTA variant */
  .strip.cta {
    gap: 0.625rem;
    color: var(--rumi-text-secondary);
  }
  .cta-headline { color: var(--rumi-text-primary); font-weight: 600; }
  .cta-sub { color: var(--rumi-text-muted); font-size: 0.75rem; }
  .cta-link { color: var(--rumi-action); text-decoration: none; font-weight: 500; }
  .cta-link:hover { text-decoration: underline; }

  /* Skeleton */
  .strip.skeleton { pointer-events: none; }
  .sk-cell {
    width: 7rem; height: 0.9375rem; border-radius: 4px;
    background: linear-gradient(90deg, rgba(255,255,255,0.04), rgba(255,255,255,0.08), rgba(255,255,255,0.04));
    background-size: 200% 100%;
    animation: sk-shimmer 1.2s ease-in-out infinite;
  }
  .sk-div { width: 1px; height: 1.125rem; background: var(--rumi-border); }
  @keyframes sk-shimmer { 0% { background-position: 200% 0; } 100% { background-position: -200% 0; } }

  /* Mobile */
  @media (max-width: 768px) {
    .strip { padding: 0.375rem 0.75rem; gap: 0.75rem; font-size: 0.75rem; }
    .divider { display: none; }
    .cell-label { font-size: 0.625rem; }
    .cell-val { font-size: 0.8125rem; }
    .cell-unit { display: none; } /* "Borrowed 1,500" without " icUSD" suffix on mobile */
    .caret { font-size: 0.6875rem; }
    .health-label { display: none; } /* color alone carries meaning on mobile */
    .breakdown { padding: 0 0.75rem 0.5rem; gap: 0.375rem; }
    .pill { padding: 0.1875rem 0.5rem; font-size: 0.6875rem; }
    .strip.cta .cta-sub { display: none; } /* keep CTA compact */
  }
</style>
