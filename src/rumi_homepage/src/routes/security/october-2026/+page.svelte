<script>
  const report = {
    date: 'October 8, 2026',
    datetime: '2026-10-08',
    source: 'origin/main at 50245136d8c2',
    snapshot: 'Mainnet readback · October 8, 2026'
  };

  const states = [
    { name: 'Merged source', meaning: 'The change is present on the repository main branch.' },
    { name: 'Installed', meaning: 'The canister reports the module hash shown in this snapshot.' },
    { name: 'Paused', meaning: 'The live system reports its emergency pause as active.' },
    { name: 'Not installed', meaning: 'The mainnet canister still reports its earlier module.' }
  ];

  const changes = [
    {
      pr: '#466',
      title: '3pool upgrade and state preservation',
      summary: 'Merged into main and installed on October 8. The rumi_3pool canister (fohh4-yyaaa-aaaap-qtkpa-cai) reports module SHA-256 b6e5e2eab401c8e1551a33acc134369a92744efb769ed4e34a7feb9923a7a1af. The upgrade readback recorded unchanged LP supply, reserves, admin fees, and virtual price, with zero pending claims.',
      state: 'Installed · readback recorded',
      tone: 'installed',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/466'
    },
    {
      pr: '#459',
      title: 'Stability Pool containment',
      summary: 'The rumi_stability_pool canister (tmhzi-dqaaa-aaaap-qrd6q-cai) is emergency-paused. Its module remains 54245b779feb30c4da7ac3f60e9cb62afb99f1accd2ea5affc7adda8a28e4754; the upgrade is pending. The pause snapshot recorded 13 depositors and an unchanged aggregate balance.',
      state: 'Paused · upgrade pending',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/459'
    },
    {
      pr: '#455',
      title: 'Bot upgrade pause',
      summary: 'Source is merged. The liquidation_bot canister (nygob-3qaaa-aaaap-qttcq-cai) still reports its earlier module hash, f336ccf377d16d6ab675fe958205ba9fa3a799da4daf6d21b2d08f26871235c6.',
      state: 'Merged · not installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/455'
    },
    {
      pr: '#456',
      title: 'P08 account migration',
      summary: 'Source is merged. The rumi_protocol_backend canister (tfesu-vyaaa-aaaap-qrd7a-cai) still reports its earlier module hash, 1714712f12525a5058c288bde8f456b09e2e893e4ac51fe7a82992ac07b0ecf1. This row does not imply installation on the other affected canisters.',
      state: 'Merged · backend not installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/456'
    },
    {
      pr: '#457',
      title: 'P08 populated-state upgrade proof',
      summary: 'The source and upgrade evidence are merged. The live backend remains at the earlier module hash shown above; this report makes no claim that the change is installed.',
      state: 'Merged · backend not installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/457'
    },
    {
      pr: '#458',
      title: 'BOT-10 recovery guard',
      summary: 'Source is merged. The live backend and liquidation bot still report their earlier module hashes shown above; this report makes no claim that either change is installed.',
      state: 'Merged · not installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/458'
    },
    {
      pr: '#460',
      title: 'CL-02 collateral-return proof',
      summary: 'Source is merged. The live backend still reports its earlier module hash shown above; this report makes no claim that the change is installed.',
      state: 'Merged · backend not installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/460'
    }
  ];

  const limits = [
    {
      system: 'Points',
      status: 'Not assessed in this snapshot',
      detail: 'This update has no current Points canister or epoch readback, so it makes no claim about the active epoch, driver, or awards.'
    },
    {
      system: 'Scope',
      status: 'Selected status only',
      detail: 'These entries cover named merge and release states. They do not establish that every finding is fixed, every component is current, or the protocol has received a formal third-party audit.'
    }
  ];
</script>

<svelte:head>
  <title>October 2026 Security Status · Rumi Protocol</title>
  <meta
    name="description"
    content="A dated Rumi Protocol security follow-up report separating merged source, installed modules, and current mainnet pause status."
  />
</svelte:head>

<main>
  <section class="masthead">
    <div class="page-width masthead-inner">
      <p class="eyebrow"><span class="status-mark" aria-hidden="true"></span> Rumi Protocol / Security notes</p>
      <div class="hero-grid">
        <div class="hero-copy">
          <p class="issue-line">Status report <span>—</span> <time datetime={report.datetime}>{report.date}</time></p>
          <h1>Security update,<br /><span>where things stand.</span></h1>
          <p class="lede">
            This snapshot separates merged source from mainnet installation. The 3pool upgrade
            is installed with recorded state invariants; the Stability Pool remains paused, and
            backend and bot follow-up changes are not yet installed.
          </p>
        </div>
        <aside class="report-stamp" aria-label={report.snapshot}>
          <span class="stamp-label">Publication state</span>
          <strong>{report.snapshot}</strong>
          <p>The module hashes and state notes below come from a controller-authorized canister readback recorded by the release operator on October 8. This page is a dated snapshot, not live telemetry.</p>
          <a href="/security">View security archive <span aria-hidden="true">→</span></a>
        </aside>
      </div>
      <div class="rule-caption"><span>Source cutoff · {report.source} · October 8, 2026</span><span>Selected remediation and release follow-up</span></div>
    </div>
  </section>

  <section class="page-width section evidence-section" aria-labelledby="evidence-title">
    <div class="section-heading">
      <p class="section-label">How to read this page</p>
      <h2 id="evidence-title">Each stage needs its own proof.</h2>
      <p>A merged source change does not establish installation. Module hashes and the pause state are reported separately from source history.</p>
    </div>
    <div class="state-key">
      {#each states as item}
        <article class="key-item">
          <span class="badge" class:badge-held={item.name === 'Not installed' || item.name === 'Paused'} class:badge-installed={item.name === 'Installed'}>{item.name}</span>
          <p>{item.meaning}</p>
        </article>
      {/each}
    </div>
  </section>

  <section class="updates-section" aria-labelledby="updates-title">
    <div class="page-width section">
      <div class="section-heading section-heading-wide">
        <p class="section-label">Selected changes</p>
        <h2 id="updates-title">Change and release status</h2>
        <p>This October 8 snapshot reports the highest evidence stage available for each change. A source merge and a live installation remain separate states.</p>
      </div>
      <div class="change-list">
        {#each changes as item}
          <article class="change-row">
            <div class="change-ref"><span>{item.pr}</span><span class="change-rule" aria-hidden="true"></span></div>
            <div class="change-main">
              <h3>{item.title}</h3>
              <p>{item.summary}</p>
              <a href={item.href} target="_blank" rel="noopener noreferrer">Review public source record <span aria-hidden="true">↗</span></a>
            </div>
            <div class="change-state">
              <span class="badge" class:badge-held={item.tone === 'held'} class:badge-installed={item.tone === 'installed'}>{item.state}</span>
            </div>
          </article>
        {/each}
      </div>
    </div>
  </section>

  <section class="page-width section held-section" aria-labelledby="held-title">
    <div class="section-heading">
      <p class="section-label">Scope and limits</p>
      <h2 id="held-title">What this snapshot does not establish</h2>
      <p>This is a selected status update, not a full finding inventory.</p>
    </div>
    <div class="held-list">
      {#each limits as item}
        <article class="held-row">
          <div class="held-heading"><h3>{item.system}</h3><span class="badge badge-held">{item.status}</span></div>
          <p>{item.detail}</p>
        </article>
      {/each}
    </div>
  </section>

  <section class="limits-band" aria-labelledby="limits-title">
    <div class="page-width limits-inner">
      <div>
        <p class="section-label">Scope</p>
        <h2 id="limits-title">A status record, not a clearance.</h2>
      </div>
      <p>
        Source review, tests, builds, installation, activation, and live behavior are distinct claims.
        This page records an October 8 snapshot; it does not publish private audit material or
        establish that every finding is closed.
      </p>
    </div>
  </section>

  <footer class="page-width page-foot">
    <span>Evidence date: <time datetime={report.datetime}>{report.date}</time></span>
    <a href="/security">Back to security reviews <span aria-hidden="true">↗</span></a>
  </footer>
</main>

<style>
  main { color: var(--rumi-text-primary); --report-muted: #a09bb5; }
  .page-width { width: min(1080px, calc(100% - 3rem)); margin-inline: auto; }
  .masthead { position: relative; overflow: hidden; border-bottom: 1px solid var(--rumi-border); background: linear-gradient(115deg, color-mix(in srgb, var(--rumi-purple-accent) 9%, var(--rumi-bg-primary)), var(--rumi-bg-primary) 48%, color-mix(in srgb, #e7a94c 5%, var(--rumi-bg-primary))); }
  .masthead-inner { padding-block: clamp(3rem, 7vw, 6.5rem) 1.5rem; }
  .eyebrow, .section-label { margin: 0 0 1rem; color: var(--rumi-teal); font-size: .73rem; font-weight: 700; letter-spacing: .13em; text-transform: uppercase; }
  .eyebrow { display: flex; align-items: center; gap: .7rem; }
  .status-mark { width: .55rem; height: .55rem; border: 2px solid var(--rumi-teal); border-radius: 50%; }
  .hero-grid { display: grid; grid-template-columns: minmax(0, 1.5fr) minmax(245px, .65fr); align-items: end; gap: clamp(2rem, 7vw, 6rem); }
  .issue-line { margin: 0 0 1rem; color: var(--report-muted); font-size: .88rem; }
  .issue-line span { padding-inline: .4rem; color: var(--rumi-purple-accent); }
  .issue-line time { color: var(--rumi-text-secondary); }
  h1 { max-width: 740px; margin: 0; font-size: clamp(2.75rem, 6.4vw, 5.25rem); line-height: .98; letter-spacing: -.065em; }
  h1 span { color: var(--rumi-purple-accent); }
  .lede { max-width: 670px; margin: 1.6rem 0 0; color: var(--rumi-text-secondary); font-size: clamp(1rem, 1.6vw, 1.16rem); line-height: 1.75; }
  .report-stamp { padding: 1.2rem 1.25rem; border: 1px solid color-mix(in srgb, #e7a94c 32%, var(--rumi-border)); border-top: 3px solid #e7a94c; background: color-mix(in srgb, var(--rumi-bg-surface1) 88%, transparent); }
  .stamp-label { display: block; margin-bottom: .65rem; color: #e7bb72; font-size: .68rem; font-weight: 700; letter-spacing: .12em; text-transform: uppercase; }
  .report-stamp strong { display: block; font-size: 1.12rem; line-height: 1.4; }
  .report-stamp p { margin: .65rem 0 1rem; color: var(--report-muted); font-size: .82rem; line-height: 1.6; }
  .report-stamp a, .change-main a, .page-foot a { color: var(--rumi-teal-bright); text-decoration: none; text-underline-offset: 4px; }
  .report-stamp a:hover, .change-main a:hover, .page-foot a:hover { text-decoration: underline; }
  .rule-caption { display: flex; justify-content: space-between; gap: 1rem; margin-top: clamp(3rem, 7vw, 5.8rem); padding-block: .8rem; border-top: 1px solid var(--rumi-border); color: var(--report-muted); font-size: .73rem; }
  .rule-caption span:first-child { color: var(--rumi-text-secondary); font-weight: 700; }
  .section { padding-block: clamp(3.5rem, 7vw, 6rem); }
  .section-heading { max-width: 680px; margin-bottom: 2rem; }
  .section-label { margin-bottom: .7rem; }
  .section-heading h2, .limits-inner h2 { margin: 0; font-size: clamp(1.8rem, 3.5vw, 2.75rem); line-height: 1.1; letter-spacing: -.045em; }
  .section-heading > p:last-child { margin: .85rem 0 0; color: var(--report-muted); line-height: 1.7; }
  .state-key { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); border-top: 1px solid var(--rumi-border); border-left: 1px solid var(--rumi-border); }
  .key-item { min-height: 118px; padding: 1rem; border-right: 1px solid var(--rumi-border); border-bottom: 1px solid var(--rumi-border); background: color-mix(in srgb, var(--rumi-bg-surface1) 58%, transparent); }
  .key-item p { max-width: 250px; margin: .75rem 0 0; color: var(--report-muted); font-size: .8rem; line-height: 1.55; }
  .badge { display: inline-flex; align-items: center; width: fit-content; min-height: 1.7rem; padding: .28rem .55rem; border: 1px solid color-mix(in srgb, var(--rumi-purple-accent) 35%, var(--rumi-border)); border-radius: 999px; color: #c5b3ff; background: color-mix(in srgb, var(--rumi-purple-accent) 10%, transparent); font-size: .68rem; font-weight: 700; line-height: 1.25; }
  .badge-held { border-color: rgba(231,169,76,.38); color: #edc77f; background: rgba(231,169,76,.09); }
  .badge-installed { border-color: color-mix(in srgb, var(--rumi-teal) 38%, var(--rumi-border)); color: var(--rumi-teal-bright); background: color-mix(in srgb, var(--rumi-teal) 9%, transparent); }
  .updates-section { border-block: 1px solid var(--rumi-border); background: color-mix(in srgb, var(--rumi-bg-surface1) 45%, var(--rumi-bg-primary)); }
  .section-heading-wide { max-width: 760px; }
  .change-list { border-top: 1px solid var(--rumi-border); }
  .change-row { display: grid; grid-template-columns: 100px minmax(0, 1fr) minmax(150px, auto); gap: 1.5rem; padding-block: 1.55rem; border-bottom: 1px solid var(--rumi-border); }
  .change-ref { display: flex; align-items: flex-start; gap: .7rem; color: var(--rumi-teal); font-size: .82rem; font-weight: 700; }
  .change-rule { width: 1px; min-height: 32px; background: color-mix(in srgb, var(--rumi-teal) 44%, var(--rumi-border)); }
  .change-main h3, .held-heading h3 { margin: 0; font-size: 1.08rem; letter-spacing: -.015em; }
  .change-main p, .held-row > p { max-width: 700px; margin: .55rem 0 .65rem; color: var(--rumi-text-secondary); font-size: .87rem; line-height: 1.7; overflow-wrap: anywhere; }
  .change-main a { font-size: .78rem; font-weight: 600; }
  .change-state { display: flex; flex-direction: column; align-items: flex-end; gap: .45rem; }
  .held-list { border-top: 1px solid var(--rumi-border); }
  .held-row { padding-block: 1.25rem; border-bottom: 1px solid var(--rumi-border); }
  .held-heading { display: flex; align-items: center; justify-content: space-between; gap: 1rem; }
  .held-row > p { margin-bottom: 0; }
  .limits-band { border-block: 1px solid var(--rumi-border); background: linear-gradient(105deg, color-mix(in srgb, var(--rumi-purple-accent) 8%, var(--rumi-bg-primary)), var(--rumi-bg-primary) 55%); }
  .limits-inner { display: grid; grid-template-columns: minmax(230px, .7fr) 1fr; gap: clamp(2rem, 7vw, 6rem); align-items: start; padding-block: clamp(2.5rem, 5vw, 4rem); }
  .limits-inner > p { max-width: 620px; margin: 0; color: var(--rumi-text-secondary); line-height: 1.8; }
  .page-foot { display: flex; justify-content: space-between; gap: 1rem; padding-block: 1.2rem 2rem; color: var(--report-muted); font-size: .75rem; }
  a:focus-visible { outline: 2px solid var(--rumi-teal-bright); outline-offset: 4px; border-radius: 2px; }
  @media (max-width: 760px) {
    .page-width { width: min(100% - 2rem, 610px); }
    .hero-grid, .limits-inner { grid-template-columns: 1fr; gap: 1.75rem; }
    .report-stamp { max-width: 480px; }
    .rule-caption { flex-direction: column; gap: .3rem; }
    .state-key { grid-template-columns: repeat(2, minmax(0, 1fr)); }
    .change-row { grid-template-columns: 72px minmax(0, 1fr); gap: .9rem; }
    .change-state { grid-column: 2; align-items: flex-start; }
    .held-heading { align-items: flex-start; flex-direction: column; gap: .65rem; }
    .page-foot { flex-direction: column; }
  }
  @media (max-width: 430px) {
    .state-key { grid-template-columns: 1fr; }
  }
</style>
