<svelte:head><title>Liquidation Mechanics - Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Liquidation Mechanics</h1>

  <section class="doc-section">
    <h2 class="doc-heading">When Liquidation Happens</h2>
    <p>A vault becomes eligible for liquidation when its collateral ratio drops below the minimum threshold. In normal operation (General Availability mode), this threshold is 133%. In Recovery mode, it rises to 150%.</p>
    <p>The protocol checks vault health every time the ICP price updates — approximately every 60 seconds. Liquidation is not instant on price movement; it depends on the next price fetch cycle.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">How Liquidation Works</h2>
    <p>Any user can liquidate an undercollateralized vault. The liquidator pays the vault's full icUSD debt and receives the vault's ICP collateral at a 10% bonus — meaning they get ICP worth 110% of the debt they repaid, up to the total collateral in the vault.</p>
    <p>If the vault's collateral is worth less than 110% of the debt (deep undercollateralization), the liquidator receives all available collateral. Any excess collateral above the 110% is returned to the original vault owner.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Liquidation Example</h2>
    <p>Suppose you have a vault with 10 ICP (worth $100 at $10/ICP) and 70 icUSD debt. Your collateral ratio is 143% — safe. ICP drops to $7. Now your 10 ICP is worth $70, and your ratio is 100% — well below the 133% threshold.</p>
    <p>A liquidator repays your 70 icUSD debt and receives ICP worth $77 (70 × 1.10). That's 11 ICP at $7/ICP — but you only have 10 ICP, so the liquidator gets all 10 ICP. Your vault is closed. You keep the 70 icUSD you originally borrowed, but your ICP is gone.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Protocol Modes</h2>
    <p>The protocol operates in one of three modes based on the system-wide total collateral ratio:</p>
    <p><strong>General Availability</strong> — total CR is above 150%. Normal operations. Liquidation threshold is 133%. Borrowing fee is 0.5%.</p>
    <p><strong>Recovery</strong> — total CR drops below 150%. Liquidation threshold rises to 150% to protect the system. Borrowing fee drops to 0% to encourage repayment.</p>
    <p><strong>Read-Only</strong> — total CR drops below 100%, or the oracle reports a price below $0.01. All state-changing operations are paused. No new borrows, no liquidations. The protocol waits for conditions to improve.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Transfer Processing</h2>
    <p>When a liquidation occurs, the protocol attempts to transfer ICP to the liquidator immediately. If the transfer fails (e.g., due to a temporary ledger issue), the transfer is queued and retried with exponential backoff — 1s, 2s, 4s, 8s, 16s. A health monitor also checks for stuck transfers every 5 minutes as a fallback.</p>
  </section>
</article>
