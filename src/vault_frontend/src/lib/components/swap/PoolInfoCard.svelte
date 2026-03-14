<script lang="ts">
  import { walletStore } from '../../stores/wallet';
  import {
    POOL_TOKENS,
    formatTokenAmount,
  } from '../../services/threePoolService';
  import type { PoolStatus } from '../../services/threePoolService';
  import { formatStableTokenDisplay } from '../../utils/format';

  export let poolStatus: PoolStatus | null = null;
  export let userLpBalance: bigint = 0n;
  export let apy: number | null = null;

  $: isConnected = $walletStore.isConnected;

  // Pool stats
  $: totalLp = poolStatus?.lp_total_supply ?? 0n;
  $: swapFeePct = poolStatus ? (Number(poolStatus.swap_fee_bps) / 100).toFixed(2) : '0.00';
  $: virtualPrice = poolStatus?.virtual_price ?? 0n;
  $: tokenPrice = virtualPrice > 0n
    ? '$' + (Number(virtualPrice) / 1e18).toFixed(4)
    : '—';
  $: apyFormatted = apy !== null ? (apy * 100).toFixed(2) + '%' : '—';

  // User share
  $: userSharePct = (() => {
    if (!totalLp || totalLp === 0n || userLpBalance === 0n) return '0.00';
    const share = (Number(userLpBalance) * 100) / Number(totalLp);
    return share < 0.01 && share > 0 ? '<0.01' : share.toFixed(2);
  })();

  // User's proportional value per token
  $: userTokenShares = (() => {
    if (!poolStatus || totalLp === 0n || userLpBalance === 0n) return null;
    return POOL_TOKENS.map((token, i) => {
      const bal = poolStatus!.balances[i] ?? 0n;
      const share = (bal * userLpBalance) / totalLp;
      return { token, amount: share };
    });
  })();

  // Total pool TVL (sum of all balances, normalised to 8 decimals for display)
  $: poolTvl = (() => {
    if (!poolStatus) return '0.00';
    let total = 0;
    for (let i = 0; i < 3; i++) {
      const bal = Number(poolStatus.balances[i]);
      const dec = POOL_TOKENS[i].decimals;
      total += bal / Math.pow(10, dec);
    }
    return total.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  })();
</script>

<div class="info-card">
  <!-- YOUR POSITION section (only when connected with LP) -->
  {#if isConnected && userLpBalance > 0n}
    <h4 class="group-heading">Your Position</h4>
    <div class="stats-stack">
      <div class="stat-row">
        <span class="stat-label">3USD Balance</span>
        <span class="stat-value bold">{formatTokenAmount(userLpBalance, 8)}</span>
      </div>
      <div class="stat-row">
        <span class="stat-label">Pool Share</span>
        <span class="stat-value">{userSharePct}%</span>
      </div>

      <!-- Per-token value breakdown -->
      {#if userTokenShares}
        <div class="stat-row align-top">
          <span class="stat-label">Value</span>
          <span class="stat-value-stack">
            {#each userTokenShares as { token, amount }}
              <span class="gain-line">
                <span class="collateral-dot" style="background:{token.color}"></span>
                <span class="gain-ticker">{token.symbol}</span>
                <span class="gain-value">{formatTokenAmount(amount, token.decimals)}</span>
              </span>
            {/each}
          </span>
        </div>
      {/if}
    </div>

    <div class="group-divider"></div>
  {/if}

  <!-- 3USD Stats section (always shown) -->
  <h4 class="group-heading">3USD Stats</h4>
  <div class="stats-stack">
    <div class="stat-row">
      <span class="stat-label">TVL</span>
      <span class="stat-value bold">${poolTvl}</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">Swap Fee</span>
      <span class="stat-value">{swapFeePct}%</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">3USD Price</span>
      <span class="stat-value">{tokenPrice}</span>
    </div>
    <div class="stat-row">
      <span class="stat-label">APY</span>
      <span class="stat-value apy">{apyFormatted}</span>
    </div>

    <!-- Pool balances (stacked, ticker-first) -->
    {#if poolStatus}
      <div class="stat-row align-top">
        <span class="stat-label">Balances</span>
        <span class="stat-value-stack">
          {#each POOL_TOKENS as token, i}
            <span class="gain-line">
              <span class="collateral-dot" style="background:{token.color}"></span>
              <span class="gain-ticker">{token.symbol}</span>
              <span class="gain-value">{formatTokenAmount(poolStatus.balances[i], token.decimals)}</span>
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

  /* ── Section headings (match EarnInfoCard) ── */
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

  /* ── Stats layout (match EarnInfoCard) ── */
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

  .stat-value.apy {
    color: #4ade80;
  }

  /* ── Stacked values (match EarnInfoCard) ── */
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

  /* ── Ticker-first lines (match EarnInfoCard) ── */
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
</style>
