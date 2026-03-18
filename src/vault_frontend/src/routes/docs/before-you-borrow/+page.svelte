<script lang="ts">
  import { onMount } from 'svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { collateralStore } from '$lib/stores/collateralStore';
  import type { CollateralInfo, InterestSplitEntryDTO } from '$lib/services/types';
  import { get } from 'svelte/store';

  import { protocolService } from '$lib/services/protocol';

  let ckstableFeePct = '0.05';
  let collaterals: CollateralInfo[] = [];
  let borrowingFeeCurve: [number, number][] = [];
  let interestSplit: InterestSplitEntryDTO[] = [];

  function splitPct(dest: string): string {
    const entry = interestSplit.find(s => s.destination === dest);
    return entry ? (Number(entry.bps) / 100).toFixed(0) + '%' : '—';
  }
  $: collateralSymbols = collaterals.map(c => c.symbol).join(', ') || 'ICP';

  onMount(async () => {
    try {
      const [ckFee, status] = await Promise.all([
        publicActor.get_ckstable_repay_fee() as Promise<number>,
        protocolService.getProtocolStatus(),
      ]);
      ckstableFeePct = (Number(ckFee) * 100).toFixed(2);
      interestSplit = status.interestSplit ?? [];
      borrowingFeeCurve = status.borrowingFeeCurveResolved ?? [];

      await collateralStore.fetchSupportedCollateral();
      const state = get(collateralStore);
      collaterals = state.collaterals;
    } catch (e) {
      console.error('Failed to fetch fees:', e);
    }
  });
</script>

<svelte:head><title>Before You Borrow | Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Before You Borrow</h1>

  <section class="doc-section">
    <h2 class="doc-heading">How Borrowing Works</h2>
    <p>You deposit collateral into a vault, then borrow icUSD against it. The icUSD is minted at the time of borrowing and does not come from a pool. Your collateral stays locked in the vault until you repay the debt and withdraw it.</p>
    <p>Each vault is independent. You can have multiple vaults, each with its own collateral and debt. The protocol currently supports <strong>{collateralSymbols}</strong> as collateral{collaterals.length > 1 ? ', each with its own parameters' : ''}.</p>
  </section>

  {#if collaterals.length > 0}
  <section class="doc-section">
    <h2 class="doc-heading">Per-Collateral Requirements</h2>
    <div class="table-wrap">
      <table class="doc-table">
        <thead>
          <tr>
            <th>Collateral</th>
            <th>Liquidation Ratio</th>
            <th>Borrowing Threshold</th>
            <th>Min Borrow</th>
            <th>Borrowing Fee</th>
          </tr>
        </thead>
        <tbody>
          {#each collaterals as c (c.principal)}
            <tr>
              <td class="col-symbol">{c.symbol}</td>
              <td class="live">{(c.liquidationCr * 100).toFixed(0)}%</td>
              <td class="live">{(c.minimumCr * 100).toFixed(0)}%</td>
              <td class="live">{c.minVaultDebt > 0 ? `${c.minVaultDebt / 1e8} icUSD` : '—'}</td>
              <td class="live">{(c.borrowingFee * 100).toFixed(1)}%</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </section>
  {/if}

  <section class="doc-section">
    <h2 class="doc-heading">Borrowing Fee</h2>
    <p>A one-time borrowing fee is deducted from the icUSD you mint. Each collateral type has its own base rate (shown in the table above), and the effective fee depends on how your vault's projected collateral ratio compares to the system average.</p>
    <p>Vaults borrowing at or above the system collateral ratio pay the base rate. Vaults borrowing closer to the minimum collateral ratio pay a higher rate via a dynamic multiplier. This discourages risky borrows that could weaken overall protocol health.</p>
    <p>The fee is deducted upfront: if you borrow 100 icUSD at a 0.5% effective rate, you receive 99.5 icUSD and owe 100 icUSD.</p>
    <p>See <a href="/docs/parameters" class="doc-link">Protocol Parameters</a> for the full fee curve and multiplier details.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Interest</h2>
    <p>Your vault accrues interest continuously on outstanding debt. The base annual rate (APR) is shown per collateral type in <a href="/docs/parameters" class="doc-link">Protocol Parameters</a>. Two factors can increase the effective rate:</p>
    <ul class="doc-list">
      <li><strong>Vault CR multiplier:</strong> vaults closer to the liquidation threshold pay a higher rate than well-collateralized vaults.</li>
      <li><strong>Recovery mode multiplier:</strong> if the system enters Recovery mode, a system-wide multiplier increases rates for all vaults.</li>
    </ul>
    <p>Interest is applied to your debt before every vault mutation (borrow, repay, withdraw, liquidation) and ticked forward every 5 minutes by a background timer. This means your debt grows over time. A vault sitting just above the liquidation threshold can drift into liquidation purely from accrued interest, even without any price movement.</p>
    <p>Interest revenue is split three ways: <span class="live">{splitPct('three_pool')}</span> is donated to the <a href="/docs/three-pool" class="doc-link">3pool</a> (boosting LP token value), <span class="live">{splitPct('stability_pool')}</span> is distributed to stability pool depositors as icUSD, and <span class="live">{splitPct('treasury')}</span> goes to the protocol treasury.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Repaying Your Debt</h2>
    <p>You can repay your icUSD debt at any time, in full or partially. You can also repay using <strong>ckUSDT</strong> or <strong>ckUSDC</strong> instead of icUSD. Stablecoin repayments are treated at a 1:1 rate with icUSD, minus a {ckstableFeePct}% conversion fee. The conversion fee is sent to the protocol treasury. The protocol checks the stablecoin's live price and rejects repayment if it has depegged outside the $0.95–$1.05 range.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Managing Collateral</h2>
    <p>You can add more collateral to your vault at any time to improve your collateral ratio. You can also <strong>withdraw collateral partially</strong>, taking some out while keeping the vault open, as long as your CR stays above the borrowing threshold.</p>
    <p>The maximum you can withdraw is: <code>current collateral - (debt &times; min ratio &divide; collateral price)</code>. The protocol calculates this for you and rejects withdrawals that would put your vault at risk.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Closing Your Vault</h2>
    <p>To close a vault, you must first repay all outstanding icUSD debt, then withdraw your collateral. The protocol also offers a <strong>withdraw-and-close</strong> operation that does both steps atomically in a single call. Dust amounts below 0.0005 icUSD are forgiven automatically on close.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">What You Should Understand</h2>
    <p>Collateral assets are volatile. A sharp price drop can push your vault below the liquidation threshold faster than you can react. There is no grace period and no notification system. Liquidation is immediate and automated.</p>
    <p>Higher collateral ratios give you more buffer. A vault at 200% can absorb a much larger price drop than one at 140%.</p>
    <p>Your vault's collateral can also be affected by redemptions. When icUSD holders redeem and protocol reserves are insufficient, collateral is taken from the lowest-CR vaults. See <a href="/docs/redemptions" class="doc-link">Redemptions</a> for details.</p>
    <p>The protocol allows only one operation per user at a time. If you submit a second transaction before the first completes, it will be rejected. Wait for confirmations before taking another action.</p>
    <p>This protocol is in beta. See the <a href="/docs/beta" class="doc-link">beta disclaimer</a> and <a href="/docs/risks" class="doc-link">risk documentation</a> for full details.</p>
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
    font-variant-numeric: tabular-nums;
  }
  .doc-table tbody tr:last-child td { border-bottom: none; }
  .col-symbol { font-weight: 600; color: var(--rumi-text-primary); }
  .live { color: var(--rumi-action); font-weight: 600; }

  .doc-list {
    padding-left: 1.25rem;
    display: flex; flex-direction: column; gap: 0.35rem;
    margin: 0.5rem 0;
  }
  .doc-list li {
    font-size: 0.875rem; color: var(--rumi-text-secondary); line-height: 1.5;
  }
</style>
