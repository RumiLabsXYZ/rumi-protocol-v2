<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { POINTS_ENABLED } from '$lib/config';
  import { isConnected, principal } from '$lib/stores/wallet';
  import { myPointsStore } from '$lib/stores/pointsStore';
  import { getEpochStatus, getPointsConfig, getLeaderboard } from '$lib/services/pointsService';
  import { bodyState, deriveRank } from '$lib/utils/points';
  import type { PublicEpochStatus, PointsConfig } from '$declarations/rumi_points/rumi_points.did';
  import SeasonBanner from '$lib/components/points/SeasonBanner.svelte';
  import EarnCta from '$lib/components/points/EarnCta.svelte';
  import PointsSummary from '$lib/components/points/PointsSummary.svelte';

  let status = $state<PublicEpochStatus | null>(null);
  let config = $state<PointsConfig | null>(null);
  let rank = $state<number | null>(null);

  onMount(async () => {
    // Route gate: the section is hidden in nav until the canister is configured;
    // also block direct-URL access so we never call an unconfigured canister.
    if (!POINTS_ENABLED) {
      goto('/');
      return;
    }
    try {
      [status, config] = await Promise.all([getEpochStatus(), getPointsConfig()]);
    } catch (e) {
      // Non-fatal: the banner degrades to its loading/unknown state.
      console.error('[points] season status load failed', e);
    }
  });

  // Load / reset the connected wallet's points as the principal changes.
  $effect(() => {
    if (!POINTS_ENABLED) return;
    const p = $principal;
    if ($isConnected && p) {
      myPointsStore.load(p);
      // Best-effort rank from the top slice (no get_my_rank endpoint exists).
      getLeaderboard(0, 1000)
        .then((rows) => {
          rank = deriveRank(rows, p.toText());
        })
        .catch(() => {
          rank = null;
        });
    } else {
      myPointsStore.reset();
      rank = null;
    }
  });

  function retry() {
    const p = $principal;
    if (p) myPointsStore.load(p);
  }

  const body = $derived(
    bodyState({
      connected: $isConnected,
      excluded: $myPointsStore.excluded,
      state: $myPointsStore.state,
    }),
  );
</script>

<svelte:head><title>Points · Rumi</title></svelte:head>

<div class="max-w-3xl mx-auto px-4 py-6 flex flex-col gap-4">
  <h1 class="text-xl font-semibold text-gray-100">Airdrop Points</h1>

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
    <EarnCta />
  {:else if body === 'excluded'}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 text-sm text-gray-300">
      This address is excluded from the airdrop (protocol-owned).
    </div>
  {:else if body === 'enrolled' && $myPointsStore.state}
    <PointsSummary state={$myPointsStore.state} {rank} />
    <EarnCta heading="Earn more" />
  {:else}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 text-sm text-gray-300">
      You're not earning points yet. Take a qualifying action to enroll automatically.
    </div>
    <EarnCta />
  {/if}
</div>
