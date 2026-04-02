<script lang="ts">
  interface Props {
    symbol: string;
    principalId?: string;
    size?: 'sm' | 'md';
    linked?: boolean;
  }

  let { symbol, principalId, size = 'sm', linked = true }: Props = $props();

  // These map to the actual files in the static/ directory:
  // Note: ckETH, ckUSDT, ckUSDC logos are not yet available
  const logos: Record<string, string> = {
    ICP: '/icp-token-dark.svg',
    ckBTC: '/ckBTC_logo.svg',
    ckXAUT: '/ckXAUT_logo.svg',
    icUSD: '/icusd-logo_v3.svg',
    BOB: '/bob-logo.png',
    EXE: '/exe-logo.svg',
    ckETH: '/cketh-logo.svg',
    nICP: '/nicp-logo.svg',
  };

  const sizeClasses = size === 'sm'
    ? 'text-xs px-2 py-0.5 gap-1'
    : 'text-sm px-3 py-1 gap-1.5';

  const imgSize = size === 'sm' ? 'w-3.5 h-3.5' : 'w-5 h-5';
</script>

{#if linked && principalId}
  <a href="/explorer/token/{principalId}" class="inline-flex items-center {sizeClasses} bg-gray-700/50 rounded-full hover:bg-gray-600/50 transition-colors">
    {#if logos[symbol]}
      <img src={logos[symbol]} alt={symbol} class="{imgSize} rounded-full" />
    {/if}
    <span class="font-medium text-gray-200">{symbol}</span>
  </a>
{:else}
  <span class="inline-flex items-center {sizeClasses} bg-gray-700/50 rounded-full">
    {#if logos[symbol]}
      <img src={logos[symbol]} alt={symbol} class="{imgSize} rounded-full" />
    {/if}
    <span class="font-medium text-gray-200">{symbol}</span>
  </span>
{/if}
