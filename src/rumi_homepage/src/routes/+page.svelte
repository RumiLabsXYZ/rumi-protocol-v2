<script>
  let appUrl = 'https://app.rumiprotocol.xyz';

  // ── Single source of truth for supported collateral ──
  // Update this list when adding new assets. Everything else derives from it.
  const collateralTokens = [
    { name: 'ICP', logo: '/icp-token-dark.svg' },
    { name: 'ckBTC', logo: '/ckBTC_logo.svg' },
    { name: 'ckXAUT', logo: '/ckXAUT_logo.svg' }
  ];
  $: tokenNames = collateralTokens.map(t => t.name);
  $: collateralListOr = tokenNames.length > 2
    ? tokenNames.slice(0, -1).join(', ') + ', or ' + tokenNames[tokenNames.length - 1]
    : tokenNames.join(' or ');

  $: protocolSuite = [
    {
      name: 'Borrow',
      tag: 'LIVE',
      tagColor: '#34d399',
      description: `Deposit ${collateralListOr} as collateral and mint icUSD. Each asset has its own risk parameters, liquidation thresholds, and interest rates.`,
      href: appUrl,
      cta: 'Launch App'
    },
    {
      name: 'Earn',
      tag: 'LIVE',
      tagColor: '#34d399',
      description: 'Deposit icUSD to the Stability Pool to earn interest from borrowers and profit from liquidations. The pool absorbs under-collateralized vaults automatically.',
      href: appUrl + '/stability-pool',
      cta: 'View Stability Pool'
    }
  ];

  $: steps = [
    { num: '01', title: 'Deposit Collateral', desc: `Lock ${collateralListOr} in your personal vault.` },
    { num: '02', title: 'Mint icUSD', desc: 'Borrow icUSD against your collateral at dynamic rates based on protocol health.' },
    { num: '03', title: 'Use in DeFi', desc: 'Trade, provide liquidity, or hold. Your collateral keeps working while you borrow.' },
    { num: '04', title: 'Repay & Unlock', desc: 'Return icUSD (or repay with ckUSDT/ckUSDC) anytime to reclaim your collateral.' }
  ];

  const trustSignals = [
    { title: 'Open Source', desc: 'Every line of code is public and verifiable on GitHub.' },
    { title: 'Fully On-Chain', desc: 'No bridges, no off-chain oracles, no custodians. Frontend, backend, ledger, price feeds: all canisters.' },
    { title: 'Audited', desc: 'Security audit completed by AVAI, with a Code4rena contest planned to further harden the protocol.' },
    { title: 'Path to Decentralization', desc: 'All canisters are currently under dev control. The plan is to hand governance to an SNS DAO.' },
  ];
</script>

<!-- Hero -->
<section class="hero-section">
  <div class="hero-glow"></div>
  <div class="max-w-5xl mx-auto px-6 py-24 md:py-32 text-center relative z-10">
    <p class="text-sm font-medium tracking-widest uppercase mb-6 animate-in"
       style="color: var(--rumi-purple-accent); animation-delay: 0.1s;">
      Native Stablecoin Protocol on ICP
    </p>
    <h1 class="hero-headline animate-in" style="animation-delay: 0.25s;">
      Don't sell your crypto.<br/>Borrow against it.
    </h1>
    <p class="text-lg md:text-xl max-w-2xl mx-auto mb-10 animate-in"
       style="color: var(--rumi-text-secondary); animation-delay: 0.4s; line-height: 1.7;">
      Mint icUSD against your crypto. No bridges, no intermediaries, everything on-chain.
    </p>
    <div class="flex flex-col sm:flex-row gap-4 justify-center animate-in" style="animation-delay: 0.55s;">
      <a href={appUrl} target="_blank" rel="noopener" class="cta-primary">Launch App</a>
      <a href="/Rumi-Protocol-v2-Whitepaper.pdf" target="_blank" class="cta-secondary">Read Whitepaper</a>
    </div>
  </div>
</section>

<!-- Trust Strip -->
<section class="border-y" style="background: var(--rumi-bg-surface1); border-color: var(--rumi-border);">
  <div class="max-w-5xl mx-auto px-6 py-5">
    <div class="trust-strip">
      <div class="trust-strip-tokens">
        <span class="token-pill">
          <img src="/icusd-logo_v3.svg" alt="icUSD logo" class="token-logo" />
          icUSD
        </span>
        <span class="token-arrow">←</span>
        {#each collateralTokens as token}
          <span class="token-pill">
            <img src={token.logo} alt="{token.name} logo" class="token-logo" />
            {token.name}
          </span>
        {/each}
      </div>
      <span class="trust-strip-divider"></span>
      <div class="trust-strip-signals">
        <span>Audited by AVAI</span>
        <span class="trust-sep">·</span>
        <span>Open Source</span>
        <span class="trust-sep">·</span>
        <span>Live on Mainnet</span>
      </div>
    </div>
  </div>
</section>

<!-- Protocol Suite -->
<section class="max-w-5xl mx-auto px-6 py-20">
  <h2 class="section-heading mb-3">The Protocol</h2>
  <p class="text-sm mb-10" style="color: var(--rumi-text-secondary); max-width: 560px; line-height: 1.7;">
    Deposit any supported asset into a personal vault and borrow icUSD at dynamic rates.
    The Stability Pool backs the system, absorbing liquidations and earning returns for depositors.
    Everything runs on-chain, live on mainnet.
  </p>
  <div class="grid grid-cols-1 md:grid-cols-2 gap-5">
    {#each protocolSuite as protocol}
      <div class="product-card">
        <div class="flex items-center gap-3 mb-4">
          <h3 class="text-lg font-semibold" style="color: var(--rumi-text-primary);">{protocol.name}</h3>
          <span class="tag" style="color: {protocol.tagColor}; border-color: {protocol.tagColor}30;">{protocol.tag}</span>
        </div>
        <p class="text-sm mb-6" style="color: var(--rumi-text-secondary); line-height: 1.65;">{protocol.description}</p>
        {#if protocol.href}
          <a href={protocol.href} target="_blank" rel="noopener" class="cta-primary inline-block text-sm">{protocol.cta} →</a>
        {/if}
      </div>
    {/each}
  </div>
</section>

<!-- How It Works -->
<section class="py-20 border-t" style="border-color: var(--rumi-border);">
  <div class="max-w-5xl mx-auto px-6">
    <h2 class="section-heading mb-3">How It Works</h2>
    <p class="text-sm mb-12" style="color: var(--rumi-text-secondary); max-width: 420px;">
      Four steps from collateral to stablecoin.
    </p>
    <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-5">
      {#each steps as step}
        <div class="step-card">
          <div class="step-num">{step.num}</div>
          <h3 class="text-base font-semibold mb-2" style="color: var(--rumi-text-primary);">{step.title}</h3>
          <p class="text-sm" style="color: var(--rumi-text-secondary); line-height: 1.6;">{step.desc}</p>
        </div>
      {/each}
    </div>
  </div>
</section>

<!-- Why ICP -->
<section class="py-20 border-t" style="border-color: var(--rumi-border);">
  <div class="max-w-5xl mx-auto px-6">
    <h2 class="section-heading mb-3">Why the Internet Computer?</h2>
    <p class="text-sm mb-4" style="color: var(--rumi-text-secondary); max-width: 560px; line-height: 1.7;">
      ICP can do things most blockchains can't: serve full web applications, run autonomous
      canister logic on timers, and provide native price feeds through the Exchange Rate
      Canister. That makes it possible to build a complete CDP protocol with zero off-chain
      infrastructure.
    </p>
    <p class="text-sm" style="color: var(--rumi-text-secondary); max-width: 560px; line-height: 1.7;">
      Rumi runs entirely inside canisters. The frontend, backend, ledger, and price feeds
      are all on-chain. No bridges, no external oracles, no custodians. The protocol is
      served directly from the blockchain to your browser.
    </p>
  </div>
</section>

<!-- Trust -->
<section class="py-20 border-t" style="border-color: var(--rumi-border);">
  <div class="max-w-5xl mx-auto px-6">
    <h2 class="section-heading mb-3">Built for Trust</h2>
    <p class="text-sm mb-10" style="color: var(--rumi-text-secondary); max-width: 440px;">
      DeFi requires transparency. Here's how we earn it.
    </p>
    <div class="grid grid-cols-1 sm:grid-cols-2 gap-5">
      {#each trustSignals as signal}
        <div class="trust-card">
          <h3 class="text-sm font-semibold mb-2" style="color: var(--rumi-text-primary);">{signal.title}</h3>
          <p class="text-sm" style="color: var(--rumi-text-secondary); line-height: 1.6;">{signal.desc}</p>
        </div>
      {/each}
    </div>
  </div>
</section>

<!-- CTA -->
<section class="py-20 border-t" style="border-color: var(--rumi-border);">
  <div class="max-w-3xl mx-auto px-6 text-center">
    <h2 class="text-2xl md:text-3xl font-bold mb-4" style="color: var(--rumi-text-primary);">Ready to borrow icUSD?</h2>
    <p class="text-sm mb-8" style="color: var(--rumi-text-secondary); line-height: 1.7; max-width: 400px; margin-left: auto; margin-right: auto;">
      Connect your wallet, deposit collateral, and mint icUSD in minutes.
      No KYC, no intermediaries, no bridges.
    </p>
    <a href={appUrl} target="_blank" rel="noopener" class="cta-primary">Launch App</a>
  </div>
</section>

<style>
  .hero-section {
    position: relative;
    overflow: hidden;
  }

  .hero-glow {
    position: absolute;
    top: -100px;
    left: 50%;
    transform: translate(-50%);
    width: 700px;
    height: 500px;
    background: radial-gradient(ellipse, rgba(209,118,232,0.06) 0%, rgba(52,211,153,0.03) 40%, transparent 70%);
    pointer-events: none;
  }

  .hero-headline {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 3rem;
    font-weight: 700;
    line-height: 1.15;
    letter-spacing: -0.03em;
    margin-bottom: 1.5rem;
    background: linear-gradient(135deg, var(--rumi-text-primary) 30%, var(--rumi-purple-accent) 70%, var(--rumi-action) 100%);
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    background-clip: text;
  }

  @media (min-width: 768px) {
    .hero-headline { font-size: 4rem; }
  }

  .section-heading {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.5rem;
    font-weight: 700;
    color: var(--rumi-purple-accent);
    letter-spacing: -0.02em;
  }

  .cta-primary {
    display: inline-block;
    padding: 0.75rem 2rem;
    background: var(--rumi-action);
    color: var(--rumi-bg-primary);
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-weight: 500;
    font-size: 0.9375rem;
    border-radius: 0.5rem;
    text-decoration: none;
    transition: all 0.2s ease;
  }

  .cta-primary:hover {
    background: var(--rumi-action-bright);
    box-shadow: 0 0 24px rgba(52,211,153,0.15);
  }

  .cta-secondary {
    display: inline-block;
    padding: 0.75rem 2rem;
    background: transparent;
    color: var(--rumi-text-secondary);
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-weight: 500;
    font-size: 0.9375rem;
    border: 1px solid var(--rumi-border-hover);
    border-radius: 0.5rem;
    text-decoration: none;
    transition: all 0.2s ease;
  }

  .cta-secondary:hover {
    border-color: var(--rumi-action);
    color: var(--rumi-action);
  }

  .product-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
    transition: border-color 0.2s ease;
    box-shadow: inset 0 1px rgba(200,210,240,0.03), 0 2px 8px -2px rgba(8,11,22,0.6);
  }

  .product-card:hover {
    border-color: var(--rumi-border-hover);
  }

  .tag {
    font-size: 0.6875rem;
    font-weight: 600;
    letter-spacing: 0.08em;
    padding: 0.125rem 0.5rem;
    border: 1px solid;
    border-radius: 9999px;
  }

  .step-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.25rem;
  }

  .step-num {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-action);
    margin-bottom: 0.75rem;
    opacity: 0.7;
  }

  .trust-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.25rem;
  }

  .trust-strip {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 1.5rem;
    flex-wrap: wrap;
  }

  .trust-strip-tokens {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .token-pill {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    font-size: 0.8125rem;
    font-weight: 500;
    color: var(--rumi-text-primary);
    letter-spacing: 0.01em;
  }

  .token-arrow {
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
    opacity: 0.5;
    margin: 0 0.125rem;
  }

  .token-logo {
    width: 20px;
    height: 20px;
    border-radius: 50%;
    flex-shrink: 0;
    object-fit: contain;
  }

  .trust-strip-divider {
    width: 1px;
    height: 1rem;
    background: var(--rumi-border-hover);
    flex-shrink: 0;
  }

  .trust-strip-signals {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.8125rem;
    color: var(--rumi-text-muted);
  }

  .trust-sep {
    opacity: 0.4;
  }

  @media (max-width: 640px) {
    .trust-strip {
      flex-direction: column;
      gap: 0.75rem;
    }
    .trust-strip-divider {
      width: 2rem;
      height: 1px;
    }
  }
</style>
