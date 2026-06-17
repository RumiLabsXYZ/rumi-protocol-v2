<script lang="ts">
  import type {
    PrincipalState,
    Venue,
    AssetType,
  } from '$declarations/rumi_points/rumi_points.did';
  import { formatPoints, qualifyingActionLabel } from '$lib/utils/points';
  import { depositMultiplierLabel, formatSharePct, type EarnVenue } from '$lib/utils/pointsRules';
  import MultiplierBadge from './MultiplierBadge.svelte';

  interface Props {
    state: PrincipalState;
    rank: number | null;
    shareBps?: number | null;
  }
  let { state, rank, shareBps = null }: Props = $props();

  function venueKey(v: Venue): EarnVenue {
    if ('Vault' in v) return 'vault';
    if ('StabilityPool' in v) return 'stabilityPool';
    if ('ThreePool' in v) return 'threePool';
    return 'amm';
  }
  function venueLabel(v: EarnVenue): string {
    return v === 'vault' ? 'Vault debt'
      : v === 'stabilityPool' ? 'Stability pool'
      : v === 'threePool' ? '3pool liquidity'
      : 'AMM liquidity';
  }
  function assetSymbol(a: AssetType): string {
    if ('IcUsd' in a) return 'icUSD';
    if ('CkUsdc' in a) return 'ckUSDC';
    if ('CkUsdt' in a) return 'ckUSDT';
    if ('ThreeUsd' in a) return '3USD';
    if ('Icp' in a) return 'ICP';
    return '';
  }
  // recorded_value_usd is usd_e8s (USD * 1e8). Reduce via bigint to stay exact.
  function usd(raw: bigint): string {
    const cents = raw / 1_000_000n; // 1e8 / 1e6 = 100 → hundredths of a dollar
    return `$${(Number(cents) / 100).toLocaleString('en-US', { maximumFractionDigits: 2 })}`;
  }

  const positions = $derived(
    state.active_deposits.map(([key, rec]) => {
      const v = venueKey(key.venue);
      const sym = assetSymbol(key.asset);
      return {
        venue: venueLabel(v),
        symbol: sym,
        mult: depositMultiplierLabel(v, sym),
        value: usd(rec.recorded_value_usd),
      };
    }),
  );
  const enrolledDate = $derived(
    new Date(Number(state.registered_at_ns / 1_000_000n)).toLocaleDateString(),
  );
</script>

<div class="flex flex-col gap-4">
  <div class="grid grid-cols-3 gap-3">
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <p class="text-xs text-gray-400 uppercase tracking-wider">Your points</p>
      <p class="text-2xl font-semibold text-gray-100 mt-1 tabular-nums">{formatPoints(state.total_points)}</p>
      <p class="text-xs text-gray-500">USD-days</p>
    </div>
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <p class="text-xs text-gray-400 uppercase tracking-wider">Rank</p>
      <p class="text-2xl font-semibold text-gray-100 mt-1 tabular-nums">{rank !== null ? `#${rank}` : '—'}</p>
      <p class="text-xs text-gray-500">{rank !== null ? 'on the leaderboard' : 'outside the top ranks'}</p>
    </div>
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <p class="text-xs text-gray-400 uppercase tracking-wider">Est. share</p>
      <p class="text-2xl font-semibold text-emerald-300 mt-1 tabular-nums">{shareBps !== null ? formatSharePct(shareBps) : '—'}</p>
      <p class="text-xs text-gray-500">of the Season 1 pool</p>
    </div>
  </div>

  {#if positions.length > 0}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <p class="text-sm font-medium text-gray-200 mb-3">Where you're earning</p>
      <ul class="flex flex-col gap-2">
        {#each positions as p}
          <li class="flex items-center justify-between gap-3 rounded-lg border border-gray-700/40 bg-gray-900/30 px-3 py-2">
            <span class="min-w-0">
              <span class="block text-sm text-gray-100">{p.venue}</span>
              <span class="block text-xs text-gray-500">{p.symbol} · {p.value}</span>
            </span>
            <MultiplierBadge multiplier={0} label={p.mult} />
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 text-sm text-gray-300">
    <p>Enrolled {enrolledDate} · first action: {qualifyingActionLabel(state.first_qualifying_action)}</p>
  </div>
</div>
