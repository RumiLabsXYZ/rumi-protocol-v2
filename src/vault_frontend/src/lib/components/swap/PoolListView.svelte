<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  import { walletStore } from '../../stores/wallet';
  import {
    threePoolService,
    POOL_TOKENS,
    formatTokenAmount,
  } from '../../services/threePoolService';
  import { ammService, type PoolInfo } from '../../services/ammService';
  import { getThreePoolApy } from '../../services/threePoolApyService';
  import { CANISTER_IDS } from '../../config';
  import type { PoolStatus } from '../../services/threePoolService';

  const dispatch = createEventDispatcher();

  let threePoolStatus: PoolStatus | null = null;
  let ammPool: PoolInfo | null = null;
  let userThreePoolLp = 0n;
  let userAmmLp = 0n;
  let loading = true;

  // APY state for both pools (null while loading, number once resolved)
  let threePoolApyPct: number | null = null;
  let threePoolInterestAprPct = 0;
  let threePoolSwapFeeAprPct = 0;
  let showThreePoolTooltip = false;

  $: isConnected = $walletStore.isConnected;

  onMount(loadData);

  async function loadData() {
    loading = true;
    try {
      const [tpStatus, ammPools] = await Promise.all([
        threePoolService.getPoolStatus(),
        ammService.getPools().catch(() => [] as PoolInfo[]),
      ]);
      threePoolStatus = tpStatus;
      // Find the 3USD/ICP pool specifically
      const threePoolId = CANISTER_IDS.THREEPOOL;
      const icpLedgerId = CANISTER_IDS.ICP_LEDGER;
      ammPool = ammPools.find(p => {
        const a = p.token_a.toText();
        const b = p.token_b.toText();
        return (a === threePoolId && b === icpLedgerId) || (a === icpLedgerId && b === threePoolId);
      }) ?? null;

      if (isConnected && $walletStore.principal) {
        const promises: Promise<any>[] = [
          threePoolService.getLpBalance($walletStore.principal),
        ];
        if (ammPool) {
          promises.push(ammService.getLpBalance(ammPool.pool_id, $walletStore.principal));
        }
        const [tpLp, ammLpResult] = await Promise.all(promises);
        userThreePoolLp = tpLp;
        userAmmLp = ammLpResult ?? 0n;
      }

      // APY queries fire in parallel; never block the cards on them
      loadThreePoolApy().catch(e => console.warn('3pool APY failed:', e));
    } catch (e) {
      console.error('Failed to load pool data:', e);
    } finally {
      loading = false;
    }
  }

  async function loadThreePoolApy() {
    const r = await getThreePoolApy();
    threePoolInterestAprPct = r.interest_apr_pct;
    threePoolSwapFeeAprPct = r.swap_fee_apr_pct;
    threePoolApyPct = r.total_apy_pct;
  }

  function threePoolTvl(): string {
    if (!threePoolStatus) return '$0.00';
    let total = 0;
    for (let i = 0; i < 3; i++) {
      const bal = Number(threePoolStatus.balances[i]);
      total += bal / Math.pow(10, POOL_TOKENS[i].decimals);
    }
    return '$' + total.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  }

  function ammTvl(): string {
    if (!ammPool) return '$0.00';
    const threePoolId = CANISTER_IDS.THREEPOOL;
    const isTokenA3USD = ammPool.token_a.toText() === threePoolId;
    const threeUsdReserve = isTokenA3USD ? ammPool.reserve_a : ammPool.reserve_b;
    const threeUsdValue = Number(threeUsdReserve) / 1e8;
    return '~$' + (threeUsdValue * 2).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  }

  function selectPool(pool: 'threepool' | 'amm') {
    dispatch('select', { pool });
  }
</script>

{#if loading}
  <div class="loading">Loading pools...</div>
{:else}
  <div class="pool-list">
    <button class="pool-card" on:click={() => selectPool('threepool')}>
      <div class="pool-header">
        <div class="pool-pair">
          <div class="pool-dots">
            {#each POOL_TOKENS as t}
              <span class="pool-dot" style="background:{t.color}"></span>
            {/each}
          </div>
          <span class="pool-name">3pool</span>
          <span class="pool-tokens">icUSD / ckUSDT / ckUSDC</span>
        </div>
        <!-- svelte-ignore a11y-mouse-events-have-key-events -->
        <div
          class="apy-badge"
          on:mouseover|stopPropagation={() => { showThreePoolTooltip = true; }}
          on:mouseleave={() => { showThreePoolTooltip = false; }}
        >
          {#if threePoolApyPct === null}
            <span class="apy-loading">… APY</span>
          {:else}
            <svg class="apy-arrow" width="9" height="9" viewBox="0 0 10 10" fill="none">
              <path d="M5 8V2M5 2L2 5M5 2L8 5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
            {threePoolApyPct.toFixed(1)}% APY
          {/if}

          {#if showThreePoolTooltip && threePoolApyPct !== null}
            <div class="apy-tooltip">
              <div class="apy-tooltip-caret"></div>
              <p>
                Interest {threePoolInterestAprPct.toFixed(2)}% + Swap fees {threePoolSwapFeeAprPct.toFixed(2)}%
                = total {threePoolApyPct.toFixed(2)}%
              </p>
            </div>
          {/if}
        </div>
      </div>
      <div class="pool-stats">
        <div class="pool-stat">
          <span class="stat-label">TVL</span>
          <span class="stat-value">{threePoolTvl()}</span>
        </div>
        <div class="pool-stat">
          <span class="stat-label">Fee</span>
          <span class="stat-value">{threePoolStatus ? (Number(threePoolStatus.swap_fee_bps) / 100).toFixed(2) + '%' : '—'}</span>
        </div>
        {#if isConnected && userThreePoolLp > 0n}
          <div class="pool-stat">
            <span class="stat-label">Your LP</span>
            <span class="stat-value lp-value">{formatTokenAmount(userThreePoolLp, 8)}</span>
          </div>
        {/if}
      </div>
      <div class="pool-action">Add Liquidity →</div>
    </button>

    {#if ammPool}
      <button
        class="pool-card pool-card-paused"
        disabled={!isConnected || userAmmLp === 0n}
        aria-label={userAmmLp > 0n
          ? 'Manage your paused 3USD / ICP liquidity position'
          : '3USD / ICP liquidity is temporarily paused'}
        on:click={() => selectPool('amm')}
      >
        <div class="pool-header">
          <div class="pool-pair">
            <div class="pool-dots">
              <span class="pool-dot" style="background:#34d399"></span>
              <span class="pool-dot" style="background:#29abe2"></span>
            </div>
            <span class="pool-name">3USD / ICP</span>
          </div>
          <span class="pool-paused-badge">Paused</span>
        </div>
        <div class="pool-stats">
          <div class="pool-stat">
            <span class="stat-label">TVL</span>
            <span class="stat-value">{ammTvl()}</span>
          </div>
          <div class="pool-stat">
            <span class="stat-label">Fee</span>
            <span class="stat-value">{(ammPool.fee_bps / 100).toFixed(2)}%</span>
          </div>
          {#if isConnected && userAmmLp > 0n}
            <div class="pool-stat">
              <span class="stat-label">Your LP</span>
              <span class="stat-value lp-value">{formatTokenAmount(userAmmLp, 8)}</span>
            </div>
          {/if}
        </div>
        <div class="pool-action">
          {userAmmLp > 0n ? 'Manage position →' : 'Liquidity temporarily paused'}
        </div>
      </button>
    {:else}
      <div class="pool-card pool-card-empty">
        <span class="pool-name">3USD / ICP</span>
        <span class="pool-empty-text">Pool not yet created</span>
      </div>
    {/if}

    <p class="liquidity-explainer">
      Supply stablecoins (icUSD, ckUSDT, or ckUSDC) to the 3pool to receive 3USD and
      earn the pool's protocol-interest and swap-fee yield.
    </p>
  </div>
{/if}

<style>
  .loading {
    text-align: center;
    padding: 2rem;
    color: var(--rumi-text-muted);
    font-size: 0.875rem;
  }

  .pool-list {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .pool-card {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    padding: 1rem 1.25rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    cursor: pointer;
    transition: all 0.15s ease;
    text-align: left;
    width: 100%;
    color: inherit;
    font-family: inherit;
  }

  .pool-card:hover {
    border-color: var(--rumi-teal);
    box-shadow: 0 0 0 1px rgba(45, 212, 191, 0.1);
  }

  .pool-card-empty {
    opacity: 0.5;
    cursor: default;
  }

  .pool-card-empty:hover {
    border-color: var(--rumi-border);
    box-shadow: none;
  }

  .pool-card-paused {
    cursor: not-allowed;
    opacity: 0.45;
    filter: grayscale(1);
  }

  .pool-card-paused:hover {
    border-color: var(--rumi-border);
    box-shadow: none;
  }

  .pool-card-paused:not(:disabled) {
    cursor: pointer;
  }

  .pool-card-paused .pool-action {
    color: var(--rumi-text-muted);
  }

  .pool-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }

  .pool-pair {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
  }

  .pool-dots {
    display: flex;
    gap: 0.125rem;
  }

  .pool-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    display: inline-block;
  }

  .pool-name {
    font-size: 0.9375rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
  }

  .pool-tokens {
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
  }

  .pool-empty-text {
    font-size: 0.8125rem;
    color: var(--rumi-text-muted);
  }

  .pool-paused-badge {
    padding: 0.1875rem 0.625rem;
    border: 1px solid var(--rumi-border);
    border-radius: 1rem;
    color: var(--rumi-text-muted);
    font-size: 0.75rem;
    font-weight: 600;
    white-space: nowrap;
  }

  .pool-stats {
    display: flex;
    gap: 1.5rem;
  }

  .pool-stat {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }

  .stat-label {
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .stat-value {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    font-variant-numeric: tabular-nums;
  }

  .lp-value {
    color: var(--rumi-teal);
  }

  .pool-action {
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-teal);
  }

  /* ── APY badge (mirrors /3usd page styling) ── */
  .apy-badge {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.1875rem 0.625rem;
    background: rgba(74, 222, 128, 0.1);
    border: 1px solid rgba(74, 222, 128, 0.3);
    border-radius: 1rem;
    font-size: 0.75rem;
    font-weight: 600;
    color: #4ade80;
    white-space: nowrap;
    flex-shrink: 0;
  }

  .apy-arrow { color: #4ade80; flex-shrink: 0; }

  .apy-loading {
    color: var(--rumi-text-muted);
    font-weight: 500;
  }

  .apy-tooltip {
    position: absolute;
    top: calc(100% + 0.5rem);
    right: 0;
    z-index: 50;
    width: 16rem;
    padding: 0.625rem 0.75rem;
    background: #1e293b;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 0.5rem;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
    font-size: 0.6875rem;
    font-weight: 400;
    line-height: 1.5;
    color: #cbd5e1;
    text-align: left;
    white-space: normal;
    animation: tooltipFade 0.15s ease-out;
  }

  @keyframes tooltipFade {
    from { opacity: 0; transform: translateY(4px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .apy-tooltip-caret {
    position: absolute;
    top: -5px;
    right: 0.875rem;
    transform: rotate(45deg);
    width: 10px;
    height: 10px;
    background: #1e293b;
    border-top: 1px solid rgba(255, 255, 255, 0.08);
    border-left: 1px solid rgba(255, 255, 255, 0.08);
  }

  .apy-tooltip p { margin: 0; }

  /* ── Explainer under the cards ── */
  .liquidity-explainer {
    margin: 0.5rem 0 0;
    max-width: 60ch;
    font-size: 0.75rem;
    line-height: 1.6;
    color: var(--rumi-text-muted);
  }
</style>
