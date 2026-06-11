<script lang="ts">
  import { onMount } from 'svelte';
  import { ApiClient } from '$lib/services/protocol/apiClient';

  let stats: any = null;
  let loaded = false;

  function formatE8s(val: number): string {
    return (val / 1e8).toFixed(2);
  }

  function formatUsd(val: number): string {
    return '$' + (val / 1e8).toFixed(2);
  }

  onMount(async () => {
    try {
      stats = await ApiClient.getPublicData<any>('get_bot_stats');
    } catch (e) {
      console.error('Failed to fetch bot stats:', e);
    }
    loaded = true;
  });
</script>

<svelte:head><title>Liquidation Bot | Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Liquidation Bot</h1>

  <section class="doc-section">
    <h2 class="doc-heading">What Is the Liquidation Bot</h2>
    <p>The liquidation bot is an autonomous canister that monitors unhealthy vaults and liquidates them without requiring any human intervention. It is the protocol's <strong>first line of defense</strong> against undercollateralized positions — liquidating vaults faster and more reliably than manual liquidators.</p>
    <p>The bot operates entirely on-chain as an Internet Computer canister. There is no off-chain infrastructure, no keeper network, and no reliance on third-party bots. The protocol backend notifies the bot whenever it detects liquidatable vaults during its regular price-check cycle (every 5 minutes).</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">How It Works</h2>
    <p>The bot uses a <strong>credit-based</strong> liquidation model. Unlike manual liquidators who must have icUSD upfront, the bot receives collateral first and then sells it to repay the protocol:</p>
    <ol class="flow-list">
      <li><strong>Notification</strong> — The backend detects an unhealthy vault during its price check and sends the vault details to the bot.</li>
      <li><strong>Liquidation on credit</strong> — The bot calls the backend to liquidate the vault. The backend reduces the vault's debt, seizes the proportional collateral (plus the liquidation bonus), and transfers the ICP directly to the bot. No icUSD is needed upfront.</li>
      <li><strong>Swap ICP for ckUSDC</strong> — The bot sells the seized ICP for ckUSDC in a single-hop swap on <a href="https://app.icpswap.com" class="doc-link" target="_blank" rel="noopener">ICPSwap</a>, with a configured maximum slippage bound.</li>
      <li><strong>Deposit to reserves</strong> — The ckUSDC is deposited back into the protocol's stablecoin reserves, covering the debt that was erased in step 2. (Those same reserves back <a href="/docs/redemptions" class="doc-link">reserve redemptions</a>.)</li>
      <li><strong>Surplus to treasury</strong> — Any remaining ICP (from the liquidation bonus exceeding the debt) is sent to the protocol treasury.</li>
    </ol>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Failure Escalation</h2>
    <p>The bot makes <strong>one liquidation attempt per vault</strong>. If the DEX swap fails, the bot returns the seized ICP to the backend, verifies the funds actually arrived, and only then cancels the liquidation (restoring the vault's debt and collateral). It does not try to liquidate that vault again. Individual call phases (claiming, confirming, cancelling) have small bounded retries to ride out transient network errors, but a failed liquidation is never re-run.</p>
    <p>The escalation chain after a bot failure is:</p>
    <ol class="flow-list">
      <li><strong>Bot fails</strong> — The collateral is returned, the liquidation is cancelled, and the vault remains undercollateralized.</li>
      <li><strong>Stability Pool</strong> — On the next price check cycle (up to 5 minutes), the backend routes the vault to the stability pool, which gets one attempt using depositors' funds.</li>
      <li><strong>Manual liquidation</strong> — If neither the bot nor the stability pool handles it, the vault appears on the <a href="/liquidations" class="doc-link">Manual Liquidation</a> page where any user can liquidate it directly.</li>
    </ol>
    <p>This design avoids compounding failures. If a DEX is down or liquidity is thin, repeatedly retrying the same swap would waste cycles and could leave partial state. Failing fast and escalating is safer.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Budget</h2>
    <p>The bot operates within a configurable budget, enforced by the backend at claim time, that limits the total debt it can cover. This prevents runaway spending if market conditions cause a cascade of liquidations. The budget is set by the protocol admin and can be reset or adjusted at any time.</p>
    {#if loaded && stats}
      <div class="stats-card">
        <div class="stat-row">
          <span class="stat-label">Monthly budget</span>
          <span class="stat-value">{formatUsd(Number(stats.budget_total_e8s))}</span>
        </div>
        <div class="stat-row">
          <span class="stat-label">Remaining</span>
          <span class="stat-value">{formatUsd(Number(stats.budget_remaining_e8s))}</span>
        </div>
        <div class="stat-row">
          <span class="stat-label">Total debt covered (all time)</span>
          <span class="stat-value">{formatUsd(Number(stats.total_debt_covered_e8s))}</span>
        </div>
      </div>
    {/if}
    <p>When the budget is exhausted, the bot stops liquidating and the stability pool and manual liquidators take over. The budget resets when the admin sets a new period.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Liquidation Priority</h2>
    <p>When the protocol detects undercollateralized vaults, it notifies the liquidation bot first (for collateral types the bot supports). The bot processes its queue on a 30-second timer cycle. Vaults the bot doesn't handle within a 5-minute window, or can't handle at all, are routed to the stability pool on the next check cycle.</p>
    <p>The priority chain is:</p>
    <ol class="flow-list">
      <li><strong>Liquidation Bot</strong> — Automated, fastest response. Credit-based (no upfront capital). Limited by budget and to admin-approved collateral types.</li>
      <li><strong>Stability Pool</strong> — Automated fallback, one attempt per vault. Uses depositors' stablecoins. No budget limit (constrained by pool size).</li>
      <li><strong>Manual Liquidators</strong> — Any user with icUSD, ckUSDC, or ckUSDT can liquidate directly. Available 24/7 via the <a href="/liquidations" class="doc-link">Liquidate</a> page.</li>
    </ol>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">What This Means for Vault Owners</h2>
    <p>The liquidation bot makes liquidations faster and more certain. If your vault drops below the liquidation threshold, expect it to be liquidated within minutes — not hours. The bot does not negotiate or wait. It liquidates as soon as the backend notifies it.</p>
    <p>This is by design: faster liquidations protect the protocol's overall health and prevent cascading failures. Vault owners should maintain a healthy collateral ratio with adequate buffer above the liquidation threshold.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">What This Means for Manual Liquidators</h2>
    <p>The bot handles most liquidations automatically. Manual liquidation opportunities will be rarer — they primarily arise when the bot's budget is exhausted, the stability pool is depleted, or a DEX swap fails. The <a href="/liquidations" class="doc-link">Manual Liquidation</a> page shows any vaults that remain undercollateralized after the bot and stability pool have had their chance.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Risks</h2>
    <p><strong>DEX liquidity risk:</strong> The bot relies on ICPSwap's ICP/ckUSDC pool for swapping collateral. If the pool has insufficient liquidity, the swap may fail or execute with high slippage, reducing the value recovered. A maximum slippage tolerance is configured to prevent unacceptable trades.</p>
    <p><strong>Price movement during swap:</strong> There is a delay between seizing collateral and completing the swap. If the collateral price drops significantly during this window, the bot may recover less than the debt it covered, creating a deficit that is tracked in the protocol's <a href="/docs/liquidation" class="doc-link">deficit account</a>.</p>
    <p><strong>Smart contract risk:</strong> The liquidation bot is a separate canister with its own code. Bugs in the swap logic, amount calculations, or inter-canister calls could result in failed liquidations or lost funds.</p>
    <p><strong>Budget exhaustion:</strong> If many vaults become undercollateralized simultaneously (e.g., a market crash), the bot's budget may run out before all vaults are processed. Remaining vaults fall through to the stability pool and manual liquidators.</p>
  </section>
</article>

<style>
  .flow-list {
    padding-left: 1.25rem;
    display: flex; flex-direction: column; gap: 0.5rem;
    margin: 0.5rem 0;
  }
  .flow-list li {
    font-size: 0.875rem; color: var(--rumi-text-secondary); line-height: 1.5;
  }
  .stats-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1rem 1.25rem;
    margin: 0.75rem 0;
    display: flex; flex-direction: column; gap: 0.5rem;
  }
  .stat-row {
    display: flex; justify-content: space-between; align-items: center;
  }
  .stat-label {
    font-size: 0.8125rem; color: var(--rumi-text-secondary);
  }
  .stat-value {
    font-size: 0.875rem; font-weight: 600; color: var(--rumi-text-primary);
    font-family: 'Inter', monospace;
  }
</style>
