<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { Principal } from '@dfinity/principal';
  import OhlcChart from '$components/explorer/OhlcChart.svelte';
  import PriceLineChart from '$components/explorer/PriceLineChart.svelte';
  import VolatilityGauge from '$components/explorer/VolatilityGauge.svelte';
  import CeilingBar from '$components/explorer/CeilingBar.svelte';
  import {
    fetchOhlc, fetchTwap, fetchVolatility, fetchPriceSeries
  } from '$services/explorer/analyticsService';
  import {
    fetchCollateralConfigs, fetchCollateralTotals, fetchAllVaults
  } from '$services/explorer/explorerService';
  import {
    e8sToNumber, formatCompact, COLLATERAL_SYMBOLS, COLLATERAL_COLORS,
    getCollateralSymbol
  } from '$utils/explorerChartHelpers';
  import { decodeRustDecimal } from '$utils/decimalUtils';
  import type { OhlcCandle } from '$declarations/rumi_analytics/rumi_analytics.did';

  const tokenId = $derived($page.params.id);
  const symbol = $derived(
    COLLATERAL_SYMBOLS[tokenId as keyof typeof COLLATERAL_SYMBOLS]
      ?? getCollateralSymbol(tokenId)
  );
  const assetColor = $derived(
    COLLATERAL_COLORS[symbol as keyof typeof COLLATERAL_COLORS] ?? '#888'
  );

  // Chart view state
  type ChartView = 'candle' | 'line';
  let chartView: ChartView = $state('candle');

  // Bucket size for OHLC
  type BucketSize = '1h' | '4h' | '1d';
  let bucketSize: BucketSize = $state('4h');
  const BUCKET_SECS: Record<BucketSize, bigint> = {
    '1h': 3600n,
    '4h': 14400n,
    '1d': 86400n,
  };

  // Data state
  let candles: OhlcCandle[] = $state([]);
  let candleLoading = $state(true);

  let priceData: { timestamp_ns: bigint; price: number }[] = $state([]);
  let priceLoading = $state(true);

  let volatilityValue: number | null = $state(null);
  let currentPrice = $state(0);
  let twapPrice = $state(0);
  let config: any = $state(null);
  let vaultCount = $state(0);
  let totalCollateral = $state(0);
  let totalDebt = $state(0);
  let infoLoading = $state(true);
  let initialLoadDone = $state(false);
  let error: string | null = $state(null);

  async function loadCandles() {
    if (!initialLoadDone) return; // Avoid double-fetch on mount
    candleLoading = true;
    try {
      const principal = Principal.fromText(tokenId);
      const result = await fetchOhlc(principal, BUCKET_SECS[bucketSize]);
      candles = result?.candles ?? [];
    } catch (e) {
      console.error('[markets] OHLC fetch error:', e);
      candles = [];
    } finally {
      candleLoading = false;
    }
  }

  onMount(async () => {
    let principal: Principal;
    try {
      principal = Principal.fromText(tokenId);
    } catch {
      error = `Invalid principal: "${tokenId}"`;
      infoLoading = false;
      candleLoading = false;
      priceLoading = false;
      return;
    }

    // Parallel fetch of all data
    const [ohlcResult, twapResult, volResult, priceResult, configsResult, totalsResult, vaultsResult] =
      await Promise.allSettled([
        fetchOhlc(principal, BUCKET_SECS[bucketSize]),
        fetchTwap(),
        fetchVolatility(principal),
        fetchPriceSeries(500),
        fetchCollateralConfigs(),
        fetchCollateralTotals(),
        fetchAllVaults(),
      ]);

    // OHLC candles
    if (ohlcResult.status === 'fulfilled' && ohlcResult.value) {
      candles = ohlcResult.value.candles ?? [];
    }
    candleLoading = false;

    // TWAP / current price
    if (twapResult.status === 'fulfilled' && twapResult.value) {
      const entry = twapResult.value.entries?.find((e: any) => {
        const pid = e.collateral?.toText?.() ?? String(e.collateral);
        return pid === tokenId;
      });
      if (entry) {
        currentPrice = entry.latest_price;
        twapPrice = entry.twap_price;
      }
    }

    // Volatility
    if (volResult.status === 'fulfilled' && volResult.value) {
      volatilityValue = volResult.value.annualized_vol_pct;
    }

    // Price series for line chart
    if (priceResult.status === 'fulfilled' && priceResult.value) {
      priceData = priceResult.value
        .flatMap((snap: any) => {
          const match = snap.prices?.find((p: any) => {
            const pid = typeof p[0] === 'object' ? p[0]?.toText?.() ?? String(p[0]) : String(p[0]);
            return pid === tokenId;
          });
          if (match) {
            return [{ timestamp_ns: snap.timestamp_ns, price: match[1] }];
          }
          return [];
        });
    }
    priceLoading = false;

    // Config
    if (configsResult.status === 'fulfilled') {
      const configs = configsResult.value ?? [];
      config = configs.find((c: any) => {
        const pid = c.ledger_canister_id?.toText?.() ?? String(c.ledger_canister_id);
        return pid === tokenId;
      });
    }

    // Totals
    if (totalsResult.status === 'fulfilled') {
      const tots = totalsResult.value ?? [];
      const match = tots.find((t: any) => {
        const pid = t.collateral_type?.toText?.() ?? String(t.collateral_type ?? '');
        return pid === tokenId;
      });
      if (match) {
        totalCollateral = match.total_collateral_e8s != null ? e8sToNumber(match.total_collateral_e8s) : 0;
        totalDebt = match.total_debt_e8s != null ? e8sToNumber(match.total_debt_e8s) : 0;
        vaultCount = match.vault_count != null ? Number(match.vault_count) : 0;
      }
    }

    // Vault count fallback
    if (vaultCount === 0 && vaultsResult.status === 'fulfilled') {
      const vaults = vaultsResult.value ?? [];
      vaultCount = vaults.filter((v: any) => {
        const ct = v.collateral_type?.toText?.() ?? String(v.collateral_type ?? '');
        return ct === tokenId;
      }).length;
    }

    infoLoading = false;
    initialLoadDone = true;
  });

  // Re-fetch candles when bucket size changes (after initial load)
  $effect(() => {
    const _ = bucketSize;
    if (initialLoadDone) {
      loadCandles();
    }
  });

  // Derived config values
  const liquidationRatio = $derived(config ? decodeRustDecimal(config.liquidation_ratio) : 0);
  const borrowThreshold = $derived(config ? decodeRustDecimal(config.borrow_threshold_ratio) : 0);
  const borrowingFee = $derived(config ? decodeRustDecimal(config.borrowing_fee) : 0);
  const liquidationBonus = $derived(config ? decodeRustDecimal(config.liquidation_bonus) : 0);
  const interestRateApr = $derived(config?.interest_rate_apr ? decodeRustDecimal(config.interest_rate_apr) : 0);
  const debtCeilingRaw = $derived(config?.debt_ceiling ?? 0n);
  const isUnlimited = $derived(
    typeof debtCeilingRaw === 'bigint'
      ? debtCeilingRaw >= 18446744073709551615n
      : Number(debtCeilingRaw) >= Number.MAX_SAFE_INTEGER
  );
  const debtCeiling = $derived(e8sToNumber(Number(debtCeilingRaw)));
  const collateralValueUsd = $derived(totalCollateral * currentPrice);

  function formatPct(val: number, decimals = 1): string {
    return `${(val * 100).toFixed(decimals)}%`;
  }

  function formatPrice(price: number): string {
    if (price >= 10000) return `$${price.toLocaleString(undefined, { maximumFractionDigits: 0 })}`;
    if (price >= 100) return `$${price.toFixed(1)}`;
    if (price >= 1) return `$${price.toFixed(2)}`;
    if (price > 0) return `$${price.toFixed(6)}`;
    return '--';
  }
</script>

<svelte:head>
  <title>{symbol} Market | Rumi Explorer</title>
</svelte:head>

<div class="space-y-4">
  <!-- Back link -->
  <a href="/explorer/markets" class="inline-flex items-center gap-1 text-xs text-gray-500 hover:text-teal-400 transition-colors">
    <svg class="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2"><path stroke-linecap="round" stroke-linejoin="round" d="M15 19l-7-7 7-7"/></svg>
    All Markets
  </a>

  {#if error}
    <div class="explorer-card text-center py-12">
      <p class="text-pink-400 text-sm mb-2">{error}</p>
      <a href="/explorer/markets" class="text-xs text-teal-400 hover:text-teal-300">Back to Markets</a>
    </div>
  {:else}
  <!-- Header -->
  <div class="explorer-card">
    <div class="flex flex-wrap items-center gap-4">
      <div class="flex items-center gap-3">
        <span class="w-4 h-4 rounded-full" style="background: {assetColor};"></span>
        <h1 class="text-xl font-semibold text-gray-100">{symbol}</h1>
      </div>

      {#if infoLoading}
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      {:else}
        <div class="flex items-baseline gap-2">
          <span class="text-2xl font-semibold tabular-nums text-gray-100">
            {formatPrice(currentPrice)}
          </span>
          {#if twapPrice > 0 && twapPrice !== currentPrice}
            <span class="text-sm text-gray-500">
              TWAP: {formatPrice(twapPrice)}
            </span>
          {/if}
        </div>
        <div class="ml-auto">
          <VolatilityGauge value={volatilityValue} />
        </div>
      {/if}
    </div>
  </div>

  <!-- Chart Section -->
  <div>
    <!-- Chart view tabs + bucket selector -->
    <div class="flex items-center justify-between mb-2">
      <div class="flex gap-1">
        <button
          class="px-2.5 py-1 text-xs rounded-md transition-colors {chartView === 'candle' ? 'bg-teal-500/15 text-teal-300' : 'text-gray-500 hover:text-gray-300'}"
          onclick={() => chartView = 'candle'}
        >
          Candlestick
        </button>
        <button
          class="px-2.5 py-1 text-xs rounded-md transition-colors {chartView === 'line' ? 'bg-teal-500/15 text-teal-300' : 'text-gray-500 hover:text-gray-300'}"
          onclick={() => chartView = 'line'}
        >
          Line
        </button>
      </div>

      {#if chartView === 'candle'}
        <div class="flex gap-1">
          {#each (['1h', '4h', '1d'] as const) as size}
            <button
              class="px-2.5 py-1 text-xs rounded-md transition-colors {bucketSize === size ? 'bg-teal-500/15 text-teal-300' : 'text-gray-500 hover:text-gray-300'}"
              onclick={() => bucketSize = size}
            >
              {size}
            </button>
          {/each}
        </div>
      {/if}
    </div>

    {#if chartView === 'candle'}
      <OhlcChart {candles} {symbol} loading={candleLoading} />
    {:else}
      <PriceLineChart data={priceData} {symbol} color={assetColor} loading={priceLoading} />
    {/if}
  </div>

  <!-- Stats Grid -->
  <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
    <div class="explorer-card">
      <div class="text-xs text-gray-500 mb-1">Collateral Locked</div>
      <div class="text-lg font-semibold tabular-nums text-gray-200">
        {infoLoading ? '--' : formatCompact(totalCollateral)}
        <span class="text-xs text-gray-500">{symbol}</span>
      </div>
      {#if !infoLoading && collateralValueUsd > 0}
        <div class="text-xs text-gray-500 mt-0.5">${formatCompact(collateralValueUsd)}</div>
      {/if}
    </div>
    <div class="explorer-card">
      <div class="text-xs text-gray-500 mb-1">Total Debt</div>
      <div class="text-lg font-semibold tabular-nums text-gray-200">
        {infoLoading ? '--' : formatCompact(totalDebt)}
        <span class="text-xs text-gray-500">icUSD</span>
      </div>
    </div>
    <div class="explorer-card">
      <div class="text-xs text-gray-500 mb-1">Vaults</div>
      <div class="text-lg font-semibold tabular-nums text-gray-200">
        {infoLoading ? '--' : vaultCount}
      </div>
    </div>
    <div class="explorer-card">
      <div class="text-xs text-gray-500 mb-1">Debt Ceiling</div>
      {#if infoLoading}
        <div class="text-lg font-semibold text-gray-200">--</div>
      {:else if isUnlimited}
        <div class="text-lg font-semibold text-gray-400">Unlimited</div>
      {:else}
        <div class="mb-1">
          <CeilingBar used={totalDebt} total={debtCeiling} unlimited={false} />
        </div>
        <div class="text-xs text-gray-500 tabular-nums">
          {formatCompact(totalDebt)} / {formatCompact(debtCeiling)} icUSD
        </div>
      {/if}
    </div>
  </div>

  <!-- Config Parameters -->
  {#if config && !infoLoading}
    <div class="explorer-card">
      <h3 class="text-sm font-medium text-gray-300 mb-3">Collateral Parameters</h3>
      <div class="grid grid-cols-1 md:grid-cols-3 gap-x-6 gap-y-0">
        <div class="flex justify-between py-2 border-b" style="border-color: var(--rumi-border);">
          <span class="text-xs text-gray-500">Liquidation Ratio</span>
          <span class="text-sm tabular-nums text-gray-300">{formatPct(liquidationRatio)}</span>
        </div>
        <div class="flex justify-between py-2 border-b" style="border-color: var(--rumi-border);">
          <span class="text-xs text-gray-500">Borrow Threshold</span>
          <span class="text-sm tabular-nums text-gray-300">{formatPct(borrowThreshold)}</span>
        </div>
        <div class="flex justify-between py-2 border-b" style="border-color: var(--rumi-border);">
          <span class="text-xs text-gray-500">Liquidation Bonus</span>
          <span class="text-sm tabular-nums text-gray-300">{formatPct(liquidationBonus)}</span>
        </div>
        <div class="flex justify-between py-2 border-b" style="border-color: var(--rumi-border);">
          <span class="text-xs text-gray-500">Borrowing Fee</span>
          <span class="text-sm tabular-nums text-gray-300">{borrowingFee > 0 ? formatPct(borrowingFee, 2) : '0%'}</span>
        </div>
        <div class="flex justify-between py-2 border-b" style="border-color: var(--rumi-border);">
          <span class="text-xs text-gray-500">Interest APR</span>
          <span class="text-sm tabular-nums text-gray-300">{interestRateApr > 0 ? formatPct(interestRateApr) : '0%'}</span>
        </div>
        <div class="flex justify-between py-2 border-b" style="border-color: var(--rumi-border);">
          <span class="text-xs text-gray-500">Min Vault Debt</span>
          <span class="text-sm tabular-nums text-gray-300">
            {config.min_vault_debt ? `${e8sToNumber(Number(config.min_vault_debt))} icUSD` : '--'}
          </span>
        </div>
      </div>
    </div>
  {/if}

  <!-- Links -->
  <div class="text-center py-2">
    <a href="/explorer/token/{tokenId}" class="text-xs text-gray-500 hover:text-gray-400 transition-colors">
      View full token detail page (vaults, events) &rarr;
    </a>
  </div>
  {/if}
</div>
