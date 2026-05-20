<script lang="ts">
  import { onMount } from 'svelte';
  import { walletStore } from '../../lib/stores/wallet';
  import SwapInterface from '../../lib/components/swap/SwapInterface.svelte';
  import SwapLiquidityToggle from '../../lib/components/swap/SwapLiquidityToggle.svelte';
  import PoolListView from '../../lib/components/swap/PoolListView.svelte';
  import AmmLiquidityPanel from '../../lib/components/swap/AmmLiquidityPanel.svelte';
  import LiquidityInterface from '../../lib/components/swap/LiquidityInterface.svelte';
  import { getAmm1Apy } from '../../lib/services/amm1ApyService';
  import {
    threePoolService,
    calculateTotalApy,
    POOL_TOKENS,
  } from '../../lib/services/threePoolService';
  import { ammService, type PoolInfo } from '../../lib/services/ammService';
  import { ProtocolService } from '../../lib/services/protocol';
  import { publicActor } from '../../lib/services/protocol/apiClient';
  import { CANISTER_IDS } from '../../lib/config';

  let mode: 'swap' | 'liquidity' = 'swap';
  let liquidityView: 'list' | 'threepool' | 'amm' = 'list';

  // APY state for the combined hero. Each null while loading.
  let threePoolApyPct: number | null = null;
  let threePoolTvlIcusd = 0;
  let ammApyPct: number | null = null;
  let ammTvlIcusd = 0;

  // TVL-weighted combined APY. Only computed once both pools resolve.
  $: combinedApy =
    threePoolApyPct !== null && ammApyPct !== null && (threePoolTvlIcusd + ammTvlIcusd) > 0
      ? (threePoolApyPct * threePoolTvlIcusd + ammApyPct * ammTvlIcusd) /
        (threePoolTvlIcusd + ammTvlIcusd)
      : null;

  onMount(() => {
    // Fire both APY loaders in parallel; never block the page render
    loadThreePoolApy().catch(e => console.warn('3pool APY (swap hero) failed:', e));
    loadAmmApy().catch(e => console.warn('AMM1 APY (swap hero) failed:', e));
  });

  async function loadThreePoolApy() {
    const [status, protocolStatus, interestSplit, swapFees7d] = await Promise.all([
      threePoolService.getPoolStatus(),
      ProtocolService.getProtocolStatus().catch(() => null),
      (publicActor.get_interest_split() as Promise<{ destination: string; bps: bigint }[]>).catch(() => null),
      threePoolService.getSwapFeesOverWindow(7).catch(() => 0n),
    ]);

    let poolTvlE8s = 0;
    for (let i = 0; i < status.balances.length; i++) {
      const token = POOL_TOKENS[i];
      if (token) {
        const normalized = token.decimals === 8
          ? Number(status.balances[i])
          : Number(status.balances[i]) * 100;
        poolTvlE8s += normalized;
      }
    }
    threePoolTvlIcusd = poolTvlE8s / 1e8;

    if (!protocolStatus) {
      threePoolApyPct = 0;
      return;
    }
    const threePoolEntry = interestSplit?.find(e => e.destination === 'three_pool');
    const threePoolShareBps = threePoolEntry ? Number(threePoolEntry.bps) : 5000;
    const apy = calculateTotalApy(
      threePoolShareBps,
      protocolStatus.perCollateralInterest,
      threePoolTvlIcusd,
      swapFees7d,
    );
    threePoolApyPct = apy !== null ? apy * 100 : 0;
  }

  async function loadAmmApy() {
    const pools = await ammService.getPools().catch(() => [] as PoolInfo[]);
    const threePoolId = CANISTER_IDS.THREEPOOL;
    const icpLedgerId = CANISTER_IDS.ICP_LEDGER;
    const ammPool = pools.find(p => {
      const a = p.token_a.toText();
      const b = p.token_b.toText();
      return (a === threePoolId && b === icpLedgerId) || (a === icpLedgerId && b === threePoolId);
    });
    if (!ammPool) {
      ammApyPct = 0;
      ammTvlIcusd = 0;
      return;
    }
    const isTokenA3USD = ammPool.token_a.toText() === threePoolId;
    const threeUsdReserve = isTokenA3USD ? ammPool.reserve_a : ammPool.reserve_b;
    // Approximate USD TVL as 2x the 3USD-side reserve (balanced-pool assumption)
    ammTvlIcusd = (Number(threeUsdReserve) / 1e8) * 2;
    const r = await getAmm1Apy();
    ammApyPct = r.total_apy_pct;
  }

  function handleSuccess() {
    walletStore.refreshBalance();
  }

  function handlePoolSelect(e: CustomEvent<{ pool: 'threepool' | 'amm' }>) {
    liquidityView = e.detail.pool;
  }

  function handleBack() {
    liquidityView = 'list';
  }

  function handleModeChange() {
    liquidityView = 'list';
  }

  function switchToLiquidityTab() {
    mode = 'liquidity';
    liquidityView = 'list';
  }
</script>

<svelte:head>
  <title>{mode === 'swap' ? 'Swap' : 'Liquidity'} | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <div class="page-header">
    <h1 class="page-title">{mode === 'swap' ? 'Swap' : 'Liquidity'}</h1>
  </div>

  {#if combinedApy !== null && mode === 'swap'}
    <div class="earn-banner">
      <span class="earn-label">Earn up to {combinedApy.toFixed(2)}% APY</span>
      <button on:click={switchToLiquidityTab} class="earn-cta">Provide liquidity →</button>
    </div>
  {/if}

  <div class="action-column">
    <div class="action-panel">
      <SwapLiquidityToggle bind:mode on:change={handleModeChange} />

      {#if mode === 'swap'}
        <SwapInterface on:success={handleSuccess} />
      {:else if liquidityView === 'list'}
        <PoolListView on:select={handlePoolSelect} />
      {:else if liquidityView === 'threepool'}
        <div>
          <button class="back-link" on:click={handleBack}>← All pools</button>
          <p class="explainer">Deposit stablecoins to mint 3USD</p>
          <LiquidityInterface on:success={handleSuccess} />
        </div>
      {:else if liquidityView === 'amm'}
        <AmmLiquidityPanel on:success={handleSuccess} on:back={handleBack} />
      {/if}
    </div>
  </div>
</div>

<style>
  .page-container {
    max-width: 420px;
    margin: 0 auto;
    padding-bottom: 4rem;
  }

  .page-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 1.75rem;
    animation: fadeSlideIn 0.5s ease-out both;
  }

  @keyframes fadeSlideIn {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .action-column {
    display: flex;
    flex-direction: column;
    align-items: center;
  }

  .action-column > :global(*) { width: 100%; }

  .action-panel {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
    box-shadow:
      inset 0 1px 0 0 rgba(200, 210, 240, 0.03),
      0 2px 8px -2px rgba(8, 11, 22, 0.6);
  }

  .back-link {
    background: none;
    border: none;
    color: var(--rumi-teal);
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    padding: 0;
    margin-bottom: 1rem;
  }

  .back-link:hover { text-decoration: underline; }

  .earn-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    margin-bottom: 1rem;
    padding: 0.625rem 0.9375rem;
    background: rgba(74, 222, 128, 0.08);
    border: 1px solid rgba(74, 222, 128, 0.25);
    border-radius: 0.625rem;
    animation: fadeSlideIn 0.4s ease-out 0.1s both;
  }

  .earn-label {
    font-size: 0.875rem;
    font-weight: 600;
    color: #4ade80;
    font-variant-numeric: tabular-nums;
  }

  .earn-cta {
    background: none;
    border: none;
    color: #4ade80;
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    padding: 0.25rem 0.5rem;
    border-radius: 0.375rem;
    transition: background-color 0.15s;
    font-family: inherit;
    white-space: nowrap;
  }

  .earn-cta:hover {
    background-color: rgba(74, 222, 128, 0.12);
  }

  .explainer {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    margin: 0 0 1.25rem;
    line-height: 1.5;
  }

  @media (max-width: 520px) {
    .page-container {
      padding-left: 0.5rem;
      padding-right: 0.5rem;
    }
  }
</style>
