<section class="max-w-4xl mx-auto px-6 py-20 md:py-28">
  <p class="text-sm font-medium tracking-widest uppercase mb-6"
     style="color: var(--rumi-purple-accent);">
    About
  </p>

  <h1 class="text-3xl md:text-4xl font-bold mb-8" 
      style="font-family: 'Circular Std', 'Inter', sans-serif; color: var(--rumi-text-primary); letter-spacing: -0.02em;">
    Building DeFi infrastructure for the Internet Computer
  </h1>

  <div class="prose-section">
    <p>
      Rumi Labs builds decentralized financial protocols natively on the Internet Computer.
      Our flagship product is <strong>icUSD</strong>, a CDP stablecoin that lets you borrow
      a stable dollar against your crypto without selling it.
    </p>

    <p>
      The protocol is fully on-chain. The frontend, the backend, the ledger, the price
      feeds: everything runs inside ICP canisters. No bridges, no off-chain oracles,
      no external dependencies. When you interact with Rumi Protocol, you're interacting
      directly with canisters served from the blockchain to your browser.
    </p>

    <h2>How it works</h2>

    <p>
      Users deposit collateral (ICP, ckBTC, or ckXAUT) into personal vaults and mint
      icUSD against it. Each collateral type has its own minimum collateral ratio,
      liquidation threshold, and interest rate. If a vault's collateral ratio drops too
      low during a market downturn, it can be liquidated, and anyone can participate as
      a liquidator.
    </p>

    <p>
      The Stability Pool is the protocol's second layer of protection. Depositors supply
      icUSD that absorbs liquidated debt, earning discounted collateral and interest from
      borrowers in return. This keeps the system solvent without requiring a central operator.
    </p>

    <p>
      icUSD holders can redeem against the protocol's reserve of ckUSDT and ckUSDC
      (accumulated from repayments) at a 0.3% fee, or directly against vaults for the
      underlying collateral. Both paths are subject to the Redemption Margin Ratio (RMR),
      a dynamic discount that adjusts based on the system's overall collateral health.
      When the system is well-collateralized, redeemers receive a small discount; when
      it needs rebalancing, redemptions approach par.
    </p>

    <h2>Why ICP</h2>

    <p>
      The Internet Computer can do things most blockchains can't: serve full web applications
      directly, run autonomous canister logic on timers, and provide native price feeds through
      the Exchange Rate Canister. That makes it possible to build a complete CDP protocol with
      zero off-chain infrastructure.
    </p>

    <p>
      That said, DeFi on ICP is harder in some ways. There's no atomic composability between
      canisters, cross-canister calls are asynchronous, and the ecosystem tooling is still
      maturing. We build here because the ICP ecosystem needs foundational DeFi primitives,
      and a CDP stablecoin is the most fundamental one.
    </p>

    <h2>Open source & audited</h2>

    <p>
      The entire codebase is public on <a href="https://github.com/RumiLabsXYZ/rumi-protocol-v2" target="_blank" rel="noopener">GitHub</a>.
      The protocol has been through an internal three-pass security review and an external
      pre-audit by AVAI; both are fully remediated and published on the
      <a href="/security">security page</a>. All canister IDs are public so anyone can
      verify the code that's running.
    </p>

    <p>
      All canisters are currently mutable and under developer control. The plan is to
      transition governance to an SNS DAO, but that comes after the protocol is
      battle-tested and the code has been thoroughly audited.
    </p>

    <h2>What's next</h2>

    <p>
      Adding new collateral types is essentially drag and drop at this point, with more
      assets on the way. Beyond that, we're working toward in-app stableswap LPs with
      routing to external liquidity, cross-chain collateral and borrowing, and eventual
      transition to decentralized governance through the ICP Service Nervous System (SNS).
    </p>
  </div>
</section>

<style>
  .prose-section {
    max-width: 640px;
  }

  .prose-section p {
    color: var(--rumi-text-secondary);
    font-size: 0.9375rem;
    line-height: 1.75;
    margin-bottom: 1.5rem;
  }

  .prose-section strong {
    color: var(--rumi-text-primary);
  }

  .prose-section h2 {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.25rem;
    font-weight: 700;
    color: var(--rumi-text-primary);
    margin-top: 2.5rem;
    margin-bottom: 1rem;
    letter-spacing: -0.02em;
  }

  .prose-section a {
    color: var(--rumi-action);
    text-decoration: none;
    transition: color 0.15s ease;
  }

  .prose-section a:hover {
    color: var(--rumi-action-bright);
  }
</style>
