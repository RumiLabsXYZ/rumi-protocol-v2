<script lang="ts">
  import { onMount } from 'svelte';
  import { protocolService } from '$lib/services/protocol';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { collateralStore } from '$lib/stores/collateralStore';
  import type { CollateralInfo } from '$lib/services/types';
  import { get } from 'svelte/store';

  let recoveryModeThreshold = 0;
  let liqProtocolPct = '3';
  let liqKeepPct = '97';
  let collaterals: CollateralInfo[] = [];
  let loaded = false;

  // Use first collateral for the worked example
  $: exCollateral = collaterals.length > 0 ? collaterals[0] : null;
  $: exSymbol = exCollateral?.symbol ?? 'ICP';
  $: exLiqPct = exCollateral ? (exCollateral.liquidationCr * 100).toFixed(0) : '133';
  $: exPenaltyPct = exCollateral ? ((exCollateral.liquidationBonus - 1) * 100).toFixed(0) : '15';
  $: exPenaltyMult = exCollateral ? (exCollateral.liquidationBonus * 100).toFixed(0) : '115';
  $: exBonus = exCollateral?.liquidationBonus ?? 1.15;
  $: recoveryPct = recoveryModeThreshold > 0 ? (recoveryModeThreshold * 100).toFixed(0) : '—';

  onMount(async () => {
    try {
      const [status, lpShare] = await Promise.all([
        protocolService.getProtocolStatus(),
        publicActor.get_liquidation_protocol_share() as Promise<number>,
      ]);
      if (status.recoveryModeThreshold > 0) recoveryModeThreshold = status.recoveryModeThreshold;
      const lps = Number(lpShare) * 100;
      liqProtocolPct = lps.toFixed(0);
      liqKeepPct = (100 - lps).toFixed(0);

      await collateralStore.fetchSupportedCollateral();
      const state = get(collateralStore);
      collaterals = state.collaterals;
    } catch (e) {
      console.error('Failed to fetch protocol status:', e);
    }
    loaded = true;
  });
</script>

<svelte:head><title>Liquidation Mechanics | Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Liquidation Mechanics</h1>

  <section class="doc-section">
    <h2 class="doc-heading">When Liquidation Happens</h2>
    <p>A vault becomes eligible for liquidation when its collateral ratio drops below the <a href="/docs/parameters" class="doc-link">liquidation threshold</a> for that collateral type. Each collateral has its own threshold; see <a href="/docs/parameters" class="doc-link">Protocol Parameters</a> for the current values. In Recovery mode, the threshold rises to the <a href="/docs/parameters" class="doc-link">borrowing threshold</a> for each type.</p>
    <p>The protocol checks vault health every time collateral prices update, approximately every 5 minutes via background polling. Price-sensitive operations (liquidations, borrows, etc.) also trigger an on-demand price refresh if the cached price is older than 60 seconds. Liquidation is not instant on price movement. It depends on the next price update.</p>
    <p>Interest accrual also affects vault health. Since interest increases your debt over time, a vault sitting just above the liquidation threshold can drift below it purely from accrued interest, even without any price change. Interest is applied before every vault operation and ticked forward every 60 seconds.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Who Liquidates</h2>
    <p>The protocol uses a three-tier liquidation cascade. Each tier gets one attempt per vault, in order:</p>
    <ol class="flow-list">
      <li><strong><a href="/docs/liquidation-bot" class="doc-link">Liquidation Bot</a></strong> — Notified first for supported collateral types. An autonomous canister that liquidates vaults on credit, sells the seized collateral on a DEX, and deposits the proceeds back to protocol reserves. Handles most liquidations within minutes.</li>
      <li><strong><a href="/docs/stability-pool" class="doc-link">Stability Pool</a></strong> — Receives vaults the bot can't or doesn't handle: unsupported collateral types, bot failures, exhausted budget, or vaults the bot hasn't processed within its 5-minute window. The pool gets exactly one attempt per vault; if it fails, the vault is not retried.</li>
      <li><strong>Manual Liquidators</strong> — Vaults that fall through both automated tiers land in the manual queue, where any user can liquidate directly via the <a href="/liquidations" class="doc-link">Liquidate</a> page using icUSD, ckUSDC, or ckUSDT.</li>
    </ol>
    <p>The sections below describe the mechanics that apply to all liquidation methods.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Full Liquidation</h2>
    <p>Any user can liquidate an undercollateralized vault. The liquidator pays the vault's full icUSD debt and receives collateral worth the debt plus the <a href="/docs/parameters" class="doc-link">liquidation penalty</a> for that collateral type. The penalty varies by collateral; see <a href="/docs/parameters" class="doc-link">Protocol Parameters</a> for current values. A {liqProtocolPct}% protocol fee is taken from the bonus before payout: the liquidator receives {liqKeepPct}% of the bonus, and {liqProtocolPct}% goes to the protocol treasury.</p>
    <p>If the vault's collateral is worth less than the debt plus penalty (deep undercollateralization), the liquidator receives all available collateral. For full liquidations, any excess collateral above the penalty is returned to the original vault owner. For partial liquidations, the excess remains in the vault since it stays open.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Partial Liquidation</h2>
    <p>Liquidators can also repay only a portion of a vault's debt. The maximum amount is capped at the amount needed to restore the vault's collateral ratio to the <a href="/docs/parameters" class="doc-link">Recovery Target CR</a> for that collateral type. The formula is the same as in Recovery mode:</p>
    <p class="doc-formula">max repay = (target CR &times; debt &minus; collateral value) &divide; (target CR &minus; liquidation penalty)</p>
    <p>If the requested amount is less than this cap, the liquidator pays their chosen amount. The liquidator receives collateral proportional to the debt they repay, plus the <a href="/docs/parameters" class="doc-link">liquidation penalty</a> for that collateral type. Partial liquidations leave the vault open with reduced debt and reduced collateral.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Paying with ckUSDT or ckUSDC</h2>
    <p>Liquidators can pay with ckUSDT or ckUSDC instead of icUSD. These are treated at a 1:1 rate with icUSD, minus a small conversion fee (see <a href="/docs/parameters" class="doc-link">Protocol Parameters</a>). The protocol checks the stablecoin's live price via the XRC oracle and rejects the transaction if the coin has depegged outside the $0.95–$1.05 range.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Liquidation Example</h2>
    <p>Suppose you have an {exSymbol} vault with 10 {exSymbol} (worth $100 at $10/{exSymbol}) and 70 icUSD debt. Your collateral ratio is 143%, which is safe. {exSymbol} drops to $7. Now your 10 {exSymbol} is worth $70, and your ratio is 100%, well below the {exLiqPct}% threshold.</p>
    <p>A liquidator repays your 70 icUSD debt and receives {exSymbol} worth ${(70 * exBonus).toFixed(0)} (70 &times; {exBonus.toFixed(2)}, including the {exPenaltyPct}% {exSymbol} liquidation penalty). That's {(70 * exBonus / 7).toFixed(1)} {exSymbol} at $7/{exSymbol}, but you only have 10 {exSymbol}, so the liquidator gets all 10 {exSymbol}. Your vault is closed. You keep the 70 icUSD you originally borrowed, but your {exSymbol} is gone.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Recovery Mode: Targeted Liquidation</h2>
    <p>When the protocol enters Recovery mode (total system CR below <span class="live">{recoveryPct}%</span>), the liquidation threshold rises to the <a href="/docs/parameters" class="doc-link">borrowing threshold</a> for each collateral type. Vaults between their liquidation ratio and borrowing threshold become liquidatable, but they are <strong>not</strong> fully liquidated.</p>
    <p class="doc-note">The Recovery mode threshold is not a fixed number. It is a debt-weighted average of all collateral types' borrowing thresholds, recalculated on each price tick. As the system's collateral mix shifts, the threshold shifts with it. See <a href="/docs/parameters" class="doc-link">Protocol Parameters</a> for the current value and calculation details.</p>
    <p>Instead, the protocol calculates the minimum amount of debt that needs to be repaid to restore the vault's collateral ratio to the <a href="/docs/parameters" class="doc-link">Recovery Target CR</a> for that collateral type. The liquidator pays only that amount and receives proportional collateral plus the collateral's <a href="/docs/parameters" class="doc-link">liquidation penalty</a>. The vault remains open with reduced debt and collateral at approximately the target CR.</p>
    <p>The formula is:</p>
    <p class="doc-formula">repay = (target CR &times; debt &minus; collateral value) &divide; (target CR &minus; penalty multiplier)</p>
    <p>Vaults below their liquidation ratio are still fully liquidated in both normal and Recovery mode.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Protocol Modes</h2>
    <p>The protocol operates in one of four modes based on the system-wide total collateral ratio. The Recovery mode threshold (currently <span class="live">{recoveryPct}%</span>) is dynamic: it is a debt-weighted average of all collateral types' borrowing thresholds, so it shifts as the system's collateral composition changes. See <a href="/docs/parameters" class="doc-link">Protocol Parameters</a> for details.</p>
    <p><strong>General Availability:</strong> total CR is above the Recovery mode threshold. Normal operations. Liquidation uses each collateral type's own <a href="/docs/parameters" class="doc-link">liquidation ratio</a>.</p>
    <p><strong>Recovery:</strong> total CR drops below the Recovery mode threshold. Liquidation threshold rises to the <a href="/docs/parameters" class="doc-link">borrowing threshold</a> for each type. The minimum collateral ratio for new borrows and withdrawals is raised to the <a href="/docs/parameters" class="doc-link">Recovery Target CR</a>. Vaults between the liquidation ratio and borrowing threshold get targeted partial liquidation.</p>
    <p><strong>Read-Only:</strong> all state-changing operations are paused. No new borrows, no liquidations, no redemptions. Read-Only is triggered by any of: total CR dropping below 100%, the oracle reporting a price below $0.01, the oracle circuit breaker (three consecutive failed price fetches; this one clears automatically when the oracle recovers), or accumulated bad debt crossing an admin-set threshold. The protocol waits for conditions to improve.</p>
    <p><strong>Frozen:</strong> emergency kill-switch activated manually by the protocol admin. All state-changing operations are halted until the admin unfreezes the protocol.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Liquidation Surge Breaker</h2>
    <p>The protocol tracks the total icUSD debt cleared by liquidations in a rolling window (30 minutes by default). If that total crosses an admin-set ceiling, a circuit breaker trips: the protocol stops routing new vaults to the liquidation bot and stability pool until an operator reviews the situation and resets the breaker. Manual liquidation stays open the whole time, so genuinely unhealthy vaults can still be cleared.</p>
    <p>This is a safety backstop against cascading or erroneous mass liquidations (for example, from a bad oracle print). It does not change your vault's liquidation threshold.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Bad Debt Accounting</h2>
    <p>If a liquidation or redemption clears more debt than the seized collateral was worth (deep undercollateralization), the shortfall is recorded in a protocol-level <strong>deficit account</strong> rather than being silently absorbed. A configurable fraction of protocol fees is routed to paying this deficit down over time, and if the deficit ever crosses an admin-set threshold, the protocol enters Read-Only mode until operators intervene.</p>
    <p class="doc-note">An older mechanism that redistributed a failed vault's debt and collateral across all other vaults of the same collateral type still exists in the codebase, but it is not wired to any automatic trigger today. Bad debt is handled through the deficit account instead.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Transfer Processing</h2>
    <p>When a liquidation occurs, the protocol attempts to transfer collateral to the liquidator immediately. If the transfer fails (e.g., due to a temporary ledger issue), it is queued and retried every 5 seconds, up to 60 attempts (about 5 minutes of retries). Transfers that still fail after that are flagged in the logs for manual intervention by the protocol admin.</p>
  </section>
</article>

<style>
  .doc-formula {
    font-family: 'Inter', monospace;
    font-size: 0.875rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    padding: 0.75rem 1rem;
    text-align: center;
    color: var(--rumi-text-primary);
    font-weight: 500;
  }
  .doc-note {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    background: var(--rumi-bg-surface1);
    border-left: 3px solid var(--rumi-action);
    border-radius: 0 0.5rem 0.5rem 0;
    padding: 0.625rem 1rem;
    margin: 0.5rem 0;
  }
  .flow-list {
    padding-left: 1.25rem;
    display: flex; flex-direction: column; gap: 0.5rem;
    margin: 0.5rem 0;
  }
  .flow-list li {
    font-size: 0.875rem; color: var(--rumi-text-secondary); line-height: 1.5;
  }
  .live { color: var(--rumi-action); font-weight: 600; }
</style>
