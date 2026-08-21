<script lang="ts">
  import MultiplierBadge from './MultiplierBadge.svelte';
  import { AMM1_LIQUIDITY_PAUSED } from '$lib/config';
  import type { EarnVenue } from '$lib/utils/pointsRules';

  interface Props {
    heading?: string;
    /** Venues the viewer is already earning in — marked instead of pitched. */
    activeVenues?: ReadonlySet<EarnVenue>;
  }
  let { heading = 'Ways to earn points', activeVenues = new Set<EarnVenue>() }: Props = $props();

  interface Action {
    venue: EarnVenue;
    label: string;
    desc: string;
    href: string;
    mult: number;
  }

  // Curated, sorted high → low. Each venue shows its best available multiplier.
  // The AMM row is dropped while AMM1 deposits are paused — never advertise an
  // action the app itself blocks.
  const actions: Action[] = [
    { venue: 'threePool', label: 'Provide 3pool liquidity', desc: 'Pair ckUSDC + ckUSDT for the highest boost; icUSD earns 1×.', href: '/3usd', mult: 5 },
    { venue: 'stabilityPool', label: 'Deposit to the stability pool', desc: 'Backstop liquidations — 3USD earns 2×, icUSD 1×.', href: '/stability-pool', mult: 2 },
    ...(AMM1_LIQUIDITY_PAUSED
      ? []
      : [{ venue: 'amm' as EarnVenue, label: 'Add 3USD/ICP to the AMM', desc: 'Provide liquidity to the Rumi AMM.', href: '/swap', mult: 2 }]),
    { venue: 'vault', label: 'Mint icUSD', desc: 'Borrow icUSD against your vault collateral.', href: '/', mult: 1 },
  ];
</script>

<div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
  <p class="text-sm font-medium text-gray-200 mb-3">{heading}</p>
  <ul class="flex flex-col gap-2">
    {#each actions as a (a.venue)}
      <li>
        <a
          href={a.href}
          class="flex items-center justify-between gap-3 rounded-lg border border-gray-700/40 bg-gray-900/30 px-3 py-2 hover:border-emerald-500/40 transition-colors"
        >
          <span class="min-w-0">
            <span class="flex items-center gap-2 text-sm text-gray-100">
              {a.label}
              {#if activeVenues.has(a.venue)}
                <span class="text-[10px] uppercase tracking-wider text-emerald-400/90 border border-emerald-400/30 rounded-full px-1.5 py-px">active</span>
              {/if}
            </span>
            <span class="block text-xs text-gray-500">{a.desc}</span>
          </span>
          <MultiplierBadge multiplier={a.mult} size="md" />
        </a>
      </li>
    {/each}
  </ul>
</div>
