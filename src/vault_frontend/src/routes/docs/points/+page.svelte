<svelte:head><title>Points &amp; Airdrop | Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Points &amp; Airdrop (Season 1)</h1>

  <section class="doc-section">
    <h2 class="doc-heading">What Points Are</h2>
    <p>Season 1 points track how much value you put to work in the protocol, and for how long. Points are measured in <strong>USD-days</strong>: one dollar of qualifying activity held for one day earns one base point, scaled by the activity's multiplier below. Points determine relative allocations for the Season 1 airdrop. Claiming opens after the season ends; allocation and claiming details will be announced separately.</p>
    <p>Points are tracked by a dedicated on-chain canister and are visible on the <a href="/points" class="doc-link">Airdrop</a> page, with a public <a href="/points/leaderboard" class="doc-link">leaderboard</a>. Points themselves are not a token, have no cash value, and cannot be transferred.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Season 1 Window</h2>
    <p>Season 1 runs from <strong>June 1, 2026</strong> to <strong>August 31, 2026</strong> (UTC). You are enrolled automatically the first time you take a qualifying action: minting icUSD from a vault, repaying a vault, depositing into the 3pool, depositing into the stability pool, or providing AMM liquidity. Activity before your enrollment does not earn points retroactively, and no further points accrue after the season closes.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">What Earns Points</h2>
    <div class="table-wrap">
      <table class="doc-table">
        <thead>
          <tr><th>Activity</th><th>Multiplier</th></tr>
        </thead>
        <tbody>
          <tr><td>icUSD debt outstanding in a vault</td><td class="mult">1x</td></tr>
          <tr><td>icUSD deposited in the 3pool</td><td class="mult">1x</td></tr>
          <tr><td>ckUSDC or ckUSDT deposited in the 3pool (unmatched)</td><td class="mult">3x</td></tr>
          <tr><td>ckUSDC and ckUSDT deposited together (matched pair)</td><td class="mult">5x</td></tr>
          <tr><td>icUSD in the stability pool</td><td class="mult">1x</td></tr>
          <tr><td>3USD in the stability pool</td><td class="mult">2x</td></tr>
          <tr><td>3USD/ICP liquidity in the Rumi AMM</td><td class="mult">2x</td></tr>
        </tbody>
      </table>
    </div>
    <p>Multipliers stack <strong>across activities</strong>: borrowing against a vault, depositing in the 3pool, and depositing in the stability pool all earn independently at the same time. They do not stack within a single activity.</p>
    <p>For the matched-pair rate, the matched portion is twice the smaller of your ckUSDC and ckUSDT deposits at 5x; whatever is left over on the larger side earns the unmatched 3x rate. Adding a token-sized amount of one coin does not flip your whole position to 5x.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">The 3USD Holding Rule</h2>
    <p>3pool deposit points stay active only while you still hold the 3USD that the deposit minted. The points canister verifies your 3USD across your wallet balance, your stability pool deposit, and your share of AMM liquidity. If you sell or transfer the 3USD away, the corresponding deposit stops earning; the buyer does not inherit it. This prevents deposit-and-dump farming while leaving you free to move 3USD between Rumi venues.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">How Accrual Works</h2>
    <p>The season is divided into <strong>weekly epochs</strong>. During each epoch, the points canister captures your balances at <strong>two randomized snapshot times</strong> at least 48 hours apart. The snapshot times are derived from a committed seed that is only revealed after the epoch closes, so they cannot be predicted in advance. Of the two snapshots, the one with the <strong>lower total</strong> is the one that counts.</p>
    <p>The practical effect: parking funds for a few hours around an expected snapshot earns nothing. Capital that stays deployed through the week earns full credit.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Eligibility</h2>
    <p>Protocol-owned canisters (the backend, pools, treasury, bot, and ledgers) are excluded from earning points, and the excluded list is publicly queryable on the points canister. There are no other eligibility gates: any principal that takes a qualifying action is enrolled. Splitting funds across multiple wallets confers no advantage, since points are proportional to dollar-days regardless of how they are divided.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Leaderboard &amp; Transparency</h2>
    <p>The <a href="/points/leaderboard" class="doc-link">leaderboard</a> shows ranked principals and their points. Your own dashboard on the <a href="/points" class="doc-link">Airdrop</a> page shows your total, rank, enrollment date, and which venues you are currently earning from. All accrual state lives in the points canister and can be queried directly by anyone.</p>
  </section>
</article>

<style>
  .table-wrap {
    overflow-x: auto;
    margin: 0.5rem 0;
  }
  .doc-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.875rem;
  }
  .doc-table th {
    text-align: left;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--rumi-text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0.5rem 0.75rem;
    border-bottom: 2px solid var(--rumi-border);
    white-space: nowrap;
  }
  .doc-table td {
    padding: 0.5rem 0.75rem;
    border-bottom: 1px solid var(--rumi-border);
    color: var(--rumi-text-secondary);
  }
  .doc-table tbody tr:last-child td { border-bottom: none; }
  .mult { color: var(--rumi-action); font-weight: 600; font-variant-numeric: tabular-nums; }
</style>
