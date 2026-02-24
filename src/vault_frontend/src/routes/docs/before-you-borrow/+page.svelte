<script lang="ts">
  import { onMount } from 'svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { collateralStore } from '$lib/stores/collateralStore';
  import { get } from 'svelte/store';

  let borrowingFeePct = '0.5';
  let ckstableFeePct = '0.05';
  let liqPct = '133';
  let minBorrow = '0.1';

  onMount(async () => {
    try {
      const [bFee, ckFee] = await Promise.all([
        publicActor.get_borrowing_fee() as Promise<number>,
        publicActor.get_ckstable_repay_fee() as Promise<number>,
      ]);
      borrowingFeePct = (Number(bFee) * 100).toFixed(1);
      ckstableFeePct = (Number(ckFee) * 100).toFixed(2);

      await collateralStore.fetchSupportedCollateral();
      const state = get(collateralStore);
      const icpConfig = state.collaterals.find(c => c.symbol === 'ICP');
      if (icpConfig) {
        liqPct = (icpConfig.liquidationCr * 100).toFixed(0);
        const minDebt = icpConfig.minVaultDebt / 1e8;
        if (minDebt > 0) minBorrow = minDebt.toString();
      }
    } catch (e) {
      console.error('Failed to fetch fees:', e);
    }
  });
</script>

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
    <p>The minimum collateral deposit is 0.001 ICP. The minimum borrow amount is {minBorrow} icUSD. Your vault must maintain a collateral ratio of at least {liqPct}% at all times. If ICP's price drops and your ratio falls below {liqPct}%, your vault becomes eligible for liquidation.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Fees</h2>
    <p>A one-time borrowing fee of {borrowingFeePct}% is deducted from the icUSD you borrow. If you borrow 100 icUSD, you receive {(100 - parseFloat(borrowingFeePct)).toFixed(1)} icUSD and owe 100 icUSD. There is no ongoing interest. The fee drops to 0% if the protocol enters Recovery mode.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Repaying Your Debt</h2>
    <p>You can repay your icUSD debt at any time — in full or partially. You can also repay using <strong>ckUSDT</strong> or <strong>ckUSDC</strong> instead of icUSD. Stablecoin repayments are treated at a 1:1 rate with icUSD, minus a {ckstableFeePct}% conversion fee. The protocol checks the stablecoin's live price and rejects repayment if it has depegged outside the $0.95–$1.05 range.</p>
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
