<script lang="ts">
  import { onMount } from 'svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { threePoolService } from '$lib/services/threePoolService';

  type InterestSplitEntry = { destination: string; bps: bigint };
  let interestSplit: InterestSplitEntry[] = [];

  function splitPct(dest: string): string {
    const entry = interestSplit.find(s => s.destination === dest);
    return entry ? (Number(entry.bps) / 100).toFixed(0) + '%' : '—';
  }

  // Pool state
  let tokenSymbols: string[] = [];
  let tokenDecimals: number[] = [];
  let swapFeeBps = 0n;
  let adminFeeBps = 0n;
  let currentA = 0n;
  let balances: bigint[] = [];
  let lpTotalSupply = 0n;
  let virtualPrice = 0n;
  let loaded = false;

  function fmtBalance(bal: bigint, decimals: number): string {
    const divisor = 10 ** decimals;
    return (Number(bal) / divisor).toLocaleString(undefined, { maximumFractionDigits: 2 });
  }

  function fmtVirtualPrice(vp: bigint): string {
    // Virtual price is scaled by 1e18
    return (Number(vp) / 1e18).toFixed(6);
  }

  onMount(async () => {
    try {
      const [poolStatus, split] = await Promise.all([
        threePoolService.getPoolStatus(),
        publicActor.get_interest_split() as Promise<InterestSplitEntry[]>,
      ]);

      tokenSymbols = poolStatus.tokens.map(t => t.symbol);
      tokenDecimals = poolStatus.tokens.map(t => Number(t.decimals));
      swapFeeBps = poolStatus.swap_fee_bps;
      adminFeeBps = poolStatus.admin_fee_bps;
      currentA = poolStatus.current_a;
      balances = poolStatus.balances;
      lpTotalSupply = poolStatus.lp_total_supply;
      virtualPrice = poolStatus.virtual_price;
      interestSplit = split;
    } catch (e) {
      console.error('Failed to fetch 3pool data:', e);
    }
    loaded = true;
  });
</script>

<svelte:head><title>3pool (Stablecoin AMM) | Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">3pool (Stablecoin AMM)</h1>

  <section class="doc-section">
    <h2 class="doc-heading">What Is the 3pool</h2>
    <p>The 3pool is a StableSwap automated market maker (AMM) for stablecoins on the Internet Computer. It uses a Curve-style invariant optimized for assets that should trade near 1:1, allowing swaps between stablecoins with minimal slippage and low fees.</p>
    <p>The pool serves two purposes: it provides deep stablecoin liquidity for the Rumi ecosystem, and it strengthens the icUSD peg by enabling efficient arbitrage between icUSD and other stablecoins.</p>
  </section>

  {#if loaded}
  <section class="doc-section">
    <h2 class="doc-heading">Pool Tokens</h2>
    <p>The pool currently holds three stablecoins:</p>
    <div class="params-table">
      {#each tokenSymbols as sym, i}
      <div class="param">
        <span class="param-label">{sym}</span>
        <span class="param-val live">{fmtBalance(balances[i] ?? 0n, tokenDecimals[i] ?? 8)}</span>
      </div>
      {/each}
      <div class="param">
        <span class="param-label">LP Token Supply (3USD)</span>
        <span class="param-val live">{fmtBalance(lpTotalSupply, 8)}</span>
      </div>
      <div class="param">
        <span class="param-label">Virtual Price</span>
        <span class="param-val live">{fmtVirtualPrice(virtualPrice)}</span>
      </div>
    </div>
  </section>
  {/if}

  <section class="doc-section">
    <h2 class="doc-heading">How Swaps Work</h2>
    <p>To swap one stablecoin for another, you approve the input token for spending (ICRC-2) and then call the pool's <code>swap</code> function specifying the input token, output token, amount, and minimum output. The pool calculates the output using the StableSwap invariant and transfers the result to your wallet.</p>
    <p>A small swap fee is applied to each trade. The swap fee is currently <span class="live">{loaded ? (Number(swapFeeBps) / 100).toFixed(2) + '%' : '—'}</span>. Of that fee, <span class="live">{loaded ? (Number(adminFeeBps) / 100).toFixed(0) + '%' : '—'}</span> is retained as a protocol admin fee and the remainder accrues to LP holders via virtual price growth.</p>
    <p>If the pool's balances are imbalanced (e.g., heavy on one token), swaps that improve the balance receive slightly better rates than swaps that worsen it. This is an inherent property of the StableSwap curve, not a separate mechanism.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Adding Liquidity</h2>
    <p>You can add liquidity by depositing one, two, or all three stablecoins. The pool mints 3USD LP tokens proportional to the value you add relative to the total pool. Approve each token (ICRC-2) before calling <code>add_liquidity</code>.</p>
    <p>Deposits that are proportional to the current pool balances incur no additional fee. Imbalanced deposits (e.g., adding only one token) may incur a small imbalance penalty because they shift the pool away from equilibrium. This penalty is typically very small for stablecoin pools.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Removing Liquidity</h2>
    <p>There are two ways to remove liquidity:</p>
    <ul class="doc-list">
      <li><strong>Proportional withdrawal</strong> — burns LP tokens and returns a proportional share of all three pool tokens. No fee applies.</li>
      <li><strong>Single-token withdrawal</strong> — burns LP tokens and returns only one chosen token. An imbalance fee may apply since this shifts the pool balance.</li>
    </ul>
    <p>There is no lock-up period. You can withdraw liquidity at any time.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">LP Token (3USD)</h2>
    <p>When you add liquidity, you receive 3USD — an ICRC-1 token minted by the 3pool canister. 3USD represents your proportional share of the pool's total value. It is a standard ICRC-1 token: you can transfer it, check your balance, and use it in any ICRC-1-compatible application.</p>
    <p>The value of 3USD is tracked by the <strong>virtual price</strong>, which represents how much underlying value each LP token is worth. The virtual price starts at 1.0 and grows over time as the pool earns revenue from swap fees and interest donations. It never decreases under normal conditions.</p>
    <p>Current virtual price: <span class="live">{loaded ? fmtVirtualPrice(virtualPrice) : '—'}</span></p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Interest Revenue</h2>
    <p>The Rumi Protocol donates a share of all vault interest revenue to the 3pool. Currently, <span class="live">{splitPct('three_pool')}</span> of all interest collected from borrowers is sent to the pool via the <code>donate</code> function. This increases the virtual price of 3USD, meaning LP holders earn yield passively without taking any action.</p>
    <p>The remaining interest is split between the <a href="/docs/stability-pool" class="doc-link">stability pool</a> (<span class="live">{splitPct('stability_pool')}</span>) and the protocol treasury (<span class="live">{splitPct('treasury')}</span>). This split is admin-configurable — see <a href="/docs/parameters" class="doc-link">Protocol Parameters</a> for current values.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">APY</h2>
    <p>The 3pool APY comes from two sources:</p>
    <ul class="doc-list">
      <li><strong>Swap fee revenue</strong> — every swap generates fees, a portion of which accrues to LP holders via virtual price growth.</li>
      <li><strong>Interest donations</strong> — the protocol donates vault interest to the pool, directly increasing virtual price.</li>
    </ul>
    <p>APY is calculated from observed virtual price growth over time (24 hours, 7 days, or 30 days). The pool takes snapshots of the virtual price every 6 hours for this purpose. Past performance does not guarantee future returns — APY depends on swap volume and protocol interest revenue.</p>
  </section>

  {#if loaded}
  <section class="doc-section">
    <h2 class="doc-heading">Pool Parameters</h2>
    <div class="params-table">
      <div class="param">
        <span class="param-label">Swap Fee</span>
        <span class="param-val live">{(Number(swapFeeBps) / 100).toFixed(2)}%</span>
      </div>
      <div class="param">
        <span class="param-label">Admin Fee (fraction of swap fee)</span>
        <span class="param-val live">{(Number(adminFeeBps) / 100).toFixed(0)}%</span>
      </div>
      <div class="param">
        <span class="param-label">Amplification Coefficient (A)</span>
        <span class="param-val live">{currentA.toString()}</span>
      </div>
      <div class="param">
        <span class="param-label">Interest Donation Share</span>
        <span class="param-val live">{splitPct('three_pool')}</span>
      </div>
    </div>
    <p class="doc-note">All parameters shown in <span class="live-indicator">teal</span> are admin-configurable and reflect the current on-chain state.</p>
  </section>
  {/if}

  <section class="doc-section">
    <h2 class="doc-heading">Amplification Coefficient</h2>
    <p>The amplification coefficient (A) controls how tightly the pool prices assets near 1:1. A higher A means the pool behaves more like a constant-sum (x + y = k) curve near equilibrium, providing very low slippage for small trades. A lower A means the pool behaves more like a constant-product (x × y = k) curve, with more slippage but better protection against extreme imbalances.</p>
    <p>The A parameter can be changed by the pool admin using a gradual ramp — it transitions linearly over a set period (typically hours to days) to avoid sudden liquidity disruption. The current value is <span class="live">{loaded ? currentA.toString() : '—'}</span>.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Risks</h2>
    <p><strong>Depeg risk:</strong> If one of the stablecoins in the pool loses its peg (e.g., ckUSDT depegs to $0.90), arbitrageurs will swap the depegged token into the pool and extract the healthy tokens. LP holders end up holding a disproportionate share of the depegged token. The StableSwap curve provides some protection compared to constant-product AMMs, but sustained depegs can still cause losses.</p>
    <p><strong>Smart contract risk:</strong> The 3pool is a separate canister from the main Rumi Protocol. It has its own code and its own risks. A bug in the pool's StableSwap math, rounding, or token transfer logic could result in loss of funds. The pool has not been formally audited.</p>
    <p><strong>Admin key risk:</strong> The pool admin can change swap fees, admin fees, and the amplification coefficient. Malicious or careless parameter changes could disadvantage liquidity providers. The admin can also pause the pool, preventing swaps and withdrawals.</p>
    <p><strong>Impermanent loss:</strong> For stablecoin pools, impermanent loss is minimal under normal conditions because all tokens trade near the same price. However, during a depeg event, impermanent loss can become significant and may exceed fee income.</p>
    <p>This pool is part of a beta protocol. See the <a href="/docs/beta" class="doc-link">beta disclaimer</a> for full details.</p>
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
  .param-val.live { color: var(--rumi-action); }
  .live { color: var(--rumi-action); font-weight: 600; }
  .live-indicator { color: var(--rumi-action); font-weight: 600; }
  .doc-list {
    padding-left: 1.25rem;
    display: flex; flex-direction: column; gap: 0.35rem;
    margin: 0.5rem 0;
  }
  .doc-list li {
    font-size: 0.875rem; color: var(--rumi-text-secondary); line-height: 1.5;
  }
  .doc-note {
    font-size: 0.8125rem; color: var(--rumi-text-muted);
    margin-top: 0.75rem; font-style: italic;
  }
</style>
