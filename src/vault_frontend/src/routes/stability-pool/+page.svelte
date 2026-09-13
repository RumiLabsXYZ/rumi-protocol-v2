<script lang="ts">
  import EarnSubNav from '../../lib/components/layout/EarnSubNav.svelte';
  import StabilityPoolTab from '../../lib/components/liquidations/StabilityPoolTab.svelte';
</script>

<svelte:head>
  <title>Stability Pool | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <EarnSubNav active="stability-pool" />

  <div class="page-header">
    <h1 class="page-title">Stability Pool</h1>
  </div>

  <StabilityPoolTab />

  <p class="manual-liquidate-note">
    Prefer to liquidate a specific vault yourself instead of depositing?
    <a href="/liquidations?tab=manual" class="manual-liquidate-link">Go to Manual Liquidate</a>.
  </p>
</div>

<style>
  .page-container { max-width: 820px; margin: 0 auto; padding-bottom: 4rem; }

  .page-header {
    margin-bottom: 1rem;
    animation: fadeSlideIn 0.5s ease-out both;
  }

  @keyframes fadeSlideIn {
    from { opacity: 0; transform: translateY(12px); }
    to { opacity: 1; transform: translateY(0); }
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
