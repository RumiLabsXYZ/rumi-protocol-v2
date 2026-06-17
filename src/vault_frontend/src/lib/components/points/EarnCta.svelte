<script lang="ts">
  import MultiplierBadge from './MultiplierBadge.svelte';

  interface Props {
    heading?: string;
  }
  let { heading = 'Ways to earn points' }: Props = $props();

  // Curated, sorted high → low. Each venue shows its best available multiplier.
  const actions = [
    { label: 'Provide 3pool liquidity', desc: 'Pair ckUSDC + ckUSDT for the highest boost.', href: '/3usd', mult: 5 },
    { label: 'Deposit 3USD to the stability pool', desc: 'Backstop liquidations and earn double.', href: '/stability-pool', mult: 2 },
    { label: 'Add 3USD/ICP to the AMM', desc: 'Provide liquidity to the Rumi AMM.', href: '/swap', mult: 2 },
    { label: 'Mint icUSD', desc: 'Borrow against ICP, ckBTC, or ckXAUT collateral.', href: '/', mult: 1 },
  ];
</script>

<div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
  <p class="text-sm font-medium text-gray-200 mb-3">{heading}</p>
  <ul class="flex flex-col gap-2">
    {#each actions as a}
      <li>
        <a
          href={a.href}
          class="flex items-center justify-between gap-3 rounded-lg border border-gray-700/40 bg-gray-900/30 px-3 py-2 hover:border-emerald-500/40 transition-colors"
        >
          <span class="min-w-0">
            <span class="block text-sm text-gray-100">{a.label}</span>
            <span class="block text-xs text-gray-500">{a.desc}</span>
          </span>
          <MultiplierBadge multiplier={a.mult} size="md" />
        </a>
      </li>
    {/each}
  </ul>
</div>
