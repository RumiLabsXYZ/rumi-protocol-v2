<script lang="ts">
  interface Props {
    collateralRatio: number;
    liquidationRatio: number;
  }

  let { collateralRatio, liquidationRatio }: Props = $props();

  const safeThreshold = liquidationRatio * 1.5;

  const color = $derived(
    collateralRatio <= liquidationRatio ? 'bg-red-500' :
    collateralRatio <= safeThreshold ? 'bg-yellow-500' :
    'bg-green-500'
  );

  const label = $derived(
    collateralRatio <= liquidationRatio ? 'At Risk' :
    collateralRatio <= safeThreshold ? 'Caution' :
    'Healthy'
  );

  const widthPct = $derived(Math.min(100, Math.max(5, (collateralRatio / (safeThreshold * 1.2)) * 100)));
</script>

<div class="flex items-center gap-3">
  <div class="flex-1 h-2 bg-gray-700 rounded-full overflow-hidden">
    <div class="{color} h-full rounded-full transition-all duration-300" style="width: {widthPct}%"></div>
  </div>
  <span class="text-xs font-medium {collateralRatio <= liquidationRatio ? 'text-red-400' : collateralRatio <= safeThreshold ? 'text-yellow-400' : 'text-green-400'}">
    {collateralRatio.toFixed(0)}% — {label}
  </span>
</div>
