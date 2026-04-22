<script lang="ts">
  /**
   * Vault CR-over-time chart. Dual axis:
   *   - Left axis + line: collateral ratio (%), green/amber/red by health.
   *   - Right axis + muted line: collateral price ($).
   * A shaded red band at y ≤ liquidationCR visualizes the danger zone.
   */
  interface Point {
    t: number; // ns since epoch
    cr: number; // ratio (1.5 = 150%)
    price: number;
  }

  interface Props {
    points: Point[];
    liquidationCR: number;
    width?: number;
    height?: number;
  }

  let { points, liquidationCR, width = 780, height = 220 }: Props = $props();

  const padL = 40;
  const padR = 50;
  const padY = 18;

  const plotW = $derived(width - padL - padR);
  const plotH = $derived(height - padY * 2);

  const scale = $derived.by(() => {
    if (!points.length) return null;
    const ts = points.map((p) => p.t);
    const crs = points.map((p) => (isFinite(p.cr) ? Math.min(p.cr, liquidationCR * 4) : liquidationCR * 4));
    const prices = points.map((p) => p.price).filter((p) => p > 0);
    const minT = Math.min(...ts);
    const maxT = Math.max(...ts);
    const crMax = Math.max(...crs, liquidationCR * 1.5);
    const crMin = Math.min(...crs, liquidationCR * 0.9);
    const pMax = prices.length ? Math.max(...prices) : 1;
    const pMin = prices.length ? Math.min(...prices) : 0;
    return { minT, maxT, crMin, crMax, pMin, pMax };
  });

  function x(t: number): number {
    if (!scale || scale.maxT === scale.minT) return padL;
    return padL + ((t - scale.minT) / (scale.maxT - scale.minT)) * plotW;
  }
  function yCR(cr: number): number {
    if (!scale || scale.crMax === scale.crMin) return padY + plotH / 2;
    const capped = isFinite(cr) ? Math.min(cr, scale.crMax) : scale.crMax;
    return padY + (1 - (capped - scale.crMin) / (scale.crMax - scale.crMin)) * plotH;
  }
  function yPrice(p: number): number {
    if (!scale || scale.pMax === scale.pMin) return padY + plotH / 2;
    return padY + (1 - (p - scale.pMin) / (scale.pMax - scale.pMin)) * plotH;
  }

  const crPath = $derived.by(() => {
    if (!scale || !points.length) return '';
    return points
      .map((p, i) => `${i === 0 ? 'M' : 'L'} ${x(p.t).toFixed(1)} ${yCR(p.cr).toFixed(1)}`)
      .join(' ');
  });

  const pricePath = $derived.by(() => {
    if (!scale || !points.length) return '';
    const withPrice = points.filter((p) => p.price > 0);
    if (!withPrice.length) return '';
    return withPrice
      .map((p, i) => `${i === 0 ? 'M' : 'L'} ${x(p.t).toFixed(1)} ${yPrice(p.price).toFixed(1)}`)
      .join(' ');
  });

  const liqZoneTop = $derived(scale ? yCR(liquidationCR) : padY);
  const plotBottom = $derived(padY + plotH);

  const latestCR = $derived(points.length ? points[points.length - 1].cr : 0);
  const crColor = $derived.by(() => {
    if (!isFinite(latestCR)) return '#34d399';
    if (latestCR < liquidationCR * 1.1) return '#f43f5e';
    if (latestCR < liquidationCR * 1.5) return '#fbbf24';
    return '#34d399';
  });
</script>

<div>
  {#if !points.length}
    <div class="text-xs text-gray-500 py-16 text-center">No history to chart.</div>
  {:else}
    <svg viewBox="0 0 {width} {height}" class="w-full h-auto" preserveAspectRatio="xMidYMid meet">
      <!-- Liquidation zone shading -->
      <rect
        x={padL}
        y={liqZoneTop}
        width={plotW}
        height={Math.max(0, plotBottom - liqZoneTop)}
        fill="rgba(244, 63, 94, 0.08)"
      />
      <!-- Liquidation threshold line -->
      <line
        x1={padL}
        x2={padL + plotW}
        y1={liqZoneTop}
        y2={liqZoneTop}
        stroke="rgba(244, 63, 94, 0.5)"
        stroke-dasharray="4 4"
        stroke-width="1"
      />
      <!-- Price line (muted) -->
      {#if pricePath}
        <path d={pricePath} fill="none" stroke="rgba(148, 163, 184, 0.5)" stroke-width="1.2" />
      {/if}
      <!-- CR line (health color) -->
      <path d={crPath} fill="none" stroke={crColor} stroke-width="1.8" stroke-linejoin="round" />
      <!-- Y axis labels -->
      <text x={padL - 6} y={padY + 4} text-anchor="end" font-size="9" fill="#6b7280">
        {(scale ? scale.crMax * 100 : 0).toFixed(0)}%
      </text>
      <text x={padL - 6} y={liqZoneTop + 3} text-anchor="end" font-size="9" fill="#f43f5e">
        {(liquidationCR * 100).toFixed(0)}%
      </text>
      <text x={width - padR + 6} y={padY + 4} text-anchor="start" font-size="9" fill="#6b7280">
        ${(scale?.pMax ?? 0).toFixed(2)}
      </text>
      <text x={width - padR + 6} y={plotBottom + 4} text-anchor="start" font-size="9" fill="#6b7280">
        ${(scale?.pMin ?? 0).toFixed(2)}
      </text>
    </svg>
    <div class="flex items-center gap-4 text-[10px] text-gray-500 mt-1 px-10">
      <span class="inline-flex items-center gap-1.5"><span class="w-3 h-0.5 rounded" style="background: {crColor}"></span>CR</span>
      <span class="inline-flex items-center gap-1.5"><span class="w-3 h-0.5 rounded bg-slate-400/60"></span>Price</span>
      <span class="inline-flex items-center gap-1.5"><span class="w-3 h-1.5 rounded bg-rose-500/20"></span>Liquidation zone</span>
    </div>
  {/if}
</div>
