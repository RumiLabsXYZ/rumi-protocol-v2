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

  // Chord-diagram layout. Each token gets one arc on the ring sized by its
  // total throughput (in + out). Ribbons connect arcs, sized by per-edge
  // volume. Compared to the previous bipartite Sankey this surfaces every
  // token once (instead of twice — same token on left and right) and makes
  // round-trip pairs read as a single thicker bidirectional band rather than
  // two parallel ribbons that crowd out everything else.

  const VIEW_W = 600;
  const VIEW_H = 420;
  const CX = VIEW_W / 2;
  const CY = VIEW_H / 2;
  const OUTER_R = 160;
  const INNER_R = 148;
  const LABEL_R = OUTER_R + 14;
  // Total angular space taken by gaps between arcs. Spread evenly so even a
  // single dominant pair leaves visible breathing room between tokens.
  const TOTAL_GAP_ANGLE = Math.PI / 18; // 10° total
  const TWO_PI = Math.PI * 2;

  type TokenKey = string;

  interface TokenArc {
    key: TokenKey;
    symbol: string;
    total: bigint;
    color: string;
    /** Arc start angle (radians, 0 = 3 o'clock, increases clockwise). */
    start: number;
    end: number;
  }

  interface Ribbon {
    fromArc: TokenArc;
    toArc: TokenArc;
    fromStart: number;
    fromEnd: number;
    toStart: number;
    toEnd: number;
    edge: TokenFlowEdge;
    href: string;
  }

  // Per-token color. Hash the principal so a token keeps its color across
  // re-renders even if we add/remove edges.
  const palette = [
    CHART_COLORS.teal,
    CHART_COLORS.purple,
    CHART_COLORS.action,
    CHART_COLORS.caution,
    CHART_COLORS.danger,
  ];
  function colorFor(key: TokenKey): string {
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

  function pointOnCircle(angle: number, r: number): [number, number] {
    return [CX + r * Math.cos(angle), CY + r * Math.sin(angle)];
  }

  /** Path for an annular wedge — the visible arc rectangle for a token. */
  function arcPath(start: number, end: number): string {
    const [x0o, y0o] = pointOnCircle(start, OUTER_R);
    const [x1o, y1o] = pointOnCircle(end, OUTER_R);
    const [x1i, y1i] = pointOnCircle(end, INNER_R);
    const [x0i, y0i] = pointOnCircle(start, INNER_R);
    const largeArc = end - start > Math.PI ? 1 : 0;
    return [
      `M ${x0o} ${y0o}`,
      `A ${OUTER_R} ${OUTER_R} 0 ${largeArc} 1 ${x1o} ${y1o}`,
      `L ${x1i} ${y1i}`,
      `A ${INNER_R} ${INNER_R} 0 ${largeArc} 0 ${x0i} ${y0i}`,
      'Z',
    ].join(' ');
  }

  /** Filled ribbon between two arc segments, curving through the center.
   *  Both ends sweep along the inner radius so the ribbon meets the arc flush. */
  function ribbonPath(fs: number, fe: number, ts: number, te: number): string {
    const [a1x, a1y] = pointOnCircle(fs, INNER_R);
    const [a2x, a2y] = pointOnCircle(fe, INNER_R);
    const [b1x, b1y] = pointOnCircle(ts, INNER_R);
    const [b2x, b2y] = pointOnCircle(te, INNER_R);
    const fLarge = fe - fs > Math.PI ? 1 : 0;
    const tLarge = te - ts > Math.PI ? 1 : 0;
    return [
      `M ${a1x} ${a1y}`,
      `A ${INNER_R} ${INNER_R} 0 ${fLarge} 1 ${a2x} ${a2y}`,
      `Q ${CX} ${CY} ${b1x} ${b1y}`,
      `A ${INNER_R} ${INNER_R} 0 ${tLarge} 1 ${b2x} ${b2y}`,
      `Q ${CX} ${CY} ${a1x} ${a1y}`,
      'Z',
    ].join(' ');
  }

  const layout = $derived.by<{ arcs: TokenArc[]; ribbons: Ribbon[] }>(() => {
    if (edges.length === 0) return { arcs: [], ribbons: [] };

    // Aggregate per-token throughput. Self-loops (token in == token out)
    // would otherwise count twice against a single token's arc; we skip
    // them entirely (no real swap goes X→X anyway).
    const totals = new Map<TokenKey, bigint>();
    const validEdges: TokenFlowEdge[] = [];
    for (const e of edges) {
      const fromKey = e.from_token.toText();
      const toKey = e.to_token.toText();
      if (fromKey === toKey) continue;
      validEdges.push(e);
      totals.set(fromKey, (totals.get(fromKey) ?? 0n) + e.volume_usd_e8s);
      totals.set(toKey, (totals.get(toKey) ?? 0n) + e.volume_usd_e8s);
    }

    if (validEdges.length === 0) return { arcs: [], ribbons: [] };

    const grandTotal = Number(
      Array.from(totals.values()).reduce((s, v) => s + v, 0n)
    ) || 1;

    // Order tokens by total volume descending — biggest first, so the
    // dominant pair is at the top of the ring where the eye lands first.
    const sorted = Array.from(totals.entries()).sort((a, b) => {
      if (b[1] > a[1]) return 1;
      if (b[1] < a[1]) return -1;
      return 0;
    });

    const n = sorted.length;
    const gapEach = TOTAL_GAP_ANGLE / n;
    const usableAngle = TWO_PI - TOTAL_GAP_ANGLE;
    let cursor = -Math.PI / 2; // start at 12 o'clock

    const arcs: TokenArc[] = [];
    const arcByKey = new Map<TokenKey, TokenArc>();
    for (const [key, total] of sorted) {
      const span = (Number(total) / grandTotal) * usableAngle;
      const arc: TokenArc = {
        key,
        symbol: getTokenSymbol(key),
        total,
        color: colorFor(key),
        start: cursor,
        end: cursor + span,
      };
      arcs.push(arc);
      arcByKey.set(key, arc);
      cursor += span + gapEach;
    }

    // Within each arc, allocate sub-slots per edge. Sort each token's
    // edges by counterparty's overall volume (so ribbons attach next to
    // the biggest neighbor) for a stable visual order.
    interface SlotSpec {
      key: TokenKey;
      counterpartyTotal: bigint;
      edgeIdx: number;
      width: number;
    }
    const outSlots = new Map<TokenKey, SlotSpec[]>();
    const inSlots = new Map<TokenKey, SlotSpec[]>();
    for (const t of arcs) {
      outSlots.set(t.key, []);
      inSlots.set(t.key, []);
    }
    validEdges.forEach((e, idx) => {
      const fromKey = e.from_token.toText();
      const toKey = e.to_token.toText();
      const fromArc = arcByKey.get(fromKey)!;
      const toArc = arcByKey.get(toKey)!;
      const fromSpan = fromArc.end - fromArc.start;
      const toSpan = toArc.end - toArc.start;
      const vol = Number(e.volume_usd_e8s);
      // Each edge's slot on a token = (edge volume / token total) × token's arc span.
      const fromWidth = (vol / Number(fromArc.total)) * fromSpan;
      const toWidth = (vol / Number(toArc.total)) * toSpan;
      outSlots.get(fromKey)!.push({
        key: toKey,
        counterpartyTotal: toArc.total,
        edgeIdx: idx,
        width: fromWidth,
      });
      inSlots.get(toKey)!.push({
        key: fromKey,
        counterpartyTotal: fromArc.total,
        edgeIdx: idx,
        width: toWidth,
      });
    });
    const sortByCp = (a: SlotSpec, b: SlotSpec) => {
      if (b.counterpartyTotal > a.counterpartyTotal) return 1;
      if (b.counterpartyTotal < a.counterpartyTotal) return -1;
      return 0;
    };
    for (const [, list] of outSlots) list.sort(sortByCp);
    for (const [, list] of inSlots) list.sort(sortByCp);

    // Convention: outgoing slots stack from arc-start, incoming slots
    // stack after them. Same per-token order on both sides ⇒ ribbons
    // never cross within a single token's arc.
    const slotPositions = new Map<number, { from: [number, number]; to: [number, number] }>();
    for (const arc of arcs) {
      let pos = arc.start;
      for (const s of outSlots.get(arc.key)!) {
        const start = pos;
        const end = pos + s.width;
        const existing = slotPositions.get(s.edgeIdx) ?? { from: [0, 0], to: [0, 0] };
        existing.from = [start, end];
        slotPositions.set(s.edgeIdx, existing);
        pos = end;
      }
      for (const s of inSlots.get(arc.key)!) {
        const start = pos;
        const end = pos + s.width;
        const existing = slotPositions.get(s.edgeIdx) ?? { from: [0, 0], to: [0, 0] };
        existing.to = [start, end];
        slotPositions.set(s.edgeIdx, existing);
        pos = end;
      }
    }

    const ribbons: Ribbon[] = validEdges.map((edge, idx) => {
      const slot = slotPositions.get(idx)!;
      const fromArc = arcByKey.get(edge.from_token.toText())!;
      const toArc = arcByKey.get(edge.to_token.toText())!;
      return {
        fromArc,
        toArc,
        fromStart: slot.from[0],
        fromEnd: slot.from[1],
        toStart: slot.to[0],
        toEnd: slot.to[1],
        edge,
        href: activityHref(edge.from_token.toText(), edge.to_token.toText()),
      };
    });

    return { arcs, ribbons };
  });

  let hovered = $state<number | null>(null);

  function labelTransform(arc: TokenArc): string {
    const mid = (arc.start + arc.end) / 2;
    const [lx, ly] = pointOnCircle(mid, LABEL_R);
    // Rotate label tangentially when arc isn't near top/bottom — keeps
    // labels readable on side arcs at small token volumes.
    return `translate(${lx} ${ly})`;
  }

  function labelAnchor(arc: TokenArc): 'start' | 'middle' | 'end' {
    const mid = (arc.start + arc.end) / 2;
    const cos = Math.cos(mid);
    if (cos > 0.2) return 'start';
    if (cos < -0.2) return 'end';
    return 'middle';
  }
</script>

<div class="space-y-2">
  {#if loading}
    <div class="text-sm text-gray-500 py-12 text-center">Loading flows…</div>
  {:else if layout.arcs.length === 0}
    <div class="text-sm text-gray-500 py-12 text-center">
      No token flow in this window.
    </div>
  {:else}
    <svg
      role="img"
      aria-label="Token flow chord diagram"
      viewBox={`0 0 ${VIEW_W} ${VIEW_H}`}
      class="w-full h-auto overflow-visible"
    >
      <!-- Ribbons first so arcs draw on top. -->
      {#each layout.ribbons as ribbon, i (ribbon.fromArc.key + '->' + ribbon.toArc.key)}
        <a href={ribbon.href} aria-label="Activity: {ribbon.fromArc.symbol} → {ribbon.toArc.symbol}">
          <path
            role="presentation"
            d={ribbonPath(ribbon.fromStart, ribbon.fromEnd, ribbon.toStart, ribbon.toEnd)}
            fill={ribbon.fromArc.color}
            fill-opacity={hovered === null || hovered === i ? 0.45 : 0.12}
            stroke={ribbon.fromArc.color}
            stroke-opacity={hovered === i ? 0.75 : 0}
            stroke-width="0.5"
            class="transition-opacity"
            onmouseenter={() => (hovered = i)}
            onmouseleave={() => (hovered = null)}
          >
            <title>
              {ribbon.fromArc.symbol} → {ribbon.toArc.symbol} ·
              ${formatCompact(e8sToNumber(ribbon.edge.volume_usd_e8s))} ·
              {Number(ribbon.edge.swap_count).toLocaleString()} swaps
            </title>
          </path>
        </a>
      {/each}

      <!-- Token arcs and labels -->
      {#each layout.arcs as arc (arc.key)}
        <path
          d={arcPath(arc.start, arc.end)}
          fill={arc.color}
          fill-opacity="0.8"
        >
          <title>
            {arc.symbol} · ${formatCompact(e8sToNumber(arc.total))} total throughput
          </title>
        </path>
        <text
          transform={labelTransform(arc)}
          text-anchor={labelAnchor(arc)}
          dy="0.35em"
          fill="currentColor"
          class="fill-gray-300 text-[11px] font-medium pointer-events-none"
        >{arc.symbol}</text>
      {/each}
    </svg>
  {/if}
</div>
