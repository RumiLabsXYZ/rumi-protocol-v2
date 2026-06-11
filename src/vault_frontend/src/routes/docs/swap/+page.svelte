<script lang="ts">
  import { onMount } from 'svelte';
  import { protocolService } from '$lib/services/protocol';
  import type { InterestSplitEntryDTO } from '$lib/services/types';

  let interestSplit: InterestSplitEntryDTO[] = [];

  function splitPct(dest: string): string {
    const entry = interestSplit.find(s => s.destination === dest);
    return entry ? (Number(entry.bps) / 100).toFixed(0) + '%' : '—';
  }

  onMount(async () => {
    try {
      const status = await protocolService.getProtocolStatus();
      interestSplit = status.interestSplit ?? [];
    } catch (e) {
      console.error('Failed to fetch interest split:', e);
    }
  });
</script>

<svelte:head><title>Swap &amp; AMM | Rumi Docs</title></svelte:head>

<article class="doc-page">
  <h1 class="doc-title">Swap &amp; AMM</h1>

  <section class="doc-section">
    <h2 class="doc-heading">What the Swap Page Does</h2>
    <p>The <a href="/swap" class="doc-link">Swap</a> page is a router over Rumi's own liquidity. Depending on the pair you pick, it routes through the <a href="/docs/three-pool" class="doc-link">3pool</a> (stablecoins), Rumi's pair AMM (3USD/ICP), or both in sequence. For the 3USD/ICP and icUSD/ICP legs it also quotes the corresponding ICPSwap pool and uses whichever venue pays out more.</p>
    <ul class="doc-list">
      <li><strong>Stablecoin ↔ stablecoin</strong> (icUSD, ckUSDT, ckUSDC): single swap through the 3pool.</li>
      <li><strong>Stablecoin → 3USD</strong>: a one-sided 3pool deposit (mints 3USD).</li>
      <li><strong>3USD → stablecoin</strong>: a single-token 3pool withdrawal (burns 3USD).</li>
      <li><strong>3USD ↔ ICP</strong>: Rumi's AMM or ICPSwap's 3USD/ICP pool, whichever quotes better.</li>
      <li><strong>ICP ↔ stablecoin</strong>: two hops through 3USD (AMM leg plus 3pool leg).</li>
      <li><strong>icUSD ↔ ICP</strong>: routed directly through ICPSwap's icUSD/ICP pool when that beats the two-hop route.</li>
    </ul>
    <p>Quotes shown in the UI are net amounts: every outbound transfer pays the output token's ledger fee, and your minimum-output (slippage) bound is enforced against the net amount you actually receive. The default slippage tolerance is 0.5% and you can adjust it per trade.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">The Rumi AMM (3USD/ICP)</h2>
    <p>Alongside the stablecoin 3pool, Rumi runs a constant-product (x &times; y = k) AMM with a <strong>3USD/ICP</strong> pool. It is the venue that lets you move between the yield-bearing stable side of the ecosystem (3USD) and ICP in one step. Each swap pays a pool fee, a portion of which is retained by the protocol; the rest accrues to liquidity providers.</p>
    <p>You can provide liquidity from the <a href="/swap" class="doc-link">Swap</a> page's liquidity panel. Deposits mint LP shares representing your fraction of the pool, and you can remove liquidity at any time with no lock-up.</p>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">LP Earnings</h2>
    <p>AMM liquidity providers earn from three sources:</p>
    <ul class="doc-list">
      <li><strong>Swap fees</strong>, which accrue to the pool with every trade.</li>
      <li><strong>Protocol interest revenue:</strong> <span class="live">{splitPct('amm1')}</span> of all vault interest is sent to the AMM as icUSD rewards, accrued per LP share. You collect these with the <strong>Claim</strong> button on the swap page.</li>
      <li><strong>3USD pass-through:</strong> roughly half the pool sits in 3USD, which keeps earning the 3pool's yield via virtual price growth. The "effective APY" shown in the app is the AMM's own yield plus about half the 3pool APY for this reason.</li>
    </ul>
  </section>

  <section class="doc-section">
    <h2 class="doc-heading">Risks</h2>
    <p><strong>Impermanent loss:</strong> unlike the stablecoin 3pool, the 3USD/ICP pair holds a volatile asset. If the ICP price moves significantly after you deposit, the value of your LP position can underperform simply holding the two assets. Fee and reward income may not cover the difference.</p>
    <p><strong>Smart contract risk:</strong> the AMM is its own canister with its own code. It is covered by the protocol's recurring security reviews (reports at <a href="https://rumiprotocol.com/security" class="doc-link" target="_blank" rel="noopener">rumiprotocol.com/security</a>), but it has not been audited by a traditional human-led security firm. See the <a href="/docs/beta" class="doc-link">beta disclaimer</a>.</p>
    <p><strong>External venue risk:</strong> routes that touch ICPSwap depend on ICPSwap's own contracts and liquidity, which Rumi does not control.</p>
  </section>
</article>

<style>
  .doc-list {
    padding-left: 1.25rem;
    display: flex; flex-direction: column; gap: 0.35rem;
    margin: 0.5rem 0;
  }
  .doc-list li {
    font-size: 0.875rem; color: var(--rumi-text-secondary); line-height: 1.5;
  }
  .live { color: var(--rumi-action); font-weight: 600; }
</style>
