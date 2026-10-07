<script>
  const publishedReviews = [
    {
      title: 'Full-protocol security review',
      date: 'June 9, 2026',
      kind: 'Internal differential review',
      description:
        'A review of the ICP-native protocol and related services, anchored to the source snapshot named in the report. Its findings and deployment notes describe that review period; they are not a statement about today’s installed code.',
      slug: 'rumi-security-review-2026-06-09',
      featured: true
    },
    {
      title: 'Combined security review',
      date: 'May 2, 2026',
      kind: 'Internal review + AVAI automated pre-audit',
      description:
        'A historical close-out of the April internal review and AVAI’s automated pre-audit. AVAI’s work was an automated pre-audit, not a traditional human-led audit engagement.',
      slug: 'rumi-combined-security-review-2026-05-02',
      featured: false
    },
    {
      title: 'AVAI external pre-audit',
      date: 'April 24, 2026',
      kind: 'Automated external pre-audit',
      description:
        'A snapshot-specific automated review with its own scope, limitations, and findings. Read the report before drawing conclusions from its summary.',
      slug: 'rumi-avai-external-audit-2026-04-24',
      featured: false
    },
    {
      title: 'Internal three-pass review',
      date: 'April 22, 2026',
      kind: 'Internal review',
      description:
        'The report records the review scope, remediation decisions, and status at the time. It does not establish the state of later source or deployed modules.',
      slug: 'rumi-internal-review-2026-04-22',
      featured: false
    }
  ];

  const releaseEvidence = [
    {
      number: '01',
      title: 'Upgrade and migration checks',
      text: 'Review representative populated-state upgrades and confirm relevant obligation continuity across canisters.'
    },
    {
      number: '02',
      title: 'Cross-canister and ledger outcomes',
      text: 'Continue targeted runtime checks for ambiguous calls, refunds, archive history, and recovery paths where relevant.'
    },
    {
      number: '03',
      title: 'Live-state reconciliation',
      text: 'Reconcile identified operational inventories and document evidence-backed outcomes for held or legacy operations.'
    },
    {
      number: '04',
      title: 'Release artifact and activation',
      text: 'Verify artifact interfaces and compare artifact hashes with installed modules; consider route admission separately against its applicable evidence.'
    }
  ];
</script>

<svelte:head>
  <title>Security · Rumi Protocol</title>
  <meta
    name="description"
    content="Security review history, current release status, and responsible disclosure for Rumi Protocol. Status reviewed October 7, 2026."
  />
</svelte:head>

<main>
  <section class="hero">
    <div class="page-width hero-inner">
      <p class="eyebrow"><span class="eyebrow-mark" aria-hidden="true"></span> Security &amp; disclosure</p>
      <div class="hero-grid">
        <div>
          <h1>Security is a process,<br /><span>not a badge.</span></h1>
          <p class="hero-copy">
            This page separates published review history from current source work and deployed
            code. A passing test, a source change, and an installed canister are different kinds
            of evidence.
          </p>
          <p class="review-date">Status reviewed <time datetime="2026-10-07">October 7, 2026</time></p>
        </div>

        <aside class="status-card" aria-labelledby="release-status-title">
          <div class="status-card-top">
            <span class="status-dot" aria-hidden="true"></span>
            <span>Release status</span>
            <span class="status-label">In progress</span>
          </div>
          <h2 id="release-status-title">Remediation is not live</h2>
          <p>
            The broader October security-remediation branch remains unmerged and has not been
            installed on mainnet. Separately, the CL-01 3pool hotfix (PR #421) has merged to main
            and its production artifact has been built. The artifact has not been installed: on
            October 7, 2026, the live 3pool module hash did not match the artifact hash.
          </p>
          <div class="status-divider"></div>
          <p class="status-footnote">
            No complete finding-closure or release-verification claim is made here. The CL-01
            artifact SHA-256 was e3182b5d545b279619ffeb10032feffdd222904fa20860d4755b9fabe37c00af;
            the observed live 3pool module SHA-256 was
            e89ad4f73a16012a8a7b3e389db13121aefe611ec6b459b3fcd270350ea92eab. Recheck exact
            installed hashes before describing the hotfix as deployed.
          </p>
        </aside>
      </div>

      <div class="evidence-rail" aria-label="Security evidence states">
        <div class="evidence-step published">
          <span class="step-icon" aria-hidden="true">01</span>
          <div><strong>Published reviews</strong><span>Historical source snapshots</span></div>
        </div>
        <span class="rail-line" aria-hidden="true"></span>
        <div class="evidence-step working">
          <span class="step-icon" aria-hidden="true">02</span>
          <div><strong>Source remediation</strong><span>Unmerged; release evidence open</span></div>
        </div>
        <span class="rail-line" aria-hidden="true"></span>
        <div class="evidence-step live">
          <span class="step-icon" aria-hidden="true">03</span>
          <div><strong>Installed code</strong><span>Separate on-chain verification</span></div>
        </div>
      </div>
    </div>
  </section>

  <section class="section page-width" aria-labelledby="what-we-know">
    <div class="section-heading">
      <p class="section-kicker">Current picture</p>
      <h2 id="what-we-know">What the evidence says</h2>
      <p>These statements describe different stages of the work and should be read separately.</p>
    </div>

    <div class="state-grid">
      <article class="state-card state-published">
        <span class="state-icon" aria-hidden="true">↗</span>
        <p class="state-label">Public record</p>
        <h3>Reviews are available</h3>
        <p>
          The reports below document specific code snapshots, scope, and limitations. The latest
          published full-protocol report is dated June 9, 2026.
        </p>
      </article>

      <article class="state-card state-source">
        <span class="state-icon" aria-hidden="true">◷</span>
        <p class="state-label">Source work</p>
        <h3>A newer review is still being closed out</h3>
        <p>
          A later internal review led to changes in a release branch. Some targeted tests and
          source checks have passed, while additional finding adjudication and operational
          evidence remain open. No final public report for that cycle is posted here.
        </p>
      </article>

      <article class="state-card state-live">
        <span class="state-icon" aria-hidden="true">⌁</span>
        <p class="state-label">Mainnet</p>
        <h3>Source changes do not update canisters</h3>
        <p>
          The October remediation branch was unmerged and undeployed at the date above. A source
          fix does not prove an upgrade, feature activation, migration, or successful live
          operation.
        </p>
      </article>
    </div>
  </section>

  <section class="section page-width" aria-labelledby="october-report-title">
    <article class="report-card featured october-report-card">
      <div class="report-card-top">
        <span class="report-kind">Status report · October 7, 2026</span>
        <span class="latest-tag">Current update</span>
      </div>
      <h2 id="october-report-title">October security review status</h2>
      <p class="report-description">
        Read the public summary of what is merged, what remains draft source work, what has not
        been installed, and why the Points pause policy is not a live safeguard.
      </p>
      <div class="report-links">
        <a href="/security/october-2026">Read the October status report <span aria-hidden="true">↗</span></a>
      </div>
    </article>
  </section>
  <section class="release-section" aria-labelledby="release-evidence-title">
    <div class="page-width release-inner">
      <div class="section-heading release-heading">
        <p class="section-kicker">Current follow-up</p>
        <h2 id="release-evidence-title">Additional verification work</h2>
        <p>
          These areas describe ongoing follow-up, not blanket release gates. Whether a particular
          item blocks a release depends on the applicable finding and its acceptance criteria.
        </p>
      </div>
      <ol class="evidence-list">
        {#each releaseEvidence as item}
          <li>
            <span class="evidence-number">{item.number}</span>
            <div>
              <h3>{item.title}</h3>
              <p>{item.text}</p>
            </div>
          </li>
        {/each}
      </ol>
      <p class="release-note">
        Feature admission, merging, deployment, and live verification are separate decisions.
        None should be inferred from the existence of a remediation branch.
      </p>
    </div>
  </section>

  <section class="section page-width" aria-labelledby="review-library-title">
    <div class="section-heading">
      <p class="section-kicker">Review archive</p>
      <h2 id="review-library-title">Published reports</h2>
      <p>
        Each report is a historical record tied to its own scope and source snapshot. Read the
        full report for methodology, limitations, and the status recorded at that time.
      </p>
    </div>

    <div class="report-grid">
      {#each publishedReviews as report}
        <article class:featured={report.featured} class="report-card">
          <div class="report-card-top">
            <span class="report-kind">{report.kind}</span>
            {#if report.featured}<span class="latest-tag">Latest published</span>{/if}
          </div>
          <h3>{report.title}</h3>
          <p class="report-date">{report.date}</p>
          <p class="report-description">{report.description}</p>
          <div class="report-links">
            <a href="/audits/{report.slug}.md" target="_blank" rel="noopener">Read report <span aria-hidden="true">↗</span></a>
            <a class="pdf-link" href="/audits/{report.slug}.pdf" target="_blank" rel="noopener">PDF <span aria-hidden="true">↗</span></a>
          </div>
        </article>
      {/each}
    </div>
  </section>

  <section class="section disclosure-section" aria-labelledby="disclosure-title">
    <div class="page-width disclosure-grid">
      <div>
        <p class="section-kicker">Help us protect users</p>
        <h2 id="disclosure-title">Report a vulnerability privately</h2>
        <p>
          If you believe you have found a vulnerability in a Rumi Protocol canister or frontend,
          please report it privately rather than disclosing it publicly. Include the affected
          component and enough detail for us to investigate safely; do not include user balances
          or secrets.
        </p>
      </div>
      <div class="disclosure-card">
        <p class="disclosure-response">We aim to acknowledge reports within 48 hours.</p>
        <a class="disclosure-link" href="mailto:info@rumiprotocol.com">
          Email the security contact <span aria-hidden="true">↗</span>
        </a>
        <a
          class="advisory-link"
          href="https://github.com/RumiLabsXYZ/rumi-protocol-v2/security/advisories"
          target="_blank"
          rel="noopener"
        >Open a private GitHub Security Advisory <span aria-hidden="true">↗</span></a>
        <p class="bounty-note">Rumi does not currently operate a paid bug-bounty programme.</p>
      </div>
    </div>
  </section>

  <footer class="page-width page-foot">
    <span>Security status last reviewed October 7, 2026.</span>
    <a href="https://github.com/RumiLabsXYZ/rumi-protocol-v2" target="_blank" rel="noopener">
      Verify source and release history <span aria-hidden="true">↗</span>
    </a>
  </footer>
</main>

<style>
  main { color: var(--rumi-text-primary); }
  .page-width { width: min(1120px, calc(100% - 3rem)); margin-inline: auto; }
  .hero {
    position: relative;
    overflow: hidden;
    border-bottom: 1px solid var(--rumi-border);
    background:
      radial-gradient(ellipse at 12% 0%, color-mix(in srgb, var(--rumi-purple-accent) 16%, transparent), transparent 52%),
      radial-gradient(ellipse at 92% 70%, color-mix(in srgb, var(--rumi-action) 8%, transparent), transparent 38%),
      var(--rumi-bg-primary);
  }
  .hero-inner { padding-top: clamp(4rem, 8vw, 7rem); padding-bottom: 2.75rem; }
  .eyebrow, .section-kicker {
    display: flex; align-items: center; gap: .65rem; margin: 0 0 1rem;
    color: var(--rumi-purple-accent); font-size: .72rem; font-weight: 700;
    letter-spacing: .16em; text-transform: uppercase;
  }
  .eyebrow-mark { width: .5rem; height: .5rem; border-radius: 50%; background: var(--rumi-purple-accent); box-shadow: 0 0 18px color-mix(in srgb, var(--rumi-purple-accent) 65%, transparent); }
  .hero-grid { display: grid; grid-template-columns: minmax(0, 1.1fr) minmax(300px, .8fr); align-items: center; gap: clamp(2rem, 6vw, 5rem); }
  h1 { max-width: 720px; margin: 0; font-size: clamp(2.6rem, 6.3vw, 5.4rem); line-height: .99; letter-spacing: -.055em; }
  h1 span { color: var(--rumi-purple-accent); }
  .hero-copy { max-width: 620px; margin: 1.6rem 0 1rem; color: var(--rumi-text-secondary); font-size: 1.08rem; line-height: 1.75; }
  .review-date { color: var(--rumi-text-muted); font-size: .84rem; }
  .review-date time { color: var(--rumi-text-secondary); }
  .status-card { padding: 1.5rem; border: 1px solid color-mix(in srgb, #f6bd60 35%, var(--rumi-border)); border-radius: 1rem; background: linear-gradient(150deg, color-mix(in srgb, #f6bd60 8%, var(--rumi-bg-surface1)), var(--rumi-bg-surface1) 72%); box-shadow: 0 24px 80px rgba(0,0,0,.22); }
  .status-card-top { display: flex; align-items: center; gap: .55rem; color: #f4c46e; font-size: .72rem; font-weight: 700; letter-spacing: .12em; text-transform: uppercase; }
  .status-dot { width: .5rem; height: .5rem; border-radius: 50%; background: #f4c46e; box-shadow: 0 0 0 4px rgba(244,196,110,.12); }
  .status-label { margin-left: auto; padding: .32rem .55rem; border: 1px solid rgba(244,196,110,.25); border-radius: 999px; font-size: .62rem; letter-spacing: .08em; }
  .status-card h2 { margin: 1.2rem 0 .55rem; font-size: 1.45rem; letter-spacing: -.03em; }
  .status-card p { margin: 0; color: var(--rumi-text-secondary); font-size: .9rem; line-height: 1.65; }
  .status-divider { height: 1px; margin: 1.1rem 0; background: var(--rumi-border); }
  .status-card .status-footnote { color: var(--rumi-text-muted); font-size: .77rem; }
  .evidence-rail { display: grid; grid-template-columns: 1fr minmax(28px,.15fr) 1fr minmax(28px,.15fr) 1fr; align-items: center; gap: .75rem; margin-top: clamp(3rem, 7vw, 5.5rem); padding: 1.1rem 1.3rem; border: 1px solid var(--rumi-border); border-radius: .9rem; background: color-mix(in srgb, var(--rumi-bg-surface1) 76%, transparent); }
  .evidence-step { display: flex; align-items: center; gap: .8rem; min-width: 0; }
  .step-icon { display: grid; width: 2.2rem; height: 2.2rem; flex: 0 0 auto; place-items: center; border: 1px solid var(--rumi-border); border-radius: .65rem; color: var(--rumi-purple-accent); font-size: .68rem; font-weight: 700; }
  .evidence-step div { display: grid; gap: .2rem; }
  .evidence-step strong { font-size: .82rem; }
  .evidence-step span:last-child { color: var(--rumi-text-muted); font-size: .7rem; }
  .evidence-step.working .step-icon { color: #f4c46e; border-color: rgba(244,196,110,.3); }
  .evidence-step.live .step-icon { color: var(--rumi-teal); }
  .rail-line { height: 1px; background: linear-gradient(90deg, var(--rumi-purple-accent), rgba(244,196,110,.6)); }
  .section { padding-block: clamp(4rem, 8vw, 6.5rem); }
  .section-heading { max-width: 680px; margin-bottom: 2rem; }
  .section-kicker { margin-bottom: .7rem; color: var(--rumi-teal); }
  .section-heading h2, .disclosure-grid h2 { margin: 0; font-size: clamp(1.8rem, 3.4vw, 2.7rem); letter-spacing: -.04em; }
  .section-heading > p:last-child, .disclosure-grid > div:first-child > p:last-child { margin: .85rem 0 0; color: var(--rumi-text-muted); line-height: 1.7; }
  .state-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 1rem; }
  .state-card { position: relative; min-height: 245px; padding: 1.4rem; border: 1px solid var(--rumi-border); border-radius: .9rem; background: var(--rumi-bg-surface1); }
  .state-card::before { position: absolute; inset: 0 auto 0 0; width: 2px; border-radius: .9rem 0 0 .9rem; background: var(--rumi-purple-accent); content: ''; opacity: .7; }
  .state-source::before { background: #f4c46e; }
  .state-live::before { background: var(--rumi-teal); }
  .state-icon { display: grid; width: 2rem; height: 2rem; place-items: center; border-radius: .6rem; background: color-mix(in srgb, var(--rumi-purple-accent) 12%, var(--rumi-bg-surface2)); color: var(--rumi-purple-accent); font-size: .95rem; }
  .state-source .state-icon { color: #f4c46e; background: rgba(244,196,110,.1); }
  .state-live .state-icon { color: var(--rumi-teal); background: color-mix(in srgb, var(--rumi-teal) 10%, var(--rumi-bg-surface2)); }
  .state-label { margin: 1.2rem 0 .35rem; color: var(--rumi-text-muted); font-size: .66rem; font-weight: 700; letter-spacing: .13em; text-transform: uppercase; }
  .state-card h3 { margin: 0 0 .65rem; font-size: 1.1rem; }
  .state-card > p:last-child { margin: 0; color: var(--rumi-text-secondary); font-size: .86rem; line-height: 1.7; }
  .release-section { border-block: 1px solid var(--rumi-border); background: linear-gradient(120deg, rgba(14,18,34,.95), rgba(10,14,26,.96)); }
  .release-inner { padding-block: clamp(4rem, 8vw, 6rem); }
  .release-heading { margin-bottom: 2.3rem; }
  .evidence-list { display: grid; grid-template-columns: repeat(2, 1fr); gap: .9rem; margin: 0; padding: 0; list-style: none; }
  .evidence-list li { display: flex; gap: 1rem; padding: 1.2rem; border: 1px solid var(--rumi-border); border-radius: .8rem; background: rgba(20,26,46,.58); }
  .evidence-number { flex: 0 0 auto; padding-top: .1rem; color: var(--rumi-purple-accent); font-size: .72rem; font-weight: 700; letter-spacing: .1em; }
  .evidence-list h3 { margin: 0 0 .4rem; font-size: .98rem; }
  .evidence-list p { margin: 0; color: var(--rumi-text-muted); font-size: .82rem; line-height: 1.65; }
  .release-note { margin: 1.4rem 0 0; padding-left: 1rem; border-left: 2px solid #f4c46e; color: var(--rumi-text-secondary); font-size: .82rem; line-height: 1.65; }
  .report-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1rem; }
  .report-card { display: flex; min-height: 280px; flex-direction: column; padding: 1.4rem; border: 1px solid var(--rumi-border); border-radius: .9rem; background: var(--rumi-bg-surface1); }
  .report-card.featured { border-color: color-mix(in srgb, var(--rumi-purple-accent) 38%, var(--rumi-border)); background: linear-gradient(135deg, color-mix(in srgb, var(--rumi-purple-accent) 7%, var(--rumi-bg-surface1)), var(--rumi-bg-surface1) 65%); }
  .report-card-top { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: .5rem; }
  .report-kind, .latest-tag { color: var(--rumi-text-muted); font-size: .66rem; font-weight: 600; letter-spacing: .1em; text-transform: uppercase; }
  .latest-tag { padding: .3rem .5rem; border-radius: 999px; background: rgba(209,118,232,.12); color: var(--rumi-purple-accent); font-size: .59rem; }
  .report-card h3 { margin: 1rem 0 .2rem; font-size: 1.2rem; }
  .report-date { margin: 0; color: var(--rumi-text-muted); font-size: .75rem; }
  .report-description { margin: .9rem 0 1.2rem; color: var(--rumi-text-secondary); font-size: .83rem; line-height: 1.65; }
  .report-links { display: flex; gap: 1.2rem; margin-top: auto; padding-top: .9rem; border-top: 1px solid var(--rumi-border); }
  .report-links a, .page-foot a { color: var(--rumi-purple-accent); font-size: .82rem; font-weight: 600; text-decoration: none; }
  .report-links .pdf-link { color: var(--rumi-text-muted); font-weight: 500; }
  a:hover { text-decoration: underline; text-underline-offset: 3px; }
  a:focus-visible { outline: 2px solid var(--rumi-teal-bright); outline-offset: 4px; border-radius: 2px; }
  .disclosure-section { border-top: 1px solid var(--rumi-border); background: radial-gradient(ellipse at 85% 100%, rgba(52,211,153,.07), transparent 42%); }
  .disclosure-grid { display: grid; grid-template-columns: 1fr minmax(300px, .72fr); gap: 3rem; align-items: center; }
  .disclosure-grid > div:first-child > p:last-child { max-width: 620px; }
  .disclosure-card { padding: 1.4rem; border: 1px solid color-mix(in srgb, var(--rumi-teal) 25%, var(--rumi-border)); border-radius: .9rem; background: var(--rumi-bg-surface1); }
  .disclosure-response { margin: 0 0 1rem; color: var(--rumi-text-secondary); font-size: .82rem; }
  .disclosure-link, .advisory-link { display: flex; justify-content: space-between; gap: 1rem; padding: .85rem 0; border-top: 1px solid var(--rumi-border); color: var(--rumi-teal-bright); font-size: .84rem; font-weight: 600; text-decoration: none; }
  .advisory-link { color: var(--rumi-text-secondary); font-size: .79rem; font-weight: 500; }
  .bounty-note { margin: .75rem 0 0; color: var(--rumi-text-muted); font-size: .7rem; }
  .page-foot { display: flex; justify-content: space-between; gap: 1rem; padding-block: 1.3rem 2rem; border-top: 1px solid var(--rumi-border); color: var(--rumi-text-muted); font-size: .72rem; }

  @media (max-width: 760px) {
    .page-width { width: min(100% - 2rem, 600px); }
    .hero-grid, .disclosure-grid { grid-template-columns: 1fr; }
    .hero-grid { gap: 2rem; }
    .status-card { max-width: 540px; }
    .evidence-rail { grid-template-columns: 1fr; gap: .7rem; padding: 1rem; }
    .rail-line { width: 1px; height: 16px; margin-left: 1.05rem; background: linear-gradient(180deg, var(--rumi-purple-accent), rgba(244,196,110,.6)); }
    .state-grid { grid-template-columns: 1fr; }
    .state-card { min-height: auto; }
    .evidence-list, .report-grid { grid-template-columns: 1fr; }
    .disclosure-grid { gap: 1.5rem; }
    .page-foot { flex-direction: column; }
  }
</style>
