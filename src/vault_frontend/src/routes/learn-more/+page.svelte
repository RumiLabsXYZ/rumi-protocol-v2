<script lang="ts">
  const concepts = [
    {
      title: "Vaults",
      description: "Deposit ICP as collateral to borrow icUSD. Each vault must maintain at least 130% collateralization to stay healthy."
    },
    {
      title: "icUSD",
      description: "A stablecoin soft-pegged to the US Dollar, minted against ICP collateral. Use it across the Internet Computer ecosystem."
    },
    {
      title: "Liquidation",
      description: "If a vault's collateral ratio drops below 130%, it becomes eligible for liquidation. Liquidators repay the debt and claim discounted collateral."
    },
    {
      title: "Stability Pool",
      description: "Deposit icUSD to earn liquidation bonuses. The pool provides instant liquidity when vaults are liquidated, and depositors receive a 10% discount on claimed collateral."
    }
  ];

  const steps = [
    { title: "Connect Wallet", description: "Use Internet Identity, Plug, or Oisy to connect." },
    { title: "Create a Vault", description: "Deposit ICP as collateral." },
    { title: "Borrow icUSD", description: "Mint stablecoins against your collateral at a minimum 130% ratio." },
    { title: "Manage Position", description: "Add collateral, repay debt, or withdraw — keep your ratio healthy." }
  ];
</script>

<svelte:head>
  <title>RUMI Protocol - Learn</title>
</svelte:head>

<div class="page-container">
  <h1 class="page-title learn-title">Understanding Rumi Protocol</h1>
  <p class="page-subtitle">
    Rumi enables you to borrow icUSD stablecoins using ICP as collateral — DeFi stability native to the Internet Computer.
  </p>

  <!-- Core Concepts -->
  <section class="section">
    <h2 class="section-title">Core Concepts</h2>
    <div class="concept-grid">
      {#each concepts as concept}
        <div class="concept-card">
          <h3 class="concept-heading">{concept.title}</h3>
          <p class="concept-text">{concept.description}</p>
        </div>
      {/each}
    </div>
  </section>

  <!-- How It Works -->
  <section class="section">
    <h2 class="section-title">How It Works</h2>
    <div class="steps-list">
      {#each steps as step, i}
        <div class="step-row">
          <div class="step-number">{i + 1}</div>
          <div>
            <h3 class="step-heading">{step.title}</h3>
            <p class="step-text">{step.description}</p>
          </div>
        </div>
      {/each}
    </div>
  </section>

  <!-- Protocol Parameters -->
  <section class="section">
    <h2 class="section-title">Protocol Parameters</h2>
    <div class="params-card">
      <div class="param-row">
        <span class="param-label">Minimum Collateral Ratio</span>
        <span class="param-value">130%</span>
      </div>
      <div class="param-row">
        <span class="param-label">Liquidation Threshold</span>
        <span class="param-value">130%</span>
      </div>
      <div class="param-row">
        <span class="param-label">Borrowing Fee</span>
        <span class="param-value">0.5%</span>
      </div>
      <div class="param-row">
        <span class="param-label">Liquidation Bonus</span>
        <span class="param-value">10%</span>
      </div>
    </div>
  </section>
</div>

<style>
  .page-container { max-width: 800px; margin: 0 auto; }

  /* Learn page uses Secondary Brand — gradient title allowed here */
  .learn-title {
    background: linear-gradient(135deg, var(--rumi-purple-accent), var(--rumi-action));
    background-clip: text;
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    color: transparent;
  }
  .page-subtitle {
    font-size: 1rem;
    color: var(--rumi-text-secondary);
    line-height: 1.6;
    max-width: 600px;
    margin-bottom: 3rem;
  }

  .section { margin-bottom: 3rem; }

  /* Concept grid */
  .concept-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 1rem; }
  .concept-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.25rem;
    transition: box-shadow 0.2s ease, border-color 0.2s ease;
  }
  .concept-card:hover {
    border-color: rgba(209, 118, 232, 0.08);
    box-shadow: inset 0 0 20px 0 rgba(209, 118, 232, 0.06);
  }
  .concept-heading {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    margin-bottom: 0.375rem;
  }
  .concept-text { font-size: 0.8125rem; color: var(--rumi-text-secondary); line-height: 1.5; }

  /* Steps */
  .steps-list { display: flex; flex-direction: column; gap: 1.25rem; }
  .step-row { display: flex; align-items: flex-start; gap: 1rem; }
  .step-number {
    flex-shrink: 0;
    width: 2rem; height: 2rem;
    display: flex; align-items: center; justify-content: center;
    border-radius: 50%;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    font-family: 'Inter', sans-serif;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-muted);
  }
  .step-heading {
    font-size: 0.9375rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    margin-bottom: 0.125rem;
  }
  .step-text { font-size: 0.8125rem; color: var(--rumi-text-secondary); }

  /* Params card */
  .params-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.25rem;
    display: flex; flex-direction: column; gap: 0.75rem;
  }
  .param-row { display: flex; justify-content: space-between; align-items: baseline; }
  .param-label { font-size: 0.8125rem; color: var(--rumi-text-secondary); }
  .param-value {
    font-family: 'Inter', sans-serif;
    font-size: 0.9375rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
  }

  @media (max-width: 768px) {
    .concept-grid { grid-template-columns: 1fr; }
  }
</style>
