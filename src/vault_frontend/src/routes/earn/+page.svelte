<script lang="ts">
  import { onMount } from 'svelte';
  import { beforeNavigate } from '$app/navigation';
  import EarnSubNav from '../../lib/components/layout/EarnSubNav.svelte';
  import LoadingSpinner from '../../lib/components/common/LoadingSpinner.svelte';
  import {
    freshEarnSnapshot,
    withTimeout,
    resolveEarnDestination,
    EARN_RATE_TIMEOUT_MS,
    UNAVAILABLE_RATE,
  } from '../../lib/services/earnRates';
  import { replaceRoute } from '../../lib/utils/earnNavigation';

  // A fresh `/earn` visit has no opinion of its own: it lands on whichever
  // pool currently has the higher live APY, then replaces itself in history
  // so Back never bounces through this transient page. Bounded per-rate wait
  // means a stalled canister query can't hang the redirect. A manual click on
  // either tab below fires `beforeNavigate` immediately (before the old page
  // unmounts) and also eventually unmounts this page; either guard alone is
  // enough to make a late-arriving result never override that choice.
  let cancelled = false;

  beforeNavigate(() => { cancelled = true; });

  // Reset the shared rate cache and kick off both loads synchronously during
  // this component's own init, i.e. before the `EarnSubNav` below mounts, so
  // that child reuses these same in-flight promises instead of duplicating
  // the query.
  const { loadThreeUsdRate, loadSpRate } = freshEarnSnapshot();

  onMount(() => {
    (async () => {
      const [threeUsd, sp] = await Promise.all([
        withTimeout(loadThreeUsdRate(), EARN_RATE_TIMEOUT_MS, () => UNAVAILABLE_RATE),
        withTimeout(loadSpRate(), EARN_RATE_TIMEOUT_MS, () => UNAVAILABLE_RATE),
      ]);
      if (cancelled) return;
      replaceRoute(resolveEarnDestination(threeUsd, sp));
    })();

    return () => { cancelled = true; };
  });
</script>

<svelte:head>
  <title>Earn | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <EarnSubNav active={null} />

  <div class="loading-state">
    <LoadingSpinner />
    <p class="loading-text">Finding the best current rate…</p>
  </div>
</div>

<style>
  .page-container { max-width: 820px; margin: 0 auto; padding-bottom: 4rem; }

  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 4rem 1rem;
    color: var(--rumi-text-secondary);
  }

  .loading-text { margin-top: 1rem; font-size: 0.875rem; }

  @media (max-width: 520px) {
    .page-container { padding-left: 0.5rem; padding-right: 0.5rem; }
  }
</style>
