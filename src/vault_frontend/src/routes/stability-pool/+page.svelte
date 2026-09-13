<script lang="ts">
  import EarnSubNav from '../../lib/components/layout/EarnSubNav.svelte';
  import StabilityPoolTab from '../../lib/components/liquidations/StabilityPoolTab.svelte';
</script>

<svelte:head>
  <title>Stability Pool | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <EarnSubNav active="stability-pool" />

  <p class="pool-description">
    Supply stablecoins that stand ready to repay undercollateralized vault debt. When a
    liquidation happens, your deposit converts into that vault's collateral, at a discount
    to its market value. icUSD deposits also earn a share of ongoing borrowing interest,
    excluding any liquidation gains; ckUSDC, ckUSDT, and 3USD deposits take part in
    liquidations but don't earn that interest.
  </p>

  <StabilityPoolTab showApyBadge={false} />

  <p class="manual-liquidate-note">
    Prefer to liquidate a specific vault yourself instead of depositing?
    <a href="/liquidations?tab=manual" class="manual-liquidate-link">Go to Manual Liquidate</a>.
  </p>
</div>

<style>
  .page-container { max-width: 820px; margin: 0 auto; padding-bottom: 4rem; }

  @keyframes fadeSlideIn {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
  }

  .pool-description {
    max-width: 68ch;
    margin: 0 0 1.5rem;
    font-size: 0.8125rem;
    line-height: 1.6;
    color: var(--rumi-text-secondary);
    animation: fadeSlideIn 0.5s ease-out both;
  }

  .manual-liquidate-note {
    margin: 1.5rem 0 0;
    text-align: center;
    font-size: 0.75rem;
    line-height: 1.6;
    color: var(--rumi-text-muted);
  }

  .manual-liquidate-link {
    color: var(--rumi-teal);
    text-decoration: none;
  }

  .manual-liquidate-link:hover { text-decoration: underline; }

  @media (max-width: 520px) {
    .page-container { padding-left: 0.5rem; padding-right: 0.5rem; }
  }
</style>
