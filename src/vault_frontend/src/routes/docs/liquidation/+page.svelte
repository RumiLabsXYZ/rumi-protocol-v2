<script lang="ts">
  import { onMount } from 'svelte';
  import { protocolService } from '$lib/services/protocol';
  import { collateralStore } from '$lib/stores/collateralStore';
  import type { CollateralInfo } from '$lib/services/types';
  import { get } from 'svelte/store';

  let liquidationBonus = 1.15;
  let recoveryTargetCr = 1.55;
  let recoveryPct = '150';
  let collaterals: CollateralInfo[] = [];
  let loaded = false;

  $: bonusPct = ((liquidationBonus - 1) * 100).toFixed(0);
  $: bonusMult = (liquidationBonus * 100).toFixed(0);
  $: targetPct = (recoveryTargetCr * 100).toFixed(0);
  // Summary string like "133% for ICP, 125% for ckETH"
  $: liqSummary = collaterals.map(c => `${(c.liquidationCr * 100).toFixed(0)}% for ${c.symbol}`).join(', ');
  $: borrowSummary = collaterals.map(c => `${(c.minimumCr * 100).toFixed(0)}% for ${c.symbol}`).join(', ');
  // Use first collateral as the example default
  $: exLiqPct = collaterals.length > 0 ? (collaterals[0].liquidationCr * 100).toFixed(0) : '133';
  $: exBorrowPct = collaterals.length > 0 ? (collaterals[0].minimumCr * 100).toFixed(0) : '150';
  $: exSymbol = collaterals.length > 0 ? collaterals[0].symbol : 'ICP';

  onMount(async () => {
    try {
      const status = await protocolService.getProtocolStatus();
      if (status.liquidationBonus > 0) liquidationBonus = status.liquidationBonus;
      if (status.recoveryTargetCr > 0) recoveryTargetCr = status.recoveryTargetCr;
      if (status.recoveryModeThreshold > 0) recoveryPct = (status.recoveryModeThreshold * 100).toFixed(0);

      await collateralStore.fetchSupportedCollateral();
      const state = get(collateralStore);
      collaterals = state.collaterals;
    } catch (e) {
      console.error('Failed to fetch protocol status:', e);
    }
    loaded = true;
  });
</script>

<svelte:head><title>Liquidation Mechanics - Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Liquidation Mechanics</h1>

  <section class="doc-section">
    <h2 class="doc-heading">When Liquidation Happens</h2>
    <p>A vault becomes eligible for liquidation when its collateral ratio drops below the liquidation threshold for that collateral type{liqSummary ? ` (${liqSummary})` : ''}. In Recovery mode, the threshold rises to the borrowing threshold{borrowSummary ? ` (${borrowSummary})` : ''}.</p>
    <p>The protocol checks vault health every time collateral prices update — approximately every 5 minutes via background polling. Price-sensitive operations (liquidations, borrows, etc.) also trigger an on-demand price refresh if the cached price is older than 30 seconds. Liquidation is not instant on price movement; it depends on the next price update.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Full Liquidation</h2>
    <p>Any user can liquidate an undercollateralized vault. The liquidator pays the vault's full icUSD debt and receives the vault's collateral at a {bonusPct}% bonus — meaning they get collateral worth {bonusMult}% of the debt they repaid, up to the total collateral in the vault.</p>
    <p>If the vault's collateral is worth less than {bonusMult}% of the debt (deep undercollateralization), the liquidator receives all available collateral. For full liquidations, any excess collateral above the {bonusMult}% is returned to the original vault owner. For partial liquidations, the excess remains in the vault since it stays open.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Partial Liquidation</h2>
    <p>Liquidators can also repay only a portion of a vault's debt. The maximum amount is capped at the amount needed to restore the vault's collateral ratio to the Recovery Target CR. The formula is the same as in Recovery mode:</p>
    <p class="doc-formula">max repay = (target CR &times; debt &minus; collateral value) &divide; (target CR &minus; liquidation bonus)</p>
    <p>If the requested amount is less than this cap, the liquidator pays their chosen amount. The liquidator receives collateral proportional to the debt they repay, plus the same {bonusPct}% bonus. Partial liquidations leave the vault open with reduced debt and reduced collateral.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Paying with ckUSDT or ckUSDC</h2>
    <p>Liquidators can pay with ckUSDT or ckUSDC instead of icUSD. These are treated at a 1:1 rate with icUSD, minus a small conversion fee (see <a href="/docs/parameters" class="doc-link">Protocol Parameters</a>). The protocol checks the stablecoin's live price via the XRC oracle and rejects the transaction if the coin has depegged outside the $0.95–$1.05 range.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Liquidation Example</h2>
    <p>Suppose you have an {exSymbol} vault with 10 {exSymbol} (worth $100 at $10/{exSymbol}) and 70 icUSD debt. Your collateral ratio is 143% — safe. {exSymbol} drops to $7. Now your 10 {exSymbol} is worth $70, and your ratio is 100% — well below the {exLiqPct}% threshold.</p>
    <p>A liquidator repays your 70 icUSD debt and receives {exSymbol} worth ${(70 * liquidationBonus).toFixed(0)} (70 &times; {liquidationBonus.toFixed(2)}). That's {(70 * liquidationBonus / 7).toFixed(1)} {exSymbol} at $7/{exSymbol} — but you only have 10 {exSymbol}, so the liquidator gets all 10 {exSymbol}. Your vault is closed. You keep the 70 icUSD you originally borrowed, but your {exSymbol} is gone.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Recovery Mode — Targeted Liquidation</h2>
    <p>When the protocol enters Recovery mode (total system CR below {recoveryPct}%), the liquidation threshold rises to the borrowing threshold for each collateral type. Vaults between their liquidation ratio and borrowing threshold become liquidatable — but they are <strong>not</strong> fully liquidated.</p>
    <p>Instead, the protocol calculates the minimum amount of debt that needs to be repaid to restore the vault's collateral ratio to {targetPct}%. The liquidator pays only that amount and receives proportional collateral plus the {bonusPct}% bonus. The vault remains open with reduced debt and collateral at approximately {targetPct}% CR.</p>
    <p>The formula is:</p>
    <p class="doc-formula">repay = ({targetPct}% &times; debt &minus; collateral value) &divide; ({targetPct}% &minus; {bonusMult}%)</p>
    <p>Vaults below their liquidation ratio are still fully liquidated in both normal and Recovery mode.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Protocol Modes</h2>
    <p>The protocol operates in one of three modes based on the system-wide total collateral ratio:</p>
    <p><strong>General Availability</strong> — total CR is above {recoveryPct}%. Normal operations. Liquidation uses each collateral type's own liquidation ratio.</p>
    <p><strong>Recovery</strong> — total CR drops below {recoveryPct}%. Liquidation threshold rises to the borrowing threshold for each type. Borrowing fee drops to 0% to encourage repayment. Vaults between the liquidation ratio and borrowing threshold get targeted partial liquidation.</p>
    <p><strong>Read-Only</strong> — total CR drops below 100%, or the oracle reports a price below $0.01. All state-changing operations are paused. No new borrows, no liquidations. The protocol waits for conditions to improve.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Redistribution</h2>
    <p>If a vault is deeply undercollateralized and no liquidator steps in, the protocol can <strong>redistribute</strong> the vault's remaining debt and collateral across all other vaults of the same collateral type. Each surviving vault absorbs a share proportional to its own collateral:</p>
    <p class="doc-formula">share = your collateral &divide; total other vault collateral</p>
    <p>Your vault gains both extra collateral and extra debt from the redistributed vault. The net effect is a slight decrease in your collateral ratio. Redistribution is a last resort — it protects the protocol from bad debt but spreads the cost across all vault owners.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Transfer Processing</h2>
    <p>When a liquidation occurs, the protocol attempts to transfer collateral to the liquidator immediately. If the transfer fails (e.g., due to a temporary ledger issue), the transfer is queued and retried with exponential backoff — 1s, 2s, 4s, 8s, 16s. A health monitor also checks for stuck transfers every 5 minutes as a fallback.</p>
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
</style>
