<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';
  import { Principal } from '@dfinity/principal';
  import { POINTS_ENABLED } from '$lib/config';
  import { isConnected, principal } from '$lib/stores/wallet';
  import { myPointsStore } from '$lib/stores/pointsStore';
  import { getEpochStatus, getPointsConfig, getLeaderboard } from '$lib/services/pointsService';
  import { bodyState, seasonState } from '$lib/utils/points';
  import { truncatePrincipal } from '$lib/utils/principalHelpers';
  import type { PublicEpochStatus, PointsConfig } from '$declarations/rumi_points/rumi_points.did';
  import type { EarnVenue } from '$lib/utils/pointsRules';
  import SeasonBanner from '$lib/components/points/SeasonBanner.svelte';
  import EarnCta from '$lib/components/points/EarnCta.svelte';
  import PointsSummary from '$lib/components/points/PointsSummary.svelte';

  let status = $state<PublicEpochStatus | null>(null);
  let config = $state<PointsConfig | null>(null);
  let rank = $state<number | null>(null);
  // Guards the async rank fetch: a stale response for a previous principal
  // must not land on the current view (same pattern as pointsStore's seq).
  let rankSeq = 0;

  /**
   * Read-only support view: /points?view=<principal> renders that principal's
   * dashboard without a wallet. Everything on this page is a public canister
   * query, so this exposes nothing — it lets an operator see exactly what a
   * user sees when they report a problem.
   */
  const viewOverride = $derived.by(() => {
    const raw = $page.url.searchParams.get('view');
    if (!raw) return null;
    try {
      return Principal.fromText(raw);
    } catch {
      console.warn('[points] invalid ?view principal ignored:', raw);
      return null;
    }
  });

  const viewedPrincipal = $derived(viewOverride ?? ($isConnected ? $principal : null));

  async function loadSeason() {
    try {
      [status, config] = await Promise.all([getEpochStatus(), getPointsConfig()]);
    } catch (e) {
      // Non-fatal: the banner degrades to its loading/unknown state. The service
      // retries with backoff; Retry re-runs this so a transient blip recovers.
      console.error('[points] season status load failed', e);
    }
  }

  onMount(() => {
    // Route gate: the section is hidden in nav until the canister is configured;
    // also block direct-URL access so we never call an unconfigured canister.
    if (!POINTS_ENABLED) {
      goto('/');
      return;
    }
    loadSeason();
  });

  // Load / reset the viewed wallet's points as the principal (or ?view) changes.
  $effect(() => {
    if (!POINTS_ENABLED) return;
    const p = viewedPrincipal;
    if (p) {
      myPointsStore.load(p);
      // Best-effort rank from the top slice (no get_my_rank endpoint). Share of
      // pool is deliberately NOT surfaced to users; see PointsSummary.
      const mySeq = ++rankSeq;
      getLeaderboard(0, 1000)
        .then((rows) => {
          if (mySeq !== rankSeq) return;
          const me = rows.find((e) => e.principal.toText() === p.toText());
          rank = me ? me.rank : null;
        })
        .catch(() => {
          if (mySeq === rankSeq) rank = null;
        });
    } else {
      rankSeq++;
      myPointsStore.reset();
      rank = null;
    }
  });

  function retry() {
    // Recover the season banner, the personal points, and the detail sections.
    if (!status || !config) loadSeason();
    const p = viewedPrincipal;
    if (p) myPointsStore.load(p);
  }

  const body = $derived(
    bodyState({
      connected: viewOverride !== null || $isConnected,
      excluded: $myPointsStore.excluded,
      state: $myPointsStore.state,
    }),
  );

  /** After season end, stop pitching actions that no longer earn anything. */
  const seasonEnded = $derived(
    seasonState(status, config, BigInt(Date.now()) * 1_000_000n) === 'ended',
  );

  /** Venues with a live earning position — marked "active" in Earn more. */
  const activeVenues = $derived.by(() => {
    const set = new Set<EarnVenue>();
    for (const r of $myPointsStore.detail.live?.rows ?? []) set.add(r.meta.venue);
    return set;
  });
</script>

<svelte:head><title>Points · Rumi</title></svelte:head>

<div class="max-w-3xl mx-auto px-4 py-6 flex flex-col gap-4">
  <h1 class="text-xl font-semibold text-gray-100">Airdrop Points</h1>

  {#if viewOverride}
    <div class="rounded-xl bg-sky-400/10 border border-sky-400/30 px-4 py-2.5 text-xs text-sky-200">
      Read-only view of <span class="font-mono">{truncatePrincipal(viewOverride.toText())}</span> — public
      data only. <a href="/points" class="underline">Back to my points</a>
    </div>
  {/if}

  <SeasonBanner {status} {config} />

  {#if $myPointsStore.loading}
    <div class="flex justify-center py-12">
      <div class="w-7 h-7 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if $myPointsStore.error}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 flex items-center justify-between gap-3">
      <span class="text-sm text-gray-300">Couldn't load your points.</span>
      <button class="px-3 py-1.5 rounded-lg text-sm border border-gray-700/50 text-teal-400 hover:border-teal-500/40" onclick={retry}>
        Retry
      </button>
    </div>
  {:else if body === 'disconnected'}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 text-sm text-gray-300">
      Connect your wallet to see your points. Points accrue automatically when you use the protocol.
    </div>
    {#if !seasonEnded}<EarnCta />{/if}
  {:else if body === 'excluded'}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 text-sm text-gray-300">
      This address is excluded from the airdrop (protocol-owned).
    </div>
  {:else if body === 'enrolled' && $myPointsStore.state}
    <PointsSummary
      state={$myPointsStore.state}
      {rank}
      detail={$myPointsStore.detail}
      {status}
      {config}
    />
    {#if !seasonEnded}<EarnCta heading="Earn more" {activeVenues} />{/if}
  {:else}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 text-sm text-gray-300">
      {viewOverride
        ? "This wallet isn't earning points yet — it has taken no qualifying action."
        : "You're not earning points yet. Take a qualifying action to enroll automatically."}
    </div>
    {#if !seasonEnded}<EarnCta />{/if}
  {/if}
</div>
