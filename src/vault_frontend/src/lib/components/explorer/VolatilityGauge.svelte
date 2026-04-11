<script lang="ts">
  interface Props {
    value: number | null;
    label?: string;
  }

  let { value, label = 'Annualized Volatility' }: Props = $props();

  const displayValue = $derived(value != null ? `${value.toFixed(1)}%` : '--');
  const severity = $derived.by(() => {
    if (value == null) return 'unknown';
    if (value < 20) return 'low';
    if (value < 50) return 'medium';
    if (value < 80) return 'high';
    return 'extreme';
  });

  const colorMap: Record<string, string> = {
    low: 'text-teal-400',
    medium: 'text-violet-400',
    high: 'text-amber-400',
    extreme: 'text-pink-400',
    unknown: 'text-gray-500',
  };

  const bgMap: Record<string, string> = {
    low: 'bg-teal-400/10',
    medium: 'bg-violet-400/10',
    high: 'bg-amber-400/10',
    extreme: 'bg-pink-400/10',
    unknown: 'bg-gray-500/10',
  };

  const labelMap: Record<string, string> = {
    low: 'Low',
    medium: 'Medium',
    high: 'High',
    extreme: 'Extreme',
    unknown: '--',
  };
</script>

<div class="flex items-center gap-3 px-3 py-2 rounded-lg {bgMap[severity]}">
  <div class="text-right">
    <div class="text-lg font-semibold tabular-nums {colorMap[severity]}">{displayValue}</div>
    <div class="text-xs text-gray-500">{label}</div>
  </div>
  <div class="px-2 py-0.5 rounded text-xs font-medium {colorMap[severity]} {bgMap[severity]}">
    {labelMap[severity]}
  </div>
</div>
