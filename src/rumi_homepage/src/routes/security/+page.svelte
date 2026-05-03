<script>
  const audits = [
    {
      title: 'Combined Security Review',
      tag: 'Latest · Recommended',
      date: 'May 2, 2026',
      authors: 'Internal + AVAI',
      scope: 'Unified close-out of every finding from the internal three-pass review and the AVAI external pre-audit, including Wave 14 remediations.',
      summary: 'Eight HIGH, thirty-six MEDIUM, twenty-five LOW findings across both reviews. All resolved on mainnet, deferred-by-design, or accepted with documented watch thresholds. Anchored to backend hash 0xc6b99934.',
      slug: 'rumi-combined-security-review-2026-05-02',
    },
    {
      title: 'Internal Three-Pass Review',
      tag: 'Internal',
      date: 'April 22, 2026',
      authors: 'Rumi Labs',
      scope: 'Eleven specialist analysis passes covering ~63k lines of Rust across 9 canisters and ~30k lines of TypeScript/Svelte across both frontends. Anchored to commit 28e9896.',
      summary: '73 findings shipped across 13 numbered remediation waves over nine days, with three independent verification passes after fixes landed.',
      slug: 'rumi-internal-review-2026-04-22',
    },
    {
      title: 'AVAI External Pre-Audit',
      tag: 'External',
      date: 'April 24, 2026',
      authors: 'AVAI',
      scope: 'Automated pre-audit sweep against the Breitner IC Canister Security Guidelines and a CDP protocol domain checklist. Anchored to commit e749620d.',
      summary: '20 numbered findings (4 IC-hygiene, 16 CDP-domain). Twelve overlapped with internal-review fixes already shipped; eight net-new findings closed in Wave 14a/b/c.',
      slug: 'rumi-avai-external-audit-2026-04-24',
    },
  ];
</script>

<svelte:head>
  <title>Security · Rumi Protocol</title>
  <meta name="description" content="Audit reports, security philosophy, and responsible disclosure for Rumi Protocol." />
</svelte:head>

<section class="max-w-4xl mx-auto px-6 py-20 md:py-28">
  <p class="eyebrow">Security</p>
  <h1 class="page-title">Audits and disclosures</h1>
  <p class="page-lead">
    Rumi Protocol is a CDP stablecoin running entirely on the Internet Computer. Security is
    treated as continuous work, not a one-time milestone. The reports below cover every
    canister in the protocol and document the remediation status of every finding raised.
  </p>
</section>

<section class="max-w-4xl mx-auto px-6 pb-12">
  <h2 class="section-h2">Audit reports</h2>
  <p class="section-sub">
    Each report is provided as a PDF for human reading and as Markdown for AI agents,
    indexers, and grep. Both formats contain the same findings.
  </p>

  <div class="audit-stack">
    {#each audits as audit}
      <article class="audit-card">
        <div class="audit-head">
          <h3 class="audit-title">{audit.title}</h3>
          <span class="audit-tag">{audit.tag}</span>
        </div>
        <div class="audit-meta">
          <span>{audit.date}</span>
          <span class="meta-sep">·</span>
          <span>{audit.authors}</span>
        </div>
        <p class="audit-scope">{audit.scope}</p>
        <p class="audit-summary">{audit.summary}</p>
        <div class="audit-actions">
          <a href="/audits/{audit.slug}.pdf" target="_blank" rel="noopener" class="btn-primary">
            Read PDF
          </a>
          <a href="/audits/{audit.slug}.md" target="_blank" rel="noopener" class="btn-secondary">
            Markdown <span class="btn-hint">(for agents)</span>
          </a>
        </div>
      </article>
    {/each}
  </div>
</section>

<section class="max-w-4xl mx-auto px-6 py-16 border-t" style="border-color: var(--rumi-border);">
  <h2 class="section-h2">Scope and methodology</h2>
  <div class="prose-block">
    <p>
      The reviews cover the full v2 stack: the core CDP backend, stability pool, 3pool AMM,
      treasury, liquidation bot, icUSD ledger, and both SvelteKit frontends. Threat models
      examined include async-state races at every <code>await</code> boundary, oracle
      integrity and staleness, ICRC transfer hygiene and double-spend windows, stable-memory
      upgrade safety, stability-pool accounting invariants, redemption peg defense, caller
      authorization across canister boundaries, debt and interest accounting, liquidation
      mechanics, inter-canister call failure modes, and cycle / DoS exposure.
    </p>
    <p>
      Findings were tracked as structured records with severity, mechanism, exploit scenario,
      and recommended remediation. Every finding was either resolved on mainnet with a
      regression test, deferred-by-design until SNS migration with an explicit governance
      commitment, or accepted as housekeeping with a documented watch threshold.
    </p>
    <p>
      The protocol ran continuously on mainnet throughout the review and remediation cycle.
      No fund losses, no protocol pauses, no emergency interventions.
    </p>
  </div>
</section>

<section class="max-w-4xl mx-auto px-6 py-16 border-t" style="border-color: var(--rumi-border);">
  <h2 class="section-h2">Responsible disclosure</h2>
  <div class="prose-block">
    <p>
      If you believe you've found a vulnerability in any Rumi Protocol canister or frontend,
      please report it privately rather than disclosing publicly.
    </p>
    <ul>
      <li>
        <strong>Email:</strong>
        <a href="mailto:security@rumiprotocol.com">security@rumiprotocol.com</a>
      </li>
      <li>
        <strong>GitHub Security Advisories:</strong>
        <a href="https://github.com/RumiLabsXYZ/rumi-protocol-v2/security/advisories" target="_blank" rel="noopener">
          rumi-protocol-v2 advisories
        </a>
      </li>
    </ul>
    <p>
      Please include reproduction steps, affected canister IDs, and any proof-of-concept
      material. We aim to acknowledge reports within 48 hours.
    </p>
  </div>
</section>

<section class="max-w-4xl mx-auto px-6 py-16 border-t" style="border-color: var(--rumi-border);">
  <h2 class="section-h2">What's next</h2>
  <div class="prose-block">
    <p>
      A Code4rena audit contest is planned to broaden adversarial coverage. Admin-rotation
      hygiene findings are deferred-by-design and will land alongside the SNS migration,
      replacing the current developer-controlled controllers with DAO governance.
    </p>
  </div>
</section>

<style>
  .eyebrow {
    font-size: 0.875rem;
    font-weight: 500;
    letter-spacing: 0.15em;
    text-transform: uppercase;
    color: var(--rumi-purple-accent);
    margin-bottom: 1.5rem;
  }
  .page-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: clamp(2rem, 4vw, 2.5rem);
    font-weight: 700;
    letter-spacing: -0.02em;
    color: var(--rumi-text-primary);
    margin-bottom: 1.5rem;
  }
  .page-lead {
    color: var(--rumi-text-secondary);
    font-size: 1rem;
    line-height: 1.7;
    max-width: 640px;
  }

  .section-h2 {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.5rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    margin-bottom: 0.5rem;
    letter-spacing: -0.01em;
  }
  .section-sub {
    color: var(--rumi-text-muted);
    font-size: 0.9375rem;
    line-height: 1.6;
    margin-bottom: 2rem;
    max-width: 640px;
  }

  .audit-stack { display: flex; flex-direction: column; gap: 1rem; }

  .audit-card {
    padding: 1.75rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    transition: border-color 0.2s ease, transform 0.2s ease;
  }
  .audit-card:hover {
    border-color: var(--rumi-border-strong, rgba(255,255,255,0.18));
  }

  .audit-head {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-wrap: wrap;
    margin-bottom: 0.5rem;
  }
  .audit-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
  }
  .audit-tag {
    font-size: 0.6875rem;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    padding: 0.25rem 0.5rem;
    border-radius: 0.25rem;
    color: var(--rumi-purple-accent);
    border: 1px solid color-mix(in srgb, var(--rumi-purple-accent) 30%, transparent);
  }
  .audit-meta {
    display: flex;
    gap: 0.5rem;
    font-size: 0.8125rem;
    color: var(--rumi-text-muted);
    margin-bottom: 1rem;
  }
  .meta-sep { opacity: 0.6; }
  .audit-scope, .audit-summary {
    color: var(--rumi-text-secondary);
    font-size: 0.9375rem;
    line-height: 1.65;
    margin-bottom: 0.75rem;
  }
  .audit-summary { color: var(--rumi-text-muted); }

  .audit-actions {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
    margin-top: 1rem;
  }
  .btn-primary, .btn-secondary {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-weight: 500;
    font-size: 0.875rem;
    padding: 0.5rem 1rem;
    border-radius: 0.5rem;
    text-decoration: none;
    transition: all 0.15s ease;
    white-space: nowrap;
  }
  .btn-primary {
    background: var(--rumi-action);
    color: var(--rumi-bg-primary);
  }
  .btn-primary:hover {
    background: var(--rumi-action-bright);
    box-shadow: 0 0 20px rgba(52,211,153,0.15);
  }
  .btn-secondary {
    background: transparent;
    color: var(--rumi-text-secondary);
    border: 1px solid var(--rumi-border);
  }
  .btn-secondary:hover {
    color: var(--rumi-text-primary);
    border-color: var(--rumi-border-strong, rgba(255,255,255,0.2));
  }
  .btn-hint {
    color: var(--rumi-text-muted);
    font-weight: 400;
    font-size: 0.75rem;
    margin-left: 0.25rem;
  }

  .prose-block { max-width: 640px; }
  .prose-block p {
    color: var(--rumi-text-secondary);
    font-size: 0.9375rem;
    line-height: 1.7;
    margin-bottom: 1rem;
  }
  .prose-block ul {
    list-style: none;
    padding: 0;
    margin: 0 0 1rem 0;
  }
  .prose-block li {
    color: var(--rumi-text-secondary);
    font-size: 0.9375rem;
    line-height: 1.8;
  }
  .prose-block a {
    color: var(--rumi-action);
    text-decoration: none;
  }
  .prose-block a:hover { text-decoration: underline; }
  .prose-block code {
    font-family: 'JetBrains Mono', monospace;
    font-size: 0.85em;
    padding: 0.1rem 0.3rem;
    background: var(--rumi-bg-surface1);
    border-radius: 0.25rem;
  }
</style>
