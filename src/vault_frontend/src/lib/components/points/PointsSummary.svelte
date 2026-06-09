<script lang="ts">
  import type { PrincipalState, Venue } from '$declarations/rumi_points/rumi_points.did';
  import { formatPoints, qualifyingActionLabel } from '$lib/utils/points';

  interface Props {
    state: PrincipalState;
    rank: number | null;
  }
  let { state, rank }: Props = $props();

  function venueLabel(v: Venue): string {
    if ('Vault' in v) return 'Vault debt';
    if ('StabilityPool' in v) return 'Stability pool';
    if ('ThreePool' in v) return '3pool liquidity';
    if ('Amm' in v) return 'AMM liquidity';
    return 'Position';
  }
  // Distinct venues currently earning, for a short breakdown.
  const venues = $derived(
    Array.from(new Set(state.active_deposits.map(([k]) => venueLabel(k.venue)))),
  );
  const enrolledDate = $derived(
    new Date(Number(state.registered_at_ns / 1_000_000n)).toLocaleDateString(),
  );
</script>

<div class="flex flex-col gap-4">
  <div class="grid grid-cols-2 gap-3">
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <p class="text-xs text-gray-400 uppercase tracking-wider">Your points</p>
      <p class="text-2xl font-semibold text-gray-100 mt-1">{formatPoints(state.total_points)}</p>
      <p class="text-xs text-gray-500">USD-days</p>
    </div>
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <p class="text-xs text-gray-400 uppercase tracking-wider">Rank</p>
      <p class="text-2xl font-semibold text-gray-100 mt-1">{rank !== null ? `#${rank}` : '—'}</p>
      <p class="text-xs text-gray-500">{rank !== null ? 'on the leaderboard' : 'outside the top ranks'}</p>
    </div>
  </div>

  <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 text-sm text-gray-300">
    <p>Enrolled {enrolledDate} · first action: {qualifyingActionLabel(state.first_qualifying_action)}</p>
    {#if venues.length > 0}
      <p class="text-xs text-gray-500 mt-2">Currently earning from: {venues.join(', ')}</p>
    {/if}
  </div>
</div>
