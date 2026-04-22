<script lang="ts">
  /**
   * CRDial — the Rumi signature component. A half-circle dial that visualizes
   * a vault's collateral ratio (cr) against its liquidation floor. Zones map to
   * health bands: red (< liquidation * 1.1), amber (< liquidation * 1.5),
   * green (≥ liquidation * 1.5). Infinity (no debt) renders as a full green dial.
   *
   * The arc sweep is 180° at the top (a gauge, not a full circle). cr maps to a
   * sweep where liquidation = ~15% of sweep and 3x liquidation = 100%. Capped
   * visually at 3x so healthy vaults don't all look identical.
   */
  interface Props {
    cr: number;
    liquidationCR: number;
    size?: 'sm' | 'md' | 'lg';
    label?: string;
  }

  let { cr, liquidationCR, size = 'md', label }: Props = $props();

  const dims: Record<'sm' | 'md' | 'lg', { w: number; stroke: number; fontMain: string; fontSub: string }> = {
    sm: { w: 44, stroke: 6, fontMain: 'text-[10px]', fontSub: 'text-[8px]' },
    md: { w: 120, stroke: 11, fontMain: 'text-xl', fontSub: 'text-[10px]' },
    lg: { w: 200, stroke: 16, fontMain: 'text-4xl', fontSub: 'text-xs' },
  };

  const d = $derived(dims[size]);
  const cx = $derived(d.w / 2);
  const cy = $derived(d.w / 2);
  const r = $derived((d.w - d.stroke) / 2 - 2);

  const isInfinite = $derived(!isFinite(cr) || cr > 1000);

  // Map cr to dial position [0, 1]. 0 = liquidation, 1 = 3x liquidation.
  const fillRatio = $derived.by(() => {
    if (isInfinite) return 1;
    if (liquidationCR <= 0) return 1;
    const normalized = (cr - liquidationCR) / (liquidationCR * 2);
    return Math.max(0, Math.min(1, normalized));
  });

  const healthColor = $derived.by(() => {
    if (isInfinite) return '#34d399';
    if (cr < liquidationCR * 1.1) return '#f43f5e';
    if (cr < liquidationCR * 1.5) return '#fbbf24';
    return '#34d399';
  });

  // Half-circle arc geometry. Arc runs from (cx - r, cy) to (cx + r, cy) above center.
  const circumference = $derived(Math.PI * r);
  const dashOffset = $derived(circumference * (1 - fillRatio));

  const crDisplay = $derived.by(() => {
    if (isInfinite) return '∞';
    return `${(cr * 100).toFixed(0)}%`;
  });

  const showLabel = $derived(size !== 'sm');
  const crLabel = $derived(label ?? (size === 'sm' ? null : 'CR'));
</script>

<div class="inline-flex flex-col items-center" title="Collateral Ratio: {crDisplay} (liq at {(liquidationCR * 100).toFixed(0)}%)">
  <svg
    width={d.w}
    height={d.w / 2 + d.stroke}
    viewBox="0 0 {d.w} {d.w / 2 + d.stroke}"
    class="overflow-visible"
  >
    <!-- Track (gray background arc) -->
    <path
      d="M {d.stroke / 2 + 2} {cy} A {r} {r} 0 0 1 {d.w - d.stroke / 2 - 2} {cy}"
      fill="none"
      stroke="rgba(75, 85, 99, 0.35)"
      stroke-width={d.stroke}
      stroke-linecap="round"
    />
    <!-- Liquidation marker: thin tick at 0% of arc (left side) -->
    <line
      x1={d.stroke / 2 + 2}
      y1={cy - 1}
      x2={d.stroke / 2 + 2}
      y2={cy - d.stroke - 3}
      stroke="rgba(244, 63, 94, 0.6)"
      stroke-width={size === 'sm' ? 1 : 2}
      stroke-linecap="round"
    />
    <!-- Active arc -->
    <path
      d="M {d.stroke / 2 + 2} {cy} A {r} {r} 0 0 1 {d.w - d.stroke / 2 - 2} {cy}"
      fill="none"
      stroke={healthColor}
      stroke-width={d.stroke}
      stroke-linecap="round"
      stroke-dasharray={circumference}
      stroke-dashoffset={dashOffset}
      style="transition: stroke-dashoffset 400ms ease, stroke 400ms ease;"
    />
  </svg>
  {#if showLabel}
    <div class="-mt-1 flex flex-col items-center leading-none">
      <span class="{d.fontMain} font-bold tabular-nums" style="color: {healthColor}">{crDisplay}</span>
      {#if crLabel}<span class="{d.fontSub} text-gray-500 uppercase tracking-wider mt-0.5">{crLabel}</span>{/if}
    </div>
  {/if}
</div>
