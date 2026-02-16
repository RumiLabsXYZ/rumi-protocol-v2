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
      Our flagship product is <strong>icUSD</strong> — a CDP stablecoin that lets ICP holders 
      borrow a stable dollar against their tokens without selling.
    </p>

    <p>
      The protocol is fully on-chain. The frontend, the backend, the ledger, the price feeds — 
      everything runs inside ICP canisters. There are no bridges, no off-chain oracles, 
      no external dependencies. When you interact with Rumi Protocol, you're interacting 
      directly with smart contracts served from the blockchain to your browser.
    </p>

    <h2>How it works</h2>

    <p>
      Users deposit ICP into personal vaults and mint icUSD stablecoins against that collateral. 
      Vaults must maintain a minimum 133% collateral ratio. If a vault falls below this threshold 
      during a market downturn, it can be liquidated — and anyone can participate as a liquidator 
      to earn a reward.
    </p>

    <p>
      The Stability Pool provides a second layer of protection: depositors supply icUSD that 
      absorbs liquidated debt, earning discounted ICP collateral in return. This mechanism 
      keeps the system solvent without requiring a central operator.
    </p>

    <h2>Why ICP</h2>

    <p>
      The Internet Computer excels at things most blockchains can't do: serve web applications 
      directly, run autonomous services on timers, and provide on-chain price feeds through its 
      Exchange Rate Canister. These properties make it possible to build a complete lending 
      protocol with zero off-chain infrastructure.
    </p>

    <p>
      That said, DeFi on ICP is harder in some ways — there's no atomic composability between 
      canisters, cross-canister calls are asynchronous, and the ecosystem tooling is still 
      maturing. We build here because the ICP ecosystem needs foundational DeFi primitives, 
      and a CDP stablecoin is the most fundamental one.
    </p>

    <h2>Open source & audited</h2>

    <p>
      The entire codebase is public on <a href="https://github.com/RumiLabsXYZ/rumi-protocol-v2" target="_blank" rel="noopener">GitHub</a>. 
      A security audit was completed by AVAI — the full report is available on this site. 
      All canister IDs are public so anyone can verify the code running the protocol.
    </p>

    <h2>What's next</h2>

    <p>
      We're working toward multi-collateral support (ckBTC, ckETH), yield strategies, 
      and eventual transition to decentralized governance through the ICP Service Nervous System (SNS). 
      The goal is a protocol that runs without anyone's permission — owned and governed by its users.
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
