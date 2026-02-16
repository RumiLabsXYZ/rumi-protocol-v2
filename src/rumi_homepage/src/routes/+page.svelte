<script>
  const protocolSuite = [
    {
      name: 'Rumi Protocol',
      tag: 'LIVE',
      tagColor: '#34d399',
      description: 'Borrow icUSD against your ICP. A fully on-chain CDP engine with liquidation protection, stability pools, and transparent governance.',
      href: 'https://app.rumiprotocol.com',
      cta: 'Launch App'
    },
    {
      name: 'Coming Soon',
      tag: 'NEXT',
      tagColor: '#d176e8',
      description: 'Multi-collateral support, yield strategies, and more DeFi primitives — all native to ICP.',
      href: null,
      cta: null
    }
  ];

  const stats = [
    { label: 'Collateral Type', value: 'ICP' },
    { label: 'Stablecoin', value: 'icUSD' },
    { label: 'Min Collateral Ratio', value: '133%' },
    { label: 'Chain', value: '100% On-Chain' }
  ];

  const steps = [
    { num: '01', title: 'Deposit ICP', desc: 'Lock ICP as collateral in your personal vault.' },
    { num: '02', title: 'Mint icUSD', desc: 'Borrow icUSD stablecoins against your collateral.' },
    { num: '03', title: 'Use in DeFi', desc: 'Trade, provide liquidity, or hold — all on ICP.' },
    { num: '04', title: 'Repay & Unlock', desc: 'Return icUSD anytime to reclaim your ICP.' }
  ];

  const trustSignals = [
    { title: 'Open Source', desc: 'Every line of code is public and verifiable on GitHub.' },
    { title: 'Fully On-Chain', desc: 'No bridges, no oracles off-chain, no custodians. 100% Internet Computer.' },
    { title: 'Independently Audited', desc: 'Security audit completed by AVAI. Full report available on this site.' },
    { title: 'SNS Roadmap', desc: 'Moving toward decentralized governance via the Service Nervous System.' },
  ];
</script>

<!-- Hero -->
<section class="hero-section">
  <div class="hero-glow"></div>
  <div class="max-w-5xl mx-auto px-6 py-24 md:py-32 text-center relative z-10">
    <p class="text-sm font-medium tracking-widest uppercase mb-6 animate-in"
       style="color: var(--rumi-purple-accent); animation-delay: 0.1s;">
      DeFi Infrastructure on the Internet Computer
    </p>
    <h1 class="hero-headline animate-in" style="animation-delay: 0.25s;">
      Don't sell your ICP.<br/>Borrow instead.
    </h1>
    <p class="text-lg md:text-xl max-w-2xl mx-auto mb-10 animate-in"
       style="color: var(--rumi-text-secondary); animation-delay: 0.4s; line-height: 1.7;">
      Rumi is the first native stablecoin protocol on the Internet Computer.
      Collateralize ICP to mint icUSD and access stable liquidity.
    </p>
    <div class="flex flex-col sm:flex-row gap-4 justify-center animate-in" style="animation-delay: 0.55s;">
      <a href="https://app.rumiprotocol.com" target="_blank" rel="noopener" class="cta-primary">Launch App</a>
      <a href="/Rumi-Protocol-v2-Whitepaper.pdf" target="_blank" class="cta-secondary">Read Whitepaper</a>
    </div>
  </div>
</section>

<!-- Stats Bar -->
<section class="border-y" style="background: var(--rumi-bg-surface1); border-color: var(--rumi-border);">
  <div class="max-w-5xl mx-auto px-6 py-6 grid grid-cols-2 md:grid-cols-4 gap-6">
    {#each stats as stat}
      <div class="text-center">
        <div class="text-xs font-medium tracking-wider uppercase mb-1" style="color: var(--rumi-text-muted);">
          {stat.label}
        </div>
        <div class="text-xl font-bold" style="color: var(--rumi-text-primary);
             font-variant-numeric: tabular-nums;">
          {stat.value}
        </div>
      </div>
    {/each}
  </div>
</section>

<!-- Protocol Suite -->
<section class="max-w-5xl mx-auto px-6 py-20">
  <h2 class="section-heading mb-3">The Protocol Suite</h2>
  <p class="text-sm mb-10" style="color: var(--rumi-text-secondary); max-width: 480px;">
    A growing ecosystem of DeFi primitives, purpose-built for the Internet Computer.
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
      Four steps from ICP holder to icUSD borrower.
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
      Most DeFi stablecoins depend on off-chain components — oracles, bridges, custodians. 
      Rumi Protocol runs entirely inside ICP canisters. The frontend, the backend, the ledger, 
      the price feeds — all on-chain. No external dependencies. No bridge risk.
    </p>
    <p class="text-sm" style="color: var(--rumi-text-secondary); max-width: 560px; line-height: 1.7;">
      This is what "fully decentralized" actually looks like: a protocol that can run 
      without anyone's permission, served directly from the blockchain to your browser.
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
      Connect your wallet, deposit ICP, and mint icUSD in minutes. 
      No KYC, no intermediaries, no bridges.
    </p>
    <a href="https://app.rumiprotocol.com" target="_blank" rel="noopener" class="cta-primary">Launch App</a>
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
</style>
