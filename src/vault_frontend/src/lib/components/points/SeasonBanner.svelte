<script lang="ts">
  import type { PublicEpochStatus, PointsConfig } from '$declarations/rumi_points/rumi_points.did';
  import { seasonState } from '$lib/utils/points';

  interface Props {
    status: PublicEpochStatus | null;
    config: PointsConfig | null;
  }
  let { status, config }: Props = $props();

  // ms remaining until season end, for a coarse day countdown.
  const phase = $derived(seasonState(status, config, BigInt(Date.now()) * 1_000_000n));
  const daysLeft = $derived.by(() => {
    if (!config) return null;
    const endMs = Number(config.season_end_ns / 1_000_000n);
    const d = Math.ceil((endMs - Date.now()) / 86_400_000);
    return d > 0 ? d : 0;
  });
  const epochIndex = $derived(status ? Number(status.current_epoch_index) : 0);
</script>

<div class="rounded-xl bg-gray-800/30 border border-gray-700/50 px-4 py-3 flex items-center justify-between gap-3">
  {#if phase === 'live'}
    <div>
      <p class="text-sm font-medium text-teal-400">Season 1 is live</p>
      <p class="text-xs text-gray-400">Epoch {epochIndex}{#if daysLeft !== null} · {daysLeft} day{daysLeft === 1 ? '' : 's'} left{/if}</p>
    </div>
  {:else if phase === 'pre'}
    <div>
      <p class="text-sm font-medium text-gray-200">Season 1 starts soon</p>
      <p class="text-xs text-gray-400">Start earning now — your positions are counted once the season opens.</p>
    </div>
  {:else if phase === 'ended'}
    <div>
      <p class="text-sm font-medium text-gray-200">Season 1 has ended</p>
      <p class="text-xs text-gray-400">Allocations are being finalized. Claiming is coming soon.</p>
    </div>
  {:else}
    <div>
      <p class="text-sm font-medium text-gray-300">Airdrop points</p>
      <p class="text-xs text-gray-500">Loading season status…</p>
    </div>
  {/if}
</div>
