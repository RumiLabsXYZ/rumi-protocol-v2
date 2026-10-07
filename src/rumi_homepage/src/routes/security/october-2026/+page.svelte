<script>
  const evidence = [
    {
      state: 'Merged source',
      title: 'PR #421 is merged',
      detail:
        'The CL-01 3pool hotfix is present on main. Merge status describes source history; it does not mean the production canister has been upgraded.',
      tone: 'merged',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/421',
      link: 'View PR #421'
    },
    {
      state: 'Not installed',
      title: 'The PR #421 artifact is not live',
      detail:
        'As of October 7, 2026, the artifact had not been installed. This report makes no claim that the corresponding change is active on mainnet.',
      tone: 'pending'
    },
    {
      state: 'Draft source',
      title: 'PR #422 is unmerged and not deployable',
      detail:
        'The broader security-remediation work remains a draft pull request. Its source and review progress do not establish a release artifact or a mainnet change.',
      tone: 'draft',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/422',
      link: 'View draft PR #422'
    }
  ];
</script>

<svelte:head>
  <title>October 2026 Security Review Status · Rumi Protocol</title>
  <meta
    name="description"
    content="Public October 7, 2026 update on Rumi Protocol security-review and release status, with source, merge, and mainnet installation clearly separated."
  />
</svelte:head>

<main>
  <section class="hero">
    <div class="page-width hero-inner">
      <p class="eyebrow"><span class="eyebrow-mark" aria-hidden="true"></span> Security status report</p>
      <div class="hero-grid">
        <div>
          <h1>October review.<br /><span>Current status.</span></h1>
          <p class="hero-copy">
            This public update records the state of selected security follow-up and release work
            on October 7, 2026. It distinguishes code changes from changes installed on mainnet.
          </p>
          <p class="review-date">Status date <time datetime="2026-10-07">October 7, 2026</time></p>
        </div>

        <aside class="status-card" aria-labelledby="status-title">
          <div class="status-card-top">
            <span class="status-dot" aria-hidden="true"></span>
            <span>Release status</span>
            <span class="status-label">Follow-up ongoing</span>
          </div>
          <h2 id="status-title">No October remediation is confirmed live</h2>
          <p>
            PR #421 is merged, but its artifact was not installed as of this report date. The
            broader draft PR #422 is unmerged and not deployable. The Points pause policy is not
            live.
          </p>
          <div class="status-divider"></div>
          <p class="status-footnote">
            Source review, merge, artifact production, canister installation, and live behavior
            are separate evidence states. This page only claims the states listed below.
          </p>
        </aside>
      </div>

      <div class="evidence-rail" aria-label="Security work evidence states">
        <div class="evidence-step published">
          <span class="step-icon" aria-hidden="true">01</span>
          <div><strong>Source work</strong><span>Changes and review history</span></div>
        </div>
        <span class="rail-line" aria-hidden="true"></span>
        <div class="evidence-step working">
          <span class="step-icon" aria-hidden="true">02</span>
          <div><strong>Release state</strong><span>Merge and artifact status</span></div>
        </div>
        <span class="rail-line" aria-hidden="true"></span>
        <div class="evidence-step live">
          <span class="step-icon" aria-hidden="true">03</span>
          <div><strong>Mainnet</strong><span>Installation is a separate step</span></div>
        </div>
      </div>
    </div>
  </section>

  <section class="section page-width" aria-labelledby="evidence-title">
    <div class="section-heading">
      <p class="section-kicker">What is known</p>
      <h2 id="evidence-title">Three different release states</h2>
      <p>
        Each statement below is limited to the state recorded on the report date. A code change
        alone does not establish an installed or active safeguard.
      </p>
    </div>

    <div class="state-grid">
      {#each evidence as item}
        <article class="state-card" class:state-source={item.tone === 'draft'} class:state-live={item.tone === 'pending'}>
          <span class="state-icon" aria-hidden="true">{item.tone === 'merged' ? '✓' : item.tone === 'draft' ? '◷' : '—'}</span>
          <p class="state-label">{item.state}</p>
          <h3>{item.title}</h3>
          <p>{item.detail}</p>
          {#if item.href}
            <a class="evidence-link" href={item.href} target="_blank" rel="noopener noreferrer">
              {item.link} <span aria-hidden="true">↗</span>
            </a>
          {/if}
        </article>
      {/each}
    </div>
  </section>

  <section class="release-section" aria-labelledby="points-title">
    <div class="page-width release-inner">
      <div class="section-heading release-heading">
        <p class="section-kicker">Points</p>
        <h2 id="points-title">The pause policy is not live</h2>
        <p>
          The proposed Points pause policy must not be treated as an operating control. This
          report does not claim that Points activity can currently be paused under that policy.
        </p>
      </div>
      <p class="release-note">
        A policy described in source or review notes is not evidence of an active mainnet
        safeguard. Live behavior requires separate verification after implementation and release.
      </p>
    </div>
  </section>

  <section class="section page-width" aria-labelledby="limits-title">
    <div class="section-heading">
      <p class="section-kicker">Scope and limits</p>
      <h2 id="limits-title">What this update does not establish</h2>
    </div>
    <ul class="limits-list">
      <li>It does not claim that all security findings are fixed or closed.</li>
      <li>It does not claim that PR #421 has been installed or is active on mainnet.</li>
      <li>It does not treat draft PR #422 as merged, release-ready, or deployable.</li>
      <li>It does not claim that the Points pause policy is live.</li>
      <li>It is a high-level status summary, not publication of the private audit package or exploit details.</li>
    </ul>
    <p class="source-note">
      The public review archive and responsible-disclosure contact are on the
      <a href="/security">Security page</a>. For the public source history, see the
      <a href="https://github.com/RumiLabsXYZ/rumi-protocol-v2" target="_blank" rel="noopener noreferrer">Rumi Protocol repository</a>.
    </p>
  </section>

  <footer class="page-width page-foot">
    <span>Report date: October 7, 2026.</span>
    <a href="/security">Back to security reviews <span aria-hidden="true">↗</span></a>
  </footer>
</main>

<style>
  main { color: var(--rumi-text-primary); }
  .page-width { width: min(1120px, calc(100% - 3rem)); margin-inline: auto; }
  .hero { position: relative; overflow: hidden; border-bottom: 1px solid var(--rumi-border); background: radial-gradient(ellipse at 12% 0%, color-mix(in srgb, var(--rumi-purple-accent) 16%, transparent), transparent 52%), radial-gradient(ellipse at 92% 70%, color-mix(in srgb, var(--rumi-action) 8%, transparent), transparent 38%), var(--rumi-bg-primary); }
  .hero-inner { padding-top: clamp(4rem, 8vw, 7rem); padding-bottom: 2.75rem; }
  .eyebrow, .section-kicker { display: flex; align-items: center; gap: .65rem; margin: 0 0 1rem; color: var(--rumi-purple-accent); font-size: .72rem; font-weight: 700; letter-spacing: .16em; text-transform: uppercase; }
  .eyebrow-mark { width: .5rem; height: .5rem; border-radius: 50%; background: var(--rumi-purple-accent); box-shadow: 0 0 18px color-mix(in srgb, var(--rumi-purple-accent) 65%, transparent); }
  .hero-grid { display: grid; grid-template-columns: minmax(0, 1.1fr) minmax(300px, .8fr); align-items: center; gap: clamp(2rem, 6vw, 5rem); }
  h1 { max-width: 720px; margin: 0; font-size: clamp(2.6rem, 6.3vw, 5.4rem); line-height: .99; letter-spacing: -.055em; }
  h1 span { color: var(--rumi-purple-accent); }
  .hero-copy { max-width: 620px; margin: 1.6rem 0 1rem; color: var(--rumi-text-secondary); font-size: 1.08rem; line-height: 1.75; }
  .review-date { color: var(--rumi-text-muted); font-size: .84rem; }
  .review-date time { color: var(--rumi-text-secondary); }
  .status-card { padding: 1.5rem; border: 1px solid color-mix(in srgb, #f4c46e 35%, var(--rumi-border)); border-radius: 1rem; background: linear-gradient(150deg, color-mix(in srgb, #f4c46e 8%, var(--rumi-bg-surface1)), var(--rumi-bg-surface1) 72%); box-shadow: 0 24px 80px rgba(0,0,0,.22); }
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
  .section-heading h2 { margin: 0; font-size: clamp(1.8rem, 3.4vw, 2.7rem); letter-spacing: -.04em; }
  .section-heading > p:last-child { margin: .85rem 0 0; color: var(--rumi-text-muted); line-height: 1.7; }
  .state-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 1rem; }
  .state-card { position: relative; display: flex; min-height: 290px; flex-direction: column; align-items: flex-start; padding: 1.4rem; border: 1px solid var(--rumi-border); border-radius: .9rem; background: var(--rumi-bg-surface1); }
  .state-card::before { position: absolute; inset: 0 auto 0 0; width: 2px; border-radius: .9rem 0 0 .9rem; background: var(--rumi-purple-accent); content: ''; opacity: .7; }
  .state-source::before { background: #f4c46e; }
  .state-live::before { background: var(--rumi-teal); }
  .state-icon { display: grid; width: 2rem; height: 2rem; place-items: center; border-radius: .6rem; background: color-mix(in srgb, var(--rumi-purple-accent) 12%, var(--rumi-bg-surface2)); color: var(--rumi-purple-accent); font-size: .95rem; }
  .state-source .state-icon { color: #f4c46e; background: rgba(244,196,110,.1); }
  .state-live .state-icon { color: var(--rumi-teal); background: color-mix(in srgb, var(--rumi-teal) 10%, var(--rumi-bg-surface2)); }
  .state-label { margin: 1.2rem 0 .35rem; color: var(--rumi-text-muted); font-size: .66rem; font-weight: 700; letter-spacing: .13em; text-transform: uppercase; }
  .state-card h3 { margin: 0 0 .65rem; font-size: 1.1rem; }
  .state-card > p:not(.state-label) { margin: 0; color: var(--rumi-text-secondary); font-size: .86rem; line-height: 1.7; }
  .evidence-link { margin-top: auto; padding-top: 1rem; color: var(--rumi-purple-accent); font-size: .82rem; font-weight: 600; text-decoration: none; }
  .release-section { border-block: 1px solid var(--rumi-border); background: linear-gradient(120deg, rgba(14,18,34,.95), rgba(10,14,26,.96)); }
  .release-inner { padding-block: clamp(4rem, 8vw, 6rem); }
  .release-heading { margin-bottom: 1rem; }
  .release-note { margin: 1.4rem 0 0; padding: 1rem 1.1rem; border-left: 2px solid #f4c46e; background: rgba(20,26,46,.58); color: var(--rumi-text-secondary); font-size: .9rem; line-height: 1.7; }
  .limits-list { display: grid; gap: .65rem; max-width: 780px; margin: 0; padding-left: 1.2rem; color: var(--rumi-text-secondary); line-height: 1.7; }
  .limits-list li::marker { color: var(--rumi-purple-accent); }
  .source-note { max-width: 780px; margin-top: 2rem; color: var(--rumi-text-muted); font-size: .85rem; line-height: 1.7; }
  .source-note a, .page-foot a { color: var(--rumi-purple-accent); text-underline-offset: 3px; }
  .page-foot { display: flex; justify-content: space-between; gap: 1rem; padding-block: 1.3rem 2rem; border-top: 1px solid var(--rumi-border); color: var(--rumi-text-muted); font-size: .72rem; }
  a:focus-visible { outline: 2px solid var(--rumi-teal-bright); outline-offset: 4px; border-radius: 2px; }
  @media (max-width: 760px) {
    .page-width { width: min(100% - 2rem, 600px); }
    .hero-grid { grid-template-columns: 1fr; gap: 2rem; }
    .status-card { max-width: 540px; }
    .evidence-rail { grid-template-columns: 1fr; gap: .7rem; padding: 1rem; }
    .rail-line { width: 1px; height: 16px; margin-left: 1.05rem; background: linear-gradient(180deg, var(--rumi-purple-accent), rgba(244,196,110,.6)); }
    .state-grid { grid-template-columns: 1fr; }
    .state-card { min-height: auto; }
    .page-foot { flex-direction: column; }
  }
</style>
