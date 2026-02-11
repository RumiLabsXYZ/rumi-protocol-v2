<svelte:head><title>Protocol Parameters - Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Protocol Parameters</h1>
  <p class="doc-intro">These are the current constants and thresholds as implemented in the Rumi Protocol backend. Values are hardcoded and can only change through a canister upgrade.</p>

  <section class="doc-section">
    <h2 class="doc-heading">Collateral & Borrowing</h2>
    <div class="params-table">
      <div class="param"><span class="param-label">Minimum Collateral Ratio</span><span class="param-val">133%</span></div>
      <div class="param"><span class="param-label">Recovery Mode Threshold</span><span class="param-val">150% (system-wide)</span></div>
      <div class="param"><span class="param-label">Borrowing Fee</span><span class="param-val">0.5% (0% in Recovery)</span></div>
      <div class="param"><span class="param-label">Liquidation Bonus</span><span class="param-val">10%</span></div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Minimums</h2>
    <div class="params-table">
      <div class="param"><span class="param-label">Minimum ICP Deposit</span><span class="param-val">0.001 ICP</span></div>
      <div class="param"><span class="param-label">Minimum Borrow Amount</span><span class="param-val">1 icUSD</span></div>
      <div class="param"><span class="param-label">Minimum Partial Repayment</span><span class="param-val">0.01 icUSD</span></div>
      <div class="param"><span class="param-label">Dust Threshold (auto-forgiven)</span><span class="param-val">0.000001 icUSD</span></div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Oracle & Timing</h2>
    <div class="params-table">
      <div class="param"><span class="param-label">Price Source</span><span class="param-val">IC Exchange Rate Canister (XRC)</span></div>
      <div class="param"><span class="param-label">Price Fetch Interval</span><span class="param-val">60 seconds</span></div>
      <div class="param"><span class="param-label">Read-Only Price Floor</span><span class="param-val">$0.01</span></div>
      <div class="param"><span class="param-label">Stuck Transfer Timeout</span><span class="param-val">15 minutes</span></div>
      <div class="param"><span class="param-label">Health Monitor Interval</span><span class="param-val">5 minutes</span></div>
    </div>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Protocol Modes</h2>
    <div class="params-table">
      <div class="param"><span class="param-label">General Availability</span><span class="param-val">Total CR ≥ 150%</span></div>
      <div class="param"><span class="param-label">Recovery</span><span class="param-val">Total CR &lt; 150%</span></div>
      <div class="param"><span class="param-label">Read-Only</span><span class="param-val">Total CR &lt; 100% or oracle failure</span></div>
    </div>
  </section>
</article>

<style>
  .params-table { display: flex; flex-direction: column; gap: 0.5rem; }
  .param {
    display: flex; justify-content: space-between; align-items: baseline;
    padding: 0.5rem 0; border-bottom: 1px solid var(--rumi-border);
  }
  .param:last-child { border-bottom: none; }
  .param-label { font-size: 0.8125rem; color: var(--rumi-text-secondary); }
  .param-val {
    font-family: 'Inter', sans-serif; font-size: 0.875rem;
    font-weight: 600; color: var(--rumi-text-primary);
    font-variant-numeric: tabular-nums;
  }
</style>
