<script lang="ts">
  interface Props {
    heading?: string;
  }
  let { heading = 'Ways to earn points' }: Props = $props();

  const actions = [
    { label: 'Mint icUSD', desc: 'Open a vault and borrow icUSD against collateral.', href: '/' },
    { label: 'Deposit to the stability pool', desc: 'Earn while backstopping liquidations.', href: '/stability-pool' },
    { label: 'Provide liquidity', desc: 'Add liquidity to the 3pool or AMM.', href: '/liquidity' },
  ];
</script>

<div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
  <p class="text-sm font-medium text-gray-200 mb-3">{heading}</p>
  <ul class="flex flex-col gap-2">
    {#each actions as a}
      <li>
        <a
          href={a.href}
          class="flex items-center justify-between gap-3 rounded-lg border border-gray-700/40 bg-gray-900/30 px-3 py-2 hover:border-teal-500/40 transition-colors"
        >
          <span>
            <span class="block text-sm text-gray-100">{a.label}</span>
            <span class="block text-xs text-gray-500">{a.desc}</span>
          </span>
          <span class="text-teal-400 text-sm">→</span>
        </a>
      </li>
    {/each}
  </ul>
</div>
