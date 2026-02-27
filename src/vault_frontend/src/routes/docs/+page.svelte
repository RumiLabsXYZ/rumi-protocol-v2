<script lang="ts">
  const sections = [
    {
      title: "Before You Borrow",
      href: "/docs/before-you-borrow",
      description: "What you need to know before depositing collateral and minting icUSD."
    },
    {
      title: "Liquidation Mechanics",
      href: "/docs/liquidation",
      description: "How liquidations work, when they trigger, and what happens to your collateral."
    },
    {
      title: "What Can Go Wrong",
      href: "/docs/risks",
      description: "Protocol risks, oracle failure modes, and edge cases you should understand."
    },
    {
      title: "Redemptions",
      href: "/docs/redemptions",
      description: "How icUSD redemptions work: reserve-backed ckStable and vault spillover."
    },
    {
      title: "Protocol Parameters",
      href: "/docs/parameters",
      description: "Current constants, fees, ratios, and operational thresholds."
    },
    {
      title: "Transparency",
      href: "/transparency",
      description: "On-chain log of every admin action, including emergency icUSD mints."
    },
    {
      title: "Beta Disclaimer",
      href: "/docs/beta",
      description: "This protocol is in beta. Read the full disclosure."
    }
  ];
</script>

<svelte:head>
  <title>Docs - Rumi Protocol</title>
</svelte:head>

<div class="page-container docs-container">
  <h1 class="page-title docs-title">Documentation</h1>
  <p class="docs-subtitle">
    Rumi Protocol is a CDP system that lets you borrow icUSD stablecoins against supported collateral on the Internet Computer.
    These docs cover what's currently implemented — not future plans. Connect with <strong>Oisy</strong>, <strong>Plug</strong>, <strong>Internet Identity</strong>, or <strong>NFID</strong> to get started.
  </p>

  <nav class="docs-nav">
    {#each sections as section}
      <a href={section.href} class="docs-link">
        <h2 class="docs-link-title">{section.title}</h2>
        <p class="docs-link-desc">{section.description}</p>
        <span class="docs-arrow">→</span>
      </a>
    {/each}
  </nav>
</div>

<style>
  .docs-container { max-width: 700px; margin: 0 auto; }
  .docs-title {
    /* Inherits purple from .page-title in app.css — no override */
  }
  .docs-subtitle {
    font-size: 0.9375rem; color: var(--rumi-text-secondary);
    line-height: 1.6; margin-bottom: 2.5rem;
  }

  .docs-nav { display: flex; flex-direction: column; gap: 0.75rem; }
  .docs-link {
    display: block; position: relative;
    padding: 1.25rem 3rem 1.25rem 1.25rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    text-decoration: none;
    transition: border-color 0.15s ease, box-shadow 0.15s ease;
  }
  .docs-link:hover {
    border-color: rgba(209, 118, 232, 0.15);
    box-shadow: inset 0 0 20px 0 rgba(209, 118, 232, 0.06);
  }
  .docs-link-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1rem; font-weight: 600;
    color: var(--rumi-text-primary);
    margin-bottom: 0.25rem;
  }
  .docs-link-desc {
    font-size: 0.8125rem; color: var(--rumi-text-secondary); line-height: 1.4;
  }
  .docs-arrow {
    position: absolute; right: 1.25rem; top: 50%; transform: translateY(-50%);
    font-size: 1rem; color: var(--rumi-text-muted);
    transition: color 0.15s ease, transform 0.15s ease;
  }
  .docs-link:hover .docs-arrow {
    color: var(--rumi-action); transform: translateY(-50%) translateX(2px);
  }
</style>
