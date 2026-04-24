<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import EntityShell from '$components/explorer/entity/EntityShell.svelte';
  import CRDial from '$components/explorer/entity/CRDial.svelte';
  import MixedEventsTable from '$components/explorer/MixedEventsTable.svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import CopyButton from '$components/explorer/CopyButton.svelte';
  import StatusBadge from '$components/explorer/StatusBadge.svelte';
  import TimeAgo from '$components/explorer/TimeAgo.svelte';
  import PortfolioValueChart from '$components/explorer/PortfolioValueChart.svelte';
  import {
    fetchVaultsByOwner,
    fetchEventsByPrincipal,
    fetchCollateralConfigs,
    fetchCollateralPrices,
    fetchAllVaults,
    fetchAmmPools,
    fetchThreePoolState,
    fetch3PoolLpBalance,
    fetchAmmLpBalance,
    fetch3PoolSwapEventsByPrincipal,
    fetch3PoolLiquidityEventsByPrincipal,
    fetchIcrc1BalanceOf,
    fetchIcusdSubaccounts,
    fetchThreeUsdSubaccounts,
    fetchStabilityPoolEvents,
    fetchStabilityPoolEventCount,
    fetchAmmSwapEvents,
    fetchAmmSwapEventCount,
    fetchAmmLiquidityEvents,
    fetchAmmLiquidityEventCount,
  } from '$services/explorer/explorerService';
  import {
    formatE8s,
    formatUsdRaw,
    getTokenSymbol,
    getTokenDecimals,
    shortenPrincipal,
    getCanisterName,
    isKnownCanister,
  } from '$utils/explorerHelpers';
  import { extractEventTimestamp } from '$utils/displayEvent';
  import type { DisplayEvent } from '$utils/displayEvent';
  import { extractFacets } from '$utils/eventFacets';
  import { CANISTER_IDS } from '$lib/config';
  import { fetchTopCounterparties } from '$services/explorer/analyticsService';
  import type { TopCounterpartyRow } from '$declarations/rumi_analytics/rumi_analytics.did';

  const principalStr = $derived($page.params.principal);

  let loading = $state(true);
  let error: string | null = $state(null);

  let vaults: any[] = $state([]);
  let allVaults: any[] = $state([]);
  let configs: any[] = $state([]);
  let priceMap = $state<Map<string, number>>(new Map());
  let threePoolState: any = $state(null);
  let threePoolLp = $state<bigint>(0n);
  let ammPools: any[] = $state([]);
  let ammLpBalances = $state<Map<string, bigint>>(new Map());
  let tokenBalances = $state<Map<string, bigint>>(new Map());
  let backendEvents: [bigint, any][] = $state([]);
  let threePoolSwapEvents: any[] = $state([]);
  let threePoolLiqEvents: any[] = $state([]);
  let spEvents: any[] = $state([]);
  let ammSwapEventsMatching: any[] = $state([]);
  let ammLiqEventsMatching: any[] = $state([]);
  let icusdSubaccounts: Array<Uint8Array | number[]> = $state([]);
  let threeUsdSubaccounts: Array<Uint8Array | number[]> = $state([]);

  // ── Server-ranked counterparties (analytics) ───────────────────────────────

  /** Window selector for the analytics counterparties strip. `null` = all-time. */
  type CpWindow = '7d' | '30d' | '90d' | 'all';
  let cpWindow: CpWindow = $state('30d');
  let cpRows: TopCounterpartyRow[] = $state([]);
  let cpLoading = $state(false);

  const CP_WINDOW_NS: Record<CpWindow, bigint | undefined> = {
    '7d': 7n * 86_400n * 1_000_000_000n,
    '30d': 30n * 86_400n * 1_000_000_000n,
    '90d': 90n * 86_400n * 1_000_000_000n,
    // u64::MAX selects "everything" without special-casing on the backend.
    all: (1n << 63n) - 1n,
  };

  async function loadCounterparties(target: string, win: CpWindow) {
    if (!target) return;
    cpLoading = true;
    try {
      const resp = await fetchTopCounterparties(
        Principal.fromText(target),
        CP_WINDOW_NS[win],
        10,
      );
      cpRows = resp.rows;
    } catch (err) {
      console.error('[address page] loadCounterparties failed:', err);
      cpRows = [];
    } finally {
      cpLoading = false;
    }
  }

  $effect(() => {
    loadCounterparties(principalStr, cpWindow);
  });

  // ── Canister identity ──────────────────────────────────────────────────────

  const knownCanister = $derived(isKnownCanister(principalStr));
  const canisterName = $derived(getCanisterName(principalStr));

  // ── Vault-derived values ───────────────────────────────────────────────────

  function configFor(collateralPrincipal: string): any | null {
    return configs.find((c: any) => {
      const pid = c.ledger_canister_id?.toText?.() ?? String(c.ledger_canister_id ?? '');
      return pid === collateralPrincipal;
    }) ?? null;
  }

  function liquidationRatioOf(cfg: any): number {
    if (!cfg?.liquidation_ratio) return 1.1;
    const raw = cfg.liquidation_ratio;
    if (Array.isArray(raw)) return Number(raw) || 1.1;
    return Number(raw) || 1.1;
  }

  interface VaultRow {
    id: number;
    collPrincipal: string;
    collSymbol: string;
    collDecimals: number;
    collAmount: bigint;
    collUsd: number;
    debtE8s: bigint;
    debtUsd: number;
    cr: number;
    liqCR: number;
    status: 'Active' | 'Closed' | 'Liquidated';
    closedAtNs: number | null;
  }

  function vaultStatusKey(v: any): string {
    if (!v?.status) return 'active';
    const key = Object.keys(v.status)[0];
    return (key ?? '').toLowerCase();
  }

  const vaultRows = $derived<VaultRow[]>(
    vaults.map((v: any) => {
      const collPrincipal = v.collateral_type?.toText?.() ?? String(v.collateral_type ?? '');
      const collDecimals = getTokenDecimals(collPrincipal);
      const collAmount = BigInt(v.collateral_amount ?? 0);
      const price = priceMap.get(collPrincipal) ?? 0;
      const collUsd = (Number(collAmount) / 10 ** collDecimals) * price;
      const debtE8s = BigInt(v.borrowed_icusd_amount ?? 0) + BigInt(v.accrued_interest ?? 0);
      const debtUsd = Number(debtE8s) / 1e8;
      const cr = debtUsd > 0 ? collUsd / debtUsd : Infinity;
      const cfg = configFor(collPrincipal);
      const liqCR = liquidationRatioOf(cfg);
      const statusKey = vaultStatusKey(v);
      const status: VaultRow['status'] =
        statusKey === 'closed' ? 'Closed' : statusKey === 'liquidated' ? 'Liquidated' : 'Active';
      return {
        id: Number(v.vault_id),
        collPrincipal,
        collSymbol: getTokenSymbol(collPrincipal),
        collDecimals,
        collAmount,
        collUsd,
        debtE8s,
        debtUsd,
        cr,
        liqCR,
        status,
        closedAtNs: null,
      };
    }),
  );

  const activeVaultRows = $derived(vaultRows.filter((r) => r.status === 'Active'));
  const totalVaultEquityUsd = $derived(
    vaultRows.reduce((sum, r) => sum + Math.max(0, r.collUsd - r.debtUsd), 0),
  );

  // ── LP positions ───────────────────────────────────────────────────────────

  interface LpRow {
    poolHref: string;
    poolLabel: string;
    poolKey: string;
    sharePct: number;
    valueUsd: number;
    feesUnclaimed: string;
    lpBalance: bigint;
  }

  const threePoolVirtualPrice = $derived.by<number>(() => {
    if (!threePoolState?.virtual_price) return 1;
    const raw = Number(threePoolState.virtual_price);
    if (!Number.isFinite(raw) || raw <= 0) return 1;
    // 3pool virtual_price is stored with 18-decimal precision (see rumi_3pool::math::virtual_price).
    return raw / 1e18;
  });

  const threePoolTotalSupply = $derived.by<bigint>(() => {
    if (!threePoolState) return 0n;
    const ts = threePoolState.lp_total_supply ?? threePoolState.total_supply ?? 0n;
    return BigInt(ts);
  });

  const threePoolLpRow = $derived.by<LpRow | null>(() => {
    if (threePoolLp === 0n) return null;
    const totalSupply = threePoolTotalSupply;
    const sharePct =
      totalSupply > 0n
        ? (Number(threePoolLp) / Number(totalSupply)) * 100
        : 0;
    const valueUsd = (Number(threePoolLp) / 1e8) * threePoolVirtualPrice;
    return {
      poolHref: '/explorer/e/pool/3pool',
      poolLabel: 'Rumi 3Pool',
      poolKey: '3pool',
      sharePct,
      valueUsd,
      feesUnclaimed: '—',
      lpBalance: threePoolLp,
    };
  });

  const ammLpRows = $derived.by<LpRow[]>(() => {
    const out: LpRow[] = [];
    for (const pool of ammPools) {
      const poolId = String(pool.pool_id ?? '');
      const balance = ammLpBalances.get(poolId) ?? 0n;
      if (balance === 0n) continue;
      const totalShares = BigInt(pool.total_lp_shares ?? 0);
      const sharePct = totalShares > 0n ? (Number(balance) / Number(totalShares)) * 100 : 0;

      // Value each LP position by reconstructing its pro-rata share of both reserves.
      const tokenA = pool.token_a?.toText?.() ?? String(pool.token_a ?? '');
      const tokenB = pool.token_b?.toText?.() ?? String(pool.token_b ?? '');
      const decA = getTokenDecimals(tokenA);
      const decB = getTokenDecimals(tokenB);
      const priceA = priceMap.get(tokenA) ?? (tokenA === CANISTER_IDS.THREEPOOL ? 1 : 0);
      const priceB = priceMap.get(tokenB) ?? (tokenB === CANISTER_IDS.THREEPOOL ? 1 : 0);
      const shareRatio = totalShares > 0n ? Number(balance) / Number(totalShares) : 0;
      const valueA = (Number(BigInt(pool.reserve_a ?? 0)) / 10 ** decA) * shareRatio * priceA;
      const valueB = (Number(BigInt(pool.reserve_b ?? 0)) / 10 ** decB) * shareRatio * priceB;

      out.push({
        poolHref: `/explorer/e/pool/${poolId}`,
        poolLabel: `${getTokenSymbol(tokenA)}/${getTokenSymbol(tokenB)}`,
        poolKey: poolId,
        sharePct,
        valueUsd: valueA + valueB,
        feesUnclaimed: '—',
        lpBalance: balance,
      });
    }
    return out;
  });

  const lpRows = $derived<LpRow[]>(threePoolLpRow ? [threePoolLpRow, ...ammLpRows] : ammLpRows);
  const totalLpValueUsd = $derived(lpRows.reduce((s, r) => s + r.valueUsd, 0));

  // ── Token balances ─────────────────────────────────────────────────────────

  /**
   * Ledgers to query balances on. Collateral tokens + the two protocol stables
   * (icUSD, 3USD). We don't include the 3USD LP duplicate when it's already a
   * collateral token — KNOWN_TOKENS keys are unique.
   */
  const tokenLedgers = $derived.by<string[]>(() => {
    const set = new Set<string>();
    set.add(CANISTER_IDS.ICUSD_LEDGER);
    set.add(CANISTER_IDS.THREEPOOL); // 3USD LP is itself an ICRC-1 token
    for (const cfg of configs) {
      const pid = cfg.ledger_canister_id?.toText?.() ?? String(cfg.ledger_canister_id ?? '');
      if (pid) set.add(pid);
    }
    return Array.from(set);
  });

  interface TokenRow {
    principal: string;
    symbol: string;
    decimals: number;
    balance: bigint;
    valueUsd: number;
    pctOfPortfolio: number;
    delta24h: number | null;
  }

  const tokenRows = $derived.by<TokenRow[]>(() => {
    const rows: TokenRow[] = [];
    for (const ledger of tokenLedgers) {
      const balance = tokenBalances.get(ledger) ?? 0n;
      if (balance === 0n) continue;
      const decimals = getTokenDecimals(ledger);
      // 3USD LP is priced at virtual price, icUSD at $1, collateral via priceMap.
      let price: number;
      if (ledger === CANISTER_IDS.ICUSD_LEDGER) price = 1;
      else if (ledger === CANISTER_IDS.THREEPOOL) price = threePoolVirtualPrice;
      else price = priceMap.get(ledger) ?? 0;
      const valueUsd = (Number(balance) / 10 ** decimals) * price;
      rows.push({
        principal: ledger,
        symbol: getTokenSymbol(ledger),
        decimals,
        balance,
        valueUsd,
        pctOfPortfolio: 0,
        // Leaving 24h Δ blank rather than fabricate — the plan calls for a
        // per-principal balance delta, which the holder-series endpoint doesn't
        // expose. Revisit when `rumi_analytics.get_address_summary(p)` ships.
        delta24h: null,
      });
    }
    rows.sort((a, b) => b.valueUsd - a.valueUsd);
    const totalLiquid = rows.reduce((s, r) => s + r.valueUsd, 0);
    if (totalLiquid > 0) {
      for (const r of rows) r.pctOfPortfolio = (r.valueUsd / totalLiquid) * 100;
    }
    return rows;
  });

  const totalLiquidTokensUsd = $derived(tokenRows.reduce((s, r) => s + r.valueUsd, 0));
  const netValueUsd = $derived(totalVaultEquityUsd + totalLpValueUsd + totalLiquidTokensUsd);

  // ── Portfolio allocation donut ─────────────────────────────────────────────

  interface DonutSlice {
    label: string;
    value: number;
    color: string;
  }

  const donutSlices = $derived<DonutSlice[]>([
    { label: 'Vault equity', value: totalVaultEquityUsd, color: '#34d399' },
    { label: 'LP positions', value: totalLpValueUsd, color: '#818cf8' },
    { label: 'Liquid tokens', value: totalLiquidTokensUsd, color: '#fbbf24' },
  ]);

  const donutTotal = $derived(donutSlices.reduce((s, x) => s + x.value, 0));

  /**
   * SVG stroke-dasharray-based donut: compute (length, offset) pairs per slice
   * around a circle of circumference `C`. Order of slices is stable so colors
   * don't reshuffle on re-render.
   */
  const donutArcs = $derived.by<Array<{ slice: DonutSlice; dasharray: string; dashoffset: number }>>(() => {
    if (donutTotal <= 0) return [];
    const r = 60;
    const C = 2 * Math.PI * r;
    let offset = 0;
    return donutSlices
      .filter((s) => s.value > 0)
      .map((slice) => {
        const len = (slice.value / donutTotal) * C;
        const arc = {
          slice,
          dasharray: `${len} ${C - len}`,
          dashoffset: -offset,
        };
        offset += len;
        return arc;
      });
  });

  // ── Events ─────────────────────────────────────────────────────────────────

  /** Merge every by-principal event source into a single DisplayEvent stream. */
  const mergedEvents = $derived.by<DisplayEvent[]>(() => {
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

    for (const ev of threePoolLiqEvents) {
      out.push({
        globalIndex: BigInt(ev.id ?? 0),
        event: ev,
        source: '3pool_liquidity',
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

    for (const ev of ammSwapEventsMatching) {
      out.push({
        globalIndex: BigInt(ev.id ?? 0),
        event: ev,
        source: 'amm_swap',
        timestamp: Number(ev.timestamp ?? 0),
      });
    }

    for (const ev of ammLiqEventsMatching) {
      out.push({
        globalIndex: BigInt(ev.id ?? 0),
        event: ev,
        source: 'amm_liquidity',
        timestamp: Number(ev.timestamp ?? 0),
      });
    }

    out.sort((a, b) => b.timestamp - a.timestamp);
    return out;
  });

  const activityPreview = $derived(mergedEvents.slice(0, 50));
  const seeAllHref = $derived(
    `/explorer/activity?entity=${encodeURIComponent(`principal:${principalStr}`)}`,
  );

  // ── Relationships: top counterparties + canisters ──────────────────────────

  /**
   * Build vault collateral + owner maps keyed by vault id, shared with the
   * facet extractor so vault-relative events (redemption_on_vaults, etc.)
   * resolve tokens and counterparty principals correctly.
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

  const vaultOwnerMap = $derived.by(() => {
    const m = new Map<number, string>();
    for (const v of allVaults) {
      const id = Number(v.vault_id);
      const owner = v.owner?.toText?.() ?? (typeof v.owner === 'string' ? v.owner : '');
      if (owner) m.set(id, owner);
    }
    return m;
  });

  const RUMI_CANISTERS = new Set<string>([
    CANISTER_IDS.PROTOCOL,
    CANISTER_IDS.ICUSD_LEDGER,
    CANISTER_IDS.ICUSD_INDEX,
    CANISTER_IDS.STABILITY_POOL,
    CANISTER_IDS.TREASURY,
    CANISTER_IDS.THREEPOOL,
    CANISTER_IDS.RUMI_AMM,
    CANISTER_IDS.ANALYTICS,
    CANISTER_IDS.ICP_LEDGER,
    CANISTER_IDS.CKUSDT_LEDGER,
    CANISTER_IDS.CKUSDC_LEDGER,
    // Rumi infrastructure principals that aren't in CANISTER_IDS
    'nygob-3qaaa-aaaap-qttcq-cai', // liquidation bot
    'jagpu-pyaaa-aaaap-qtm6q-cai', // 3USD index
  ]);

  /**
   * Walk the merged event stream once, gather counterparties (principals that
   * are not `this` and are not rumi canisters) + canisters (rumi infra or any
   * principal that is a known canister). Counts are used to rank both lists.
   */
  const relationshipCounts = $derived.by(() => {
    const counterparties = new Map<string, number>();
    const canisters = new Map<string, number>();

    for (const de of mergedEvents) {
      const ef = extractFacets(de, priceMap, vaultCollateralMap, vaultOwnerMap);
      for (const p of ef.principals) {
        if (p === principalStr) continue;
        if (RUMI_CANISTERS.has(p) || isKnownCanister(p)) {
          canisters.set(p, (canisters.get(p) ?? 0) + 1);
        } else {
          counterparties.set(p, (counterparties.get(p) ?? 0) + 1);
        }
      }
      for (const c of ef.canisters) {
        canisters.set(c, (canisters.get(c) ?? 0) + 1);
      }

      // Also tag the originating source canister so we surface which subsystems
      // this principal has touched (backend / 3pool / AMM / SP).
      switch (de.source) {
        case 'backend':
          canisters.set(CANISTER_IDS.PROTOCOL, (canisters.get(CANISTER_IDS.PROTOCOL) ?? 0) + 1);
          break;
        case '3pool_swap':
        case '3pool_liquidity':
        case '3pool_admin':
          canisters.set(CANISTER_IDS.THREEPOOL, (canisters.get(CANISTER_IDS.THREEPOOL) ?? 0) + 1);
          break;
        case 'amm_swap':
        case 'amm_liquidity':
        case 'amm_admin':
          canisters.set(CANISTER_IDS.RUMI_AMM, (canisters.get(CANISTER_IDS.RUMI_AMM) ?? 0) + 1);
          break;
        case 'stability_pool':
          canisters.set(CANISTER_IDS.STABILITY_POOL, (canisters.get(CANISTER_IDS.STABILITY_POOL) ?? 0) + 1);
          break;
      }
    }

    const topCounterparties = Array.from(counterparties.entries())
      .sort((a, b) => b[1] - a[1])
      .slice(0, 5);
    const topCanisters = Array.from(canisters.entries())
      .sort((a, b) => b[1] - a[1])
      .slice(0, 8);

    return { topCounterparties, topCanisters };
  });

  // ── First seen + last active (derived from events) ─────────────────────────

  const firstSeenNs = $derived.by<number | null>(() => {
    if (!mergedEvents.length) return null;
    let earliest = Number.POSITIVE_INFINITY;
    for (const e of mergedEvents) {
      if (e.timestamp > 0 && e.timestamp < earliest) earliest = e.timestamp;
    }
    return earliest === Number.POSITIVE_INFINITY ? null : earliest;
  });

  const lastActiveNs = $derived.by<number | null>(() => {
    if (!mergedEvents.length) return null;
    return mergedEvents[0]?.timestamp || null;
  });

  // ── Subaccounts zone data ──────────────────────────────────────────────────

  function subaccountToHex(sub: Uint8Array | number[]): string {
    const bytes = sub instanceof Uint8Array ? sub : new Uint8Array(sub);
    return Array.from(bytes)
      .map((b) => (b as number).toString(16).padStart(2, '0'))
      .join('');
  }

  function isDefaultSubaccount(sub: Uint8Array | number[]): boolean {
    const bytes = sub instanceof Uint8Array ? sub : new Uint8Array(sub);
    return bytes.every((b) => b === 0);
  }

  const nonDefaultIcusdSubs = $derived(
    icusdSubaccounts.filter((s) => !isDefaultSubaccount(s)).map(subaccountToHex),
  );
  const nonDefaultThreeUsdSubs = $derived(
    threeUsdSubaccounts.filter((s) => !isDefaultSubaccount(s)).map(subaccountToHex),
  );

  const hasSubaccounts = $derived(
    nonDefaultIcusdSubs.length > 0 || nonDefaultThreeUsdSubs.length > 0,
  );

  // ── Data load ──────────────────────────────────────────────────────────────

  async function loadAddress() {
    loading = true;
    error = null;
    let principal: Principal;
    try {
      principal = Principal.fromText(principalStr);
    } catch {
      error = `Invalid principal: "${principalStr}"`;
      loading = false;
      return;
    }

    try {
      // Wave 1 — identity + portfolio donut. 5 parallel calls. These populate
      // the above-the-fold content; once they land, we set `loading = false`
      // and the rest of the page fills in progressively as Wave 2 completes.
      const [vaultsRes, configsRes, pricesRes, threePoolStateRes, threePoolLpRes] =
        await Promise.all([
          fetchVaultsByOwner(principal).catch(() => []),
          fetchCollateralConfigs().catch(() => []),
          fetchCollateralPrices().catch(() => new Map<string, number>()),
          fetchThreePoolState().catch(() => null),
          fetch3PoolLpBalance(principal).catch(() => 0n),
        ]);

      vaults = vaultsRes;
      configs = configsRes;
      priceMap = pricesRes;
      threePoolState = threePoolStateRes;
      threePoolLp = threePoolLpRes;
      loading = false;

      // Wave 2 — below-the-fold activity + relationships data. Runs in the
      // background without blocking the render. Sections hydrate as their
      // state variables update.
      void loadBelowFoldData(principal, configsRes);
    } catch (e) {
      console.error('[address page] Failed to load data:', e);
      error = 'Failed to load address data. The backend may be briefly unavailable.';
      loading = false;
    }
  }

  async function loadBelowFoldData(principal: Principal, configsRes: any[]) {
    try {
      // Wave 2a — 7 parallel calls. Everything the activity feed, LP
      // positions, and subaccount relationships need.
      const [
        allVaultsRes,
        ammPoolsRes,
        backendEventsRes,
        threePoolSwapsRes,
        threePoolLiqRes,
        icusdSubsRes,
        threeUsdSubsRes,
      ] = await Promise.all([
        fetchAllVaults().catch(() => []),
        fetchAmmPools().catch(() => []),
        fetchEventsByPrincipal(principal).catch(() => [] as [bigint, any][]),
        fetch3PoolSwapEventsByPrincipal(principal).catch(() => []),
        fetch3PoolLiquidityEventsByPrincipal(principal).catch(() => []),
        fetchIcusdSubaccounts(principal).catch(() => []),
        fetchThreeUsdSubaccounts(principal).catch(() => []),
      ]);

      allVaults = allVaultsRes;
      ammPools = ammPoolsRes;
      backendEvents = backendEventsRes;
      threePoolSwapEvents = threePoolSwapsRes;
      threePoolLiqEvents = threePoolLiqRes;
      icusdSubaccounts = icusdSubsRes;
      threeUsdSubaccounts = threeUsdSubsRes;

      // Wave 2b — depends on the pool + config lists above.
      const ledgerSet = new Set<string>([CANISTER_IDS.ICUSD_LEDGER, CANISTER_IDS.THREEPOOL]);
      for (const cfg of configsRes) {
        const pid = cfg.ledger_canister_id?.toText?.() ?? String(cfg.ledger_canister_id ?? '');
        if (pid) ledgerSet.add(pid);
      }

      // TODO: replace with rumi_analytics.get_address_summary(p) once it ships.
      const [ammLpPairs, tokenBalancePairs, spEventsFull, ammSwapFull, ammLiqFull] = await Promise.all([
        Promise.all(
          ammPoolsRes.map(async (pool: any) => {
            const poolId = String(pool.pool_id ?? '');
            const bal = await fetchAmmLpBalance(poolId, principal).catch(() => 0n);
            return [poolId, bal] as [string, bigint];
          }),
        ),
        Promise.all(
          Array.from(ledgerSet).map(async (ledger) => {
            const bal = await fetchIcrc1BalanceOf(ledger, principal).catch(() => 0n);
            return [ledger, bal] as [string, bigint];
          }),
        ),
        fetchStabilityPoolEventCount()
          .then((c) => (Number(c) > 0 ? fetchStabilityPoolEvents(0n, c) : []))
          .catch(() => []),
        fetchAmmSwapEventCount()
          .then((c) => (Number(c) > 0 ? fetchAmmSwapEvents(0n, c) : []))
          .catch(() => []),
        fetchAmmLiquidityEventCount()
          .then((c) => (Number(c) > 0 ? fetchAmmLiquidityEvents(0n, c) : []))
          .catch(() => []),
      ]);

      ammLpBalances = new Map(ammLpPairs);
      tokenBalances = new Map(tokenBalancePairs);

      // AMM + SP endpoints don't filter by principal server-side yet, so we do it here.
      const pid = principalStr;
      const matchesCaller = (e: any): boolean => {
        const caller = e?.caller?.toText?.() ?? e?.caller ?? null;
        if (caller === pid) return true;
        const et = e?.event_type ?? {};
        const k = Object.keys(et)[0];
        const user = k ? et[k]?.user?.toText?.() ?? et[k]?.user ?? null : null;
        return user === pid;
      };
      spEvents = spEventsFull.filter(matchesCaller);
      ammSwapEventsMatching = ammSwapFull.filter(matchesCaller);
      ammLiqEventsMatching = ammLiqFull.filter(matchesCaller);
    } catch (e) {
      // Below-fold errors don't surface as page-level failures — the affected
      // sections will simply show empty states. The error still goes to the
      // console for debugging.
      console.error('[address page] Wave 2 load failed:', e);
    }
  }

  onMount(loadAddress);

  const pageTitle = $derived(
    knownCanister && canisterName ? canisterName : shortenPrincipal(principalStr),
  );
</script>

<svelte:head>
  <title>{pageTitle} | Rumi Explorer</title>
</svelte:head>

<EntityShell
  title="Address"
  loading={loading}
  error={error}
  onRetry={loadAddress}
>
  {#snippet identity()}
    <!-- Identity strip -->
    <div class="space-y-3">
      <div class="flex flex-wrap items-center gap-3">
        {#if knownCanister}
          <StatusBadge status="Canister" />
          <span class="text-sm font-semibold text-white">{canisterName}</span>
        {:else}
          <span class="text-xs text-gray-500 bg-gray-800/50 border border-gray-700/50 rounded-full px-2.5 py-0.5">
            Principal
          </span>
        {/if}
      </div>
      <div class="flex items-center gap-2">
        <code class="text-sm text-gray-300 font-mono bg-gray-800/50 border border-gray-700/50 rounded-lg px-3 py-2 break-all">
          {principalStr}
        </code>
        <CopyButton text={principalStr} />
      </div>
      <div class="flex flex-wrap items-center gap-x-6 gap-y-2 text-xs text-gray-400">
        <div>
          Net value
          <span class="text-white font-mono ml-1">{formatUsdRaw(netValueUsd)}</span>
        </div>
        {#if firstSeenNs}
          <div>First seen <TimeAgo timestamp={firstSeenNs} showFull={true} /></div>
        {/if}
        {#if lastActiveNs}
          <div>Last active <TimeAgo timestamp={lastActiveNs} /></div>
        {/if}
        <div>
          <span class="text-gray-200">{vaults.length}</span> vault{vaults.length === 1 ? '' : 's'}
          · <span class="text-gray-200">{lpRows.length}</span> LP position{lpRows.length === 1 ? '' : 's'}
          · <span class="text-gray-200">{tokenRows.length}</span> token{tokenRows.length === 1 ? '' : 's'} held
        </div>
      </div>
    </div>

    <!-- Portfolio: donut + timeline placeholder -->
    <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
      <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5 space-y-3">
        <div class="text-[10px] uppercase tracking-wider text-gray-500">Allocation</div>
        {#if donutTotal <= 0}
          <div class="text-xs text-gray-500 py-6">No assets detected for this principal.</div>
        {:else}
          <div class="flex items-center gap-5">
            <svg width="140" height="140" viewBox="0 0 140 140" class="flex-shrink-0">
              <g transform="translate(70, 70) rotate(-90)">
                <circle
                  r="60"
                  fill="none"
                  stroke="rgba(75, 85, 99, 0.35)"
                  stroke-width="18"
                />
                {#each donutArcs as arc (arc.slice.label)}
                  <circle
                    r="60"
                    fill="none"
                    stroke={arc.slice.color}
                    stroke-width="18"
                    stroke-dasharray={arc.dasharray}
                    stroke-dashoffset={arc.dashoffset}
                  />
                {/each}
              </g>
              <text
                x="70"
                y="66"
                text-anchor="middle"
                class="fill-white"
                style="font-size: 13px; font-weight: 600;"
              >{formatUsdRaw(donutTotal)}</text>
              <text
                x="70"
                y="82"
                text-anchor="middle"
                class="fill-gray-500"
                style="font-size: 9px; letter-spacing: 0.05em; text-transform: uppercase;"
              >net value</text>
            </svg>
            <div class="flex-1 space-y-2">
              {#each donutSlices as slice (slice.label)}
                <div class="flex items-center justify-between text-xs">
                  <div class="flex items-center gap-2">
                    <span
                      class="inline-block w-2.5 h-2.5 rounded-sm"
                      style="background-color: {slice.color}"
                    ></span>
                    <span class="text-gray-300">{slice.label}</span>
                  </div>
                  <div class="font-mono text-gray-200">{formatUsdRaw(slice.value)}</div>
                </div>
              {/each}
            </div>
          </div>
        {/if}
      </div>

      <PortfolioValueChart principal={principalStr} />
    </div>

    <!-- Vaults table -->
    <div class="space-y-2">
      <div class="flex items-baseline justify-between">
        <h3 class="text-xs font-semibold text-gray-400 uppercase tracking-wider">
          Vaults <span class="text-gray-600">({vaults.length})</span>
        </h3>
        {#if vaults.length > 0}
          <span class="text-xs text-gray-500">
            {activeVaultRows.length} active · {vaults.length - activeVaultRows.length} closed
          </span>
        {/if}
      </div>
      {#if vaults.length === 0}
        <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl p-6 text-center">
          <p class="text-gray-500 text-xs">No vaults owned by this principal.</p>
        </div>
      {:else}
        <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl overflow-hidden">
          <table class="w-full">
            <thead>
              <tr class="border-b border-gray-700/50 text-left">
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider">Vault</th>
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider">Collateral</th>
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider text-right">Debt (icUSD)</th>
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider text-center">CR</th>
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider text-center">Status</th>
              </tr>
            </thead>
            <tbody>
              {#each vaultRows as row (row.id)}
                <tr
                  class="border-b border-gray-700/30 last:border-b-0 hover:bg-gray-700/20 transition-colors cursor-pointer"
                  onclick={() => { window.location.href = `/explorer/e/vault/${row.id}`; }}
                >
                  <td class="px-4 py-3">
                    <span class="text-blue-400 hover:text-blue-300 font-mono text-sm">#{row.id}</span>
                  </td>
                  <td class="px-4 py-3">
                    <div class="text-sm text-gray-200 font-mono">
                      {formatE8s(row.collAmount, row.collDecimals)}
                      <span class="text-gray-500 ml-1">{row.collSymbol}</span>
                    </div>
                    <div class="text-[10px] text-gray-500">{formatUsdRaw(row.collUsd)}</div>
                  </td>
                  <td class="px-4 py-3 text-right text-sm text-gray-200 font-mono">
                    {formatE8s(row.debtE8s)}
                  </td>
                  <td class="px-4 py-3">
                    <div class="flex justify-center">
                      <CRDial cr={row.cr} liquidationCR={row.liqCR} size="sm" />
                    </div>
                  </td>
                  <td class="px-4 py-3 text-center">
                    <StatusBadge status={row.status} />
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </div>

    <!-- LP Positions table -->
    <div class="space-y-2">
      <div class="flex items-baseline justify-between">
        <h3 class="text-xs font-semibold text-gray-400 uppercase tracking-wider">
          LP Positions <span class="text-gray-600">({lpRows.length})</span>
        </h3>
      </div>
      {#if lpRows.length === 0}
        <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl p-6 text-center">
          <p class="text-gray-500 text-xs">No LP positions held by this principal.</p>
        </div>
      {:else}
        <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl overflow-hidden">
          <table class="w-full">
            <thead>
              <tr class="border-b border-gray-700/50 text-left">
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider">Pool</th>
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider text-right">Share %</th>
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider text-right">Value (USD)</th>
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider text-right">Unclaimed fees</th>
              </tr>
            </thead>
            <tbody>
              {#each lpRows as row (row.poolKey)}
                <tr
                  class="border-b border-gray-700/30 last:border-b-0 hover:bg-gray-700/20 transition-colors cursor-pointer"
                  onclick={() => { window.location.href = row.poolHref; }}
                >
                  <td class="px-4 py-3 text-sm text-blue-400 font-semibold">{row.poolLabel}</td>
                  <td class="px-4 py-3 text-right text-sm text-gray-200 font-mono">
                    {row.sharePct.toFixed(row.sharePct < 0.01 ? 4 : 2)}%
                  </td>
                  <td class="px-4 py-3 text-right text-sm text-gray-200 font-mono">
                    {formatUsdRaw(row.valueUsd)}
                  </td>
                  <td class="px-4 py-3 text-right text-xs text-gray-500 font-mono">
                    {row.feesUnclaimed}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </div>

    <!-- Token balances table -->
    <div class="space-y-2">
      <div class="flex items-baseline justify-between">
        <h3 class="text-xs font-semibold text-gray-400 uppercase tracking-wider">
          Token Balances <span class="text-gray-600">({tokenRows.length})</span>
        </h3>
      </div>
      {#if tokenRows.length === 0}
        <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl p-6 text-center">
          <p class="text-gray-500 text-xs">No non-zero token balances detected.</p>
        </div>
      {:else}
        <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl overflow-hidden">
          <table class="w-full">
            <thead>
              <tr class="border-b border-gray-700/50 text-left">
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider">Token</th>
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider text-right">Balance</th>
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider text-right">$ Value</th>
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider text-right">% of liquid</th>
                <th class="px-4 py-3 text-[10px] font-medium text-gray-500 uppercase tracking-wider text-right">24h Δ</th>
              </tr>
            </thead>
            <tbody>
              {#each tokenRows as row (row.principal)}
                <tr
                  class="border-b border-gray-700/30 last:border-b-0 hover:bg-gray-700/20 transition-colors cursor-pointer"
                  onclick={() => { window.location.href = `/explorer/e/token/${row.principal}`; }}
                >
                  <td class="px-4 py-3">
                    <span class="text-blue-400 hover:text-blue-300 font-semibold text-sm">{row.symbol}</span>
                  </td>
                  <td class="px-4 py-3 text-right text-sm text-gray-200 font-mono">
                    {formatE8s(row.balance, row.decimals)}
                  </td>
                  <td class="px-4 py-3 text-right text-sm text-gray-200 font-mono">
                    {formatUsdRaw(row.valueUsd)}
                  </td>
                  <td class="px-4 py-3 text-right text-xs text-gray-400 font-mono">
                    {row.pctOfPortfolio.toFixed(1)}%
                  </td>
                  <td class="px-4 py-3 text-right text-xs font-mono">
                    {#if row.delta24h == null}
                      <span class="text-gray-600">—</span>
                    {:else if row.delta24h >= 0}
                      <span class="text-emerald-400">+{row.delta24h.toFixed(2)}%</span>
                    {:else}
                      <span class="text-rose-400">{row.delta24h.toFixed(2)}%</span>
                    {/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </div>
  {/snippet}

  {#snippet relationships()}
    <div class="grid grid-cols-1 gap-3" class:md:grid-cols-2={!hasSubaccounts} class:md:grid-cols-3={hasSubaccounts}>
      <!-- Top counterparties -->
      <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-2">
        <div class="text-[10px] uppercase tracking-wider text-gray-500">Top counterparties</div>
        {#if relationshipCounts.topCounterparties.length === 0}
          <div class="text-xs text-gray-500">No counterparty activity yet.</div>
        {:else}
          <ul class="space-y-1.5">
            {#each relationshipCounts.topCounterparties as [p, count] (p)}
              <li class="flex items-center justify-between gap-2 text-xs">
                <EntityLink type="address" value={p} />
                <span class="text-gray-500">{count} event{count === 1 ? '' : 's'}</span>
              </li>
            {/each}
          </ul>
        {/if}
      </div>

      <!-- Canisters interacted with -->
      <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-2">
        <div class="text-[10px] uppercase tracking-wider text-gray-500">Canisters interacted with</div>
        {#if relationshipCounts.topCanisters.length === 0}
          <div class="text-xs text-gray-500">No canister interactions recorded.</div>
        {:else}
          <ul class="space-y-1.5">
            {#each relationshipCounts.topCanisters as [c, count] (c)}
              <li class="flex items-center justify-between gap-2 text-xs">
                <a
                  href="/explorer/canister/{c}"
                  class="text-blue-400 hover:text-blue-300 truncate"
                  title={c}
                >
                  {getCanisterName(c) ?? shortenPrincipal(c)}
                </a>
                <span class="text-gray-500">{count}</span>
              </li>
            {/each}
          </ul>
        {/if}
      </div>

      <!-- Sub-accounts seen (hidden when empty) -->
      {#if hasSubaccounts}
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-2">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Sub-accounts seen</div>
          {#if nonDefaultIcusdSubs.length > 0}
            <div>
              <div class="text-[10px] text-gray-500 mt-1">icUSD ({nonDefaultIcusdSubs.length})</div>
              <ul class="space-y-0.5">
                {#each nonDefaultIcusdSubs.slice(0, 4) as hex (hex)}
                  <li class="font-mono text-[11px] text-gray-400 truncate" title={hex}>
                    {hex.slice(0, 8)}…{hex.slice(-6)}
                  </li>
                {/each}
              </ul>
            </div>
          {/if}
          {#if nonDefaultThreeUsdSubs.length > 0}
            <div>
              <div class="text-[10px] text-gray-500 mt-1">3USD ({nonDefaultThreeUsdSubs.length})</div>
              <ul class="space-y-0.5">
                {#each nonDefaultThreeUsdSubs.slice(0, 4) as hex (hex)}
                  <li class="font-mono text-[11px] text-gray-400 truncate" title={hex}>
                    {hex.slice(0, 8)}…{hex.slice(-6)}
                  </li>
                {/each}
              </ul>
            </div>
          {/if}
        </div>
      {/if}
    </div>

    <!-- Server-ranked counterparties (analytics.get_top_counterparties). -->
    <!-- Shown separately from the client-computed list above because it ranks -->
    <!-- by volume (not just event count), spans a chosen window, and has a -->
    <!-- wider 10-row view with an interaction-count + volume column pair. -->
    <div class="mt-3 bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-3">
      <div class="flex items-center justify-between gap-3 flex-wrap">
        <div>
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Top counterparties (by volume)</div>
          <div class="text-[10px] text-gray-600">Ranked across vault, swap, LP, and SP events</div>
        </div>
        <div class="inline-flex rounded-lg border border-gray-700/70 overflow-hidden text-[11px]">
          {#each ['7d', '30d', '90d', 'all'] as const as w (w)}
            <button
              type="button"
              class="px-2.5 py-1 border-r border-gray-700/70 last:border-r-0 transition-colors"
              class:bg-blue-500={cpWindow === w}
              class:text-white={cpWindow === w}
              class:text-gray-400={cpWindow !== w}
              class:hover:text-gray-200={cpWindow !== w}
              onclick={() => (cpWindow = w)}
            >
              {w.toUpperCase()}
            </button>
          {/each}
        </div>
      </div>

      {#if cpLoading && cpRows.length === 0}
        <div class="text-xs text-gray-500">Loading…</div>
      {:else if cpRows.length === 0}
        <div class="text-xs text-gray-500">No counterparty activity in this window.</div>
      {:else}
        <ul class="divide-y divide-gray-700/40">
          {#each cpRows as row (row.counterparty.toText())}
            {@const cp = row.counterparty.toText()}
            {@const label = getCanisterName(cp) ?? shortenPrincipal(cp)}
            <li class="py-1.5 flex items-center justify-between gap-3 text-xs">
              <a
                href="/explorer/e/address/{cp}"
                class="text-blue-400 hover:text-blue-300 truncate font-mono"
                title={cp}
              >
                {label}
              </a>
              <div class="flex items-center gap-3 shrink-0 text-gray-400">
                <span class="tabular-nums">
                  {row.interaction_count.toString()}
                  {Number(row.interaction_count) === 1 ? 'event' : 'events'}
                </span>
                <span class="text-gray-600">·</span>
                <span class="tabular-nums text-gray-300">
                  {formatE8s(row.volume_e8s)}
                </span>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/snippet}

  {#snippet activity()}
    <div class="flex items-center justify-between text-xs">
      <span class="text-gray-500">
        Showing {activityPreview.length} of {mergedEvents.length} event{mergedEvents.length === 1 ? '' : 's'}
      </span>
      {#if mergedEvents.length > activityPreview.length}
        <a
          href={seeAllHref}
          class="text-blue-400 hover:text-blue-300"
        >
          See all →
        </a>
      {/if}
    </div>
    <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl overflow-hidden">
      {#if mergedEvents.length === 0}
        <div class="px-5 py-8 text-center text-gray-500 text-sm">
          No activity found for this principal.
        </div>
      {:else}
        <MixedEventsTable
          events={activityPreview}
          vaultCollateralMap={vaultCollateralMap}
          vaultOwnerMap={vaultOwnerMap}
        />
      {/if}
    </div>
  {/snippet}
</EntityShell>
