<script lang="ts">
  import { onMount } from 'svelte';
  import DashboardCard from '$lib/components/explorer/DashboardCard.svelte';
  import CollateralBreakdownTable from '$lib/components/explorer/CollateralBreakdownTable.svelte';
  import LiquidationRiskTable from '$lib/components/explorer/LiquidationRiskTable.svelte';
  import RevenueBreakdown from '$lib/components/explorer/RevenueBreakdown.svelte';
  import ActivityFeed from '$lib/components/explorer/ActivityFeed.svelte';
  import { fetchProtocolStatus, fetchAllCollateralConfigs } from '$lib/services/explorer/explorerDataLayer';
  import { fetchEvents, fetchAllVaults, explorerEvents, explorerEventsTotalCount } from '$lib/stores/explorerStore';
  import { stabilityPoolService } from '$lib/services/stabilityPoolService';
  import { threePoolService, calculateTheoreticalApy, POOL_TOKENS } from '$lib/services/threePoolService';
  import { formatE8s } from '$lib/services/stabilityPoolService';
  import type { ProtocolStatusDTO, CollateralInfo } from '$lib/services/types';
  import { ApiClient } from '$lib/services/protocol/apiClient';
  import { resolveCollateralSymbol } from '$lib/utils/eventFormatters';

  const E8S = 100_000_000;

  // ── State ───────────────────────────────────────────────────────────────────

  let protocolStatus: ProtocolStatusDTO | null = $state(null);
  let collateralConfigs: CollateralInfo[] = $state([]);
  let allVaults: any[] = $state([]);
  let spStatus: any = $state(null);
  let tpStatus: any = $state(null);
  let tpApy: number | null = $state(null);
  let liquidatableVaults: any[] = $state([]);
  let recentEvents: Array<{ event: any; globalIndex: number }> = $state([]);
  let vaultCollateralMap: Map<number, any> = $state(new Map());
  let spLiquidations: any[] = $state([]);

  // Loading states
  let heroLoading = $state(true);
  let collateralLoading = $state(true);
  let poolsLoading = $state(true);
  let riskLoading = $state(true);
  let eventsLoading = $state(true);
  let revenueLoading = $state(true);

  // Error states
  let heroError = $state(false);
  let collateralError = $state(false);
  let poolsError = $state(false);

  // ── Derived values ─────────────────────────────────────────────────────────

  let isRecoveryMode = $derived.by(() => {
    const mode = protocolStatus?.mode;
    return mode && typeof mode === 'object' && 'Recovery' in mode;
  });

  let totalTvl = $derived.by(() => {
    if (!collateralConfigs.length || !allVaults.length) return 0;
    let total = 0;
    for (const vault of allVaults) {
      const collateralType = vault.collateral_type?.toText?.() ?? String(vault.collateral_type);
      const config = collateralConfigs.find(c => c.principal === collateralType);
      if (!config) continue;
      const amount = Number(vault.collateral_amount) / Math.pow(10, config.decimals);
      total += amount * config.price;
    }
    // Add SP deposits
    if (spStatus) {
      total += Number(spStatus.total_deposits_e8s) / E8S;
    }
    // Add 3pool TVL
    if (tpStatus) {
      for (let i = 0; i < tpStatus.balances.length; i++) {
        const decimals = tpStatus.tokens?.[i]?.decimals ?? (i === 0 ? 8 : 6);
        total += Number(tpStatus.balances[i]) / Math.pow(10, decimals);
      }
    }
    return total;
  });

  let totalOpenVaults = $derived(allVaults.filter((v: any) => Number(v.borrowed_icusd_amount) > 0 || Number(v.collateral_amount) > 0).length);

  let mintCapUtilization = $derived.by(() => {
    if (!collateralConfigs.length) return 0;
    const totalCeiling = collateralConfigs.reduce((sum, c) => sum + c.debtCeiling, 0);
    if (totalCeiling === 0) return 0;
    const totalBorrowed = (protocolStatus?.totalIcusdBorrowed ?? 0) * E8S;
    return (totalBorrowed / totalCeiling) * 100;
  });

  // Collateral rows for the breakdown table
  let collateralRows = $derived.by(() => {
    return collateralConfigs.map(config => {
      const matchingVaults = allVaults.filter((v: any) => {
        const vType = v.collateral_type?.toText?.() ?? String(v.collateral_type);
        return vType === config.principal;
      });
      const totalCollateral = matchingVaults.reduce((sum: number, v: any) => sum + Number(v.collateral_amount), 0);
      const totalDebt = matchingVaults.reduce((sum: number, v: any) => sum + Number(v.borrowed_icusd_amount), 0);
      return {
        config,
        vaultCount: matchingVaults.length,
        totalCollateral,
        totalDebt,
      };
    });
  });

  // At-risk vaults (sorted by CR ascending)
  let atRiskVaults = $derived.by(() => {
    if (!allVaults.length || !collateralConfigs.length) return [];

    const vaultsWithCr: any[] = [];
    for (const vault of allVaults) {
      const debt = Number(vault.borrowed_icusd_amount);
      if (debt === 0) continue;

      const collateralType = vault.collateral_type?.toText?.() ?? String(vault.collateral_type);
      const config = collateralConfigs.find(c => c.principal === collateralType);
      if (!config || config.price === 0) continue;

      const collateralValue = (Number(vault.collateral_amount) / Math.pow(10, config.decimals)) * config.price;
      const debtValue = debt / E8S;
      const cr = debtValue > 0 ? (collateralValue / debtValue) * 100 : Infinity;

      // Show vaults within 150% of liquidation ratio (generous threshold)
      if (cr < config.liquidationCr * 1.5) {
        vaultsWithCr.push({
          vault_id: Number(vault.vault_id),
          owner: vault.owner?.toText?.() ?? String(vault.owner),
          collateral_ratio: cr,
          collateral_type: vault.collateral_type,
          liquidation_ratio: config.liquidationCr,
          borrowed_icusd_amount: debt,
          collateral_amount: Number(vault.collateral_amount),
          collateral_decimals: config.decimals,
        });
      }
    }
    vaultsWithCr.sort((a, b) => a.collateral_ratio - b.collateral_ratio);
    return vaultsWithCr.slice(0, 5);
  });

  let totalAtRiskCount = $derived.by(() => {
    if (!allVaults.length || !collateralConfigs.length) return 0;
    let count = 0;
    for (const vault of allVaults) {
      const debt = Number(vault.borrowed_icusd_amount);
      if (debt === 0) continue;
      const collateralType = vault.collateral_type?.toText?.() ?? String(vault.collateral_type);
      const config = collateralConfigs.find(c => c.principal === collateralType);
      if (!config || config.price === 0) continue;
      const collateralValue = (Number(vault.collateral_amount) / Math.pow(10, config.decimals)) * config.price;
      const cr = (collateralValue / (debt / E8S)) * 100;
      if (cr < config.liquidationCr * 1.2) count++;
    }
    return count;
  });

  // 3Pool TVL in USD
  let threePoolTvl = $derived.by(() => {
    if (!tpStatus) return 0;
    let total = 0;
    for (let i = 0; i < tpStatus.balances.length; i++) {
      const decimals = tpStatus.tokens?.[i]?.decimals ?? (i === 0 ? 8 : 6);
      total += Number(tpStatus.balances[i]) / Math.pow(10, decimals);
    }
    return total;
  });

  // 3Pool token ratios
  let threePoolRatios = $derived.by(() => {
    if (!tpStatus || !tpStatus.balances.length) return [];
    const amounts: number[] = [];
    for (let i = 0; i < tpStatus.balances.length; i++) {
      const decimals = tpStatus.tokens?.[i]?.decimals ?? (i === 0 ? 8 : 6);
      amounts.push(Number(tpStatus.balances[i]) / Math.pow(10, decimals));
    }
    const total = amounts.reduce((s, a) => s + a, 0);
    if (total === 0) return amounts.map((_, i) => ({ symbol: POOL_TOKENS[i]?.symbol ?? `Token ${i}`, pct: 0, color: POOL_TOKENS[i]?.color ?? '#94a3b8' }));
    return amounts.map((a, i) => ({
      symbol: POOL_TOKENS[i]?.symbol ?? `Token ${i}`,
      pct: (a / total) * 100,
      color: POOL_TOKENS[i]?.color ?? '#94a3b8',
    }));
  });

  // ── Format helpers ──────────────────────────────────────────────────────────

  function formatUsd(value: number): string {
    if (value >= 1_000_000) return `$${(value / 1_000_000).toFixed(2)}M`;
    if (value >= 1_000) return `$${(value / 1_000).toFixed(1)}K`;
    return `$${value.toFixed(2)}`;
  }

  function formatCompact(value: number): string {
    if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(2)}M`;
    if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
    return value.toFixed(2);
  }

  // ── Data loading ────────────────────────────────────────────────────────────

  onMount(async () => {
    // Fire all data fetches in parallel, handling each independently
    const heroPromise = (async () => {
      try {
        protocolStatus = await fetchProtocolStatus();
      } catch (e) {
        console.error('Failed to load protocol status:', e);
        heroError = true;
      } finally {
        revenueLoading = false;
      }
    })();

    const collateralPromise = (async () => {
      try {
        collateralConfigs = await fetchAllCollateralConfigs();
      } catch (e) {
        console.error('Failed to load collateral configs:', e);
        collateralError = true;
      }
    })();

    const vaultsPromise = (async () => {
      try {
        const vaults = await fetchAllVaults();
        allVaults = vaults;
        const map = new Map<number, any>();
        for (const v of vaults) {
          map.set(Number(v.vault_id), v.collateral_type);
        }
        vaultCollateralMap = map;
      } catch (e) {
        console.error('Failed to load vaults:', e);
      }
    })();

    const spPromise = (async () => {
      try {
        spStatus = await stabilityPoolService.getPoolStatus();
        try {
          spLiquidations = await stabilityPoolService.getLiquidationHistory(20);
        } catch {}
      } catch (e) {
        console.error('Failed to load SP status:', e);
        poolsError = true;
      }
    })();

    const tpPromise = (async () => {
      try {
        tpStatus = await threePoolService.getPoolStatus();
      } catch (e) {
        console.error('Failed to load 3Pool status:', e);
      }
    })();

    const eventsPromise = (async () => {
      try {
        await fetchEvents(0);
      } catch (e) {
        console.error('Failed to load events:', e);
      }
    })();

    // Wait for all to settle
    await Promise.allSettled([heroPromise, collateralPromise, vaultsPromise, spPromise, tpPromise, eventsPromise]);

    // Compute derived data that needs multiple sources
    heroLoading = false;
    collateralLoading = false;
    poolsLoading = false;
    riskLoading = false;

    // Get recent events from the store
    const unsub = explorerEvents.subscribe(evts => {
      recentEvents = evts.slice(0, 10);
    });
    unsub();
    // Re-subscribe to get current value
    recentEvents = [...recentEvents];
    eventsLoading = false;

    // Calculate 3pool APY if we have both protocol status and pool data
    if (protocolStatus && tpStatus) {
      const threePoolSplit = protocolStatus.interestSplit.find(s => s.destination === 'three_pool');
      if (threePoolSplit && protocolStatus.perCollateralInterest.length > 0) {
        let poolTvlE8s = 0;
        for (let i = 0; i < tpStatus.balances.length; i++) {
          const decimals = tpStatus.tokens?.[i]?.decimals ?? (i === 0 ? 8 : 6);
          // Normalize to e8s
          const balance = Number(tpStatus.balances[i]);
          poolTvlE8s += decimals === 8 ? balance : balance * Math.pow(10, 8 - decimals);
        }
        tpApy = calculateTheoreticalApy(
          threePoolSplit.bps,
          protocolStatus.perCollateralInterest,
          poolTvlE8s
        );
      }
    }
  });
</script>

<div class="dashboard-page max-w-6xl mx-auto px-4 py-6 space-y-6">
  <!-- ── Hero Stats ─────────────────────────────────────────────────────── -->
  <section>
    <div class="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3">
      {#if heroLoading}
        {#each Array(6) as _}
          <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-5 animate-pulse">
            <div class="h-3 w-20 bg-gray-700 rounded mb-2"></div>
            <div class="h-7 w-16 bg-gray-700 rounded"></div>
          </div>
        {/each}
      {:else if heroError}
        <div class="col-span-full bg-gray-800/50 border border-red-800/30 rounded-xl p-5 text-center text-red-400 text-sm">
          Failed to load protocol status
        </div>
      {:else}
        <DashboardCard
          label="Protocol Mode"
          value={isRecoveryMode ? 'Recovery' : 'Normal'}
          subtitle={isRecoveryMode ? 'Elevated liquidation risk' : 'Operating normally'}
          trend={isRecoveryMode ? 'down' : 'up'}
        />
        <DashboardCard
          label="Global CR"
          value="{protocolStatus?.totalCollateralRatio?.toFixed(0) ?? '—'}%"
          subtitle="Total collateral ratio"
        />
        <DashboardCard
          label="Total TVL"
          value={formatUsd(totalTvl)}
          subtitle="Across all pools"
        />
        <DashboardCard
          label="icUSD Supply"
          value={formatCompact(protocolStatus?.totalIcusdBorrowed ?? 0)}
          subtitle="Total minted"
        />
        <DashboardCard
          label="Open Vaults"
          value={String(totalOpenVaults)}
          subtitle="Active positions"
        />
        <DashboardCard
          label="Mint Cap"
          value="{mintCapUtilization.toFixed(1)}%"
          subtitle="Debt ceiling usage"
          trend={mintCapUtilization > 80 ? 'down' : mintCapUtilization > 50 ? 'neutral' : 'up'}
        />
      {/if}
    </div>
  </section>

  <!-- ── Collateral Breakdown ───────────────────────────────────────────── -->
  <section>
    <CollateralBreakdownTable rows={collateralRows} loading={collateralLoading} />
  </section>

  <!-- ── Stability Pool + 3Pool Side-by-Side ────────────────────────────── -->
  <section class="grid grid-cols-1 lg:grid-cols-2 gap-4">
    <!-- Stability Pool Card -->
    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
      <div class="px-5 py-4 border-b border-gray-700/50">
        <h3 class="text-sm font-semibold text-gray-200">Stability Pool</h3>
      </div>
      {#if poolsLoading}
        <div class="p-5 space-y-3 animate-pulse">
          <div class="h-4 w-32 bg-gray-700 rounded"></div>
          <div class="h-4 w-24 bg-gray-700 rounded"></div>
        </div>
      {:else if !spStatus}
        <div class="p-5 text-gray-500 text-sm">Failed to load</div>
      {:else}
        <div class="p-5 space-y-3">
          <div class="flex justify-between items-center">
            <span class="text-sm text-gray-400">Total Deposits</span>
            <span class="text-sm font-semibold text-gray-200 tabular-nums">{formatE8s(spStatus.total_deposits_e8s)} icUSD</span>
          </div>
          <div class="flex justify-between items-center">
            <span class="text-sm text-gray-400">Depositors</span>
            <span class="text-sm font-semibold text-gray-200 tabular-nums">{Number(spStatus.total_depositors)}</span>
          </div>
          <div class="flex justify-between items-center">
            <span class="text-sm text-gray-400">Liquidations Executed</span>
            <span class="text-sm font-semibold text-gray-200 tabular-nums">{Number(spStatus.total_liquidations_executed)}</span>
          </div>
          {#if spStatus.collateral_gains && spStatus.collateral_gains.length > 0}
            <div class="pt-2 border-t border-gray-700/30">
              <p class="text-xs text-gray-500 mb-2 uppercase tracking-wider font-medium">Collateral Gains</p>
              <div class="space-y-1">
                {#each spStatus.collateral_gains as [ledger, amount]}
                  {@const symbol = ledger?.toText ? resolveCollateralSymbol(ledger) : '?'}
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

    <!-- 3Pool Card -->
    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
      <div class="px-5 py-4 border-b border-gray-700/50">
        <h3 class="text-sm font-semibold text-gray-200">3Pool (StableSwap)</h3>
      </div>
      {#if poolsLoading}
        <div class="p-5 space-y-3 animate-pulse">
          <div class="h-4 w-32 bg-gray-700 rounded"></div>
          <div class="h-4 w-24 bg-gray-700 rounded"></div>
        </div>
      {:else if !tpStatus}
        <div class="p-5 text-gray-500 text-sm">Failed to load</div>
      {:else}
        <div class="p-5 space-y-3">
          <div class="flex justify-between items-center">
            <span class="text-sm text-gray-400">TVL</span>
            <span class="text-sm font-semibold text-gray-200 tabular-nums">{formatUsd(threePoolTvl)}</span>
          </div>
          {#if tpApy !== null}
            <div class="flex justify-between items-center">
              <span class="text-sm text-gray-400">Theoretical APY</span>
              <span class="text-sm font-semibold text-green-400 tabular-nums">{(tpApy * 100).toFixed(2)}%</span>
            </div>
          {/if}
          <div class="flex justify-between items-center">
            <span class="text-sm text-gray-400">Swap Fee</span>
            <span class="text-sm font-semibold text-gray-200 tabular-nums">{Number(tpStatus.swap_fee_bps) / 100}%</span>
          </div>
          {#if threePoolRatios.length > 0}
            <div class="pt-2 border-t border-gray-700/30">
              <p class="text-xs text-gray-500 mb-2 uppercase tracking-wider font-medium">Token Ratios</p>
              <!-- Ratio bar -->
              <div class="flex h-2.5 rounded-full overflow-hidden bg-gray-700 mb-2">
                {#each threePoolRatios as ratio}
                  <div
                    class="h-full transition-all duration-300"
                    style="width: {ratio.pct}%; background: {ratio.color};"
                  ></div>
                {/each}
              </div>
              <div class="flex flex-wrap gap-x-3 gap-y-1">
                {#each threePoolRatios as ratio}
                  <div class="flex items-center gap-1.5">
                    <div class="w-2 h-2 rounded-full" style="background: {ratio.color};"></div>
                    <span class="text-xs text-gray-400">{ratio.symbol}</span>
                    <span class="text-xs font-medium text-gray-300 tabular-nums">{ratio.pct.toFixed(1)}%</span>
                  </div>
                {/each}
              </div>
            </div>
          {/if}
        </div>
      {/if}
    </div>
  </section>

  <!-- ── Treasury & Revenue ─────────────────────────────────────────────── -->
  <section>
    <RevenueBreakdown
      interestSplit={protocolStatus?.interestSplit ?? []}
      weightedAvgRate={protocolStatus?.weightedAverageInterestRate ?? 0}
      loading={revenueLoading}
    />
  </section>

  <!-- ── Liquidation Risk ───────────────────────────────────────────────── -->
  <section>
    <LiquidationRiskTable
      vaults={atRiskVaults}
      totalAtRisk={totalAtRiskCount}
      loading={riskLoading}
    />
  </section>

  <!-- ── Recent Activity Feed ───────────────────────────────────────────── -->
  <section>
    <ActivityFeed
      events={recentEvents}
      loading={eventsLoading}
      {vaultCollateralMap}
    />
  </section>
</div>
