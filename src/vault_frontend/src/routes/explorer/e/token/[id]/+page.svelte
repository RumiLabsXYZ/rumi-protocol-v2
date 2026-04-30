<script lang="ts">
  import { Principal } from '@dfinity/principal';
  import EntityShell from '$components/explorer/entity/EntityShell.svelte';
  import MixedEventsTable from '$components/explorer/MixedEventsTable.svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import CopyButton from '$components/explorer/CopyButton.svelte';
  import {
    fetchAllVaults,
    fetchAmmPools,
    fetchCollateralPrices,
    fetchCollateralConfigs,
    fetchEvents,
    fetchEventCount,
    fetchAmmSwapEvents,
    fetchAmmSwapEventCount,
    fetchAmmLiquidityEvents,
    fetchAmmLiquidityEventCount,
    fetchThreePoolSwapEventsV2,
    fetchThreePoolState,
    fetchStabilityPoolEvents,
    fetchStabilityPoolEventCount,
    fetchIcrc1TotalSupply,
  } from '$services/explorer/explorerService';
  import {
    fetchTopHolders,
    fetchPegStatus,
    fetchVaultSeries,
    fetchThreePoolSeries,
    fetchHolderSeries,
    fetchTokenFlow,
  } from '$services/explorer/analyticsService';
  import {
    formatE8s,
    formatUsdRaw,
    getTokenSymbol,
    getTokenDecimals,
    getTokenInfo,
    shortenPrincipal,
    formatBps,
  } from '$utils/explorerHelpers';
  import { extractEventTimestamp } from '$utils/displayEvent';
  import type { DisplayEvent } from '$utils/displayEvent';
  import { extractFacets } from '$utils/eventFacets';
  import { CANISTER_IDS } from '$lib/config';
  import type { TopHoldersResponse, TokenFlowEdge } from '$declarations/rumi_analytics/rumi_analytics.did';

  interface PageData {
    tokenPrincipal: string;
    requestedAlias: string | null;
  }

  let { data }: { data: PageData } = $props();

  const tokenPrincipal = $derived(data.tokenPrincipal);
  const requestedAlias = $derived(data.requestedAlias);

  // ── State ──────────────────────────────────────────────────────────────────

  let loading = $state(true);
  let error: string | null = $state(null);

  let priceMap = $state<Map<string, number>>(new Map());
  let configs: any[] = $state([]);
  let allVaults: any[] = $state([]);
  let ammPools: any[] = $state([]);
  let threePoolState: any = $state(null);
  let topHolders = $state<TopHoldersResponse | null>(null);
  let pegStatus: any = $state(null);
  let liveTotalSupply = $state<bigint | null>(null);

  let backendEvents: [bigint, any][] = $state([]);
  let threePoolSwapEvents: any[] = $state([]);
  let ammSwapEvents: any[] = $state([]);
  let ammLiqEvents: any[] = $state([]);
  let spEvents: any[] = $state([]);

  let vaultSeries: any[] = $state([]);
  let threePoolHistory: any[] = $state([]);
  let holderSeries: any[] = $state([]);
  let flowEdges = $state<TokenFlowEdge[]>([]);

  // ── Derived: identity ──────────────────────────────────────────────────────

  const tokenInfo = $derived(getTokenInfo(tokenPrincipal));
  const symbol = $derived(tokenInfo?.symbol ?? shortenPrincipal(tokenPrincipal));
  const tokenName = $derived(tokenInfo?.name ?? 'Unknown token');
  const tokenDecimals = $derived(tokenInfo?.decimals ?? 8);
  const knownToken = $derived(tokenInfo != null);

  const isIcUsd = $derived(tokenPrincipal === CANISTER_IDS.ICUSD_LEDGER);
  const isThreeUsd = $derived(tokenPrincipal === CANISTER_IDS.THREEPOOL);
  const isStable = $derived(isIcUsd || isThreeUsd);
  const isCollateral = $derived(
    configs.some((c: any) => {
      const pid = c.ledger_canister_id?.toText?.() ?? String(c.ledger_canister_id ?? '');
      return pid === tokenPrincipal;
    }),
  );

  // ── Derived: total supply ──────────────────────────────────────────────────

  const totalSupplyE8s = $derived.by<bigint>(() => {
    if (topHolders && topHolders.total_supply_e8s > 0n) return topHolders.total_supply_e8s;
    if (liveTotalSupply != null) return liveTotalSupply;
    return 0n;
  });

  // ── Derived: price ─────────────────────────────────────────────────────────

  const priceUsd = $derived.by<number>(() => {
    if (isIcUsd) return 1;
    if (isThreeUsd) {
      const vp = threePoolState?.virtual_price ? Number(threePoolState.virtual_price) / 1e18 : 0;
      return vp || 1;
    }
    return priceMap.get(tokenPrincipal) ?? 0;
  });

  // ── Derived: peg badge for stables ─────────────────────────────────────────

  const pegDeviationPct = $derived.by<number | null>(() => {
    if (!isStable || !pegStatus) return null;
    const max = Number(pegStatus.max_imbalance_pct ?? 0);
    if (!Number.isFinite(max)) return null;
    return max;
  });

  const pegBadgeColor = $derived.by<string>(() => {
    if (pegDeviationPct == null) return '';
    if (pegDeviationPct < 1) return 'text-emerald-400 bg-emerald-500/10 border-emerald-500/30';
    if (pegDeviationPct < 3) return 'text-amber-400 bg-amber-500/10 border-amber-500/30';
    return 'text-red-400 bg-red-500/10 border-red-500/30';
  });

  // ── Derived: pools trading this token ──────────────────────────────────────

  interface PoolRow {
    href: string;
    label: string;
    tvlUsd: number | null;
    isThreePool: boolean;
  }

  const poolRows = $derived.by<PoolRow[]>(() => {
    const rows: PoolRow[] = [];

    // 3pool: legs are icUSD, ckUSDT, ckUSDC.
    const threePoolLegs = new Set<string>([
      CANISTER_IDS.ICUSD_LEDGER,
      CANISTER_IDS.CKUSDT_LEDGER,
      CANISTER_IDS.CKUSDC_LEDGER,
    ]);
    if (threePoolLegs.has(tokenPrincipal)) {
      rows.push({
        href: '/explorer/e/pool/3pool',
        label: 'Rumi 3Pool',
        tvlUsd: null,
        isThreePool: true,
      });
    }

    for (const pool of ammPools) {
      const tokenA = pool.token_a?.toText?.() ?? String(pool.token_a ?? '');
      const tokenB = pool.token_b?.toText?.() ?? String(pool.token_b ?? '');
      if (tokenA !== tokenPrincipal && tokenB !== tokenPrincipal) continue;
      const poolId = String(pool.pool_id ?? '');
      const decA = getTokenDecimals(tokenA);
      const decB = getTokenDecimals(tokenB);
      const priceA = priceMap.get(tokenA) ?? (tokenA === CANISTER_IDS.ICUSD_LEDGER ? 1 : 0);
      const priceB = priceMap.get(tokenB) ?? (tokenB === CANISTER_IDS.ICUSD_LEDGER ? 1 : 0);
      const tvl =
        (Number(BigInt(pool.reserve_a ?? 0)) / 10 ** decA) * priceA +
        (Number(BigInt(pool.reserve_b ?? 0)) / 10 ** decB) * priceB;
      rows.push({
        href: `/explorer/e/pool/${poolId}`,
        label: `${getTokenSymbol(tokenA)}/${getTokenSymbol(tokenB)}`,
        tvlUsd: tvl,
        isThreePool: false,
      });
    }
    return rows;
  });

  // ── Derived: token flow in/out strip ───────────────────────────────────────

  interface FlowRow {
    otherToken: string;
    symbol: string;
    href: string;
    volumeUsdE8s: bigint;
    swapCount: bigint;
  }

  /** Edges where this token is the `to_token` — who sent it in. */
  const inboundFlows = $derived.by<FlowRow[]>(() => {
    const rows = flowEdges
      .filter((e) => e.to_token.toText() === tokenPrincipal)
      .map<FlowRow>((e) => {
        const other = e.from_token.toText();
        return {
          otherToken: other,
          symbol: getTokenSymbol(other),
          href: `/explorer/e/token/${other}`,
          volumeUsdE8s: e.volume_usd_e8s,
          swapCount: e.swap_count,
        };
      });
    rows.sort((a, b) => (b.volumeUsdE8s > a.volumeUsdE8s ? 1 : b.volumeUsdE8s < a.volumeUsdE8s ? -1 : 0));
    return rows.slice(0, 10);
  });

  /** Edges where this token is the `from_token` — where it went. */
  const outboundFlows = $derived.by<FlowRow[]>(() => {
    const rows = flowEdges
      .filter((e) => e.from_token.toText() === tokenPrincipal)
      .map<FlowRow>((e) => {
        const other = e.to_token.toText();
        return {
          otherToken: other,
          symbol: getTokenSymbol(other),
          href: `/explorer/e/token/${other}`,
          volumeUsdE8s: e.volume_usd_e8s,
          swapCount: e.swap_count,
        };
      });
    rows.sort((a, b) => (b.volumeUsdE8s > a.volumeUsdE8s ? 1 : b.volumeUsdE8s < a.volumeUsdE8s ? -1 : 0));
    return rows.slice(0, 10);
  });

  function formatFlowUsd(e8s: bigint): string {
    const n = Number(e8s) / 1e8;
    if (n >= 1_000_000) return `$${(n / 1_000_000).toFixed(2)}M`;
    if (n >= 1_000) return `$${(n / 1_000).toFixed(2)}k`;
    return `$${n.toFixed(2)}`;
  }

  // ── Derived: vaults using this token ───────────────────────────────────────

  interface VaultUsage {
    label: string;
    description: string;
    rows: Array<{ id: number; href: string; secondary: string }>;
  }

  const vaultUsage = $derived.by<VaultUsage>(() => {
    if (isIcUsd) {
      // icUSD is the debt token — count vaults with non-zero debt.
      const rows = allVaults
        .filter((v: any) => {
          const debt = BigInt(v.borrowed_icusd_amount ?? 0) + BigInt(v.accrued_interest ?? 0);
          return debt > 0n;
        })
        .map((v: any) => {
          const debt = BigInt(v.borrowed_icusd_amount ?? 0) + BigInt(v.accrued_interest ?? 0);
          const id = Number(v.vault_id);
          return {
            id,
            href: `/explorer/e/vault/${id}`,
            secondary: `${formatE8s(debt, 8)} icUSD debt`,
          };
        })
        .sort((a, b) => Number(b.id) - Number(a.id));
      return {
        label: `Debt token across ${rows.length} vault${rows.length === 1 ? '' : 's'}`,
        description: 'Vaults that have minted icUSD against collateral.',
        rows,
      };
    }

    if (isCollateral) {
      const rows = allVaults
        .filter((v: any) => {
          const ct = v.collateral_type?.toText?.() ?? String(v.collateral_type ?? '');
          return ct === tokenPrincipal;
        })
        .map((v: any) => {
          const id = Number(v.vault_id);
          const collDecimals = getTokenDecimals(tokenPrincipal);
          const amt = BigInt(v.collateral_amount ?? 0);
          return {
            id,
            href: `/explorer/e/vault/${id}`,
            secondary: `${formatE8s(amt, collDecimals)} ${symbol}`,
          };
        })
        .sort((a, b) => Number(b.id) - Number(a.id));
      return {
        label: `Used as collateral in ${rows.length} vault${rows.length === 1 ? '' : 's'}`,
        description: 'Vaults that have deposited this token as collateral.',
        rows,
      };
    }

    return {
      label: 'Not used in any vault',
      description: 'This token is neither collateral nor a debt token.',
      rows: [],
    };
  });

  // ── Derived: top holders rows for table ────────────────────────────────────

  interface HolderRow {
    rank: number;
    principal: string;
    balance: bigint;
    sharePct: number;
  }

  const holderRows = $derived.by<HolderRow[]>(() => {
    if (!topHolders) return [];
    return topHolders.rows.slice(0, 10).map((row, i) => ({
      rank: i + 1,
      principal: row.principal.toText(),
      balance: row.balance_e8s,
      sharePct: Number(row.share_bps) / 100,
    }));
  });

  const holdersTracked = $derived(topHolders ? topHolders.source === 'balance_tracker' : false);

  // ── Derived: holder distribution pie (top 10 + long tail) ─────────────────

  interface PieSlice {
    label: string;
    value: number;
    color: string;
    pct: number;
  }

  // Cap the pie at 10 named slices so the legend stays readable. Anything
  // beyond the top 10 is collapsed into a "Long tail" wedge so total still
  // sums to 100% of the supply.
  const PIE_PALETTE = [
    '#34d399', '#818cf8', '#fbbf24', '#f472b6', '#60a5fa',
    '#a78bfa', '#22d3ee', '#facc15', '#fb7185', '#4ade80',
  ];

  const pieSlices = $derived.by<PieSlice[]>(() => {
    if (!topHolders || topHolders.source !== 'balance_tracker') return [];
    const supply = totalSupplyE8s;
    if (supply <= 0n) return [];

    const top10 = topHolders.rows.slice(0, 10);
    const slices: PieSlice[] = top10.map((r, i) => {
      const value = Number(r.balance_e8s);
      const pct = (value / Number(supply)) * 100;
      return {
        label: shortenPrincipal(r.principal.toText(), 4),
        value,
        color: PIE_PALETTE[i] ?? '#9ca3af',
        pct,
      };
    });

    const top10Sum = top10.reduce((s, r) => s + Number(r.balance_e8s), 0);
    const tailValue = Number(supply) - top10Sum;
    const tailHolders = (topHolders.total_holders ?? 0) - top10.length;
    if (tailValue > 0 && tailHolders > 0) {
      slices.push({
        label: `Long tail (${tailHolders} holders)`,
        value: tailValue,
        color: '#4b5563',
        pct: (tailValue / Number(supply)) * 100,
      });
    }
    return slices;
  });

  const pieTotal = $derived(pieSlices.reduce((s, x) => s + x.value, 0));

  const pieArcs = $derived.by<Array<{ slice: PieSlice; dasharray: string; dashoffset: number }>>(() => {
    if (pieTotal <= 0) return [];
    const r = 60;
    const C = 2 * Math.PI * r;
    let offset = 0;
    return pieSlices
      .filter((s) => s.value > 0)
      .map((slice) => {
        const len = (slice.value / pieTotal) * C;
        const arc = { slice, dasharray: `${len} ${C - len}`, dashoffset: -offset };
        offset += len;
        return arc;
      });
  });

  // ── Derived: supply over time series ───────────────────────────────────────

  interface SupplyPoint {
    timestamp: number;
    supplyE8s: number;
  }

  const supplySeries = $derived.by<SupplyPoint[]>(() => {
    if (isIcUsd) {
      // Vault series: total_debt_e8s sums all icUSD debt = supply minted
      // through vaults. This understates real supply if mints happen outside
      // vault flow, but matches the protocol-level mint trace.
      return vaultSeries.map((row: any) => ({
        timestamp: Number(row.timestamp_ns ?? 0),
        supplyE8s: Number(row.total_debt_e8s ?? 0),
      }));
    }
    if (isThreeUsd) {
      return threePoolHistory.map((row: any) => ({
        timestamp: Number(row.timestamp_ns ?? 0),
        supplyE8s: Number(row.lp_total_supply ?? 0),
      }));
    }
    if (holderSeries.length > 0) {
      return holderSeries.map((row: any) => ({
        timestamp: Number(row.timestamp_ns ?? 0),
        supplyE8s: Number(row.total_supply_tracked_e8s ?? 0),
      }));
    }
    return [];
  });

  function buildSparklinePath(points: SupplyPoint[], width: number, height: number): string {
    if (points.length < 2) return '';
    const ys = points.map((p) => p.supplyE8s);
    const minY = Math.min(...ys);
    const maxY = Math.max(...ys);
    const yRange = maxY - minY || 1;
    const xStep = width / (points.length - 1);
    return points
      .map((p, i) => {
        const x = i * xStep;
        const y = height - ((p.supplyE8s - minY) / yRange) * height;
        return `${i === 0 ? 'M' : 'L'} ${x.toFixed(2)} ${y.toFixed(2)}`;
      })
      .join(' ');
  }

  // ── Derived: activity stream filtered to this token ────────────────────────

  /**
   * Index allVaults so token-extraction in extractFacets can resolve
   * vault-relative event types (redemption_on_vaults etc.) to the actual
   * collateral token they touch.
   */
  const vaultCollateralMap = $derived.by(() => {
    const m = new Map<number, string>();
    for (const v of allVaults) {
      const id = Number(v.vault_id);
      const ct = v.collateral_type?.toText?.() ?? String(v.collateral_type ?? '');
      if (ct) m.set(id, ct);
    }
    return m;
  });

  const allEvents = $derived.by<DisplayEvent[]>(() => {
    const out: DisplayEvent[] = [];

    for (const [gi, ev] of backendEvents) {
      out.push({
        globalIndex: gi,
        event: ev,
        source: 'backend',
        timestamp: extractEventTimestamp(ev),
      });
    }
    for (const ev of threePoolSwapEvents) {
      out.push({
        globalIndex: BigInt(ev.id ?? 0),
        event: ev,
        source: '3pool_swap',
        timestamp: Number(ev.timestamp ?? 0),
      });
    }
    for (const ev of ammSwapEvents) {
      out.push({
        globalIndex: BigInt(ev.id ?? 0),
        event: ev,
        source: 'amm_swap',
        timestamp: Number(ev.timestamp ?? 0),
      });
    }
    for (const ev of ammLiqEvents) {
      out.push({
        globalIndex: BigInt(ev.id ?? 0),
        event: ev,
        source: 'amm_liquidity',
        timestamp: Number(ev.timestamp ?? 0),
      });
    }
    for (const ev of spEvents) {
      out.push({
        globalIndex: BigInt(ev.id ?? 0),
        event: ev,
        source: 'stability_pool',
        timestamp: extractEventTimestamp(ev),
      });
    }

    out.sort((a, b) => b.timestamp - a.timestamp);
    return out;
  });

  const tokenEvents = $derived.by<DisplayEvent[]>(() => {
    return allEvents.filter((de) => {
      const facets = extractFacets(de, priceMap, vaultCollateralMap);
      return facets.tokens.includes(tokenPrincipal);
    });
  });

  const activityPreview = $derived(tokenEvents.slice(0, 50));

  const seeAllHref = $derived.by<string>(() => {
    const tokenParam = requestedAlias ?? tokenPrincipal;
    return `/explorer/activity?token=${encodeURIComponent(tokenParam)}`;
  });

  // ── Data load ──────────────────────────────────────────────────────────────

  /** Bound how many backend events we pull before client-side filtering. */
  const BACKEND_EVENT_PULL = 500n;
  const AMM_EVENT_PULL = 500n;
  const SP_EVENT_PULL = 500n;
  const THREE_POOL_SWAP_PULL = 500n;

  async function loadToken() {
    loading = true;
    error = null;
    let tokenPrincipalObj: Principal;
    try {
      tokenPrincipalObj = Principal.fromText(tokenPrincipal);
    } catch {
      error = `Invalid token principal: "${tokenPrincipal}"`;
      loading = false;
      return;
    }

    try {
      const [
        configsRes,
        pricesRes,
        allVaultsRes,
        ammPoolsRes,
        threePoolStateRes,
        topHoldersRes,
        pegRes,
        flowRes,
      ] = await Promise.all([
        fetchCollateralConfigs().catch(() => []),
        fetchCollateralPrices().catch(() => new Map<string, number>()),
        fetchAllVaults().catch(() => []),
        fetchAmmPools().catch(() => []),
        fetchThreePoolState().catch(() => null),
        fetchTopHolders(tokenPrincipalObj, 50),
        fetchPegStatus().catch(() => null),
        // 7-day window, big enough limit to catch most edges the token participates in.
        fetchTokenFlow(7n * 86_400n * 1_000_000_000n, undefined, 200)
          .then((r) => r.edges)
          .catch(() => [] as TokenFlowEdge[]),
      ]);

      configs = configsRes;
      priceMap = pricesRes;
      allVaults = allVaultsRes;
      ammPools = ammPoolsRes;
      threePoolState = threePoolStateRes;
      flowEdges = flowRes;
      topHolders = topHoldersRes;
      pegStatus = pegRes;

      // Live total supply for tokens analytics doesn't track. icUSD/3USD
      // already get total_supply from the holder snapshot, so skip the call.
      if (topHoldersRes.source !== 'balance_tracker') {
        liveTotalSupply = await fetchIcrc1TotalSupply(tokenPrincipal).catch(() => null);
      }

      // Activity sources — pulled in parallel.
      const eventCount = await fetchEventCount().catch(() => 0n);
      const eventStart = eventCount > BACKEND_EVENT_PULL ? eventCount - BACKEND_EVENT_PULL : 0n;
      const eventLength = eventCount > BACKEND_EVENT_PULL ? BACKEND_EVENT_PULL : eventCount;

      const ammSwapCount = await fetchAmmSwapEventCount().catch(() => 0n);
      const ammSwapStart = ammSwapCount > AMM_EVENT_PULL ? ammSwapCount - AMM_EVENT_PULL : 0n;
      const ammSwapLen = ammSwapCount > AMM_EVENT_PULL ? AMM_EVENT_PULL : ammSwapCount;

      const ammLiqCount = await fetchAmmLiquidityEventCount().catch(() => 0n);
      const ammLiqStart = ammLiqCount > AMM_EVENT_PULL ? ammLiqCount - AMM_EVENT_PULL : 0n;
      const ammLiqLen = ammLiqCount > AMM_EVENT_PULL ? AMM_EVENT_PULL : ammLiqCount;

      const spCount = await fetchStabilityPoolEventCount().catch(() => 0n);
      const spStart = spCount > SP_EVENT_PULL ? spCount - SP_EVENT_PULL : 0n;
      const spLen = spCount > SP_EVENT_PULL ? SP_EVENT_PULL : spCount;

      const [backendRes, threePoolSwapsRes, ammSwapsRes, ammLiqRes, spRes] = await Promise.all([
        eventLength > 0n
          ? fetchEvents(eventStart, eventLength).catch(() => ({ total: 0n, events: [] as [bigint, any][] }))
          : Promise.resolve({ total: 0n, events: [] as [bigint, any][] }),
        fetchThreePoolSwapEventsV2(0n, THREE_POOL_SWAP_PULL).catch(() => []),
        ammSwapLen > 0n ? fetchAmmSwapEvents(ammSwapStart, ammSwapLen).catch(() => []) : Promise.resolve([]),
        ammLiqLen > 0n ? fetchAmmLiquidityEvents(ammLiqStart, ammLiqLen).catch(() => []) : Promise.resolve([]),
        spLen > 0n ? fetchStabilityPoolEvents(spStart, spLen).catch(() => []) : Promise.resolve([]),
      ]);

      backendEvents = backendRes.events;
      threePoolSwapEvents = threePoolSwapsRes;
      ammSwapEvents = ammSwapsRes;
      ammLiqEvents = ammLiqRes;
      spEvents = spRes;

      // Analytics history charts — only fetch the relevant series for this
      // token's chart story so we don't blow the request budget.
      if (isIcUsd) {
        vaultSeries = await fetchVaultSeries(180).catch(() => []);
      } else if (isThreeUsd) {
        threePoolHistory = await fetchThreePoolSeries(500).catch(() => []);
      } else {
        // For unsupported tokens, holder series is the best supply proxy if it
        // exists. For collateral tokens it won't, so the chart will hide.
        try {
          holderSeries = await fetchHolderSeries(tokenPrincipalObj, 180);
        } catch {
          holderSeries = [];
        }
      }
    } catch (e: any) {
      console.error('[token page] load failed:', e);
      error = e?.message ?? 'Failed to load token data';
    } finally {
      loading = false;
    }
  }

  // Re-run on token change so sibling /e/token/[id] navigations refetch
  // rather than reusing this component with stale state.
  $effect(() => {
    void tokenPrincipal;
    loadToken();
  });
</script>

<EntityShell
  title={symbol}
  subtitle="{tokenName} · ledger {shortenPrincipal(tokenPrincipal)}"
  {loading}
  {error}
  onRetry={loadToken}
>
  {#snippet identity()}
    <div class="grid grid-cols-2 sm:grid-cols-4 gap-3">
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-3">
        <div class="text-xs text-gray-500 uppercase tracking-wider">Total supply</div>
        <div class="mt-1 text-lg font-semibold text-white">
          {totalSupplyE8s > 0n ? formatE8s(totalSupplyE8s, tokenDecimals) : '—'}
        </div>
        <div class="text-xs text-gray-500">{symbol}</div>
      </div>

      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-3">
        <div class="text-xs text-gray-500 uppercase tracking-wider">Holders</div>
        <div class="mt-1 text-lg font-semibold text-white">
          {topHolders ? topHolders.total_holders.toLocaleString() : '—'}
        </div>
        <div class="text-xs text-gray-500">
          {holdersTracked ? 'Tracked' : 'Not tracked'}
        </div>
      </div>

      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-3">
        <div class="text-xs text-gray-500 uppercase tracking-wider">Price</div>
        <div class="mt-1 text-lg font-semibold text-white">
          {priceUsd > 0 ? formatUsdRaw(priceUsd) : '—'}
        </div>
        <div class="text-xs text-gray-500">USD</div>
      </div>

      {#if isStable}
        <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-3">
          <div class="text-xs text-gray-500 uppercase tracking-wider">Peg deviation</div>
          {#if pegDeviationPct != null}
            <div class="mt-1 inline-block text-sm font-semibold px-2 py-0.5 rounded-full border {pegBadgeColor}">
              ±{pegDeviationPct.toFixed(2)}%
            </div>
            <div class="text-xs text-gray-500 mt-1">3pool max imbalance</div>
          {:else}
            <div class="mt-1 text-sm text-gray-500">—</div>
          {/if}
        </div>
      {:else}
        <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-3">
          <div class="text-xs text-gray-500 uppercase tracking-wider">Decimals</div>
          <div class="mt-1 text-lg font-semibold text-white">{tokenDecimals}</div>
          <div class="text-xs text-gray-500">ICRC-1 ledger</div>
        </div>
      {/if}
    </div>

    <div class="flex items-center gap-2 text-xs text-gray-400">
      <span class="text-gray-500">Ledger:</span>
      <code class="font-mono text-gray-300">{tokenPrincipal}</code>
      <CopyButton text={tokenPrincipal} />
    </div>

    {#if !knownToken}
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-4 text-sm text-gray-400">
        Limited data available for this token. Symbol, decimals, and price feeds
        aren't registered in the explorer; only on-chain ICRC-1 calls work.
      </div>
    {/if}
  {/snippet}

  {#snippet relationships()}
    <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
      <!-- Pools trading it -->
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-4">
        <h3 class="text-sm font-semibold text-white mb-3">Pools trading {symbol}</h3>
        {#if poolRows.length === 0}
          <div class="text-sm text-gray-500">No pools found for this token.</div>
        {:else}
          <ul class="space-y-2">
            {#each poolRows as pool (pool.href)}
              <li class="flex items-center justify-between text-sm">
                <a href={pool.href} class="text-blue-400 hover:text-blue-300 font-medium">{pool.label}</a>
                <span class="text-xs text-gray-500">
                  {pool.tvlUsd != null ? formatUsdRaw(pool.tvlUsd) : pool.isThreePool ? '3pool' : '—'}
                </span>
              </li>
            {/each}
          </ul>
        {/if}
      </div>

      <!-- Vault usage -->
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-4">
        <h3 class="text-sm font-semibold text-white mb-1">{vaultUsage.label}</h3>
        <p class="text-xs text-gray-500 mb-3">{vaultUsage.description}</p>
        {#if vaultUsage.rows.length === 0}
          <div class="text-sm text-gray-500">—</div>
        {:else}
          <ul class="space-y-1.5 max-h-64 overflow-y-auto">
            {#each vaultUsage.rows.slice(0, 20) as v (v.id)}
              <li class="flex items-center justify-between text-sm">
                <a href={v.href} class="text-blue-400 hover:text-blue-300">Vault #{v.id}</a>
                <span class="text-xs text-gray-500">{v.secondary}</span>
              </li>
            {/each}
          </ul>
          {#if vaultUsage.rows.length > 20}
            <div class="mt-2 text-xs text-gray-500">+ {vaultUsage.rows.length - 20} more</div>
          {/if}
        {/if}
      </div>

      <!-- Top holders -->
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-4">
        <h3 class="text-sm font-semibold text-white mb-3">Top holders</h3>
        {#if !holdersTracked}
          <div class="text-sm text-gray-500">
            Top holders aren't tracked for this token yet.
          </div>
        {:else if holderRows.length === 0}
          <div class="text-sm text-gray-500">No holders.</div>
        {:else}
          <table class="w-full text-sm">
            <thead>
              <tr class="text-left text-xs uppercase tracking-wider text-gray-500 border-b border-gray-700/50">
                <th class="py-1.5 pr-2 w-10">#</th>
                <th class="py-1.5 pr-2">Holder</th>
                <th class="py-1.5 pr-2 text-right">Balance</th>
                <th class="py-1.5 text-right w-16">Share</th>
              </tr>
            </thead>
            <tbody>
              {#each holderRows as row (row.principal)}
                <tr class="border-b border-gray-800/40">
                  <td class="py-1.5 pr-2 text-xs text-gray-500">{row.rank}</td>
                  <td class="py-1.5 pr-2">
                    <EntityLink type="address" value={row.principal} label={shortenPrincipal(row.principal, 5)} />
                  </td>
                  <td class="py-1.5 pr-2 text-right text-gray-300 font-mono text-xs">
                    {formatE8s(row.balance, tokenDecimals)}
                  </td>
                  <td class="py-1.5 text-right text-gray-400 text-xs">{row.sharePct.toFixed(2)}%</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      </div>

      <!-- Top movers 24h (Tier 2 backend gap) -->
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-4">
        <div class="flex items-center gap-1.5 mb-3">
          <h3 class="text-sm font-semibold text-white">Top movers 24h</h3>
          <span
            class="text-xs text-gray-500 cursor-help"
            title="Computing top movers needs a per-principal balance delta endpoint that doesn't exist yet — it would have to scan inflow and outflow transfers per holder over a window. Tracked as a separate backend task."
          >
            ⓘ
          </span>
        </div>
        <div class="text-sm text-gray-500">
          Coming with movers backend endpoint.
        </div>
      </div>

      <!-- Flows in (7d) -->
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-4">
        <h3 class="text-sm font-semibold text-white mb-3">Flows in (7d)</h3>
        {#if inboundFlows.length === 0}
          <div class="text-sm text-gray-500">No inbound swap flows in the last 7 days.</div>
        {:else}
          <ul class="space-y-1.5">
            {#each inboundFlows as row (row.otherToken)}
              <li class="flex items-center justify-between text-sm">
                <a href={row.href} class="text-blue-400 hover:text-blue-300 font-medium">{row.symbol}</a>
                <span class="text-xs text-gray-500 tabular-nums">
                  {formatFlowUsd(row.volumeUsdE8s)}
                  <span class="text-gray-600"> · {row.swapCount.toString()}</span>
                </span>
              </li>
            {/each}
          </ul>
        {/if}
      </div>

      <!-- Flows out (7d) -->
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-4">
        <h3 class="text-sm font-semibold text-white mb-3">Flows out (7d)</h3>
        {#if outboundFlows.length === 0}
          <div class="text-sm text-gray-500">No outbound swap flows in the last 7 days.</div>
        {:else}
          <ul class="space-y-1.5">
            {#each outboundFlows as row (row.otherToken)}
              <li class="flex items-center justify-between text-sm">
                <a href={row.href} class="text-blue-400 hover:text-blue-300 font-medium">{row.symbol}</a>
                <span class="text-xs text-gray-500 tabular-nums">
                  {formatFlowUsd(row.volumeUsdE8s)}
                  <span class="text-gray-600"> · {row.swapCount.toString()}</span>
                </span>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </div>
  {/snippet}

  {#snippet activity()}
    <div class="flex items-center justify-between">
      <div class="text-xs text-gray-500">
        {tokenEvents.length} event{tokenEvents.length === 1 ? '' : 's'} touching {symbol}
      </div>
      <a href={seeAllHref} class="text-xs text-blue-400 hover:text-blue-300">See all in Activity →</a>
    </div>
    {#if activityPreview.length === 0}
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-4 text-sm text-gray-500">
        No activity touching {symbol} in the last {BACKEND_EVENT_PULL.toString()} backend events.
      </div>
    {:else}
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 overflow-hidden">
        <MixedEventsTable events={activityPreview} {vaultCollateralMap} />
      </div>
    {/if}
  {/snippet}

  {#snippet analytics()}
    <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
      <!-- Holder distribution pie -->
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-4">
        <h3 class="text-sm font-semibold text-white mb-3">Holder distribution</h3>
        {#if !holdersTracked}
          <div class="text-sm text-gray-500">
            Holder distribution isn't tracked for this token yet.
          </div>
        {:else if pieSlices.length === 0}
          <div class="text-sm text-gray-500">No data.</div>
        {:else}
          <div class="flex items-center gap-4">
            <svg viewBox="-70 -70 140 140" class="w-32 h-32 -rotate-90 flex-shrink-0">
              {#each pieArcs as arc (arc.slice.label)}
                <circle
                  r="60"
                  cx="0"
                  cy="0"
                  fill="none"
                  stroke={arc.slice.color}
                  stroke-width="20"
                  stroke-dasharray={arc.dasharray}
                  stroke-dashoffset={arc.dashoffset}
                />
              {/each}
            </svg>
            <ul class="flex-1 space-y-1 text-xs">
              {#each pieSlices as s (s.label)}
                <li class="flex items-center gap-2">
                  <span class="inline-block w-2.5 h-2.5 rounded-sm" style="background: {s.color}"></span>
                  <span class="text-gray-300 truncate flex-1">{s.label}</span>
                  <span class="text-gray-500 tabular-nums">{s.pct.toFixed(1)}%</span>
                </li>
              {/each}
            </ul>
          </div>
        {/if}
      </div>

      <!-- Supply over time -->
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-4">
        <h3 class="text-sm font-semibold text-white mb-3">
          {isIcUsd ? 'Supply over time (vault debt)' : isThreeUsd ? 'LP supply over time' : 'Tracked supply over time'}
        </h3>
        {#if supplySeries.length < 2}
          <div class="text-sm text-gray-500">
            {isCollateral
              ? 'Supply timeseries not collected for collateral tokens.'
              : 'Not enough data points yet.'}
          </div>
        {:else}
          {@const latest = supplySeries[supplySeries.length - 1]}
          {@const first = supplySeries[0]}
          {@const change = latest.supplyE8s - first.supplyE8s}
          {@const pctChange = first.supplyE8s > 0 ? (change / first.supplyE8s) * 100 : 0}
          <div class="flex items-baseline justify-between mb-2">
            <span class="text-lg font-semibold text-white tabular-nums">
              {formatE8s(BigInt(Math.round(latest.supplyE8s)), tokenDecimals)}
            </span>
            <span class="text-xs {change >= 0 ? 'text-emerald-400' : 'text-red-400'}">
              {change >= 0 ? '+' : ''}{pctChange.toFixed(2)}% over period
            </span>
          </div>
          <svg viewBox="0 0 320 80" class="w-full h-20">
            <path
              d={buildSparklinePath(supplySeries, 320, 80)}
              fill="none"
              stroke="#34d399"
              stroke-width="1.5"
            />
          </svg>
          <div class="flex justify-between text-[10px] text-gray-500 mt-1">
            <span>{new Date(first.timestamp / 1_000_000).toLocaleDateString()}</span>
            <span>{new Date(latest.timestamp / 1_000_000).toLocaleDateString()}</span>
          </div>
        {/if}
      </div>

      <!-- Flow chart placeholder (Tier 2 backend gap #5) -->
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-4">
        <h3 class="text-sm font-semibold text-white mb-3">Token flow</h3>
        <div class="text-sm text-gray-500">
          Token flow visualization coming with the flow aggregator backend endpoint.
          Will show movement of {symbol} between Stability Pool, AMM pools, and wallets.
        </div>
      </div>

      <!-- Peg deviation timeseries -->
      <div class="rounded-lg border border-gray-700/60 bg-gray-900/40 p-4">
        <h3 class="text-sm font-semibold text-white mb-3">Peg deviation</h3>
        {#if !isStable}
          <div class="text-sm text-gray-500">
            Peg deviation only applies to stablecoins (icUSD, 3USD).
          </div>
        {:else if pegStatus == null}
          <div class="text-sm text-gray-500">No peg data available.</div>
        {:else}
          <div class="space-y-2">
            <div class="text-3xl font-semibold text-white tabular-nums">
              ±{pegDeviationPct?.toFixed(2) ?? '—'}%
            </div>
            <div class="text-xs text-gray-500">
              Latest 3pool max imbalance · {new Date(Number(pegStatus.timestamp_ns) / 1_000_000).toLocaleString()}
            </div>
            <div class="text-xs text-gray-600 italic mt-2">
              Historical series coming once analytics exposes peg history.
            </div>
          </div>
        {/if}
      </div>
    </div>
  {/snippet}
</EntityShell>
