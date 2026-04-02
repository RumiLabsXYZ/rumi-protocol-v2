<script lang="ts">
  import { onMount } from 'svelte';
  import StatCard from '$components/explorer/StatCard.svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import StatusBadge from '$components/explorer/StatusBadge.svelte';
  import EventRow from '$components/explorer/EventRow.svelte';
  import TokenBadge from '$components/explorer/TokenBadge.svelte';
  import VaultHealthBar from '$components/explorer/VaultHealthBar.svelte';
  import {
    fetchProtocolStatus, fetchCollateralTotals, fetchCollateralPrices,
    fetchAllVaults, fetchEventCount, fetchTreasuryStats,
    fetchInterestSplit, fetchBotStats, fetchStabilityPoolStatus,
    fetchThreePoolStatus, fetchEvents, fetchCollateralConfigs,
    fetchLiquidatableVaults, fetchAllSnapshots,
    fetchAmmPools, fetchAmmSwapEventCount, fetchSwapEventCount
  } from '$services/explorer/explorerService';
  import {
    formatE8s, formatUsd, formatCR, formatBps,
    getTokenSymbol, registerToken, classifyVaultHealth, healthColor
  } from '$utils/explorerHelpers';
  import { decodeRustDecimal } from '$utils/decimalUtils';

  const E8S = 100_000_000;

  // ── State ─────────────────────────────────────────────────────────────

  // Section 1: Hero
  let status: any = $state(null);
  let vaults: any[] = $state([]);
  let eventCount: bigint = $state(0n);
  let heroLoading = $state(true);
  let heroError: string | null = $state(null);

  // Section 2: Collateral
  let collateralTotals: any[] = $state([]);
  let collateralPrices: Map<string, number> = $state(new Map());
  let collateralConfigs: any[] = $state([]);
  let collateralLoading = $state(true);
  let collateralError: string | null = $state(null);

  // Section 3: Pools
  let spStatus: any = $state(null);
  let tpStatus: any = $state(null);
  let ammPools: any[] = $state([]);
  let poolsLoading = $state(true);
  let poolsError: string | null = $state(null);

  // Section 4: Treasury & Revenue
  let treasuryStats: any = $state(null);
  let interestSplit: any = $state(null);
  let treasuryLoading = $state(true);
  let treasuryError: string | null = $state(null);

  // Section 5: Liquidation Health
  let botStats: any = $state(null);
  let liquidatableVaults: any[] = $state([]);
  let liquidationLoading = $state(true);
  let liquidationError: string | null = $state(null);

  // Section 6: Recent Events
  let recentEvents: [bigint, any][] = $state([]);
  let eventsLoading = $state(true);
  let eventsError: string | null = $state(null);

  // Section 7: Historical Charts
  let allSnapshots: any[] = $state([]);
  let chartsLoading = $state(true);
  let chartsError: string | null = $state(null);

  type TimeRange = '24h' | '7d' | '30d' | '90d' | 'all';
  let timeRange: TimeRange = $state('7d');

  const timeRanges: { label: string; value: TimeRange }[] = [
    { label: '24h', value: '24h' },
    { label: '7d',  value: '7d' },
    { label: '30d', value: '30d' },
    { label: '90d', value: '90d' },
    { label: 'All', value: 'all' },
  ];

  // Global
  let isRefreshing = $state(false);

  // ── Derived ───────────────────────────────────────────────────────────

  let protocolMode = $derived.by(() => {
    if (!status?.mode) return 'Unknown';
    const key = Object.keys(status.mode)[0] ?? 'Unknown';
    // Make mode names human-readable
    const modeNames: Record<string, string> = {
      'GeneralAvailability': 'Normal',
      'Normal': 'Normal',
      'Recovery': 'Recovery',
      'Frozen': 'Frozen',
    };
    return modeNames[key] ?? key.replace(/([A-Z])/g, ' $1').trim();
  });

  let modeVariant = $derived.by((): 'normal' | 'recovery' | 'frozen' => {
    const m = protocolMode.toLowerCase();
    if (m === 'recovery') return 'recovery';
    if (m === 'frozen') return 'frozen';
    return 'normal';
  });

  let activeVaultCount = $derived(
    vaults.filter((v: any) => Number(v.borrowed_icusd_amount) > 0 || Number(v.collateral_amount) > 0).length
  );

  // Total TVL in USD — sum of all collateral USD values from collateral rows
  let totalTvlUsd = $derived(
    collateralRows.reduce((sum: number, row: any) => sum + (row.collateralUsd || 0), 0)
  );

  // Total debt in icUSD from collateral rows
  let totalDebtIcusd = $derived(
    collateralRows.reduce((sum: number, row: any) => sum + (row.debtHuman || 0), 0)
  );

  // collateralPrices is already a Map<string, number> from the service
  let priceMap = $derived(collateralPrices);

  // Build a config map from collateralConfigs array: principal string -> config
  let configMap = $derived.by(() => {
    const map = new Map<string, any>();
    for (const cfg of collateralConfigs) {
      const key = cfg.ledger_canister_id?.toText?.() ?? cfg.collateral_type?.toText?.() ?? '';
      if (key) map.set(key, cfg);
    }
    return map;
  });

  // Collateral rows for Section 2 — use collateralTotals (which has symbol, price, etc.)
  // enriched with config data (debt_ceiling, interest_rate_apr, status) from collateralConfigs.
  let collateralRows = $derived.by(() => {
    const rows: any[] = [];
    for (const totals of collateralTotals) {
      const principal = totals.collateral_type?.toText?.() ?? String(totals.collateral_type);
      const symbol = (totals.symbol && totals.symbol.length > 0) ? totals.symbol : getTokenSymbol(principal);
      const price = totals.price ?? 0;
      const decimals = Number(totals.decimals ?? 8);
      const totalCollateral = Number(totals.total_collateral ?? 0n);
      const totalDebt = Number(totals.total_debt ?? 0n);
      const collateralHuman = totalCollateral / Math.pow(10, decimals);
      const collateralUsd = collateralHuman * price;
      const debtHuman = totalDebt / E8S;
      const vaultCount = Number(totals.vault_count ?? 0n);

      // Enrich with config data
      const cfg = configMap.get(principal);
      const debtCeilingRaw = cfg?.debt_ceiling != null ? Number(cfg.debt_ceiling) : 0;
      // u64::MAX (18446744073709551615) means no limit — treat as unlimited
      const isUnlimited = debtCeilingRaw > 1e18;
      const debtCeilingHuman = isUnlimited ? 0 : debtCeilingRaw / E8S;
      const utilization = debtCeilingHuman > 0 ? (debtHuman / debtCeilingHuman) * 100 : 0;

      // interest_rate_apr is a Uint8Array (Rust Decimal) — decode it.
      // Also check per_collateral_interest from status for the effective rate.
      let interestRate = 0;
      if (cfg?.interest_rate_apr && (cfg.interest_rate_apr instanceof Uint8Array || Array.isArray(cfg.interest_rate_apr))) {
        interestRate = decodeRustDecimal(cfg.interest_rate_apr);
      }
      // Override with effective weighted rate from ProtocolStatus if available
      if (status?.per_collateral_interest) {
        const match = status.per_collateral_interest.find((ci: any) => {
          const ciPrincipal = ci.collateral_type?.toText?.() ?? String(ci.collateral_type);
          return ciPrincipal === principal;
        });
        if (match && match.weighted_interest_rate != null) {
          interestRate = Number(match.weighted_interest_rate);
        }
      }

      const statusKey = cfg?.status ? (typeof cfg.status === 'object' ? Object.keys(cfg.status)[0] : String(cfg.status)) : 'Active';

      rows.push({
        principal,
        symbol,
        price,
        decimals,
        totalCollateral,
        collateralHuman,
        collateralUsd,
        totalDebt,
        debtHuman,
        vaultCount,
        utilization,
        isUnlimited,
        interestRate,
        status: statusKey,
      });
    }
    return rows;
  });

  // SP utilization
  let spUtilization = $derived.by(() => {
    if (!spStatus) return 0;
    const deposits = Number(spStatus.total_deposits_e8s ?? 0n);
    if (deposits === 0) return 0;
    // Approximate utilization: how much has been used in liquidations
    return 0; // SP doesn't have a standard utilization metric
  });

  // Interest split entries
  let splitEntries = $derived.by(() => {
    if (!interestSplit) return [];
    if (Array.isArray(interestSplit)) return interestSplit;
    if (interestSplit.destinations) return interestSplit.destinations;
    return [];
  });

  // At-risk vaults sorted by CR ascending (top 10)
  let atRiskVaults = $derived.by(() => {
    if (!vaults.length || !collateralConfigs.length) return [];

    const vaultsWithCr: any[] = [];
    for (const vault of vaults) {
      const debt = Number(vault.borrowed_icusd_amount);
      if (debt === 0) continue;

      const collateralType = vault.collateral_type?.toText?.() ?? String(vault.collateral_type);
      const price = priceMap.get(collateralType) ?? 0;
      const cfg = configMap.get(collateralType);
      if (!cfg || price === 0) continue;

      const decimals = Number(cfg.decimals ?? 8);
      const collateralValue = (Number(vault.collateral_amount) / Math.pow(10, decimals)) * price;
      const debtValue = debt / E8S;
      const cr = debtValue > 0 ? collateralValue / debtValue : Infinity;

      // liquidation_ratio and borrow_threshold_ratio are Uint8Array (Rust Decimal) — decode them
      let liquidationRatio = 1.1;
      if (cfg.liquidation_ratio && (cfg.liquidation_ratio instanceof Uint8Array || Array.isArray(cfg.liquidation_ratio))) {
        liquidationRatio = decodeRustDecimal(cfg.liquidation_ratio);
      }
      let borrowThreshold = 1.5;
      if (cfg.borrow_threshold_ratio && (cfg.borrow_threshold_ratio instanceof Uint8Array || Array.isArray(cfg.borrow_threshold_ratio))) {
        borrowThreshold = decodeRustDecimal(cfg.borrow_threshold_ratio);
      }

      vaultsWithCr.push({
        vault_id: Number(vault.vault_id),
        owner: vault.owner?.toText?.() ?? String(vault.owner),
        collateral_ratio: cr,
        collateral_type: collateralType,
        liquidation_ratio: liquidationRatio,
        borrow_threshold_ratio: borrowThreshold,
        debt,
        collateral_amount: Number(vault.collateral_amount),
        decimals,
        symbol: getTokenSymbol(collateralType),
      });
    }

    vaultsWithCr.sort((a, b) => a.collateral_ratio - b.collateral_ratio);
    return vaultsWithCr.slice(0, 10);
  });

  // Vault collateral map for EventRow
  let vaultCollateralMap = $derived.by(() => {
    const map = new Map<number, any>();
    for (const v of vaults) {
      map.set(Number(v.vault_id), v.collateral_type);
    }
    return map;
  });

  // ── Data Loading ──────────────────────────────────────────────────────

  async function loadData(isRefresh = false) {
    if (isRefresh) isRefreshing = true;

    // Section 1: Hero
    const heroPromise = (async () => {
      try {
        const [s, v, ec, tpSwapCount, ammSwapCount] = await Promise.all([
          fetchProtocolStatus(),
          fetchAllVaults(),
          fetchEventCount(),
          fetchSwapEventCount(),
          fetchAmmSwapEventCount()
        ]);
        status = s;
        vaults = v;
        eventCount = (ec ?? 0n) + (tpSwapCount ?? 0n) + (ammSwapCount ?? 0n);
      } catch (e) {
        console.error('[explorer] Hero load failed:', e);
        if (!isRefresh) heroError = 'Failed to load protocol status';
      } finally {
        if (!isRefresh) heroLoading = false;
      }
    })();

    // Section 2: Collateral
    const collateralPromise = (async () => {
      try {
        const [totals, prices, configs] = await Promise.all([
          fetchCollateralTotals(),
          fetchCollateralPrices(),
          fetchCollateralConfigs()
        ]);
        collateralTotals = totals;
        collateralPrices = prices;
        collateralConfigs = configs;
        // Register tokens dynamically so getTokenSymbol() works for all collateral types
        for (const total of totals) {
          const pid = total.collateral_type?.toText?.() ?? '';
          if (pid && total.symbol) {
            registerToken(pid, total.symbol, total.symbol, Number(total.decimals ?? 8));
          }
        }
      } catch (e) {
        console.error('[explorer] Collateral load failed:', e);
        if (!isRefresh) collateralError = 'Failed to load collateral data';
      } finally {
        if (!isRefresh) collateralLoading = false;
      }
    })();

    // Section 3: Pools
    const poolsPromise = (async () => {
      try {
        const [sp, tp, amm] = await Promise.all([
          fetchStabilityPoolStatus(),
          fetchThreePoolStatus(),
          fetchAmmPools()
        ]);
        spStatus = sp;
        tpStatus = tp;
        ammPools = amm;
      } catch (e) {
        console.error('[explorer] Pools load failed:', e);
        if (!isRefresh) poolsError = 'Failed to load pool data';
      } finally {
        if (!isRefresh) poolsLoading = false;
      }
    })();

    // Section 4: Treasury
    const treasuryPromise = (async () => {
      try {
        const [ts, is] = await Promise.all([
          fetchTreasuryStats(),
          fetchInterestSplit()
        ]);
        treasuryStats = ts;
        interestSplit = is;
      } catch (e) {
        console.error('[explorer] Treasury load failed:', e);
        if (!isRefresh) treasuryError = 'Failed to load treasury data';
      } finally {
        if (!isRefresh) treasuryLoading = false;
      }
    })();

    // Section 5: Liquidation Health
    const liquidationPromise = (async () => {
      try {
        const [bs, lv] = await Promise.all([
          fetchBotStats(),
          fetchLiquidatableVaults()
        ]);
        botStats = bs;
        liquidatableVaults = lv;
      } catch (e) {
        console.error('[explorer] Liquidation load failed:', e);
        if (!isRefresh) liquidationError = 'Failed to load liquidation data';
      } finally {
        if (!isRefresh) liquidationLoading = false;
      }
    })();

    // Section 6: Recent Events
    const eventsPromise = (async () => {
      try {
        const result = await fetchEvents(0n, 20n);
        recentEvents = result.events;
      } catch (e) {
        console.error('[explorer] Events load failed:', e);
        if (!isRefresh) eventsError = 'Failed to load events';
      } finally {
        if (!isRefresh) eventsLoading = false;
      }
    })();

    // Section 7: Historical Charts
    const chartsPromise = (async () => {
      try {
        const snapshots = await fetchAllSnapshots();
        allSnapshots = snapshots ?? [];
      } catch (e) {
        console.error('[explorer] Charts load failed:', e);
        if (!isRefresh) chartsError = 'Failed to load historical data';
      } finally {
        if (!isRefresh) chartsLoading = false;
      }
    })();

    await Promise.allSettled([
      heroPromise, collateralPromise, poolsPromise,
      treasuryPromise, liquidationPromise, eventsPromise, chartsPromise
    ]);

    if (isRefresh) isRefreshing = false;
  }

  let refreshInterval: ReturnType<typeof setInterval>;

  onMount(() => {
    loadData(false);
    refreshInterval = setInterval(() => loadData(true), 30_000);
    return () => clearInterval(refreshInterval);
  });

  // ── Format helpers ────────────────────────────────────────────────────

  function formatCompactUsd(value: number | bigint): string {
    const v = Number(value);
    if (v >= 1_000_000) return `$${(v / 1_000_000).toFixed(2)}M`;
    if (v >= 1_000) return `$${(v / 1_000).toFixed(1)}K`;
    return `$${v.toFixed(2)}`;
  }

  function formatPrice(value: number | bigint): string {
    const v = Number(value);
    if (v >= 1000) return `$${v.toLocaleString(undefined, { maximumFractionDigits: 2 })}`;
    if (v >= 1) return `$${v.toFixed(2)}`;
    return `$${v.toFixed(4)}`;
  }

  function formatCompactAmount(value: number | bigint): string {
    const v = Number(value);
    if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(2)}M`;
    if (v >= 1_000) return `${(v / 1_000).toFixed(1)}K`;
    if (v >= 1) return v.toFixed(2);
    return v.toFixed(4);
  }

  function shortenOwner(principal: string): string {
    if (principal.length <= 15) return principal;
    return `${principal.slice(0, 5)}...${principal.slice(-5)}`;
  }

  // Interest split color mapping
  const splitColors: Record<string, string> = {
    stability_pool: '#34d399',
    treasury: '#818cf8',
    three_pool: '#f59e0b',
  };

  const splitLabels: Record<string, string> = {
    stability_pool: 'Stability Pool',
    treasury: 'Treasury',
    three_pool: '3Pool',
  };

  // ── Historical Chart Logic ────────────────────────────────────────────

  const filteredSnapshots = $derived(filterByTimeRange(allSnapshots, timeRange));

  function filterByTimeRange(snaps: any[], range: TimeRange): any[] {
    if (!snaps.length || range === 'all') return snaps;
    const nowNs = Date.now() * 1_000_000;
    const ranges: Record<string, number> = {
      '24h': 24 * 3600e9,
      '7d':  7  * 24 * 3600e9,
      '30d': 30 * 24 * 3600e9,
      '90d': 90 * 24 * 3600e9,
    };
    const cutoff = nowNs - (ranges[range] ?? ranges['7d']);
    return snaps.filter((s) => Number(s.timestamp) >= cutoff);
  }

  interface ChartPoint { x: number; y: number }

  const tvlData: ChartPoint[] = $derived(
    filteredSnapshots.map((s) => ({
      x: Number(s.timestamp) / 1e6,
      y: Number(s.total_collateral_value_usd) / 1e8,
    }))
  );

  const debtData: ChartPoint[] = $derived(
    filteredSnapshots.map((s) => ({
      x: Number(s.timestamp) / 1e6,
      y: Number(s.total_debt) / 1e8,
    }))
  );

  const crData: ChartPoint[] = $derived(
    filteredSnapshots.map((s) => {
      const collUsd = Number(s.total_collateral_value_usd) / 1e8;
      const debt = Number(s.total_debt) / 1e8;
      return {
        x: Number(s.timestamp) / 1e6,
        y: debt > 0 ? (collUsd / debt) * 100 : 0,
      };
    })
  );

  const chartW = 600;
  const chartH = 160;
  const chartPad = 4;

  function buildPolyline(data: ChartPoint[]): string {
    if (data.length < 2) return '';
    const xMin = data[0].x;
    const xMax = data[data.length - 1].x;
    const yMin = 0;
    const yMax = Math.max(...data.map((d) => d.y)) * 1.1 || 1;
    const xRange = xMax - xMin || 1;
    return data
      .map((d) => {
        const x = chartPad + ((d.x - xMin) / xRange) * (chartW - chartPad * 2);
        const y = chartPad + (chartH - chartPad * 2) - ((d.y - yMin) / (yMax - yMin)) * (chartH - chartPad * 2);
        return `${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(' ');
  }

  function buildFill(points: string): string {
    if (!points) return '';
    const firstX = points.split(' ')[0]?.split(',')[0] ?? '0';
    return `${firstX},${chartH} ${points} ${chartW - chartPad},${chartH}`;
  }

  function buildYLabels(data: ChartPoint[], count = 4): { y: number; label: string }[] {
    if (!data.length) return [];
    const yMax = Math.max(...data.map((d) => d.y)) * 1.1 || 1;
    return Array.from({ length: count }, (_, i) => {
      const frac = (count - 1 - i) / (count - 1);
      const val = yMax * frac;
      return {
        y: chartPad + (i / (count - 1)) * (chartH - chartPad * 2),
        label:
          val >= 1_000_000
            ? `${(val / 1_000_000).toFixed(1)}M`
            : val >= 1_000
              ? `${(val / 1_000).toFixed(0)}k`
              : val.toFixed(1),
      };
    });
  }

  const tvlPoints = $derived(buildPolyline(tvlData));
  const debtPoints = $derived(buildPolyline(debtData));
  const crPoints = $derived(buildPolyline(crData));

  const tvlFill = $derived(buildFill(tvlPoints));
  const debtFill = $derived(buildFill(debtPoints));
  const crFill = $derived(buildFill(crPoints));

  const tvlLabels = $derived(buildYLabels(tvlData));
  const debtLabels = $derived(buildYLabels(debtData));
  const crLabels = $derived(buildYLabels(crData));

  const latestTvlChart = $derived(tvlData.length > 0 ? tvlData[tvlData.length - 1].y : 0);
  const latestDebtChart = $derived(debtData.length > 0 ? debtData[debtData.length - 1].y : 0);
  const latestCrChart = $derived(crData.length > 0 ? crData[crData.length - 1].y : 0);
</script>

<svelte:head>
  <title>Explorer - Rumi Protocol</title>
</svelte:head>

<div class="max-w-7xl mx-auto px-4 py-8 space-y-8">

  <!-- Page header -->
  <div class="flex items-center justify-end">
    {#if isRefreshing}
      <div class="flex items-center gap-1.5 text-xs text-gray-500">
        <svg class="animate-spin w-3 h-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M21 12a9 9 0 1 1-6.219-8.56"/>
        </svg>
        Refreshing...
      </div>
    {/if}
  </div>

  <!-- ════════════════════════════════════════════════════════════════════
       Section 1 — Hero Stats
       ════════════════════════════════════════════════════════════════════ -->
  <section>
    {#if heroLoading}
      <div class="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3">
        {#each Array(6) as _}
          <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-5 animate-pulse">
            <div class="h-3 w-20 bg-gray-700 rounded mb-3"></div>
            <div class="h-7 w-16 bg-gray-700 rounded"></div>
          </div>
        {/each}
      </div>
    {:else if heroError}
      <div class="bg-gray-800/50 border border-red-800/30 rounded-xl p-6 text-center text-red-400 text-sm">
        {heroError}
      </div>
    {:else}
      <div class="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3">
        <StatCard label="Protocol Mode" value={protocolMode} />

        <StatCard
          label="Total TVL"
          value={totalTvlUsd > 0 ? formatCompactUsd(totalTvlUsd) : (status?.total_icp_margin != null ? formatUsd(status.total_icp_margin) : '--')}
          subtitle="All collateral locked"
        />

        <StatCard
          label="Total Debt"
          value={totalDebtIcusd > 0 ? `${formatCompactUsd(totalDebtIcusd)}` : (status?.total_icusd_borrowed != null ? `${formatE8s(status.total_icusd_borrowed)} icUSD` : '--')}
          subtitle="icUSD minted"
        />

        <StatCard
          label="System CR"
          value={status?.total_collateral_ratio != null ? formatCR(status.total_collateral_ratio) : '--'}
          subtitle="Collateral ratio"
        />

        <StatCard label="Active Vaults" value={activeVaultCount.toLocaleString()} />

        <StatCard label="Total Events" value={Number(eventCount).toLocaleString()} />
      </div>
    {/if}
  </section>

  <!-- ════════════════════════════════════════════════════════════════════
       Section 2 — Collateral Breakdown
       ════════════════════════════════════════════════════════════════════ -->
  <section>
    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
      <div class="px-5 py-4 border-b border-gray-700/50">
        <h2 class="text-sm font-semibold text-gray-200">Collateral Breakdown</h2>
      </div>

      {#if collateralLoading}
        <div class="px-5 py-12 text-center text-gray-500 text-sm animate-pulse">Loading collateral data...</div>
      {:else if collateralError}
        <div class="px-5 py-8 text-center text-red-400 text-sm">{collateralError}</div>
      {:else if collateralRows.length === 0}
        <div class="px-5 py-8 text-center text-gray-500 text-sm">No collateral types configured</div>
      {:else}
        <div class="overflow-x-auto">
          <table class="w-full text-sm">
            <thead>
              <tr class="border-b border-gray-700/50">
                <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-left">Token</th>
                <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Price</th>
                <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Total Locked</th>
                <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Locked (USD)</th>
                <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Total Debt</th>
                <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Vaults</th>
                <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Utilization</th>
                <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Interest Rate</th>
                <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-center">Status</th>
              </tr>
            </thead>
            <tbody>
              {#each collateralRows as row}
                <tr class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors">
                  <td class="px-4 py-3">
                    <TokenBadge symbol={row.symbol} principalId={row.principal} size="sm" />
                  </td>
                  <td class="px-4 py-3 text-right text-gray-200 tabular-nums">{formatPrice(row.price)}</td>
                  <td class="px-4 py-3 text-right text-gray-200 tabular-nums">{formatCompactAmount(row.collateralHuman)}</td>
                  <td class="px-4 py-3 text-right text-gray-200 tabular-nums">{formatCompactUsd(row.collateralUsd)}</td>
                  <td class="px-4 py-3 text-right text-gray-200 tabular-nums">{formatCompactUsd(row.debtHuman)}</td>
                  <td class="px-4 py-3 text-right text-gray-300 tabular-nums">{row.vaultCount}</td>
                  <td class="px-4 py-3 text-right">
                    {#if row.isUnlimited}
                      <span class="text-xs text-gray-500">No limit</span>
                    {:else if row.utilization > 999}
                      <div class="flex items-center justify-end gap-2">
                        <div class="w-16 h-1.5 bg-gray-700 rounded-full overflow-hidden">
                          <div class="h-full rounded-full bg-red-400" style="width: 100%"></div>
                        </div>
                        <span class="text-xs text-red-400 tabular-nums w-10 text-right">&gt;999%</span>
                      </div>
                    {:else}
                      <div class="flex items-center justify-end gap-2">
                        <div class="w-16 h-1.5 bg-gray-700 rounded-full overflow-hidden">
                          <div
                            class="h-full rounded-full transition-all duration-300 {row.utilization > 80 ? 'bg-red-400' : row.utilization > 50 ? 'bg-yellow-400' : 'bg-green-400'}"
                            style="width: {Math.min(100, row.utilization)}%"
                          ></div>
                        </div>
                        <span class="text-xs text-gray-400 tabular-nums w-10 text-right">{row.utilization.toFixed(0)}%</span>
                      </div>
                    {/if}
                  </td>
                  <td class="px-4 py-3 text-right text-gray-300 tabular-nums">
                    {#if row.interestRate > 0}
                      {(row.interestRate * 100).toFixed(2)}%
                    {:else}
                      --
                    {/if}
                  </td>
                  <td class="px-4 py-3 text-center">
                    <StatusBadge
                      status={row.status}
                      variant={row.status === 'Active' ? 'active' : row.status === 'Paused' ? 'paused' : row.status === 'Frozen' ? 'frozen' : 'info'}
                    />
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </div>
  </section>

  <!-- ════════════════════════════════════════════════════════════════════
       Section 3 — Pools (Stability Pool + 3Pool)
       ════════════════════════════════════════════════════════════════════ -->
  <section class="grid grid-cols-1 lg:grid-cols-3 gap-4">
    <!-- Stability Pool -->
    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
      <div class="px-5 py-4 border-b border-gray-700/50 flex items-center justify-between">
        <h2 class="text-sm font-semibold text-gray-200">Stability Pool</h2>
        {#if spStatus}
          <StatusBadge
            status={spStatus.is_paused ? 'Paused' : 'Active'}
            variant={spStatus.is_paused ? 'paused' : 'active'}
          />
        {/if}
      </div>

      {#if poolsLoading}
        <div class="p-5 space-y-3 animate-pulse">
          <div class="h-4 w-32 bg-gray-700 rounded"></div>
          <div class="h-4 w-24 bg-gray-700 rounded"></div>
          <div class="h-4 w-28 bg-gray-700 rounded"></div>
        </div>
      {:else if !spStatus}
        <div class="p-5 text-gray-500 text-sm">Failed to load stability pool data</div>
      {:else}
        <div class="p-5 space-y-3">
          <div class="flex justify-between items-center">
            <span class="text-sm text-gray-400">Total Deposits</span>
            <span class="text-sm font-semibold text-gray-200 tabular-nums">
              {formatE8s(spStatus.total_deposits_e8s)} <span class="text-gray-400">icUSD</span>
            </span>
          </div>
          <div class="flex justify-between items-center">
            <span class="text-sm text-gray-400">Depositors</span>
            <span class="text-sm font-semibold text-gray-200 tabular-nums">{Number(spStatus.total_depositors ?? spStatus.depositor_count ?? 0)}</span>
          </div>
          <div class="flex justify-between items-center">
            <span class="text-sm text-gray-400">Liquidations Executed</span>
            <span class="text-sm font-semibold text-gray-200 tabular-nums">{Number(spStatus.total_liquidations_executed ?? 0)}</span>
          </div>
          {#if spStatus.collateral_gains && spStatus.collateral_gains.length > 0}
            <div class="pt-2 border-t border-gray-700/30">
              <p class="text-xs text-gray-500 mb-2 uppercase tracking-wider font-medium">Collateral Gains</p>
              <div class="space-y-1">
                {#each spStatus.collateral_gains as [ledger, amount]}
                  {@const principal = ledger?.toText?.() ?? String(ledger)}
                  {@const symbol = getTokenSymbol(principal)}
                  {@const humanAmount = Number(amount) / E8S}
                  {#if humanAmount > 0}
                    <div class="flex justify-between text-xs">
                      <span class="text-gray-400">{symbol}</span>
                      <span class="text-gray-300 tabular-nums">{humanAmount.toFixed(4)}</span>
                    </div>
                  {/if}
                {/each}
              </div>
            </div>
          {/if}
        </div>
      {/if}
    </div>

    <!-- 3Pool -->
    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
      <div class="px-5 py-4 border-b border-gray-700/50">
        <h2 class="text-sm font-semibold text-gray-200">3Pool (StableSwap)</h2>
      </div>

      {#if poolsLoading}
        <div class="p-5 space-y-3 animate-pulse">
          <div class="h-4 w-32 bg-gray-700 rounded"></div>
          <div class="h-4 w-24 bg-gray-700 rounded"></div>
        </div>
      {:else if !tpStatus}
        <div class="p-5 text-gray-500 text-sm">Failed to load 3Pool data</div>
      {:else}
        {@const balances = tpStatus.balances ?? []}
        {@const tokens = tpStatus.tokens ?? []}
        {@const tvl = (() => {
          let total = 0;
          for (let i = 0; i < balances.length; i++) {
            const decimals = tokens[i]?.decimals ?? (i === 0 ? 8 : 6);
            total += Number(balances[i]) / Math.pow(10, decimals);
          }
          return total;
        })()}
        <div class="p-5 space-y-3">
          <div class="flex justify-between items-center">
            <span class="text-sm text-gray-400">TVL</span>
            <span class="text-sm font-semibold text-gray-200 tabular-nums">{formatCompactUsd(tvl)}</span>
          </div>
          <div class="flex justify-between items-center">
            <span class="text-sm text-gray-400">Swap Fee</span>
            <span class="text-sm font-semibold text-gray-200 tabular-nums">{(Number(tpStatus.swap_fee_bps ?? 0) / 100).toFixed(2)}%</span>
          </div>
          {#if tpStatus.total_lp_tokens != null}
            <div class="flex justify-between items-center">
              <span class="text-sm text-gray-400">LP Tokens</span>
              <span class="text-sm font-semibold text-gray-200 tabular-nums">{formatE8s(tpStatus.total_lp_tokens, 18)}</span>
            </div>
          {/if}
          {#if balances.length > 0}
            {@const poolTokenSymbols = ['icUSD', 'ckUSDT', 'ckUSDC']}
            {@const poolTokenColors = ['#818cf8', '#26A17B', '#2775CA']}
            {@const amounts = balances.map((b: any, i: number) => {
              const decimals = tokens[i]?.decimals ?? (i === 0 ? 8 : 6);
              return Number(b) / Math.pow(10, decimals);
            })}
            {@const total = amounts.reduce((s: number, a: number) => s + a, 0)}
            <div class="pt-2 border-t border-gray-700/30">
              <p class="text-xs text-gray-500 mb-2 uppercase tracking-wider font-medium">Token Ratios</p>
              <div class="flex h-2.5 rounded-full overflow-hidden bg-gray-700 mb-2">
                {#each amounts as amount, i}
                  {@const pct = total > 0 ? (amount / total) * 100 : 0}
                  <div
                    class="h-full transition-all duration-300"
                    style="width: {pct}%; background: {poolTokenColors[i] ?? '#94a3b8'};"
                  ></div>
                {/each}
              </div>
              <div class="flex flex-wrap gap-x-3 gap-y-1">
                {#each amounts as amount, i}
                  {@const pct = total > 0 ? (amount / total) * 100 : 0}
                  <div class="flex items-center gap-1.5">
                    <div class="w-2 h-2 rounded-full" style="background: {poolTokenColors[i] ?? '#94a3b8'};"></div>
                    <span class="text-xs text-gray-400">{poolTokenSymbols[i] ?? `Token ${i}`}</span>
                    <span class="text-xs font-medium text-gray-300 tabular-nums">{pct.toFixed(1)}%</span>
                  </div>
                {/each}
              </div>
            </div>
          {/if}
        </div>
      {/if}
    </div>

    <!-- AMM (Constant Product) -->
    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
      <div class="px-5 py-4 border-b border-gray-700/50">
        <h2 class="text-sm font-semibold text-gray-200">AMM (Constant Product)</h2>
      </div>

      {#if poolsLoading}
        <div class="p-5 space-y-3 animate-pulse">
          <div class="h-4 w-32 bg-gray-700 rounded"></div>
          <div class="h-4 w-24 bg-gray-700 rounded"></div>
        </div>
      {:else if ammPools.length === 0}
        <div class="p-5 text-gray-500 text-sm">No AMM pools found</div>
      {:else}
        {@const activePools = ammPools.filter(p => !p.paused)}
        {@const pausedPools = ammPools.filter(p => p.paused)}
        <div class="p-5 space-y-3">
          <div class="flex justify-between items-center">
            <span class="text-sm text-gray-400">Pools</span>
            <span class="text-sm font-semibold text-gray-200 tabular-nums">
              {activePools.length} active{#if pausedPools.length > 0}<span class="text-gray-500"> / {pausedPools.length} paused</span>{/if}
            </span>
          </div>
          {#each ammPools as pool}
            {@const tokenA = pool.token_a?.toText?.() ?? String(pool.token_a)}
            {@const tokenB = pool.token_b?.toText?.() ?? String(pool.token_b)}
            {@const symbolA = getTokenSymbol(tokenA)}
            {@const symbolB = getTokenSymbol(tokenB)}
            <div class="pt-2 border-t border-gray-700/30">
              <div class="flex justify-between items-center mb-1">
                <span class="text-xs font-medium text-gray-300">{symbolA} / {symbolB}</span>
                {#if pool.paused}
                  <span class="text-xs text-yellow-400">Paused</span>
                {/if}
              </div>
              <div class="flex justify-between items-center">
                <span class="text-xs text-gray-400">Fee</span>
                <span class="text-xs text-gray-300 tabular-nums">{(Number(pool.fee_bps) / 100).toFixed(2)}%</span>
              </div>
              <div class="flex justify-between items-center">
                <span class="text-xs text-gray-400">LP Shares</span>
                <span class="text-xs text-gray-300 tabular-nums">{Number(pool.total_lp_shares).toLocaleString()}</span>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </section>

  <!-- ════════════════════════════════════════════════════════════════════
       Section 4 — Treasury & Revenue
       ════════════════════════════════════════════════════════════════════ -->
  <section>
    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
      <div class="px-5 py-4 border-b border-gray-700/50">
        <h2 class="text-sm font-semibold text-gray-200">Treasury & Revenue</h2>
      </div>

      {#if treasuryLoading}
        <div class="px-5 py-8 text-center text-gray-500 text-sm animate-pulse">Loading treasury data...</div>
      {:else if treasuryError}
        <div class="px-5 py-8 text-center text-red-400 text-sm">{treasuryError}</div>
      {:else}
        <div class="p-5 space-y-5">
          <!-- Treasury stats -->
          {#if treasuryStats}
            <div class="grid grid-cols-1 md:grid-cols-3 gap-4">
              {#if treasuryStats.total_accrued_interest != null || treasuryStats.totalAccruedInterest != null}
                <div class="bg-gray-700/30 rounded-lg p-4">
                  <p class="text-xs text-gray-400 uppercase tracking-wider mb-1">Total Accrued Interest</p>
                  <p class="text-lg font-semibold text-white tabular-nums">
                    {formatE8s(treasuryStats.total_accrued_interest ?? treasuryStats.totalAccruedInterest ?? 0)} <span class="text-sm text-gray-400">icUSD</span>
                  </p>
                </div>
              {/if}
              {#if treasuryStats.pending_amount != null || treasuryStats.pendingAmount != null}
                <div class="bg-gray-700/30 rounded-lg p-4">
                  <p class="text-xs text-gray-400 uppercase tracking-wider mb-1">Pending Treasury</p>
                  <p class="text-lg font-semibold text-white tabular-nums">
                    {formatE8s(treasuryStats.pending_amount ?? treasuryStats.pendingAmount ?? 0)} <span class="text-sm text-gray-400">icUSD</span>
                  </p>
                </div>
              {/if}
              {#if treasuryStats.total_distributed != null || treasuryStats.totalDistributed != null}
                <div class="bg-gray-700/30 rounded-lg p-4">
                  <p class="text-xs text-gray-400 uppercase tracking-wider mb-1">Total Distributed</p>
                  <p class="text-lg font-semibold text-white tabular-nums">
                    {formatE8s(treasuryStats.total_distributed ?? treasuryStats.totalDistributed ?? 0)} <span class="text-sm text-gray-400">icUSD</span>
                  </p>
                </div>
              {/if}
            </div>
          {/if}

          <!-- Interest split visualization -->
          {#if splitEntries.length > 0}
            {@const totalBps = splitEntries.reduce((sum: number, e: any) => sum + Number(e.bps ?? 0), 0)}
            <div class="space-y-3">
              <p class="text-xs text-gray-500 uppercase tracking-wider font-medium">Interest Split</p>
              <div class="flex h-2.5 rounded-full overflow-hidden bg-gray-700">
                {#each splitEntries as entry}
                  {@const dest = entry.destination ?? entry.dest ?? ''}
                  {@const bpsNum = Number(entry.bps ?? 0)}
                  {@const pct = totalBps > 0 ? (bpsNum / totalBps) * 100 : 0}
                  <div
                    class="h-full transition-all duration-300"
                    style="width: {pct}%; background: {splitColors[dest] ?? '#94a3b8'};"
                    title="{splitLabels[dest] ?? dest}: {formatBps(bpsNum)}"
                  ></div>
                {/each}
              </div>
              <div class="flex flex-wrap gap-x-4 gap-y-1.5">
                {#each splitEntries as entry}
                  {@const dest = entry.destination ?? entry.dest ?? ''}
                  <div class="flex items-center gap-1.5">
                    <div class="w-2.5 h-2.5 rounded-full" style="background: {splitColors[dest] ?? '#94a3b8'};"></div>
                    <span class="text-xs text-gray-400">{splitLabels[dest] ?? dest}</span>
                    <span class="text-xs font-medium text-gray-300 tabular-nums">{formatBps(Number(entry.bps ?? 0))}</span>
                  </div>
                {/each}
              </div>
            </div>
          {:else}
            <div class="text-sm text-gray-500">No interest split data available</div>
          {/if}
        </div>
      {/if}
    </div>
  </section>

  <!-- ════════════════════════════════════════════════════════════════════
       Section 4b — Historical Trends
       ════════════════════════════════════════════════════════════════════ -->
  <section>
    <div class="flex items-center justify-between flex-wrap gap-3 mb-4">
      <h2 class="text-sm font-semibold uppercase tracking-wide text-gray-400">Historical Trends</h2>
      <div class="flex gap-1">
        {#each timeRanges as tr}
          <button
            class="px-3 py-1 text-xs rounded-full border transition-all {timeRange === tr.value
              ? 'bg-blue-500 text-white border-blue-500'
              : 'bg-transparent text-gray-400 border-gray-600 hover:border-gray-400 hover:text-gray-200'}"
            onclick={() => timeRange = tr.value}
          >
            {tr.label}
          </button>
        {/each}
      </div>
    </div>

    {#if chartsLoading}
      <div class="flex items-center justify-center py-16 text-gray-500 text-sm animate-pulse">
        Loading historical data...
      </div>
    {:else if chartsError}
      <div class="text-center py-8 text-red-400 text-sm">{chartsError}</div>
    {:else if filteredSnapshots.length < 2}
      <div class="flex items-center justify-center py-16 text-gray-500">
        No historical data available for this range. Snapshots are captured hourly.
      </div>
    {:else}
      <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <!-- TVL Chart -->
        <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-5">
          <div class="flex items-baseline justify-between mb-3">
            <h3 class="text-xs font-medium text-gray-400 uppercase tracking-wide">TVL (USD)</h3>
            <span class="text-sm font-mono text-emerald-400">{formatCompactUsd(latestTvlChart)}</span>
          </div>
          <div class="relative" style="padding-left: 2.5rem;">
            <div class="absolute left-0 top-0 bottom-0 w-9 pointer-events-none">
              {#each tvlLabels as lbl}
                <span
                  class="absolute right-0 text-[10px] text-gray-500 -translate-y-1/2 whitespace-nowrap"
                  style="top: {lbl.y}px"
                >{lbl.label}</span>
              {/each}
            </div>
            <svg viewBox="0 0 {chartW} {chartH}" class="w-full h-auto block" preserveAspectRatio="none">
              <defs>
                <linearGradient id="tvl-grad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stop-color="#10b981" stop-opacity="0.25" />
                  <stop offset="100%" stop-color="#10b981" stop-opacity="0.02" />
                </linearGradient>
              </defs>
              {#if tvlFill}<polygon points={tvlFill} fill="url(#tvl-grad)" />{/if}
              {#if tvlPoints}<polyline points={tvlPoints} fill="none" stroke="#10b981" stroke-width="2" stroke-linejoin="round" />{/if}
            </svg>
          </div>
        </div>

        <!-- Debt Chart -->
        <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-5">
          <div class="flex items-baseline justify-between mb-3">
            <h3 class="text-xs font-medium text-gray-400 uppercase tracking-wide">Total Debt (icUSD)</h3>
            <span class="text-sm font-mono text-purple-400">{formatCompactUsd(latestDebtChart)}</span>
          </div>
          <div class="relative" style="padding-left: 2.5rem;">
            <div class="absolute left-0 top-0 bottom-0 w-9 pointer-events-none">
              {#each debtLabels as lbl}
                <span
                  class="absolute right-0 text-[10px] text-gray-500 -translate-y-1/2 whitespace-nowrap"
                  style="top: {lbl.y}px"
                >{lbl.label}</span>
              {/each}
            </div>
            <svg viewBox="0 0 {chartW} {chartH}" class="w-full h-auto block" preserveAspectRatio="none">
              <defs>
                <linearGradient id="debt-grad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stop-color="#a855f7" stop-opacity="0.25" />
                  <stop offset="100%" stop-color="#a855f7" stop-opacity="0.02" />
                </linearGradient>
              </defs>
              {#if debtFill}<polygon points={debtFill} fill="url(#debt-grad)" />{/if}
              {#if debtPoints}<polyline points={debtPoints} fill="none" stroke="#a855f7" stroke-width="2" stroke-linejoin="round" />{/if}
            </svg>
          </div>
        </div>

        <!-- CR Chart -->
        <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-5 lg:col-span-2">
          <div class="flex items-baseline justify-between mb-3">
            <h3 class="text-xs font-medium text-gray-400 uppercase tracking-wide">System Collateral Ratio (%)</h3>
            <span class="text-sm font-mono text-yellow-400">{latestCrChart.toFixed(1)}%</span>
          </div>
          <div class="relative" style="padding-left: 2.5rem;">
            <div class="absolute left-0 top-0 bottom-0 w-9 pointer-events-none">
              {#each crLabels as lbl}
                <span
                  class="absolute right-0 text-[10px] text-gray-500 -translate-y-1/2 whitespace-nowrap"
                  style="top: {lbl.y}px"
                >{lbl.label}</span>
              {/each}
            </div>
            <svg viewBox="0 0 {chartW} {chartH}" class="w-full h-auto block" preserveAspectRatio="none">
              <defs>
                <linearGradient id="cr-grad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stop-color="#eab308" stop-opacity="0.25" />
                  <stop offset="100%" stop-color="#eab308" stop-opacity="0.02" />
                </linearGradient>
              </defs>
              {#if crFill}<polygon points={crFill} fill="url(#cr-grad)" />{/if}
              {#if crPoints}<polyline points={crPoints} fill="none" stroke="#eab308" stroke-width="2" stroke-linejoin="round" />{/if}
            </svg>
          </div>
        </div>
      </div>
    {/if}
  </section>

  <!-- ════════════════════════════════════════════════════════════════════
       Section 5 — Liquidation Health
       ════════════════════════════════════════════════════════════════════ -->
  <section class="space-y-4">
    <h2 class="text-sm font-semibold text-gray-200 px-1">Liquidation Health</h2>

    {#if liquidationLoading}
      <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-8 text-center text-gray-500 text-sm animate-pulse">
        Loading liquidation data...
      </div>
    {:else if liquidationError}
      <div class="bg-gray-800/50 border border-red-800/30 rounded-xl p-6 text-center text-red-400 text-sm">
        {liquidationError}
      </div>
    {:else}
      <!-- Bot stats + pending liquidations cards -->
      <div class="grid grid-cols-1 md:grid-cols-3 gap-4">
        {#if botStats}
          <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-4">
            <p class="text-xs text-gray-400 uppercase tracking-wider mb-1">Bot Budget Remaining</p>
            <p class="text-lg font-semibold text-white tabular-nums">
              {formatE8s(botStats.budget_remaining_e8s ?? 0n)} <span class="text-sm text-gray-400">ICP</span>
            </p>
            <p class="text-xs text-gray-500 mt-0.5">of {formatE8s(botStats.budget_total_e8s ?? 0n)} total</p>
          </div>
          <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-4">
            <p class="text-xs text-gray-400 uppercase tracking-wider mb-1">Debt Covered</p>
            <p class="text-lg font-semibold text-white tabular-nums">
              {formatE8s(botStats.total_debt_covered_e8s ?? 0n)} <span class="text-sm text-gray-400">icUSD</span>
            </p>
          </div>
          <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-4">
            <p class="text-xs text-gray-400 uppercase tracking-wider mb-1">icUSD Deposited</p>
            <p class="text-lg font-semibold text-white tabular-nums">
              {formatE8s(botStats.total_icusd_deposited_e8s ?? 0n)} <span class="text-sm text-gray-400">icUSD</span>
            </p>
          </div>
        {:else}
          <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-4 col-span-full text-gray-500 text-sm">
            Bot stats unavailable
          </div>
        {/if}
      </div>

      <!-- Pending liquidations notice -->
      {#if liquidatableVaults.length > 0}
        <div class="bg-red-500/10 border border-red-500/30 rounded-xl p-4 flex items-center gap-3">
          <span class="text-red-400 font-semibold text-sm">{liquidatableVaults.length} liquidatable vault{liquidatableVaults.length > 1 ? 's' : ''}</span>
          <span class="text-red-400/70 text-xs">Vaults awaiting liquidation processing</span>
        </div>
      {/if}

      <!-- At-risk vaults table -->
      <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
        <div class="px-5 py-4 border-b border-gray-700/50 flex items-center justify-between">
          <h3 class="text-sm font-semibold text-gray-200">At-Risk Vaults</h3>
          {#if atRiskVaults.length > 0}
            <span class="text-xs font-medium px-2 py-0.5 rounded-full bg-orange-400/10 text-orange-400 border border-orange-400/30">
              {atRiskVaults.length} vault{atRiskVaults.length > 1 ? 's' : ''}
            </span>
          {:else}
            <span class="text-xs font-medium px-2 py-0.5 rounded-full bg-green-400/10 text-green-400 border border-green-400/30">
              All healthy
            </span>
          {/if}
        </div>

        {#if atRiskVaults.length === 0}
          <div class="px-5 py-8 text-center text-gray-500 text-sm">No vaults near liquidation</div>
        {:else}
          <div class="overflow-x-auto">
            <table class="w-full text-sm">
              <thead>
                <tr class="border-b border-gray-700/50">
                  <th class="px-4 py-2.5 text-xs font-medium text-gray-400 uppercase tracking-wider text-left">Vault</th>
                  <th class="px-4 py-2.5 text-xs font-medium text-gray-400 uppercase tracking-wider text-left">Owner</th>
                  <th class="px-4 py-2.5 text-xs font-medium text-gray-400 uppercase tracking-wider text-left">Collateral</th>
                  <th class="px-4 py-2.5 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Debt</th>
                  <th class="px-4 py-2.5 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">CR</th>
                  <th class="px-4 py-2.5 text-xs font-medium text-gray-400 uppercase tracking-wider text-left w-48">Health</th>
                </tr>
              </thead>
              <tbody>
                {#each atRiskVaults as vault}
                  {@const health = classifyVaultHealth(vault.collateral_ratio, vault.liquidation_ratio, vault.borrow_threshold_ratio)}
                  <tr class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors">
                    <td class="px-4 py-2.5">
                      <EntityLink type="vault" value={String(vault.vault_id)} />
                    </td>
                    <td class="px-4 py-2.5">
                      <EntityLink type="address" value={vault.owner} />
                    </td>
                    <td class="px-4 py-2.5">
                      <TokenBadge symbol={vault.symbol} principalId={vault.collateral_type} size="sm" />
                    </td>
                    <td class="px-4 py-2.5 text-right text-gray-200 tabular-nums text-xs">
                      {formatE8s(vault.debt)} icUSD
                    </td>
                    <td class="px-4 py-2.5 text-right tabular-nums">
                      <span class="{healthColor(health)} font-medium text-xs">{formatCR(vault.collateral_ratio)}</span>
                    </td>
                    <td class="px-4 py-2.5">
                      <VaultHealthBar collateralRatio={vault.collateral_ratio * 100} liquidationRatio={vault.liquidation_ratio * 100} borrowThreshold={vault.borrow_threshold_ratio * 100} />
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </div>
    {/if}
  </section>

  <!-- ════════════════════════════════════════════════════════════════════
       Section 6 — Recent Activity
       ════════════════════════════════════════════════════════════════════ -->
  <section>
    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
      <div class="px-5 py-4 border-b border-gray-700/50 flex items-center justify-between">
        <h2 class="text-sm font-semibold text-gray-200">Recent Activity</h2>
        <a href="/explorer/events" class="text-xs text-blue-400 hover:text-blue-300 transition-colors">
          View All Events &rarr;
        </a>
      </div>

      {#if eventsLoading}
        <div class="px-5 py-8 text-center text-gray-500 text-sm animate-pulse">Loading events...</div>
      {:else if eventsError}
        <div class="px-5 py-8 text-center text-red-400 text-sm">{eventsError}</div>
      {:else if recentEvents.length === 0}
        <div class="px-5 py-8 text-center text-gray-500 text-sm">No recent events</div>
      {:else}
        <div class="divide-y divide-gray-700/30">
          {#each recentEvents as [globalIndex, event]}
            <EventRow {event} index={Number(globalIndex)} {vaultCollateralMap} />
          {/each}
        </div>
      {/if}
    </div>
  </section>

</div>
