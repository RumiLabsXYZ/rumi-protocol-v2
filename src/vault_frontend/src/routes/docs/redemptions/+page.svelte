<script lang="ts">
  import { onMount } from 'svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';

  let reserveRedemptionFee = 0;
  let redemptionFeeFloor = 0;
  let redemptionFeeCeiling = 0;
  let reserveRedemptionsEnabled = false;
  let loaded = false;

  function pctRaw(val: number, decimals = 1): string {
    if (val === undefined || val === null) return '—';
    return (val * 100).toFixed(decimals) + '%';
  }

  onMount(async () => {
    try {
      const [rrFee, rfFloor, rfCeil, rrEnabled] = await Promise.all([
        publicActor.get_reserve_redemption_fee() as Promise<number>,
        publicActor.get_redemption_fee_floor() as Promise<number>,
        publicActor.get_redemption_fee_ceiling() as Promise<number>,
        publicActor.get_reserve_redemptions_enabled() as Promise<boolean>,
      ]);
      reserveRedemptionFee = Number(rrFee);
      redemptionFeeFloor = Number(rfFloor);
      redemptionFeeCeiling = Number(rfCeil);
      reserveRedemptionsEnabled = rrEnabled;
    } catch (e) {
      console.error('Failed to fetch redemption params:', e);
    }
    loaded = true;
  });
</script>

<svelte:head><title>Redemptions - Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Redemptions</h1>

  <section class="doc-section">
    <h2 class="doc-heading">What Is Redemption</h2>
    <p>Redemption lets any icUSD holder exchange their icUSD for value at face value ($1 per icUSD), minus a fee. This creates a price floor for icUSD — if icUSD trades below $1 on the open market, arbitrageurs can buy it cheaply and redeem it for $1 worth of assets, driving the price back up.</p>
    <p>Redemption is a core peg-maintenance mechanism. It protects icUSD holders by ensuring their tokens are always backed by redeemable value.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Reserve Redemptions (Tier 1)</h2>
    <p>The protocol holds reserves of <strong>ckUSDT</strong> and <strong>ckUSDC</strong> — real stablecoins that accumulate when users repay vault debt with ckStables. When you redeem icUSD, the protocol first tries to fill your redemption from these reserves.</p>
    {#if loaded}
      <div class="fee-box">
        <span class="fee-label">Reserve Redemption Fee</span>
        <span class="fee-value" class:live={reserveRedemptionFee > 0}>{pctRaw(reserveRedemptionFee)}</span>
      </div>
      <div class="fee-box">
        <span class="fee-label">Reserve Redemptions</span>
        <span class="fee-value" class:enabled={reserveRedemptionsEnabled} class:disabled={!reserveRedemptionsEnabled}>{reserveRedemptionsEnabled ? 'Enabled' : 'Disabled'}</span>
      </div>
    {/if}
    <p>Reserve redemptions are the cleanest outcome: you burn icUSD and receive ckStables in return. No vaults are affected.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Vault Redemptions (Tier 2 — Spillover)</h2>
    <p>If the protocol's reserves don't have enough ckStables to fill the full redemption amount, the remainder "spills over" into vault redemptions. The protocol identifies the vaults with the <strong>lowest collateral ratios</strong> and redeems against them — reducing their debt but also taking an equivalent value of their ICP collateral.</p>
    {#if loaded}
      <div class="fee-box">
        <span class="fee-label">Vault Redemption Fee Floor</span>
        <span class="fee-value live">{pctRaw(redemptionFeeFloor)}</span>
      </div>
      <div class="fee-box">
        <span class="fee-label">Vault Redemption Fee Ceiling</span>
        <span class="fee-value live">{pctRaw(redemptionFeeCeiling)}</span>
      </div>
    {/if}
    <p>The vault redemption fee is dynamic. It is calculated using a base rate that increases with each redemption and decays over time:</p>
    <p class="doc-formula">fee = base_rate &times; 0.94<sup>hours_since_last_redemption</sup> + (redeemed / total_borrowed) &times; 0.5</p>
    <p>The base rate starts at zero and increases with each redemption. The 0.94 decay factor means the rate halves roughly every 11 hours of inactivity. The result is clamped between the floor and ceiling shown above. After each redemption, the base rate is updated to the newly computed fee.</p>
    <p>You receive ICP (not ckStables) from vault redemptions. The ICP is sent directly to your account.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Token Preference</h2>
    <p>When redeeming through reserves, you can choose whether to receive <strong>ckUSDT</strong> or <strong>ckUSDC</strong>. If the protocol doesn't have enough of your preferred token, it falls back to the other. If neither reserve is sufficient, the remainder spills over to vault redemptions.</p>
    <p>By default, the protocol fills ckUSDT first. You can override this by specifying your preferred token ledger when calling the redemption function.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Impact on Vault Owners</h2>
    <p><strong>Reserve tier (Tier 1):</strong> Zero impact on vaults. The protocol draws from its own stablecoin reserves. Your vault is completely unaffected.</p>
    <p><strong>Vault tier (Tier 2):</strong> Your vault's debt is reduced, but collateral is also taken proportionally. The protocol uses a <strong>water-filling algorithm</strong> — it doesn't simply drain the single lowest-CR vault. Instead, it identifies the band of vaults with the lowest CRs and distributes the redemption proportionally by debt across the band, raising them all equally. If the redemption amount would raise the entire band above the next tier, the band merges upward and the process repeats. This means redemptions affect multiple low-CR vaults simultaneously rather than wiping out one vault at a time.</p>
    <p>Vault redemption is not liquidation. Your vault remains open and you retain any remaining collateral and debt. The collateral taken is always at face value — there is no penalty or bonus applied. Keeping your vault well-collateralized reduces the chance of being redeemed against.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">How Redemption Flows</h2>
    <ol class="flow-list">
      <li>You submit icUSD for redemption.</li>
      <li>The icUSD is burned (removed from circulation).</li>
      <li><strong>Tier 1:</strong> The protocol checks its ckStable reserves and sends you stablecoins up to the available balance.</li>
      <li><strong>Tier 2:</strong> Any remaining amount after reserves is filled by taking ICP from the lowest-CR vaults.</li>
      <li>Fees are deducted from the amount you receive — flat reserve fee for Tier 1, dynamic fee for Tier 2.</li>
      <li>Reserve fees are sent to the protocol treasury. Vault redemption fees are deducted from the ICP collateral released.</li>
    </ol>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Safety: Failed Transfer Handling</h2>
    <p>On the Internet Computer, cross-canister calls are not atomic — a transfer can fail after your icUSD has already been burned. If a reserve redemption's ckStable transfer fails, the protocol automatically <strong>refunds your icUSD</strong> by minting it back to your account. You will see an error message, but your funds are safe.</p>
    <p>If both the ckStable transfer and the refund fail (an extremely unlikely scenario), the incident is logged for manual intervention by the protocol admin using the <a href="/transparency" class="doc-link">admin mint</a> function.</p>
  </section>
</article>

<style>
  .fee-box {
    display: flex; justify-content: space-between; align-items: baseline;
    padding: 0.5rem 0.75rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    margin: 0.5rem 0;
  }
  .fee-label { font-size: 0.8125rem; color: var(--rumi-text-secondary); }
  .fee-value {
    font-family: 'Inter', sans-serif; font-size: 0.875rem;
    font-weight: 600; color: var(--rumi-text-primary);
    font-variant-numeric: tabular-nums;
  }
  .fee-value.live { color: var(--rumi-action); }
  .fee-value.enabled { color: var(--rumi-safe); }
  .fee-value.disabled { color: var(--rumi-danger); }

  .doc-formula {
    font-family: 'Inter', monospace;
    font-size: 0.8125rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    padding: 0.75rem 1rem;
    text-align: center;
    color: var(--rumi-text-primary);
    font-weight: 500;
    overflow-x: auto;
  }

  .flow-list {
    padding-left: 1.25rem;
    display: flex; flex-direction: column; gap: 0.5rem;
  }
  .flow-list li {
    font-size: 0.875rem; color: var(--rumi-text-secondary); line-height: 1.5;
  }
</style>
