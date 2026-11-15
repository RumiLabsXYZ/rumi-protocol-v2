<script>
  const evidenceStages = [
    {
      label: 'Merged',
      title: 'Source change accepted',
      detail: 'PR #641 merged as e723c94b. The merged tree matched the reviewed head.',
      tone: 'source'
    },
    {
      label: 'CI',
      title: 'Hosted checks passed',
      detail: 'Frontend, archived-receipt, and backend/AMM/Treasury security jobs passed.',
      tone: 'verified'
    },
    {
      label: 'Built',
      title: 'Selected artifacts prepared',
      detail: 'Backend and Points raw artifacts were built. No production backend candidate was accepted from this tranche.',
      tone: 'built'
    },
    {
      label: 'Installed',
      title: 'Installation evidence not included',
      detail: 'The closure tranche still requires exact production artifact and readback evidence.',
      tone: 'held'
    },
    {
      label: 'Activated',
      title: 'Route activation remains separate',
      detail: 'This snapshot includes no activation evidence for EVM/public-open or native-XRP fanout.',
      tone: 'held'
    },
    {
      label: 'Recovered',
      title: 'Vault-202 recovery remains open',
      detail: 'This snapshot does not claim that the recovery completed.',
      tone: 'held'
    }
  ];

  const changes = [
    'Analytics overlap handling and event cursor behavior',
    'AMM retained-event cursor handling',
    'Stability Pool held-payout and unsupported-fee safety',
    'Liquidation bot short-payment recovery',
    'Frontend operation locking and Content Security Policy',
    'Points repayment-unit handling',
    'Backend ICP-only bot allowlist'
  ];

  const coverage = [
    {
      area: 'Core CDP paths',
      detail: 'Selected borrowing, repayment, interest, collateral-accounting, liquidation, redemption, and upgrade-safety findings.'
    },
    {
      area: 'Liquidity and recovery paths',
      detail: 'Selected Stability Pool, 3USD, AMM, Treasury, liquidation-bot, payout, refund, and ambiguous-ledger findings.'
    },
    {
      area: 'Supporting-service paths',
      detail: 'Selected Points, analytics, cycle-operation, frontend transaction-boundary, wallet-flow, and release-configuration findings.'
    },
    {
      area: 'External-rail paths',
      detail: 'Selected replay-protection, admission, finality, oracle-freshness, queue, custody, and activation-hold findings.'
    }
  ];
</script>

<svelte:head>
  <title>November 2026 Security Status · Rumi Protocol</title>
  <meta
    name="description"
    content="A dated Rumi Protocol security update separating merged source, CI, built artifacts, installation, activation, and recovery."
  />
</svelte:head>

<main>
  <section class="masthead" aria-labelledby="report-title">
    <div class="page-width masthead-inner">
      <p class="eyebrow"><span class="status-mark" aria-hidden="true"></span> Rumi Protocol / Security notes</p>
      <div class="hero-grid">
        <div class="hero-copy">
          <p class="issue-line">
            Status report <span aria-hidden="true">—</span>
            <time datetime="2026-11-14">November 14, 2026</time> · revised
            <time datetime="2026-11-15">November 15, 2026</time>
          </p>
          <h1 id="report-title">Security progress,<br /><span>with each step clear.</span></h1>
          <p class="lede">
            PR #641 is merged, and its merged tree matched the reviewed head. The hosted
            frontend, archived-receipt, and backend/AMM/Treasury security jobs passed. Those
            results describe source and CI; they do not establish installation, activation,
            or recovery in production.
          </p>
        </div>

        <aside class="report-stamp" aria-label="November status summary">
          <span class="stamp-label">Current status</span>
          <strong>Merged and CI-verified; production follow-through remains open</strong>
          <p>
            Backend and Points raw artifacts were built. A later live-interface check identified
            backend source-lineage reconciliation as required before accepting a production
            backend candidate. This snapshot does not include production-installation or
            operation-specific recovery evidence for PR #641.
          </p>
          <a href="/security">View security archive <span aria-hidden="true">→</span></a>
        </aside>
      </div>

      <div class="rule-caption" aria-label="Evidence summary">
        <span>PR #641 · merged commit e723c94b</span>
        <span>Snapshot date: November 14 · revised November 15, 2026</span>
      </div>
    </div>
  </section>

  <section class="page-width section" aria-labelledby="evidence-title">
    <div class="section-heading">
      <p class="section-label">Evidence stages</p>
      <h2 id="evidence-title">What each result establishes</h2>
      <p>
        Progress is recorded by evidence type. A result at one stage does not imply the next
        stage has happened.
      </p>
    </div>

    <ol class="stage-grid">
      {#each evidenceStages as stage}
        <li class="stage-card stage-{stage.tone}">
          <span class="badge">{stage.label}</span>
          <h3>{stage.title}</h3>
          <p>{stage.detail}</p>
        </li>
      {/each}
    </ol>
  </section>

  <section class="page-width section" aria-labelledby="coverage-title">
    <div class="section-heading section-heading-wide">
      <p class="section-label">Program areas examined</p>
      <h2 id="coverage-title">Where this closure tranche looked</h2>
      <p>
        The program revisited selected accepted findings from earlier audit packages and later
        adversarial review work, including their related cross-component paths. This public page
        summarizes that bounded closure tranche without publishing exploit instructions or
        sensitive recovery detail; it is not a claim of complete-system coverage.
      </p>
    </div>

    <div class="coverage-grid">
      {#each coverage as item}
        <article>
          <h3>{item.area}</h3>
          <p>{item.detail}</p>
        </article>
      {/each}
    </div>
  </section>

  <section class="updates-section" aria-labelledby="changes-title">
    <div class="page-width section">
      <div class="section-heading section-heading-wide">
        <p class="section-label">Selected source fixes</p>
        <h2 id="changes-title">Changes included in PR #641</h2>
        <p>These are concise area summaries, not exploit descriptions or claims that every issue is closed.</p>
      </div>

      <ul class="change-list">
        {#each changes as change, index}
          <li>
            <span class="change-number" aria-hidden="true">{String(index + 1).padStart(2, '0')}</span>
            <span>{change}</span>
          </li>
        {/each}
      </ul>

      <p class="source-link">
        <a href="https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/641" target="_blank" rel="noopener noreferrer">
          Review PR #641 on GitHub <span aria-hidden="true">↗</span>
        </a>
      </p>
    </div>
  </section>

  <section class="page-width section" aria-labelledby="holds-title">
    <div class="limits-grid">
      <div>
        <p class="section-label">Open follow-through</p>
        <h2 id="holds-title">Current holds</h2>
      </div>
      <div class="hold-copy">
        <p>
          The closure tranche still requires remaining production artifacts, exact installation
          readbacks, backend source-lineage reconciliation, and operation-specific Vault-202
          recovery evidence. This snapshot includes no activation evidence for EVM/public-open or
          native-XRP fanout.
          Those route-specific evidence gaps are not blockers for unrelated flows.
        </p>
        <p>
          This snapshot does not claim that a PR #641 artifact was installed, that an activation
          occurred, that funds moved for a recovery, or that an operation was recovered. Unrelated
          November releases are outside this statement.
        </p>
      </div>
    </div>
  </section>

  <section class="method-section" aria-labelledby="method-title">
    <div class="page-width section method-grid">
      <div>
        <p class="section-label">How to read this page</p>
        <h2 id="method-title">Method and limits</h2>
      </div>
      <div class="method-copy">
        <p>
          The merge statement is based on PR #641 and the matching reviewed and merged trees.
          CI statements refer only to the named hosted jobs. Build and candidate-verification
          statements describe artifact preparation; no backend production candidate is accepted
          by this snapshot, and artifact preparation is not installation evidence.
        </p>
        <p>
          Installation requires a production readback tied to the exact artifact. Activation
          requires route-specific evidence. Recovery requires operation-specific evidence. This
          page is a dated status snapshot, not a full audit or live system monitor.
        </p>
      </div>
    </div>
  </section>

  <section class="page-width section disclosure" aria-labelledby="disclosure-title">
    <div class="disclosure-grid">
      <div>
        <p class="section-label">Responsible disclosure</p>
        <h2 id="disclosure-title">Report a vulnerability privately</h2>
        <p>
          Please report suspected vulnerabilities privately and include the affected component
          and enough detail to investigate safely. Do not include user balances or secrets.
        </p>
      </div>
      <div class="disclosure-card">
        <a href="mailto:info@rumiprotocol.com">Email the security contact <span aria-hidden="true">↗</span></a>
        <a href="https://github.com/RumiLabsXYZ/rumi-protocol-v2/security/advisories" target="_blank" rel="noopener noreferrer">
          Open a private GitHub Security Advisory <span aria-hidden="true">↗</span>
        </a>
      </div>
    </div>
  </section>

  <footer class="page-width page-foot">
    <span>Security status snapshot dated November 14; revised November 15, 2026.</span>
    <a href="/security/october-2026">Read the October status snapshot <span aria-hidden="true">→</span></a>
  </footer>
</main>

<style>
  main { color: var(--rumi-text-primary); }
  .page-width { width: min(1120px, calc(100% - 3rem)); margin-inline: auto; }
  .masthead {
    border-bottom: 1px solid var(--rumi-border);
    background:
      radial-gradient(ellipse at 12% 0%, color-mix(in srgb, var(--rumi-purple-accent) 15%, transparent), transparent 52%),
      radial-gradient(ellipse at 92% 72%, color-mix(in srgb, var(--rumi-teal) 7%, transparent), transparent 38%),
      var(--rumi-bg-primary);
  }
  .masthead-inner { padding-block: clamp(3.5rem, 8vw, 6.5rem) 1.5rem; }
  .eyebrow, .section-label {
    margin: 0 0 1rem; color: var(--rumi-teal); font-size: .7rem; font-weight: 700;
    letter-spacing: .14em; text-transform: uppercase;
  }
  .eyebrow { display: flex; align-items: center; gap: .7rem; }
  .status-mark { width: .55rem; height: .55rem; border: 2px solid var(--rumi-teal); border-radius: 50%; }
  .hero-grid { display: grid; grid-template-columns: minmax(0, 1.45fr) minmax(260px, .75fr); align-items: end; gap: clamp(2rem, 6vw, 5rem); }
  .issue-line { margin: 0 0 1rem; color: var(--rumi-text-muted); font-size: .85rem; }
  .issue-line span { padding-inline: .35rem; color: var(--rumi-purple-accent); }
  .issue-line time { color: var(--rumi-text-secondary); }
  h1 { max-width: 740px; margin: 0; font-size: clamp(2.7rem, 6.4vw, 5.2rem); line-height: .98; letter-spacing: -.06em; }
  h1 span { color: var(--rumi-purple-accent); }
  .lede { max-width: 700px; margin: 1.5rem 0 0; color: var(--rumi-text-secondary); font-size: 1.05rem; line-height: 1.75; }
  .report-stamp { padding: 1.2rem; border: 1px solid color-mix(in srgb, #e7a94c 32%, var(--rumi-border)); border-top: 3px solid #e7a94c; background: color-mix(in srgb, var(--rumi-bg-surface1) 88%, transparent); }
  .stamp-label { display: block; margin-bottom: .65rem; color: #e7bb72; font-size: .66rem; font-weight: 700; letter-spacing: .12em; text-transform: uppercase; }
  .report-stamp strong { display: block; font-size: 1.08rem; line-height: 1.4; }
  .report-stamp p { margin: .7rem 0 1rem; color: var(--rumi-text-muted); font-size: .81rem; line-height: 1.65; }
  a { color: var(--rumi-teal-bright); text-decoration: none; text-underline-offset: 4px; }
  a:hover { text-decoration: underline; }
  a:focus-visible { outline: 2px solid var(--rumi-teal-bright); outline-offset: 4px; border-radius: 2px; }
  .rule-caption { display: flex; justify-content: space-between; gap: 1rem; margin-top: clamp(2.7rem, 6vw, 4.5rem); padding-block: .8rem; border-top: 1px solid var(--rumi-border); color: var(--rumi-text-muted); font-size: .72rem; }
  .rule-caption span:first-child { color: var(--rumi-text-secondary); font-weight: 700; }
  .section { padding-block: clamp(3.5rem, 7vw, 5.5rem); }
  .section-heading { max-width: 690px; margin-bottom: 1.8rem; }
  .section-heading-wide { max-width: 760px; }
  .section-heading h2, .limits-grid h2, .method-grid h2, .disclosure-grid h2 { margin: 0; font-size: clamp(1.8rem, 3.5vw, 2.7rem); line-height: 1.1; letter-spacing: -.045em; }
  .section-heading > p:last-child, .disclosure-grid > div:first-child > p:last-child { margin: .8rem 0 0; color: var(--rumi-text-muted); line-height: 1.7; }
  .stage-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: .8rem; margin: 0; padding: 0; list-style: none; }
  .stage-card { min-height: 182px; padding: 1.1rem; border: 1px solid var(--rumi-border); background: color-mix(in srgb, var(--rumi-bg-surface1) 76%, transparent); }
  .stage-card h3 { margin: 1rem 0 .4rem; font-size: 1rem; letter-spacing: -.015em; }
  .stage-card p { margin: 0; color: var(--rumi-text-secondary); font-size: .82rem; line-height: 1.65; }
  .badge { display: inline-flex; min-height: 1.55rem; align-items: center; padding: .2rem .52rem; border: 1px solid color-mix(in srgb, var(--rumi-purple-accent) 35%, var(--rumi-border)); border-radius: 999px; color: #c5b3ff; background: color-mix(in srgb, var(--rumi-purple-accent) 10%, transparent); font-size: .64rem; font-weight: 700; letter-spacing: .07em; text-transform: uppercase; }
  .stage-verified .badge, .stage-built .badge { border-color: color-mix(in srgb, var(--rumi-teal) 38%, var(--rumi-border)); color: var(--rumi-teal-bright); background: color-mix(in srgb, var(--rumi-teal) 9%, transparent); }
  .stage-held .badge { border-color: rgba(231,169,76,.38); color: #edc77f; background: rgba(231,169,76,.09); }
  .coverage-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); border: 1px solid var(--rumi-border); }
  .coverage-grid article { min-height: 155px; padding: 1.25rem; border-right: 1px solid var(--rumi-border); border-bottom: 1px solid var(--rumi-border); background: color-mix(in srgb, var(--rumi-bg-surface1) 54%, transparent); }
  .coverage-grid article:nth-child(2n) { border-right: 0; }
  .coverage-grid article:nth-last-child(-n + 2) { border-bottom: 0; }
  .coverage-grid h3 { margin: 0 0 .6rem; color: var(--rumi-text-primary); font-size: .98rem; }
  .coverage-grid p { margin: 0; color: var(--rumi-text-secondary); font-size: .82rem; line-height: 1.65; }
  .updates-section { border-block: 1px solid var(--rumi-border); background: color-mix(in srgb, var(--rumi-bg-surface1) 42%, var(--rumi-bg-primary)); }
  .change-list { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); margin: 0; padding: 0; border-top: 1px solid var(--rumi-border); list-style: none; }
  .change-list li { display: flex; align-items: baseline; gap: .85rem; min-height: 66px; padding: 1rem .8rem; border-bottom: 1px solid var(--rumi-border); color: var(--rumi-text-secondary); font-size: .88rem; line-height: 1.55; }
  .change-number { flex: 0 0 auto; color: var(--rumi-teal); font-size: .68rem; font-weight: 700; letter-spacing: .1em; }
  .source-link { margin: 1.25rem 0 0; font-size: .82rem; font-weight: 600; }
  .limits-grid, .method-grid { display: grid; grid-template-columns: minmax(220px, .65fr) 1fr; gap: clamp(2rem, 7vw, 5rem); align-items: start; }
  .hold-copy p, .method-copy p { margin: 0 0 .9rem; color: var(--rumi-text-secondary); line-height: 1.75; }
  .hold-copy p:last-child, .method-copy p:last-child { margin-bottom: 0; }
  .method-section { border-block: 1px solid var(--rumi-border); background: linear-gradient(110deg, color-mix(in srgb, var(--rumi-purple-accent) 7%, var(--rumi-bg-primary)), var(--rumi-bg-primary) 58%); }
  .method-grid { padding-block: clamp(3rem, 6vw, 4.5rem); }
  .disclosure { border-bottom: 1px solid var(--rumi-border); }
  .disclosure-grid { display: grid; grid-template-columns: 1fr minmax(280px, .7fr); gap: 3rem; align-items: center; }
  .disclosure-card { padding: .4rem 1.2rem; border: 1px solid color-mix(in srgb, var(--rumi-teal) 25%, var(--rumi-border)); background: var(--rumi-bg-surface1); }
  .disclosure-card a { display: flex; justify-content: space-between; gap: 1rem; padding: .9rem 0; border-bottom: 1px solid var(--rumi-border); font-size: .82rem; font-weight: 600; }
  .disclosure-card a:last-child { border-bottom: 0; color: var(--rumi-text-secondary); font-weight: 500; }
  .page-foot { display: flex; justify-content: space-between; gap: 1rem; padding-block: 1.2rem 2rem; color: var(--rumi-text-muted); font-size: .72rem; }
  @media (max-width: 760px) {
    .page-width { width: min(100% - 2rem, 610px); }
    .hero-grid, .limits-grid, .method-grid, .disclosure-grid { grid-template-columns: 1fr; gap: 1.5rem; }
    .stage-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .rule-caption, .page-foot { flex-direction: column; gap: .45rem; }
  }
  @media (max-width: 480px) {
    .stage-grid, .change-list, .coverage-grid { grid-template-columns: 1fr; }
    .coverage-grid article, .coverage-grid article:nth-child(2n), .coverage-grid article:nth-last-child(-n + 2) { border-right: 0; border-bottom: 1px solid var(--rumi-border); }
    .coverage-grid article:last-child { border-bottom: 0; }
    .stage-card { min-height: auto; }
  }
</style>
