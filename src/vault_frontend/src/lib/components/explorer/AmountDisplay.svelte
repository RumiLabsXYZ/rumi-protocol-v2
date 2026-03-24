<script lang="ts">
  import { formatTokenAmount, formatUsdRaw, getTokenSymbol } from '$utils/explorerHelpers';

  interface Props {
    amount: bigint | number;
    tokenPrincipal?: string;
    showUsd?: boolean;
    price?: number;
    size?: 'sm' | 'md' | 'lg';
  }

  let { amount, tokenPrincipal = '', showUsd = false, price, size = 'md' }: Props = $props();

  const textSizes: Record<string, string> = { sm: 'text-xs', md: 'text-sm', lg: 'text-base' };

  let textSize = $derived(textSizes[size]);
  let formatted = $derived(tokenPrincipal ? formatTokenAmount(amount, tokenPrincipal) : String(amount));
  let symbol = $derived(tokenPrincipal ? getTokenSymbol(tokenPrincipal) : '');

  let usdValue = $derived.by(() => {
    if (!showUsd || !price) return '';
    const num = typeof amount === 'bigint' ? Number(amount) / 1e8 : amount;
    return formatUsdRaw(num * price);
  });
</script>

<span class="{textSize} inline-flex items-baseline gap-1">
  <span class="font-mono text-white">{formatted}</span>
  {#if symbol}
    <span class="text-gray-400">{symbol}</span>
  {/if}
  {#if usdValue}
    <span class="text-gray-500">({usdValue})</span>
  {/if}
</span>
