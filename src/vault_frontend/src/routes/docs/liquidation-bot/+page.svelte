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
    <p>The bot uses a <strong>credit-based</strong> liquidation model. Unlike manual liquidators who must have icUSD upfront, the bot receives collateral first and then sells it to generate icUSD:</p>
    <ol class="flow-list">
      <li><strong>Notification</strong> — The backend detects an unhealthy vault during its price check and sends the vault details to the bot.</li>
      <li><strong>Liquidation on credit</strong> — The bot calls the backend to liquidate the vault. The backend reduces the vault's debt, seizes the proportional collateral (plus the liquidation bonus), and transfers the ICP directly to the bot. No icUSD is needed upfront.</li>
      <li><strong>Swap ICP for stablecoin</strong> — The bot sells the seized ICP on <a href="https://kongswap.io" class="doc-link" target="_blank" rel="noopener">KongSwap</a> for either ckUSDC or ckUSDT, whichever offers the better rate.</li>
      <li><strong>Swap stablecoin for icUSD</strong> — The bot swaps the ckStable for icUSD through Rumi's own <a href="/docs/three-pool" class="doc-link">3pool</a>.</li>
      <li><strong>Deposit to reserves</strong> — The icUSD is deposited back into the protocol's reserves, covering the debt that was erased in step 2.</li>
      <li><strong>Surplus to treasury</strong> — Any remaining ICP (from the liquidation bonus exceeding the debt) is sent to the protocol treasury.</li>
    </ol>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Best-Rate Selection</h2>
    <p>Before executing a swap, the bot queries KongSwap for quotes on both ICP/ckUSDC and ICP/ckUSDT pairs. It selects whichever pair yields more stablecoin output, ensuring the protocol gets the best available rate at the time of liquidation.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Failure Escalation</h2>
    <p>The bot follows a <strong>no-retry</strong> policy. If any step of the liquidation fails (DEX swap reverts, transfer error, etc.), the bot does not retry that vault. Instead, the escalation chain is:</p>
    <ol class="flow-list">
      <li><strong>Bot fails</strong> — The vault remains undercollateralized.</li>
      <li><strong>Stability Pool</strong> — On the next price check cycle (up to 5 minutes), the backend notifies the stability pool, which can liquidate using depositors' funds.</li>
      <li><strong>Manual liquidation</strong> — If neither the bot nor the stability pool handles it, the vault appears on the <a href="/liquidations" class="doc-link">Manual Liquidation</a> page where any user can liquidate it directly.</li>
    </ol>
    <p>This design avoids compounding failures. If a DEX is down or liquidity is thin, repeatedly retrying the same swap would waste cycles and could leave partial state. Failing fast and escalating is safer.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Budget</h2>
    <p>The bot operates within a configurable monthly budget that limits total debt it can cover. This prevents runaway spending if market conditions cause a cascade of liquidations. The budget is set by the protocol admin and can be reset or adjusted at any time.</p>
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
        <div class="stat-row">
          <span class="stat-label">Total icUSD deposited (all time)</span>
          <span class="stat-value">{formatUsd(Number(stats.total_icusd_deposited_e8s))}</span>
        </div>
      </div>
    {/if}
    <p>When the budget is exhausted, the bot stops liquidating and the stability pool and manual liquidators take over. The budget resets when the admin sets a new period.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Liquidation Priority</h2>
    <p>When the protocol detects undercollateralized vaults, it notifies <strong>both</strong> the liquidation bot and the stability pool simultaneously. The bot processes vaults on a 30-second timer cycle. In practice, the bot typically acts first because it doesn't require human intervention, but the stability pool serves as a reliable backup.</p>
    <p>The priority chain is:</p>
    <ol class="flow-list">
      <li><strong>Liquidation Bot</strong> — Automated, fastest response. Credit-based (no upfront capital). Limited by budget.</li>
      <li><strong>Stability Pool</strong> — Automated on next cycle. Uses depositors' stablecoins. No budget limit (constrained by pool size).</li>
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
    <p><strong>DEX liquidity risk:</strong> The bot relies on KongSwap and the 3pool for swapping collateral. If either pool has insufficient liquidity, the swap may fail or execute with high slippage, reducing the icUSD recovered. A maximum slippage tolerance is configured to prevent unacceptable trades.</p>
    <p><strong>Price movement during swap:</strong> There is a delay between seizing collateral and completing the swap chain. If the collateral price drops significantly during this window, the bot may recover less icUSD than the debt it covered, creating a deficit.</p>
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
