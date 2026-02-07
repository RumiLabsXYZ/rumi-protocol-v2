<svelte:head><title>Before You Borrow - Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Before You Borrow</h1>

  <section class="doc-section">
    <h2 class="doc-heading">How Borrowing Works</h2>
    <p>You deposit ICP into a vault as collateral, then borrow icUSD against it. The icUSD is minted at the time of borrowing — it doesn't come from a pool. Your ICP stays locked in the vault until you repay the debt and withdraw it.</p>
    <p>Each vault is independent. You can have multiple vaults, each with its own collateral and debt.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Minimum Requirements</h2>
    <p>The minimum collateral deposit is 0.001 ICP. The minimum borrow amount is 1 icUSD. Your vault must maintain a collateral ratio of at least 133% at all times. If ICP's price drops and your ratio falls below 133%, your vault becomes eligible for liquidation.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Fees</h2>
    <p>A one-time borrowing fee of 0.5% is deducted from the icUSD you borrow. If you borrow 100 icUSD, you receive 99.5 icUSD and owe 100 icUSD. There is no ongoing interest. The fee drops to 0% if the protocol enters Recovery mode.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Closing Your Vault</h2>
    <p>To close a vault, you must first repay all outstanding icUSD debt, then withdraw your ICP collateral. The vault can then be closed. Dust amounts below 0.000001 icUSD are forgiven automatically on close.</p>
    <p>You can also partially repay debt or add more collateral at any time to improve your collateral ratio.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">What You Should Understand</h2>
    <p>ICP is volatile. A sharp price drop can push your vault below the liquidation threshold faster than you can react. There is no grace period and no notification system — liquidation is immediate and automated.</p>
    <p>Higher collateral ratios give you more buffer. A vault at 200% can absorb a much larger price drop than one at 140%.</p>
    <p>This protocol is in beta. See the <a href="/docs/beta" class="doc-link">beta disclaimer</a> and <a href="/docs/risks" class="doc-link">risk documentation</a> for full details.</p>
  </section>
</article>
