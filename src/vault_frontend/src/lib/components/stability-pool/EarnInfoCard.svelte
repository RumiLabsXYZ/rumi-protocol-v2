<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import {
    stabilityPoolService,
    formatE8s,
    formatTokenAmount,
    symbolForLedger,
    decimalsForLedger,
  } from '../../services/stabilityPoolService';
  import { formatStableTokenDisplay } from '../../utils/format';
  import type { PoolStatus, UserPosition, CollateralInfo } from '../../services/stabilityPoolService';
  import type { ProtocolStatusDTO } from '../../services/types';
  import { CANISTER_IDS } from '../../config';

  export let poolStatus: PoolStatus | null = null;
  export let userPosition: UserPosition | null = null;
  export let protocolStatus: ProtocolStatusDTO | null = null;
  export let isConnected = false;

  const dispatch = createEventDispatcher();

  let claimLoading: Record<string, boolean> = {};
  let claimAllLoading = false;
  let toggleLoading: Record<string, boolean> = {};
  let error = '';
  let showOptOutMenu = false;
  let showOptOutTooltip = false;

  // Registries
  $: stablecoinRegistry = poolStatus?.stablecoin_registry ?? [];
  $: collateralRegistry = poolStatus?.collateral_registry ?? [];
  $: registries = { stablecoins: stablecoinRegistry, collateral: collateralRegistry };

  // Pool stats
  $: totalDepositsUsd = poolStatus ? formatStableTokenDisplay(poolStatus.total_deposits_e8s, 8) : '0.0000';
  $: depositorCount = poolStatus ? Number(poolStatus.total_depositors) : 0;
  $: stablecoinBreakdown = poolStatus?.stablecoin_balances ?? [];

  // Per-collateral APY building block: for each collateral type, compute
  // interestRate_C * poolShare * debt_C / eligible_icusd_C (= APR),
  // then convert total APR → APY via daily compounding.
  $: eligibleMap = new Map<string, number>(
    (poolStatus?.eligible_icusd_per_collateral ?? []).map(([p, v]: [any, bigint]) => [p.toText(), Number(v) / 1e8])
  );

  $: poolApy = (() => {
    if (!protocolStatus || !poolStatus) return null;
    const poolShare = (protocolStatus.interestSplit?.find(e => e.destination === 'stability_pool')?.bps ?? 0) / 10000;
    const perC = protocolStatus.perCollateralInterest;
    if (!perC || perC.length === 0 || poolShare === 0) return null;

    let totalApr = 0;
    for (const info of perC) {
      const eligible = eligibleMap.get(info.collateralType) ?? 0;
      if (eligible === 0 || info.totalDebtE8s === 0 || info.weightedInterestRate === 0) continue;
      totalApr += (info.weightedInterestRate * poolShare * info.totalDebtE8s) / eligible;
    }
    if (totalApr === 0) return null;
    const apy = Math.pow(1 + totalApr / 365, 365) - 1;
    return (apy * 100).toFixed(2);
  })();

  // User position data
  $: userStables = userPosition?.stablecoin_balances ?? [];
  $: activeStables = userStables.filter(([_, amount]: [any, bigint]) => amount > 0n);
  $: totalUsdValue = userPosition ? formatStableTokenDisplay(userPosition.total_usd_value_e8s, 8) : '0.0000';
  $: gains = userPosition?.collateral_gains ?? [];
  $: hasAnyGains = gains.some(([_, a]) => a > 0n);
  $: optedOut = new Set((userPosition?.opted_out_collateral ?? []).map(p => p.toText()));

  // Does the user hold icUSD in the pool?
  $: userHasIcusd = userStables.some(([l, a]: [any, bigint]) => l.toText() === CANISTER_IDS.ICUSD_LEDGER && a > 0n);

  // Personalized APY — only sums collateral types the user is opted in to
  $: userApy = (() => {
    if (!userHasIcusd || !protocolStatus || !poolStatus) return null;
    const poolShare = (protocolStatus.interestSplit?.find(e => e.destination === 'stability_pool')?.bps ?? 0) / 10000;
    const perC = protocolStatus.perCollateralInterest;
    if (!perC || perC.length === 0) return null;

    let totalApr = 0;
    for (const info of perC) {
      if (optedOut.has(info.collateralType)) continue;
      const eligible = eligibleMap.get(info.collateralType) ?? 0;
      if (eligible === 0 || info.totalDebtE8s === 0 || info.weightedInterestRate === 0) continue;
      totalApr += (info.weightedInterestRate * poolShare * info.totalDebtE8s) / eligible;
    }
    if (totalApr === 0) return null;
    const apy = Math.pow(1 + totalApr / 365, 365) - 1;
    return (apy * 100).toFixed(2);
  })();

  $: poolShare = (() => {
    if (!poolStatus || !userPosition || poolStatus.total_deposits_e8s === 0n) return '0.00';
    const share = (Number(userPosition.total_usd_value_e8s) / Number(poolStatus.total_deposits_e8s)) * 100;
    return share < 0.01 && share > 0 ? '<0.01' : share.toFixed(2);
  })();

  $: interestEarned = (() => {
    if (!userPosition) return null;
    const earned = (userPosition as any).total_interest_earned_e8s;
    if (!earned || earned === 0n) return null;
    return formatStableTokenDisplay(earned, 8);
  })();

  // Stablecoin dot colors
  const TOKEN_COLORS: Record<string, string> = {
    [CANISTER_IDS.ICUSD_LEDGER]: '#818cf8',
    [CANISTER_IDS.CKUSDT_LEDGER]: '#26A17B',
    [CANISTER_IDS.CKUSDC_LEDGER]: '#2775CA',
  };

  function getStablecoinColor(ledgerId: any): string {
    return TOKEN_COLORS[ledgerId.toText?.()] ?? '#94A3B8';
  }

  // Collateral gain colors
  function getCollateralColor(col: CollateralInfo): string {
    const sym = col.symbol?.toLowerCase?.() ?? '';
    if (sym === 'icp') return '#2DD4BF';
    if (sym.includes('btc')) return '#F7931A';
    if (sym.includes('xaut') || sym.includes('gold')) return '#D4A843';
    if (sym.includes('eth')) return '#627EEA';
    return '#94A3B8';
  }

  async function claimSingle(ledger: Principal) {
    const key = ledger.toText();
    try {
      claimLoading = { ...claimLoading, [key]: true };
      error = '';
      await stabilityPoolService.claimCollateral(ledger);
      dispatch('success', { action: 'claim' });
    } catch (err: any) {
      error = err.message || 'Claim failed';
    } finally {
      claimLoading = { ...claimLoading, [key]: false };
    }
  }

  async function claimAll() {
    try {
      claimAllLoading = true;
      error = '';
      await stabilityPoolService.claimAllCollateral();
      dispatch('success', { action: 'claimAll' });
    } catch (err: any) {
      error = err.message || 'Claim all failed';
    } finally {
      claimAllLoading = false;
    }
  }

  async function toggleOptOut(collateral: CollateralInfo) {
    const key = collateral.ledger_id.toText();
    const isCurrentlyOut = optedOut.has(key);
    try {
      toggleLoading = { ...toggleLoading, [key]: true };
      error = '';
      if (isCurrentlyOut) {
        await stabilityPoolService.optInCollateral(collateral.ledger_id);
      } else {
        await stabilityPoolService.optOutCollateral(collateral.ledger_id);
      }
      dispatch('success', { action: isCurrentlyOut ? 'optIn' : 'optOut' });
    } catch (err: any) {
      error = err.message || 'Toggle failed';
    } finally {
      toggleLoading = { ...toggleLoading, [key]: false };
    }
  }

  function closeOptOutMenu() { showOptOutMenu = false; }
</script>

<svelte:window on:click={closeOptOutMenu} />

<div class="info-card">
  <!-- YOUR POSITION section (only when connected with position) -->
  {#if isConnected && userPosition}
    <h4 class="group-heading">Your Position</h4>
    <div class="stats-stack">
      <!-- Personalized Interest APY (top row, only if user holds icUSD) -->
      {#if userApy !== null}
        <div class="stat-row">
          <span class="stat-label">Your Interest APY</span>
          <span class="stat-value green">{userApy}%</span>
        </div>
      {/if}

      <!-- Total Deposited with optional stablecoin breakdown -->
      <div class="stat-row" class:align-top={activeStables.length > 1}>
        <span class="stat-label">Total Deposited</span>
        {#if activeStables.length > 1}
          <span class="stat-value-stack">
            <span class="stat-value bold">${totalUsdValue}</span>
            {#each activeStables as [ledger, amount]}
              {@const sym = symbolForLedger(ledger, registries)}
              {@const dec = decimalsForLedger(ledger, registries)}
              <span class="breakdown-line">
                <span class="collateral-dot breakdown-dot" style="background:{getStablecoinColor(ledger)}"></span>
                <span class="breakdown-ticker">{sym}</span>
                <span class="gain-value">{formatStableTokenDisplay(amount, dec)}</span>
              </span>
            {/each}
          </span>
        {:else}
          <span class="stat-value bold">${totalUsdValue}</span>
        {/if}
      </div>

      <div class="stat-row">
        <span class="stat-label">Pool Share</span>
        <span class="stat-value">{poolShare}%</span>
      </div>

      <!-- Collateral Gains row (stacked values, ticker-first) -->
      <div class="stat-row align-top">
        <span class="stat-label">
          Collateral Gains
          <span class="opt-out-inline" on:click|stopPropagation>
            {#if hasAnyGains}
              <button class="claim-all-inline" on:click={claimAll} disabled={claimAllLoading}>
                {claimAllLoading ? '…' : 'Claim'}
              </button>
            {/if}
            <!-- svelte-ignore a11y-mouse-events-have-key-events -->
            <button
              class="opt-out-btn-inline"
              on:mouseover={() => { showOptOutTooltip = true; }}
              on:mouseleave={() => { showOptOutTooltip = false; }}
              on:click|stopPropagation={() => { showOptOutMenu = !showOptOutMenu; showOptOutTooltip = false; }}
            >
              Opt out
              <svg class="info-icon" width="10" height="10" viewBox="0 0 16 16" fill="none">
                <circle cx="8" cy="8" r="7" stroke="currentColor" stroke-width="1.5"/>
                <path d="M8 7v4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>
                <circle cx="8" cy="5" r="0.75" fill="currentColor"/>
              </svg>
            </button>
            {#if showOptOutTooltip && !showOptOutMenu}
              <div class="opt-out-tooltip">
                Choose which collateral types you receive during liquidations. Opted-out collateral is redistributed to other depositors.
              </div>
            {/if}
            {#if showOptOutMenu}
              <div class="opt-out-menu">
                {#each collateralRegistry as collateral}
                  {@const key = collateral.ledger_id.toText()}
                  {@const isOut = optedOut.has(key)}
                  <button
                    class="opt-out-row" class:is-out={isOut}
                    on:click={() => toggleOptOut(collateral)}
                    disabled={toggleLoading[key]}
                  >
                    <span class="opt-out-symbol">{collateral.symbol}</span>
                    {#if toggleLoading[key]}
                      <span class="mini-spinner"></span>
                    {:else}
                      <span class="opt-out-status" class:opted-out-label={isOut}>{isOut ? 'Opted out' : 'Receiving'}</span>
                    {/if}
                  </button>
                {/each}
              </div>
            {/if}
          </span>
        </span>
        <span class="stat-value-stack">
          {#each collateralRegistry as col}
            {@const key = col.ledger_id.toText()}
            {@const gainEntry = gains.find(([l]) => l.toText() === key)}
            {@const gainAmount = gainEntry ? gainEntry[1] : 0n}
            <span class="gain-line" class:gain-dim={gainAmount === 0n}>
              <span class="collateral-dot" style="background:{getCollateralColor(col)}"></span>
              <span class="gain-ticker">{col.symbol}</span>
              <span class="gain-value">{formatTokenAmount(gainAmount, col.decimals)}</span>
            </span>
          {/each}
        </span>
      </div>

      <!-- Interest earned -->
      {#if interestEarned}
        <div class="stat-row">
          <span class="stat-label">Interest Earned</span>
          <span class="stat-value green">${interestEarned}</span>
        </div>
      {/if}
    </div>

    {#if error}
      <div class="error-bar">{error}</div>
    {/if}

    <div class="group-divider"></div>
  {/if}

  <!-- POOL section (always shown) -->
  <h4 class="group-heading">Pool</h4>
  <div class="stats-stack">
    <div class="stat-row">
      <span class="stat-label">TVL</span>
      <span class="stat-value bold">${totalDepositsUsd}</span>
    </div>
    {#if poolApy !== null}
      <div class="stat-row">
        <span class="stat-label">Interest APY</span>
        <span class="stat-value">{poolApy}%</span>
      </div>
    {/if}
    <div class="stat-row">
      <span class="stat-label">Depositors</span>
      <span class="stat-value">{depositorCount}</span>
    </div>

    <!-- Pool deposits (stacked values, ticker-first) -->
    {#if stablecoinBreakdown.length > 0}
      <div class="stat-row align-top">
        <span class="stat-label">Deposits</span>
        <span class="stat-value-stack">
          {#each stablecoinBreakdown as [ledger, amount]}
            {@const sym = symbolForLedger(ledger, registries)}
            {@const dec = decimalsForLedger(ledger, registries)}
            <span class="gain-line">
              <span class="collateral-dot" style="background:{getStablecoinColor(ledger)}"></span>
              <span class="gain-ticker">{sym}</span>
              <span class="gain-value">{formatStableTokenDisplay(amount, dec)}</span>
            </span>
          {/each}
        </span>
      </div>
    {/if}
  </div>
</div>

<style>
  .info-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.25rem;
  }

  /* ── Section headings (match ProtocolStats) ── */
  .group-heading {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.625rem;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--rumi-text-muted);
    margin-bottom: 0.625rem;
  }

  .group-divider {
    height: 1px;
    background: var(--rumi-border);
    margin: 0.875rem 0;
    opacity: 0.5;
  }

  /* ── Stats layout (match ProtocolStats) ── */
  .stats-stack {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .stat-row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }

  .stat-row.align-top {
    align-items: flex-start;
  }

  .stat-label {
    font-size: 0.75rem;
    color: var(--rumi-text-secondary);
    display: flex;
    align-items: center;
    gap: 0.375rem;
    flex-wrap: wrap;
  }

  .stat-value {
    font-family: 'Inter', sans-serif;
    font-size: 0.8125rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
  }

  .stat-value.bold {
    font-weight: 700;
    color: white;
  }

  .stat-value.green {
    color: #4ade80;
    font-weight: 700;
  }

  /* ── Stacked values (match ProtocolStats Total Collateral pattern) ── */
  .stat-value-stack {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 0.125rem;
    font-family: 'Inter', sans-serif;
    font-size: 0.8125rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
  }

  .collateral-dot {
    display: inline-block;
    width: 0.375rem;
    height: 0.375rem;
    border-radius: 9999px;
    vertical-align: middle;
  }

  .gain-dim {
    color: var(--rumi-text-muted);
  }

  /* ── Ticker-first lines (collateral gains + pool deposits) ── */
  .gain-line {
    display: flex;
    align-items: baseline;
    align-self: stretch;
    gap: 0.25rem;
    white-space: nowrap;
  }

  .gain-ticker {
    flex-shrink: 0;
  }

  .gain-value {
    margin-left: auto;
  }

  /* ── Stablecoin breakdown under Total Deposited ── */
  .breakdown-line {
    display: flex;
    align-items: baseline;
    align-self: stretch;
    gap: 0.25rem;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    white-space: nowrap;
  }

  .breakdown-dot {
    opacity: 0.45;
  }

  .breakdown-ticker {
    flex-shrink: 0;
  }

  /* ── Opt-out inline controls ── */
  .opt-out-inline {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
  }

  .claim-all-inline {
    padding: 0.0625rem 0.375rem;
    background: var(--rumi-teal-dim);
    border: 1px solid var(--rumi-border-teal);
    border-radius: 0.25rem;
    color: var(--rumi-teal);
    font-size: 0.5625rem;
    font-weight: 700;
    letter-spacing: 0.02em;
    cursor: pointer;
    transition: all 0.15s ease;
  }
  .claim-all-inline:hover:not(:disabled) { background: rgba(45, 212, 191, 0.15); }
  .claim-all-inline:disabled { opacity: 0.4; cursor: not-allowed; }

  .opt-out-btn-inline {
    display: inline-flex;
    align-items: center;
    gap: 0.125rem;
    padding: 0.0625rem 0.3125rem;
    background: transparent;
    border: 1px solid var(--rumi-border);
    border-radius: 0.25rem;
    color: var(--rumi-text-muted);
    font-size: 0.5625rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    white-space: nowrap;
  }
  .opt-out-btn-inline:hover {
    border-color: var(--rumi-border-hover);
    color: var(--rumi-text-secondary);
  }
  .info-icon { flex-shrink: 0; opacity: 0.6; }

  .opt-out-tooltip {
    position: absolute;
    top: calc(100% + 0.5rem);
    right: 0;
    z-index: 20;
    width: 14rem;
    padding: 0.625rem 0.75rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    box-shadow: 0 4px 16px rgba(0,0,0,0.4);
    font-size: 0.6875rem;
    line-height: 1.5;
    color: var(--rumi-text-secondary);
    pointer-events: none;
  }

  .opt-out-menu {
    position: absolute;
    top: calc(100% + 0.375rem);
    right: 0;
    z-index: 30;
    min-width: 10rem;
    padding: 0.375rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    box-shadow: 0 4px 16px rgba(0,0,0,0.4);
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }

  .opt-out-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.4375rem 0.625rem;
    background: transparent;
    border: none;
    border-radius: 0.375rem;
    cursor: pointer;
    transition: background 0.15s ease;
    width: 100%;
    text-align: left;
  }
  .opt-out-row:hover:not(:disabled) { background: var(--rumi-bg-surface2); }
  .opt-out-row:disabled { opacity: 0.4; cursor: not-allowed; }
  .opt-out-symbol { font-size: 0.75rem; font-weight: 600; color: var(--rumi-text-primary); }
  .opt-out-status { font-size: 0.6875rem; color: var(--rumi-teal); font-weight: 500; }
  .opt-out-status.opted-out-label { color: var(--rumi-danger); }

  .mini-spinner {
    display: inline-block;
    width: 0.75rem;
    height: 0.75rem;
    border: 1.5px solid transparent;
    border-top-color: currentColor;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin { to { transform: rotate(360deg); } }

  .error-bar {
    margin-top: 0.5rem;
    padding: 0.5rem 0.625rem;
    background: rgba(224, 107, 159, 0.08);
    border: 1px solid rgba(224, 107, 159, 0.2);
    border-radius: 0.375rem;
    color: var(--rumi-danger);
    font-size: 0.75rem;
  }
</style>
