<script lang="ts">
  import type { TokenFlowEdge } from '$declarations/rumi_analytics/rumi_analytics.did';
  import { getTokenSymbol } from '$utils/explorerHelpers';
  import { formatCompact, e8sToNumber, CHART_COLORS } from '$utils/explorerChartHelpers';

  interface Props {
    edges: TokenFlowEdge[];
    loading: boolean;
    /** Time preset passed through on activity deep-links. */
    timePreset: '24h' | '7d' | '30d';
  }

  let { edges, loading, timePreset }: Props = $props();

  // A minimal bipartite Sankey. We don't do crossing-minimization — node
  // ordering is volume-desc on each side — but for the 5–20 edges this chart
  // typically sees that's enough to read the data.

  const HEIGHT = 320;
  const NODE_WIDTH = 10;
  const NODE_GAP = 6;
  const PADDING_X = 8;
  const PADDING_Y = 8;

  type NodeKey = string; // principal text

  interface NodeRow {
    key: NodeKey;
    symbol: string;
    total: bigint; // total volume passing through this side of the node
    y: number;
    height: number;
  }

  interface Ribbon {
    from: NodeRow;
    to: NodeRow;
    edge: TokenFlowEdge;
    width: number;
    yFrom: number;
    yTo: number;
    color: string;
    href: string;
  }

  // Stable palette per token so re-renders don't flicker hues. Uses the
  // existing CHART_COLORS accent set.
  const palette = [
    CHART_COLORS.teal,
    CHART_COLORS.purple,
    CHART_COLORS.action,
    CHART_COLORS.caution,
    CHART_COLORS.danger,
  ];
  function colorFor(key: NodeKey): string {
    let h = 0;
    for (const c of key) h = (h * 31 + c.charCodeAt(0)) & 0xffff;
    return palette[h % palette.length];
  }

  function activityHref(from: string, to: string): string {
    const params = new URLSearchParams();
    params.set('type', 'swap');
    params.set('token', `${from},${to}`);
    if (timePreset) params.set('time', timePreset);
    return `/explorer/activity?${params.toString()}`;
  }

  const viewBox = $derived.by(() => {
    // Use a fixed logical width; the SVG scales via CSS.
    return `0 0 600 ${HEIGHT}`;
  });

  const layout = $derived.by<{
    leftNodes: NodeRow[];
    rightNodes: NodeRow[];
    ribbons: Ribbon[];
  }>(() => {
    if (edges.length === 0) {
      return { leftNodes: [], rightNodes: [], ribbons: [] };
    }
    const LEFT_X = PADDING_X;
    const RIGHT_X = 600 - PADDING_X - NODE_WIDTH;
    const usableH = HEIGHT - 2 * PADDING_Y;

    // --- Aggregate per side ---
    const leftTotals = new Map<NodeKey, bigint>();
    const rightTotals = new Map<NodeKey, bigint>();
    for (const e of edges) {
      const fromKey = e.from_token.toText();
      const toKey = e.to_token.toText();
      leftTotals.set(fromKey, (leftTotals.get(fromKey) ?? 0n) + e.volume_usd_e8s);
      rightTotals.set(toKey, (rightTotals.get(toKey) ?? 0n) + e.volume_usd_e8s);
    }

    const grandTotal = Number(
      Array.from(leftTotals.values()).reduce((s, v) => s + v, 0n) || 1n
    );

    function buildSide(totals: Map<NodeKey, bigint>, x: number): NodeRow[] {
      const sorted = Array.from(totals.entries()).sort((a, b) =>
        b[1] > a[1] ? 1 : b[1] < a[1] ? -1 : 0
      );
      const n = sorted.length;
      const totalGapH = Math.max(0, (n - 1) * NODE_GAP);
      const proportionalH = Math.max(0, usableH - totalGapH);
      const scale = proportionalH / grandTotal;
      let cursor = PADDING_Y;
      const rows: NodeRow[] = [];
      for (const [key, total] of sorted) {
        const h = Math.max(4, Number(total) * scale);
        rows.push({
          key,
          symbol: getTokenSymbol(key),
          total,
          y: cursor,
          height: h,
        });
        cursor += h + NODE_GAP;
      }
      // Stash x on a parallel structure so the template can draw without
      // re-deriving — attach via field.
      return rows.map((r) => Object.assign(r, { x }));
    }

    const leftNodes = buildSide(leftTotals, LEFT_X) as (NodeRow & { x: number })[];
    const rightNodes = buildSide(rightTotals, RIGHT_X) as (NodeRow & { x: number })[];
    const leftByKey = new Map(leftNodes.map((n) => [n.key, n]));
    const rightByKey = new Map(rightNodes.map((n) => [n.key, n]));

    // --- Sub-slot cursors so edges stack inside each node in sort order ---
    const leftOffsets = new Map<NodeKey, number>();
    const rightOffsets = new Map<NodeKey, number>();
    for (const n of leftNodes) leftOffsets.set(n.key, 0);
    for (const n of rightNodes) rightOffsets.set(n.key, 0);

    // Edges are already sorted desc by volume — preserve that order so the
    // widest ribbon attaches at the top of each node.
    const scale = (HEIGHT - 2 * PADDING_Y) / grandTotal;

    const ribbons: Ribbon[] = edges
      .map((edge) => {
        const fromKey = edge.from_token.toText();
        const toKey = edge.to_token.toText();
        const from = leftByKey.get(fromKey);
        const to = rightByKey.get(toKey);
        if (!from || !to) return null;
        const width = Math.max(1, Number(edge.volume_usd_e8s) * scale);
        const yFrom = from.y + (leftOffsets.get(fromKey) ?? 0) + width / 2;
        const yTo = to.y + (rightOffsets.get(toKey) ?? 0) + width / 2;
        leftOffsets.set(fromKey, (leftOffsets.get(fromKey) ?? 0) + width);
        rightOffsets.set(toKey, (rightOffsets.get(toKey) ?? 0) + width);
        return {
          from,
          to,
          edge,
          width,
          yFrom,
          yTo,
          color: colorFor(fromKey),
          href: activityHref(fromKey, toKey),
        } as Ribbon;
      })
      .filter((r): r is Ribbon => r !== null);

    return { leftNodes, rightNodes, ribbons };
  });

  function ribbonPath(r: Ribbon): string {
    // Cubic bezier flowing left->right at the ribbon midline.
    const xFrom = (r.from as any).x + NODE_WIDTH;
    const xTo = (r.to as any).x;
    const midX = (xFrom + xTo) / 2;
    return `M ${xFrom} ${r.yFrom} C ${midX} ${r.yFrom}, ${midX} ${r.yTo}, ${xTo} ${r.yTo}`;
  }

  let hovered = $state<number | null>(null);
</script>

<div class="space-y-2">
  {#if loading}
    <div class="text-sm text-gray-500 py-12 text-center">Loading flows…</div>
  {:else if edges.length === 0}
    <div class="text-sm text-gray-500 py-12 text-center">
      No token flow in this window.
    </div>
  {:else}
    <svg
      role="img"
      aria-label="Token flow Sankey"
      viewBox={viewBox}
      class="w-full h-auto overflow-visible"
    >
      <!-- Ribbons render first so node rects draw on top of their endpoints. -->
      {#each layout.ribbons as ribbon, i (ribbon.from.key + '->' + ribbon.to.key)}
        <a href={ribbon.href} aria-label="Activity: {ribbon.from.symbol} → {ribbon.to.symbol}">
          <path
            role="presentation"
            d={ribbonPath(ribbon)}
            stroke={ribbon.color}
            stroke-width={ribbon.width}
            fill="none"
            stroke-opacity={hovered === null || hovered === i ? 0.55 : 0.18}
            class="transition-opacity"
            onmouseenter={() => (hovered = i)}
            onmouseleave={() => (hovered = null)}
          >
            <title>
              {ribbon.from.symbol} → {ribbon.to.symbol} ·
              ${formatCompact(e8sToNumber(ribbon.edge.volume_usd_e8s))} ·
              {Number(ribbon.edge.swap_count).toLocaleString()} swaps
            </title>
          </path>
        </a>
      {/each}

      <!-- Node rects and labels -->
      {#each layout.leftNodes as n (n.key)}
        <rect
          x={(n as any).x}
          y={n.y}
          width={NODE_WIDTH}
          height={n.height}
          fill={colorFor(n.key)}
          rx="2"
        />
        <text
          x={(n as any).x + NODE_WIDTH + 6}
          y={n.y + n.height / 2}
          dy="0.35em"
          fill="currentColor"
          class="fill-gray-300 text-[11px]"
        >{n.symbol}</text>
      {/each}

      {#each layout.rightNodes as n (n.key)}
        <rect
          x={(n as any).x}
          y={n.y}
          width={NODE_WIDTH}
          height={n.height}
          fill={colorFor(n.key)}
          rx="2"
        />
        <text
          x={(n as any).x - 6}
          y={n.y + n.height / 2}
          dy="0.35em"
          text-anchor="end"
          fill="currentColor"
          class="fill-gray-300 text-[11px]"
        >{n.symbol}</text>
      {/each}
    </svg>
  {/if}
</div>
