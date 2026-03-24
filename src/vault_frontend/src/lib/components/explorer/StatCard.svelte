<script lang="ts">
  interface Props {
    label: string;
    value: string;
    subtitle?: string;
    trend?: 'up' | 'down' | 'neutral';
    trendValue?: string;
    size?: 'sm' | 'md' | 'lg';
  }

  let { label, value, subtitle, trend, trendValue, size = 'md' }: Props = $props();

  const paddings: Record<string, string> = { sm: 'p-3', md: 'p-5', lg: 'p-6' };
  const valueSizes: Record<string, string> = { sm: 'text-lg', md: 'text-2xl', lg: 'text-3xl' };

  let padding = $derived(paddings[size]);
  let valueSize = $derived(valueSizes[size]);

  let trendColor = $derived(
    trend === 'up' ? 'text-emerald-400' : trend === 'down' ? 'text-red-400' : 'text-gray-400'
  );
  let trendArrow = $derived(trend === 'up' ? '\u2191' : trend === 'down' ? '\u2193' : '');
</script>

<div class="bg-gray-800/50 border border-gray-700/50 rounded-xl {padding}">
  <p class="text-xs text-gray-400 uppercase tracking-wide mb-1">{label}</p>
  <p class="{valueSize} font-bold text-white font-mono">{value}</p>
  {#if subtitle}
    <p class="text-xs text-gray-500 mt-1">{subtitle}</p>
  {/if}
  {#if trend && trendValue}
    <p class="text-xs mt-1 {trendColor}">
      {trendArrow}{trendValue}
    </p>
  {/if}
</div>
