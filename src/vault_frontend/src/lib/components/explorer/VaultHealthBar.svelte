<script lang="ts">
  interface Props {
    collateralRatio: number;  // as percentage (e.g., 150)
    liquidationRatio: number; // as percentage (e.g., 110)
    borrowThreshold?: number; // as percentage (e.g., 150) — min safe CR
  }

  let { collateralRatio, liquidationRatio, borrowThreshold }: Props = $props();

  // Use borrow threshold as the danger zone boundary if available
  const dangerThreshold = $derived(borrowThreshold ?? liquidationRatio * 1.36);
  const cautionThreshold = $derived(dangerThreshold * 1.25);

  const color = $derived(
    collateralRatio <= liquidationRatio ? 'bg-red-500' :
    collateralRatio <= dangerThreshold * 1.05 ? 'bg-orange-500' :
    collateralRatio <= cautionThreshold ? 'bg-yellow-500' :
    'bg-green-500'
  );

  const label = $derived(
    collateralRatio <= liquidationRatio ? 'Liquidatable' :
    collateralRatio <= dangerThreshold * 1.05 ? 'Danger' :
    collateralRatio <= cautionThreshold ? 'Caution' :
    'Healthy'
  );

  const textColor = $derived(
    collateralRatio <= liquidationRatio ? 'text-red-400' :
    collateralRatio <= dangerThreshold * 1.05 ? 'text-orange-400' :
    collateralRatio <= cautionThreshold ? 'text-yellow-400' :
    'text-green-400'
  );

  const widthPct = $derived(Math.min(100, Math.max(5, (collateralRatio / (cautionThreshold * 1.2)) * 100)));
</script>

<div class="flex items-center gap-3">
  <div class="flex-1 h-2 bg-gray-700 rounded-full overflow-hidden">
    <div class="{color} h-full rounded-full transition-all duration-300" style="width: {widthPct}%"></div>
  </div>
  <span class="text-xs font-medium {textColor}">
    {collateralRatio.toFixed(0)}% — {label}
  </span>
</div>
