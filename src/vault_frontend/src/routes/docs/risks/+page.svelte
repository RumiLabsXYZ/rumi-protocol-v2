<svelte:head><title>What Can Go Wrong - Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">What Can Go Wrong</h1>

  <section class="doc-section">
    <h2 class="doc-heading">Price Volatility</h2>
    <p>ICP can move sharply. A vault at 140% collateral ratio is only one bad candle away from liquidation. The protocol checks prices every 60 seconds — if ICP drops 10% between checks, your vault could go from safe to liquidated with no intermediate warning.</p>
    <p>There is no notification system. You are responsible for monitoring your own vaults.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Oracle Failure</h2>
    <p>The protocol gets ICP prices from the Internet Computer's Exchange Rate Canister (XRC). If the XRC fails to return a price, the protocol continues using the last known price. If the XRC returns a price below $0.01, the protocol switches to Read-Only mode and halts all operations.</p>
    <p>Risks include: stale prices leading to delayed liquidations (bad for the protocol) or premature liquidations if the XRC reports an incorrect price (bad for vault owners). The XRC is an IC system canister — Rumi has no control over its availability or accuracy.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Smart Contract Risk</h2>
    <p>Rumi's backend canisters are written in Rust and deployed on the Internet Computer. While the code has been reviewed, it has not undergone a formal third-party audit. Bugs in the vault logic, liquidation math, or state management could result in loss of funds.</p>
    <p>Canister upgrades are controlled by a set of principals (the development team). An upgrade with a bug could affect all vaults simultaneously. There is currently no time-lock or governance mechanism on upgrades.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Ledger and Transfer Failures</h2>
    <p>Operations involve multiple ledger calls (ICP transfers, icUSD minting). If a transfer fails mid-operation, the protocol uses guards to prevent double-processing and queues failed transfers for retry. However, edge cases could result in temporary inconsistencies — for example, a vault state updating before a transfer completes.</p>
    <p>The protocol includes a health monitor that checks for stuck transfers every 5 minutes and retries them, but transfers stuck for over 15 minutes may require manual intervention.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Recovery Mode Cascades</h2>
    <p>If the total system collateral ratio drops below 150%, the protocol enters Recovery mode and raises the liquidation threshold to 150%. This can cause vaults that were previously safe (e.g., at 145%) to suddenly become liquidatable — even though those individual vaults didn't change. A systemic price drop can trigger a cascade of liquidations.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Peg Stability</h2>
    <p>icUSD is designed to be worth $1, but there is no hard guarantee. The peg is maintained through overcollateralization and a redemption mechanism. If confidence in the protocol drops, icUSD could trade below $1. Rumi does not control secondary market pricing.</p>
  </section>
</article>
