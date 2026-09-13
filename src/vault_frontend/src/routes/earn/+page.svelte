<script lang="ts">
  import { onMount } from 'svelte';
  import { ProtocolService } from '../../lib/services/protocol';
  import { stabilityPoolService } from '../../lib/services/stabilityPoolService';
  import { getThreePoolApy } from '../../lib/services/threePoolApyService';
  import { liveSpApyPct } from '../../lib/utils/liveApy';
  import EarnSubNav from '../../lib/components/layout/EarnSubNav.svelte';

  // Each opportunity's rate loads and fails independently. A stalled or
  // erroring stability-pool query must never hide the 3USD card (or a
  // liquidation gain, which isn't a rate at all, must never be implied by
  // the 3USD number) and vice versa.
  type RateState = { status: 'loading' | 'ready' | 'unavailable'; pct: number | null };

  let threeUsdRate: RateState = { status: 'loading', pct: null };
  let spRate: RateState = { status: 'loading', pct: null };

  onMount(() => {
    loadThreeUsdRate();
    loadSpRate();
  });

  async function loadThreeUsdRate() {
    try {
      const r = await getThreePoolApy();
      // `complete` is false when any required input (protocol status, an
      // actual three_pool split, the swap-fee window, or a positive TVL)
      // fell back to a default, so a real fetch failure can't render as an
      // invented or misleading 0.00% APY.
      if (!r.complete || !Number.isFinite(r.total_apy_pct)) {
        threeUsdRate = { status: 'unavailable', pct: null };
        return;
      }
      threeUsdRate = { status: 'ready', pct: r.total_apy_pct };
    } catch (e) {
      console.warn('Earn overview: 3USD rate unavailable:', e);
      threeUsdRate = { status: 'unavailable', pct: null };
    }
  }

  async function loadSpRate() {
    try {
      const [protocolStatus, poolStatus] = await Promise.all([
        ProtocolService.getProtocolStatus(),
        stabilityPoolService.getPoolStatus(),
      ]);
      // Advertised rate = what a new icUSD depositor earns; excludes
      // liquidation gains and non-icUSD deposits. Same helper the Stability
      // Pool page uses, so this can't drift from the truthful figure shown
      // there.
      const pct = liveSpApyPct(protocolStatus as any, poolStatus as any);
      spRate = pct !== null && Number.isFinite(pct)
        ? { status: 'ready', pct }
        : { status: 'unavailable', pct: null };
    } catch (e) {
      console.warn('Earn overview: stability pool rate unavailable:', e);
      spRate = { status: 'unavailable', pct: null };
    }
  }
</script>

<svelte:head>
  <title>Earn | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <div class="page-header">
    <h1 class="page-title">Earn</h1>
  </div>

  <EarnSubNav active="overview" />

  <p class="intro">
    Two ways to put stablecoins to work in Rumi Protocol. They carry different kinds
    of risk and reward, so pick the one that matches what you want.
  </p>

  <div class="cards">
    <div class="earn-card">
      <div class="earn-card-header">
        <h2 class="earn-card-title">Stablecoin Liquidity <span class="earn-card-subtitle">· 3USD</span></h2>
        {#if threeUsdRate.status === 'loading'}
          <span class="rate-pill rate-loading">Loading…</span>
        {:else if threeUsdRate.status === 'ready' && threeUsdRate.pct !== null}
          <span class="rate-pill">{threeUsdRate.pct.toFixed(2)}% APY</span>
        {:else}
          <span class="rate-pill rate-unavailable">Rate unavailable</span>
        {/if}
      </div>
      <p class="earn-card-body">
        Deposit icUSD, ckUSDT, or ckUSDC, in any combination, and receive 3USD: a token
        representing your share of the pool. 3USD earns swap fees plus a share of the
        protocol's borrowing interest as the pool grows.
      </p>
      <a href="/3usd" class="earn-cta">Provide liquidity</a>
    </div>

    <div class="earn-card">
      <div class="earn-card-header">
        <h2 class="earn-card-title">Stability Pool</h2>
        {#if spRate.status === 'loading'}
          <span class="rate-pill rate-loading">Loading…</span>
        {:else if spRate.status === 'ready' && spRate.pct !== null}
          <span class="rate-pill">{spRate.pct.toFixed(2)}% Interest APY</span>
        {:else}
          <span class="rate-pill rate-unavailable">Rate unavailable</span>
        {/if}
      </div>
      <p class="earn-card-rate-note">Interest APY on icUSD, excluding liquidation gains.</p>
      <p class="earn-card-body">
        Supply stablecoins that stand ready to repay undercollateralized vault debt.
        When a liquidation happens, your deposit repays that debt and you receive the
        vault's collateral in return, at a discount to its market value. icUSD deposits
        also earn a share of ongoing borrowing interest; ckUSDC, ckUSDT, and 3USD
        deposits take part in liquidations but don't earn that interest.
      </p>
      <a href="/stability-pool" class="earn-cta">Deposit</a>
    </div>
  </div>

  <p class="earn-footnote">
    These are different kinds of exposure. Stability pool funds can be converted into
    collateral whenever a liquidation occurs, so those gains depend on liquidations
    happening. 3USD represents an ongoing share of pool liquidity rather than a fixed
    return, and its rate moves with swap activity and protocol interest. Neither figure
    above is a guaranteed yield.
  </p>
</div>

<style>
  .page-container { max-width: 820px; margin: 0 auto; padding-bottom: 4rem; }

  .page-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 1.25rem;
    animation: fadeSlideIn 0.5s ease-out both;
  }

  @keyframes fadeSlideIn {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .intro {
    margin: 0 0 1.5rem;
    max-width: 60ch;
    font-size: 0.875rem;
    line-height: 1.6;
    color: var(--rumi-text-secondary);
    animation: fadeSlideIn 0.5s ease-out 0.05s both;
  }

  .cards {
    display: grid;
    grid-template-columns: 1fr;
    gap: 1rem;
    margin-bottom: 1.5rem;
    animation: fadeSlideIn 0.5s ease-out 0.1s both;
  }

  @media (min-width: 760px) {
    .cards { grid-template-columns: 1fr 1fr; }
  }

  .earn-card {
    display: flex;
    flex-direction: column;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
    box-shadow:
      inset 0 1px 0 0 rgba(200, 210, 240, 0.03),
      0 2px 8px -2px rgba(8, 11, 22, 0.6);
  }

  .earn-card-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 0.625rem;
    margin-bottom: 0.75rem;
  }

  .earn-card-title {
    margin: 0;
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.0625rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
  }

  .earn-card-subtitle {
    font-weight: 500;
    color: var(--rumi-text-secondary);
  }

  .rate-pill {
    display: inline-flex;
    align-items: center;
    padding: 0.25rem 0.75rem;
    background: rgba(74, 222, 128, 0.1);
    border: 1px solid rgba(74, 222, 128, 0.3);
    border-radius: 1.25rem;
    font-size: 0.8125rem;
    font-weight: 600;
    color: #4ade80;
    white-space: nowrap;
  }

  .rate-pill.rate-loading {
    background: var(--rumi-bg-surface2);
    border-color: var(--rumi-border);
    color: var(--rumi-text-muted);
  }

  .rate-pill.rate-unavailable {
    background: var(--rumi-bg-surface2);
    border-color: var(--rumi-border);
    color: var(--rumi-text-muted);
  }

  .earn-card-rate-note {
    margin: 0 0 0.75rem;
    font-size: 0.6875rem;
    line-height: 1.4;
    color: var(--rumi-text-muted);
  }

  .earn-card-body {
    flex: 1;
    margin: 0 0 1.25rem;
    font-size: 0.8125rem;
    line-height: 1.6;
    color: var(--rumi-text-secondary);
  }

  .earn-cta {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    align-self: flex-start;
    padding: 0.625rem 1.25rem;
    background: var(--rumi-action);
    color: var(--rumi-bg-primary);
    border-radius: 0.5rem;
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.875rem;
    font-weight: 600;
    text-decoration: none;
    transition: background 0.15s ease, box-shadow 0.15s ease;
  }

  .earn-cta:hover {
    background: var(--rumi-action-bright);
    box-shadow: 0 0 20px rgba(52, 211, 153, 0.15);
  }

  .earn-footnote {
    margin: 0;
    max-width: 68ch;
    font-size: 0.75rem;
    line-height: 1.6;
    color: var(--rumi-text-muted);
    animation: fadeSlideIn 0.5s ease-out 0.15s both;
  }

  @media (max-width: 520px) {
    .page-container { padding-left: 0.5rem; padding-right: 0.5rem; }
  }
</style>
