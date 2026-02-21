<script lang="ts">
  import { onMount } from 'svelte';
  import { protocolService } from '$lib/services/protocol';

  let liquidationBonus = 1.15;
  let recoveryTargetCr = 1.55;
  let loaded = false;

  $: bonusPct = ((liquidationBonus - 1) * 100).toFixed(0);
  $: bonusMult = (liquidationBonus * 100).toFixed(0);
  $: targetPct = (recoveryTargetCr * 100).toFixed(0);

  onMount(async () => {
    try {
      const status = await protocolService.getProtocolStatus();
      if (status.liquidationBonus > 0) liquidationBonus = status.liquidationBonus;
      if (status.recoveryTargetCr > 0) recoveryTargetCr = status.recoveryTargetCr;
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
    <p>A vault becomes eligible for liquidation when its collateral ratio drops below the minimum threshold. In normal operation (General Availability mode), this threshold is 133%. In Recovery mode, it rises to 150%.</p>
    <p>The protocol checks vault health every time the ICP price updates — approximately every 60 seconds. Liquidation is not instant on price movement; it depends on the next price fetch cycle.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Full Liquidation</h2>
    <p>Any user can liquidate an undercollateralized vault. The liquidator pays the vault's full icUSD debt and receives the vault's ICP collateral at a {bonusPct}% bonus — meaning they get ICP worth {bonusMult}% of the debt they repaid, up to the total collateral in the vault.</p>
    <p>If the vault's collateral is worth less than {bonusMult}% of the debt (deep undercollateralization), the liquidator receives all available collateral. Any excess collateral above the {bonusMult}% is returned to the original vault owner.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Partial Liquidation</h2>
    <p>Liquidators can also repay only a portion of a vault's debt rather than the full amount. The maximum partial liquidation is capped at a configurable percentage of the vault's total debt (see <a href="/docs/parameters" class="doc-link">Protocol Parameters</a>). The liquidator receives ICP collateral proportional to the debt they repay, plus the same {bonusPct}% bonus.</p>
    <p>Partial liquidations leave the vault open with reduced debt and reduced collateral. This is useful for large vaults where repaying the full debt may not be practical.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Paying with ckUSDT or ckUSDC</h2>
    <p>Liquidators can pay with ckUSDT or ckUSDC instead of icUSD. These are treated at a 1:1 rate with icUSD, minus a small conversion fee (see <a href="/docs/parameters" class="doc-link">Protocol Parameters</a>). The protocol checks the stablecoin's live price via the XRC oracle and rejects the transaction if the coin has depegged outside the $0.95–$1.05 range.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Liquidation Example</h2>
    <p>Suppose you have a vault with 10 ICP (worth $100 at $10/ICP) and 70 icUSD debt. Your collateral ratio is 143% — safe. ICP drops to $7. Now your 10 ICP is worth $70, and your ratio is 100% — well below the 133% threshold.</p>
    <p>A liquidator repays your 70 icUSD debt and receives ICP worth ${(70 * liquidationBonus).toFixed(0)} (70 &times; {liquidationBonus.toFixed(2)}). That's {(70 * liquidationBonus / 7).toFixed(1)} ICP at $7/ICP — but you only have 10 ICP, so the liquidator gets all 10 ICP. Your vault is closed. You keep the 70 icUSD you originally borrowed, but your ICP is gone.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Recovery Mode — Targeted Liquidation</h2>
    <p>When the protocol enters Recovery mode (total system CR below 150%), the liquidation threshold rises to 150%. Vaults between 133% and 150% CR become liquidatable — but they are <strong>not</strong> fully liquidated.</p>
    <p>Instead, the protocol calculates the minimum amount of debt that needs to be repaid to restore the vault's collateral ratio to {targetPct}%. The liquidator pays only that amount and receives proportional ICP collateral plus the {bonusPct}% bonus. The vault remains open with reduced debt and collateral at approximately {targetPct}% CR.</p>
    <p>The formula is:</p>
    <p class="doc-formula">repay = ({targetPct}% &times; debt &minus; collateral value) &divide; ({targetPct}% &minus; {bonusMult}%)</p>
    <p>Vaults below 133% CR are still fully liquidated in both normal and Recovery mode.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Protocol Modes</h2>
    <p>The protocol operates in one of three modes based on the system-wide total collateral ratio:</p>
    <p><strong>General Availability</strong> — total CR is above 150%. Normal operations. Liquidation threshold is 133%. Borrowing fee applies.</p>
    <p><strong>Recovery</strong> — total CR drops below 150%. Liquidation threshold rises to 150%. Borrowing fee drops to 0% to encourage repayment. Vaults between 133–150% get targeted partial liquidation.</p>
    <p><strong>Read-Only</strong> — total CR drops below 100%, or the oracle reports a price below $0.01. All state-changing operations are paused. No new borrows, no liquidations. The protocol waits for conditions to improve.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Transfer Processing</h2>
    <p>When a liquidation occurs, the protocol attempts to transfer ICP to the liquidator immediately. If the transfer fails (e.g., due to a temporary ledger issue), the transfer is queued and retried with exponential backoff — 1s, 2s, 4s, 8s, 16s. A health monitor also checks for stuck transfers every 5 minutes as a fallback.</p>
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
