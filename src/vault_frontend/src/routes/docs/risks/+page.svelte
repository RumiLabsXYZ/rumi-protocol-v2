<script lang="ts">
  import { onMount } from 'svelte';
  import { protocolService } from '$lib/services/protocol';
  import { collateralStore } from '$lib/stores/collateralStore';
  import type { CollateralInfo } from '$lib/services/types';
  import { get } from 'svelte/store';

  let targetPct = '155';
  let recoveryPct = '150';
  let collaterals: CollateralInfo[] = [];

  $: liqSummary = collaterals.map(c => `${(c.liquidationCr * 100).toFixed(0)}% for ${c.symbol}`).join(', ');
  $: borrowSummary = collaterals.map(c => `${(c.minimumCr * 100).toFixed(0)}% for ${c.symbol}`).join(', ');

  onMount(async () => {
    try {
      const status = await protocolService.getProtocolStatus();
      if (status.recoveryTargetCr > 0) targetPct = (status.recoveryTargetCr * 100).toFixed(0);
      if (status.recoveryModeThreshold > 0) recoveryPct = (status.recoveryModeThreshold * 100).toFixed(0);

      await collateralStore.fetchSupportedCollateral();
      const state = get(collateralStore);
      collaterals = state.collaterals;
    } catch (e) {
      console.error('Failed to fetch protocol status:', e);
    }
  });
</script>

<svelte:head><title>What Can Go Wrong - Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">What Can Go Wrong</h1>

  <section class="doc-section">
    <h2 class="doc-heading">Price Volatility</h2>
    <p>Collateral assets can move sharply. A vault at 140% collateral ratio is only one bad candle away from liquidation. The protocol polls prices every 5 minutes and refreshes on-demand for operations — if a collateral asset drops sharply between updates, your vault could go from safe to liquidated with no intermediate warning.</p>
    <p>There is no notification system. You are responsible for monitoring your own vaults.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Oracle Failure</h2>
    <p>The protocol gets collateral prices from the Internet Computer's Exchange Rate Canister (XRC). If the XRC fails to return a price, the protocol continues using the last known price. If the XRC returns a price below $0.01, the protocol switches to Read-Only mode and halts all operations.</p>
    <p>Risks include: stale prices leading to delayed liquidations (bad for the protocol) or premature liquidations if the XRC reports an incorrect price (bad for vault owners). The XRC is an IC system canister — Rumi has no control over its availability or accuracy.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Smart Contract Risk</h2>
    <p>Rumi's backend canisters are written in Rust and deployed on the Internet Computer. The codebase was reviewed by an AI-powered auditing agent (<a href="https://www.avai.life/" class="doc-link" target="_blank" rel="noopener">AVAI</a>) but has not undergone a formal audit by a traditional human-led security firm. Bugs in the vault logic, liquidation math, or state management could result in loss of funds.</p>
    <p>Canister upgrades are controlled by a set of principals (the development team). An upgrade with a bug could affect all vaults simultaneously. There is currently no time-lock or governance mechanism on upgrades.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Ledger and Transfer Failures</h2>
    <p>Operations involve multiple ledger calls (collateral transfers, icUSD minting). If a transfer fails mid-operation, the protocol uses guards to prevent double-processing and queues failed transfers for retry. However, edge cases could result in temporary inconsistencies — for example, a vault state updating before a transfer completes.</p>
    <p>The protocol includes a health monitor that checks for stuck transfers every 5 minutes and retries them, but transfers stuck for over 15 minutes may require manual intervention.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Recovery Mode Cascades</h2>
    <p>If the total system collateral ratio drops below the Recovery Mode threshold (currently {recoveryPct}%), the protocol enters Recovery mode and raises the liquidation threshold to the borrowing threshold for each collateral type{borrowSummary ? ` (${borrowSummary})` : ''}. This can cause vaults that were previously safe to suddenly become liquidatable — even though those individual vaults didn't change.</p>
    <p>The Recovery Mode threshold is a <strong>debt-weighted average</strong> of all collateral types' borrowing thresholds. If new collateral types are added with different thresholds, the trigger point for Recovery Mode changes for everyone. Borrowing is still allowed in Recovery mode, but the fee drops to 0% to encourage repayment.</p>
    <p>In Recovery mode, vaults between their liquidation ratio and borrowing threshold receive <strong>targeted partial liquidation</strong> — only enough debt is repaid to restore their CR to {targetPct}%. They are not fully liquidated. Vaults below their liquidation ratio are still fully liquidated. See <a href="/docs/liquidation" class="doc-link">Liquidation Mechanics</a> for details.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Redistribution Risk</h2>
    <p>If a vault becomes deeply undercollateralized and is not liquidated by a third party, the protocol can redistribute its remaining debt and collateral across all other vaults of the same collateral type. This means your vault can absorb extra debt from a failed vault — even if your own vault is healthy. The extra debt comes with proportional extra collateral, so the net impact is a slight CR decrease. See <a href="/docs/liquidation" class="doc-link">Liquidation Mechanics</a> for the formula.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Peg Stability</h2>
    <p>icUSD is designed to be worth $1, but there is no hard guarantee. The peg is maintained through overcollateralization and a redemption mechanism. If confidence in the protocol drops, icUSD could trade below $1. Rumi does not control secondary market pricing.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Stablecoin Depeg Risk</h2>
    <p>The protocol accepts ckUSDT and ckUSDC for vault repayment and liquidation at a 1:1 rate with icUSD. If either stablecoin depegs significantly, this could allow users to repay debt at a discount. As a safeguard, the protocol checks live prices via the XRC oracle and rejects transactions if the stablecoin is trading outside the $0.95–$1.05 range.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Non-Atomic Inter-Canister Calls</h2>
    <p>Unlike Ethereum where transactions are atomic (all-or-nothing), the Internet Computer uses asynchronous inter-canister messaging. A multi-step operation (e.g., burn icUSD then send ckStable) can fail partway through. The protocol mitigates this with a compensation pattern: if a later step fails, earlier steps are automatically reversed (e.g., icUSD is refunded). However, if the reversal also fails, manual intervention via <a href="/transparency" class="doc-link">admin mint</a> may be required.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Admin Controls</h2>
    <p>The developer principal can mint icUSD directly via an <code>admin_mint_icusd</code> function. This exists as an emergency tool — for example, to refund users who lost funds due to a failed inter-canister transfer. Guardrails: each mint is capped at 1,500 icUSD with a 72-hour cooldown. Every use is recorded on-chain with a reason. See the <a href="/transparency" class="doc-link">Transparency</a> page for a full log.</p>
    <p>Beyond minting, the developer principal can adjust all configurable protocol parameters without a timelock or governance vote. This includes: borrowing fee, liquidation bonus, redemption fee floor/ceiling, reserve redemption fee, ckStable repay fee, recovery liquidation buffer, and per-collateral settings (liquidation ratio, borrow threshold, debt ceiling, status). The developer can also enable/disable reserve redemptions and individual stablecoin tokens.</p>
    <p>All parameter changes are recorded as on-chain events in the protocol's immutable event log. If the protocol transitions to SNS (DAO) governance, these functions would be controlled by governance proposals rather than a single principal.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Reserve Depletion</h2>
    <p>The protocol accumulates ckUSDT and ckUSDC reserves when users repay vault debt with stablecoins. These reserves are used to fill <a href="/docs/redemptions" class="doc-link">reserve redemptions</a> (Tier 1). If redemption demand exceeds the available reserves, the remainder spills over into vault redemptions, which take collateral from the lowest-CR vaults.</p>
    <p>Heavy redemption activity can drain reserves entirely, causing all subsequent redemptions to hit vaults directly. The protocol admin can disable reserve redemptions if reserve levels become critically low. Vault owners should be aware that redemptions can reduce their collateral even if they maintain healthy collateral ratios — vaults with the lowest CRs are targeted first.</p>
  </section>
</article>
