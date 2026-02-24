<script lang="ts">
  import { onMount } from 'svelte';
  import { protocolService } from '$lib/services/protocol';
  import { collateralStore } from '$lib/stores/collateralStore';
  import { get } from 'svelte/store';

  let targetPct = '155';
  let liqPct = '133';
  let borrowPct = '150';
  let recoveryPct = '150';

  onMount(async () => {
    try {
      const status = await protocolService.getProtocolStatus();
      if (status.recoveryTargetCr > 0) targetPct = (status.recoveryTargetCr * 100).toFixed(0);
      if (status.recoveryModeThreshold > 0) recoveryPct = (status.recoveryModeThreshold * 100).toFixed(0);

      await collateralStore.fetchSupportedCollateral();
      const state = get(collateralStore);
      const icpConfig = state.collaterals.find(c => c.symbol === 'ICP');
      if (icpConfig) {
        liqPct = (icpConfig.liquidationCr * 100).toFixed(0);
        borrowPct = (icpConfig.minimumCr * 100).toFixed(0);
      }
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
    <p>ICP can move sharply. A vault at 140% collateral ratio is only one bad candle away from liquidation. The protocol polls prices every 5 minutes and refreshes on-demand for operations — if ICP drops sharply between updates, your vault could go from safe to liquidated with no intermediate warning.</p>
    <p>There is no notification system. You are responsible for monitoring your own vaults.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Oracle Failure</h2>
    <p>The protocol gets ICP prices from the Internet Computer's Exchange Rate Canister (XRC). If the XRC fails to return a price, the protocol continues using the last known price. If the XRC returns a price below $0.01, the protocol switches to Read-Only mode and halts all operations.</p>
    <p>Risks include: stale prices leading to delayed liquidations (bad for the protocol) or premature liquidations if the XRC reports an incorrect price (bad for vault owners). The XRC is an IC system canister — Rumi has no control over its availability or accuracy.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Smart Contract Risk</h2>
    <p>Rumi's backend canisters are written in Rust and deployed on the Internet Computer. The codebase was reviewed by an AI-powered auditing agent (<a href="https://www.avai.life/" class="doc-link" target="_blank" rel="noopener">AVAI</a>) but has not undergone a formal audit by a traditional human-led security firm. Bugs in the vault logic, liquidation math, or state management could result in loss of funds.</p>
    <p>Canister upgrades are controlled by a set of principals (the development team). An upgrade with a bug could affect all vaults simultaneously. There is currently no time-lock or governance mechanism on upgrades.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Ledger and Transfer Failures</h2>
    <p>Operations involve multiple ledger calls (ICP transfers, icUSD minting). If a transfer fails mid-operation, the protocol uses guards to prevent double-processing and queues failed transfers for retry. However, edge cases could result in temporary inconsistencies — for example, a vault state updating before a transfer completes.</p>
    <p>The protocol includes a health monitor that checks for stuck transfers every 5 minutes and retries them, but transfers stuck for over 15 minutes may require manual intervention.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Recovery Mode Cascades</h2>
    <p>If the total system collateral ratio drops below the Recovery Mode threshold (currently {recoveryPct}%), the protocol enters Recovery mode and raises the liquidation threshold to the borrowing threshold (currently {borrowPct}% for ICP). This can cause vaults that were previously safe to suddenly become liquidatable — even though those individual vaults didn't change.</p>
    <p>In Recovery mode, vaults between {liqPct}% and {borrowPct}% CR receive <strong>targeted partial liquidation</strong> — only enough debt is repaid to restore their CR to {targetPct}%. They are not fully liquidated. Vaults below {liqPct}% are still fully liquidated. See <a href="/docs/liquidation" class="doc-link">Liquidation Mechanics</a> for details.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Peg Stability</h2>
    <p>icUSD is designed to be worth $1, but there is no hard guarantee. The peg is maintained through overcollateralization and a redemption mechanism. If confidence in the protocol drops, icUSD could trade below $1. Rumi does not control secondary market pricing.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Stablecoin Depeg Risk</h2>
    <p>The protocol accepts ckUSDT and ckUSDC for vault repayment and liquidation at a 1:1 rate with icUSD. If either stablecoin depegs significantly, this could allow users to repay debt at a discount. As a safeguard, the protocol checks live prices via the XRC oracle and rejects transactions if the stablecoin is trading outside the $0.95–$1.05 range.</p>
  </section>
</article>
