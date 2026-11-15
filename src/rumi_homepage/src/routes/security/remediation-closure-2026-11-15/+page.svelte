<script>
  const stages = [
    { label: 'Source', value: 'Merged', detail: 'PRs #652–#662 and #666 are in canonical main.', tone: 'done' },
    { label: 'Artifacts', value: 'Verified', detail: 'The five dependency-refresh candidates are hash-pinned.', tone: 'done' },
    { label: 'Installed', value: '4 of 5 refreshed', detail: 'Backend, bot, AMM, and Analytics match; Stability Pool remains held.', tone: 'partial' },
    { label: 'Recovery', value: 'Bounded', detail: 'Named durable inventories were queried and reconciled.', tone: 'partial' }
  ];

  const releaseRows = [
    { component: 'AMM', state: 'Refreshed', tone: 'installed', hash: '3a59c167…185e6', detail: 'PR #666 candidate installed; pool values and empty payout inventory preserved.' },
    { component: 'Analytics', state: 'Refreshed', tone: 'installed', hash: '704f6548…1f290', detail: 'PR #666 candidate installed; collector cursors and schedule preserved.' },
    { component: 'Stability Pool', state: 'Held', tone: 'held', hash: '32cf7dac…cce', detail: 'The prior remediated build remains installed and healthy. Candidate d8385398…329e9 was not installed after two interrupted requests.' },
    { component: 'Treasury', state: 'Installed', tone: 'installed', hash: '785adb02…c3832', detail: 'Exact candidate installed; balances preserved, backfill complete, and reporter set bound.' },
    { component: 'Protocol backend', state: 'Refreshed', tone: 'installed', hash: '59a64f78…44111', detail: 'PR #666 candidate installed; General Availability mode and zero protocol deficit preserved.' },
    { component: 'Liquidation bot', state: 'Refreshed', tone: 'installed', hash: '941cd13a…ab17a', detail: 'PR #666 candidate installed; journals and backend claim inventory were empty before processing resumed.' }
  ];

  const widerRows = [
    { component: '3pool', hash: 'c17b4e71…6117615', detail: 'Source-bound October 23 release. Current pool totals are readable, with zero pending claims and 561 retained donation receipts.' },
    { component: 'Points', hash: '352e269e…0aacf9', detail: 'Epoch 18 is closed through delayed legacy-B recovery. The installed artifact is an exact hash match and PR #608 is the strongest source candidate; the source-to-build chain remains partial.' },
    { component: 'Cycle Sentinel', hash: 'a863dacb…0afad8', detail: 'No active public self-recovery delivery risk. The installed hash exactly matches the preserved production artifact built from PR #662 head 5f5ff064.' }
  ];

  const fixes = [
    'Analytics overlap and retained-event cursor safety',
    'Redemption-fee decay correction',
    'Liquidation-bot oracle floor',
    'High-decimal conversion safety',
    'Bot debt and interest alignment',
    'Legacy ambiguous Stability Pool burn compensation',
    'Zero-debt accrual-clock correction',
    'Commit-time liquidation excess gate',
    'Treasury transfer-receipt accounting journal',
    'Native-XRP admission and late-funding tombstones',
    'Held protocol-payout recovery with post-await revalidation',
    'Wallet-session binding, exact repayment amounts, and partial-close recovery',
    'Patched Rust dependency graph and exact-head RustSec gate'
  ];

  const followThrough = [
    'Keep the legacy Stability Pool burn case held until exact authenticated archive proof exists.',
    'Preserve the 60 fail-closed Treasury tombstones for legacy rows that lack exact ledger provenance.',
    'Complete the remaining Points source-to-build provenance record.',
    'Install and read back the held Stability Pool dependency-refresh candidate only after the interrupted install path has an authoritative disposition.',
    'Exercise the published wallet-session protections with real extension account changes and multi-tab behavior.',
    'Continue periodic bounded inventory checks; an empty current queue is not proof that no unknown historical obligation exists.'
  ];
</script>

<svelte:head>
  <title>Security Remediation Closure · Rumi Protocol</title>
  <meta name="description" content="Rumi Protocol security remediation status, production module evidence, recovery state, and remaining release follow-through as of November 15, 2026." />
</svelte:head>

<main>
  <section class="hero" aria-labelledby="report-title">
    <div class="page-width hero-inner">
      <p class="eyebrow"><span aria-hidden="true"></span> Rumi Protocol / Security report</p>
      <div class="hero-grid">
        <div>
          <p class="date-line">Evidence snapshot · <time datetime="2026-11-15">November 15, 2026</time></p>
          <h1 id="report-title">Remediation merged.<br /><em>Rollout verified.</em></h1>
          <p class="lede">
            The accepted active-path remediation tranche and PR #666 are merged. The latest
            dependency refresh is installed on the backend, bot, AMM, and Analytics, and the
            wallet-session frontend is published. The Stability Pool refresh remains explicitly
            held after interrupted install requests.
          </p>
        </div>
        <aside class="verdict">
          <span>Executive verdict</span>
          <strong>Core remediation is live; one dependency refresh remains held.</strong>
          <p>The prior remediated Stability Pool build remains healthy. Historical proof gaps remain held or fail-closed, and empty inventories are bounded evidence rather than proof that unknown obligations never existed.</p>
          <a href="/security">Security archive <span aria-hidden="true">→</span></a>
        </aside>
      </div>
      <div class="hero-rule"><span>Canonical source f72ede81</span><span>Release tree 18b286ab</span></div>
    </div>
  </section>

  <section class="page-width section" aria-labelledby="stages-title">
    <header class="section-heading">
      <p class="section-label">Evidence model</p>
      <h2 id="stages-title">Four evidence checkpoints shown here</h2>
      <p>A result at one stage does not establish the next.</p>
    </header>
    <div class="stage-grid">
      {#each stages as stage}
        <article class="stage stage-{stage.tone}">
          <span>{stage.label}</span>
          <h3>{stage.value}</h3>
          <p>{stage.detail}</p>
        </article>
      {/each}
    </div>
  </section>

  <section class="release-section" aria-labelledby="release-title">
    <div class="page-width section">
      <header class="section-heading wide">
        <p class="section-label">Production snapshot</p>
        <h2 id="release-title">The merged tranche is live; PR #666 is four-fifths installed</h2>
        <p>Module hashes were read through the management interface. Candidate hashes remain build evidence until the same bytes are observed in production.</p>
      </header>
      <ul class="release-table" aria-label="Production module status">
        {#each releaseRows as row}
          <li class="release-row">
            <div class="component"><strong>{row.component}</strong><span class="state state-{row.tone}">{row.state}</span></div>
            <code>{row.hash}</code>
            <p>{row.detail}</p>
          </li>
        {/each}
      </ul>
    </div>
  </section>

  <section class="page-width section" aria-labelledby="wider-title">
    <header class="section-heading wide">
      <p class="section-label">Wider review</p>
      <h2 id="wider-title">Three components outside this release tranche</h2>
      <p>These readbacks address older CL findings without pretending that a current hash automatically proves source provenance.</p>
    </header>
    <div class="wider-list">
      {#each widerRows as row}
        <article>
          <div><h3>{row.component}</h3><code>{row.hash}</code></div>
          <p>{row.detail}</p>
        </article>
      {/each}
    </div>
  </section>

  <section class="fixes-section" aria-labelledby="fixes-title">
    <div class="page-width section fixes-grid">
      <header>
        <p class="section-label">Merged source</p>
        <h2 id="fixes-title">PRs #652–#662 and #666</h2>
        <p>Concise remediation areas are listed here without exploit instructions.</p>
      </header>
      <ol class="fix-list">
        {#each fixes as fix, index}
          <li><span>{String(index + 1).padStart(2, '0')}</span>{fix}</li>
        {/each}
      </ol>
    </div>
  </section>

  <section class="page-width section" aria-labelledby="recovery-title">
    <div class="recovery-grid">
      <div>
        <p class="section-label">Recovery evidence</p>
        <h2 id="recovery-title">Named inventories are clean</h2>
      </div>
      <div class="recovery-copy">
        <p>All 237 staged Stability Pool interest packets are credited, and the pool reports a consistent state with no pending chain absorbs or unallocated interest forwards.</p>
        <p>Treasury’s external-token backfill is complete with 60 fail-closed tombstones for legacy rows lacking exact ledger provenance. The queried backend recovery pages, bot claim and payment journals, and AMM unresolved payout inventory are empty.</p>
        <p class="limit">These are endpoint-limited observations at one time. They do not prove that no unindexed historical obligation exists.</p>
      </div>
    </div>
  </section>

  <section class="follow-section" aria-labelledby="follow-title">
    <div class="page-width section follow-grid">
      <header>
        <p class="section-label">Remaining follow-through</p>
        <h2 id="follow-title">Residual evidence and recovery holds</h2>
      </header>
      <ol>
        {#each followThrough as item, index}
          <li><span>{index + 1}</span><p>{item}</p></li>
        {/each}
      </ol>
    </div>
  </section>

  <section class="page-width section disclosure" aria-labelledby="disclosure-title">
    <div>
      <p class="section-label">Responsible disclosure</p>
      <h2 id="disclosure-title">Report vulnerabilities privately</h2>
      <p>Please include the affected component and enough detail to investigate safely. Do not include user secrets.</p>
    </div>
    <nav aria-label="Security contacts">
      <a href="mailto:info@rumiprotocol.com">Email the security contact <span aria-hidden="true">↗</span></a>
      <a href="https://github.com/RumiLabsXYZ/rumi-protocol-v2/security/advisories" target="_blank" rel="noopener noreferrer">Open a private advisory <span aria-hidden="true">↗</span></a>
    </nav>
  </section>
</main>

<style>
  main { color: var(--rumi-text-primary); }
  .page-width { width: min(1120px, calc(100% - 3rem)); margin-inline: auto; }
  a { color: var(--rumi-teal-bright); text-decoration: none; text-underline-offset: 4px; }
  a:hover { text-decoration: underline; }
  a:focus-visible { outline: 2px solid var(--rumi-teal-bright); outline-offset: 4px; }
  .hero { border-bottom: 1px solid var(--rumi-border); background: radial-gradient(ellipse at 10% 0%, color-mix(in srgb, var(--rumi-purple-accent) 17%, transparent), transparent 52%), radial-gradient(ellipse at 90% 75%, color-mix(in srgb, var(--rumi-teal) 8%, transparent), transparent 42%), var(--rumi-bg-primary); }
  .hero-inner { padding-block: clamp(3.5rem, 8vw, 6.5rem) 1.4rem; }
  .eyebrow, .section-label { margin: 0 0 1rem; color: var(--rumi-teal); font-size: .7rem; font-weight: 700; letter-spacing: .14em; text-transform: uppercase; }
  .eyebrow { display: flex; align-items: center; gap: .65rem; }
  .eyebrow span { width: .55rem; height: .55rem; border: 2px solid var(--rumi-teal); border-radius: 50%; }
  .hero-grid { display: grid; grid-template-columns: minmax(0, 1.45fr) minmax(270px, .72fr); align-items: end; gap: clamp(2rem, 6vw, 5rem); }
  .date-line { margin: 0 0 1rem; color: var(--rumi-text-muted); font-size: .84rem; }
  h1 { margin: 0; max-width: 760px; font-size: clamp(2.8rem, 6.3vw, 5.3rem); line-height: .98; letter-spacing: -.06em; }
  h1 em { color: var(--rumi-purple-accent); font-style: normal; }
  .lede { max-width: 710px; margin: 1.5rem 0 0; color: var(--rumi-text-secondary); font-size: 1.04rem; line-height: 1.75; }
  .verdict { padding: 1.25rem; border: 1px solid color-mix(in srgb, #e7a94c 32%, var(--rumi-border)); border-top: 3px solid #e7a94c; background: color-mix(in srgb, var(--rumi-bg-surface1) 88%, transparent); }
  .verdict > span { display: block; margin-bottom: .7rem; color: #e7bb72; font-size: .65rem; font-weight: 700; letter-spacing: .12em; text-transform: uppercase; }
  .verdict strong { display: block; font-size: 1.12rem; line-height: 1.4; }
  .verdict p { margin: .7rem 0 1rem; color: var(--rumi-text-muted); font-size: .82rem; line-height: 1.65; }
  .verdict a { font-size: .8rem; font-weight: 650; }
  .hero-rule { display: flex; justify-content: space-between; gap: 1rem; margin-top: clamp(2.8rem, 6vw, 4.5rem); padding-top: .8rem; border-top: 1px solid var(--rumi-border); color: var(--rumi-text-muted); font-size: .72rem; }
  .section { padding-block: clamp(3.5rem, 7vw, 5.5rem); }
  .section-heading { max-width: 700px; margin-bottom: 1.8rem; }
  .section-heading.wide { max-width: 790px; }
  h2 { margin: 0; font-size: clamp(1.85rem, 3.5vw, 2.8rem); line-height: 1.08; letter-spacing: -.045em; }
  .section-heading > p:last-child, .fixes-grid header > p:last-child, .disclosure > div > p:last-child { margin: .8rem 0 0; color: var(--rumi-text-muted); line-height: 1.7; }
  .stage-grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); border: 1px solid var(--rumi-border); }
  .stage { min-height: 180px; padding: 1.1rem; border-right: 1px solid var(--rumi-border); background: color-mix(in srgb, var(--rumi-bg-surface1) 66%, transparent); }
  .stage:last-child { border-right: 0; }
  .stage > span { color: var(--rumi-text-muted); font-size: .66rem; font-weight: 700; letter-spacing: .1em; text-transform: uppercase; }
  .stage h3 { margin: 1.4rem 0 .45rem; font-size: 1.3rem; }
  .stage p { margin: 0; color: var(--rumi-text-secondary); font-size: .82rem; line-height: 1.65; }
  .stage-done h3 { color: var(--rumi-teal-bright); }
  .stage-partial h3 { color: #edc77f; }
  .release-section, .follow-section { border-block: 1px solid var(--rumi-border); background: color-mix(in srgb, var(--rumi-bg-surface1) 42%, var(--rumi-bg-primary)); }
  .release-table { margin: 0; padding: 0; border-top: 1px solid var(--rumi-border); list-style: none; }
  .release-row { display: grid; grid-template-columns: minmax(180px, .6fr) minmax(150px, .55fr) 1.35fr; gap: 1.1rem; align-items: center; min-height: 92px; padding: 1rem .2rem; border-bottom: 1px solid var(--rumi-border); }
  .component { display: flex; align-items: center; gap: .7rem; }
  .component strong { font-size: .92rem; }
  .state { padding: .18rem .45rem; border: 1px solid var(--rumi-border); border-radius: 999px; font-size: .6rem; font-weight: 700; letter-spacing: .07em; text-transform: uppercase; }
  .state-installed { color: var(--rumi-teal-bright); border-color: color-mix(in srgb, var(--rumi-teal) 35%, var(--rumi-border)); }
  .state-held { color: #edc77f; border-color: color-mix(in srgb, #e7a94c 35%, var(--rumi-border)); }
  code { color: #c5b3ff; font-size: .76rem; overflow-wrap: anywhere; }
  .release-row p { margin: 0; color: var(--rumi-text-secondary); font-size: .82rem; line-height: 1.6; }
  .wider-list { border-top: 1px solid var(--rumi-border); }
  .wider-list article { display: grid; grid-template-columns: minmax(190px, .55fr) 1.45fr; gap: 2rem; padding: 1.25rem .2rem; border-bottom: 1px solid var(--rumi-border); }
  .wider-list h3 { margin: 0 0 .45rem; font-size: 1.05rem; }
  .wider-list p { margin: 0; color: var(--rumi-text-secondary); line-height: 1.7; }
  .fixes-section { border-block: 1px solid var(--rumi-border); background: linear-gradient(110deg, color-mix(in srgb, var(--rumi-purple-accent) 7%, var(--rumi-bg-primary)), var(--rumi-bg-primary) 58%); }
  .fixes-grid, .follow-grid, .recovery-grid { display: grid; grid-template-columns: minmax(220px, .65fr) 1fr; gap: clamp(2rem, 7vw, 5rem); align-items: start; }
  .fix-list { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); margin: 0; padding: 0; border-top: 1px solid var(--rumi-border); list-style: none; }
  .fix-list li { display: flex; gap: .8rem; min-height: 64px; padding: .95rem .65rem; border-bottom: 1px solid var(--rumi-border); color: var(--rumi-text-secondary); font-size: .84rem; line-height: 1.55; }
  .fix-list span { flex: none; color: var(--rumi-teal); font-size: .65rem; font-weight: 700; letter-spacing: .08em; }
  .recovery-copy p { margin: 0 0 .9rem; color: var(--rumi-text-secondary); line-height: 1.75; }
  .recovery-copy .limit { padding-left: 1rem; border-left: 2px solid #e7a94c; color: var(--rumi-text-muted); font-size: .88rem; }
  .follow-grid > ol { margin: 0; padding: 0; list-style: none; }
  .follow-grid li { display: grid; grid-template-columns: 2rem 1fr; gap: .8rem; padding: .85rem 0; border-bottom: 1px solid var(--rumi-border); }
  .follow-grid li:first-child { border-top: 1px solid var(--rumi-border); }
  .follow-grid li > span { color: var(--rumi-teal); font-size: .7rem; font-weight: 700; }
  .follow-grid li p { margin: 0; color: var(--rumi-text-secondary); line-height: 1.65; }
  .disclosure { display: grid; grid-template-columns: 1fr minmax(280px, .7fr); gap: 3rem; align-items: center; }
  .disclosure nav { padding-inline: 1.2rem; border: 1px solid color-mix(in srgb, var(--rumi-teal) 25%, var(--rumi-border)); background: var(--rumi-bg-surface1); }
  .disclosure nav a { display: flex; justify-content: space-between; gap: 1rem; padding: .95rem 0; border-bottom: 1px solid var(--rumi-border); font-size: .82rem; font-weight: 600; }
  .disclosure nav a:last-child { border-bottom: 0; color: var(--rumi-text-secondary); font-weight: 500; }
  @media (max-width: 820px) {
    .hero-grid, .fixes-grid, .follow-grid, .recovery-grid, .disclosure { grid-template-columns: 1fr; gap: 1.6rem; }
    .stage-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .stage:nth-child(2) { border-right: 0; }
    .stage:nth-child(-n + 2) { border-bottom: 1px solid var(--rumi-border); }
    .release-row { grid-template-columns: 1fr auto; }
    .release-row p { grid-column: 1 / -1; }
  }
  @media (max-width: 540px) {
    .page-width { width: min(100% - 2rem, 600px); }
    .hero-rule { flex-direction: column; gap: .4rem; }
    .stage-grid, .fix-list { grid-template-columns: 1fr; }
    .stage, .stage:nth-child(2) { border-right: 0; border-bottom: 1px solid var(--rumi-border); }
    .stage:last-child { border-bottom: 0; }
    .release-row, .wider-list article { grid-template-columns: 1fr; gap: .65rem; }
    .component { justify-content: space-between; }
  }
</style>
