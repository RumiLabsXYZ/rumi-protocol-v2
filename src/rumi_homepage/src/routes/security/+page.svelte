<script>
  const findings = [
    { sev: 'Critical', count: 0, tone: 'good' },
    { sev: 'High',     count: 8, tone: 'closed' },
    { sev: 'Medium',   count: 36, tone: 'closed' },
    { sev: 'Low',      count: 25, tone: 'closed' },
  ];

  const programStats = [
    { label: 'Unit + integration tests', value: '866' },
    { label: 'Audit fence test files',   value: '36' },
    { label: 'Remediation waves',        value: '13' },
    { label: 'Canisters in scope',       value: '9' },
    { label: 'Lines of Rust reviewed',   value: '~84k' },
    { label: 'Lines of TypeScript / Svelte reviewed', value: '~55k' },
    { label: 'Mainnet pauses during review', value: '0' },
    { label: 'Funds lost during review',     value: '0' },
  ];

  const architecturalSecurity = [
    {
      title: 'Per-user vaults',
      desc: 'Each borrower owns an isolated CDP. There is no shared collateral pool to drain. A bad actor or buggy interaction can only put their own vault at risk.',
    },
    {
      title: 'On-chain price feeds',
      desc: 'ICP/USD prices come from the Internet Computer\'s native Exchange Rate Canister (XRC), with a multi-source floor and staleness gate before any liquidation or redemption can use them.',
    },
    {
      title: 'No bridges, no off-chain workers',
      desc: 'Frontend, backend, ledger, oracle, liquidation bot, and stability pool all run inside ICP canisters. There is no off-chain server, no signer, no bridge, no relayer to compromise.',
    },
    {
      title: 'Stability Pool first, fallback second',
      desc: 'Liquidations route through the Stability Pool before falling to public liquidators. Bad debt that exceeds both layers lands in a tracked deficit account rather than silently socializing onto solvent vaults.',
    },
    {
      title: 'Deterministic upgrade safety',
      desc: 'All cross-upgrade state is held in stable structures with explicit migration tests. Pre-deploy hooks run the full unit + PocketIC suite against every release before mainnet install.',
    },
    {
      title: 'Mass-liquidation circuit breaker',
      desc: 'A per-cycle liquidation budget and oracle-deviation guard pause cascading liquidations during oracle glitches or flash crashes, preserving the protocol against a single-block wipeout.',
    },
  ];

  const exploitResistance = [
    'Async-state races at every cross-canister `await` (saga pattern + idempotent retries)',
    'Reentrancy via per-canister `CallerGuard` locks on swap, liquidity, and CDP entry points',
    'ICRC double-spend windows closed with `created_at_time` deduplication on every transfer',
    'Oracle staleness gate + multi-source floor before any price-driven action',
    'Authorization boundary between protocol and stability pool (no anonymous principals, no `dev_*` endpoints in mainnet wasm)',
    'Inter-canister call failure paths produce typed errors and stranded-fund refund queues, not silent loss',
    'Bot auto-cancel verifies on-chain collateral return before clearing pending state',
    'Unbounded query DoS closed via pagination, cached aggregates, and sharded vault checks',
  ];

  const audits = [
    {
      title: 'Combined Security Review',
      tag: 'Latest',
      tagTone: 'primary',
      date: 'May 2, 2026',
      authors: 'Internal + AVAI close-out',
      scope: 'Unified close-out of every finding from the internal three-pass review and the AVAI external pre-audit, including the eight net-new findings closed in Wave 14a/b/c.',
      summary: 'Anchored to backend hash 0xc6b99934 and 3pool hash 0xfcf49d30. Every finding from both reviews resolved, deferred-by-design, or accepted with a documented watch threshold.',
      slug: 'rumi-combined-security-review-2026-05-02',
    },
    {
      title: 'Internal Three-Pass Review',
      tag: 'Internal',
      tagTone: 'neutral',
      date: 'April 22, 2026',
      authors: 'Rumi Labs',
      scope: 'Eleven specialist analysis passes covering ~84k lines of Rust across nine canisters and ~55k lines of TypeScript/Svelte across both frontends. Anchored to commit 28e9896.',
      summary: '73 findings (8 HIGH, 36 MEDIUM, 25 LOW, 4 INFO) shipped across thirteen numbered remediation waves over nine days, then verified by three independent post-fix passes.',
      slug: 'rumi-internal-review-2026-04-22',
    },
    {
      title: 'AVAI External Pre-Audit',
      tag: 'External',
      tagTone: 'neutral',
      date: 'April 24, 2026',
      authors: 'AVAI',
      scope: 'Independent automated sweep against the Breitner IC Canister Security Guidelines and a CDP protocol domain checklist. Anchored to commit e749620d.',
      summary: '20 numbered findings (4 IC-hygiene, 16 CDP-domain). Twelve overlapped with internal-review fixes already shipped. Eight net-new findings closed in Wave 14a/b/c.',
      slug: 'rumi-avai-external-audit-2026-04-24',
    },
  ];

  const canisters = [
    { name: 'rumi_protocol_backend', id: 'tfesu-vyaaa-aaaap-qrd7a-cai', role: 'CDP engine: vaults, minting, redemption, liquidation' },
    { name: 'rumi_stability_pool',   id: 'tmhzi-dqaaa-aaaap-qrd6q-cai', role: 'Stability Pool: absorbs liquidations, distributes collateral' },
    { name: 'rumi_treasury',         id: 'tlg74-oiaaa-aaaap-qrd6a-cai', role: 'Protocol treasury: ckUSDT/ckUSDC reserves' },
    { name: 'icusd_ledger',          id: 't6bor-paaaa-aaaap-qrd5q-cai', role: 'icUSD ICRC-1/ICRC-2 ledger' },
    { name: 'rumi_3pool',            id: 'fohh4-yyaaa-aaaap-qtkpa-cai', role: 'Curve-style stableswap (icUSD / ckUSDT / ckUSDC), 3USD LP token' },
    { name: 'rumi_amm',              id: 'ijlzs-2yaaa-aaaap-quaaq-cai', role: 'AMM router for non-stable pairs' },
    { name: 'liquidation_bot',       id: 'nygob-3qaaa-aaaap-qttcq-cai', role: 'Automated liquidation triggering with cancel-and-return safety' },
    { name: 'icusd_index',           id: '6niqu-siaaa-aaaap-qrjeq-cai', role: 'icUSD ledger index canister' },
    { name: 'threeusd_index',        id: 'jagpu-pyaaa-aaaap-qtm6q-cai', role: '3USD ledger index canister' },
  ];

  function tagClass(tone) {
    return tone === 'primary' ? 'audit-tag tag-primary' : 'audit-tag';
  }
  function findingClass(tone) {
    return tone === 'good' ? 'finding-card good' : 'finding-card closed';
  }
</script>

<svelte:head>
  <title>Security · Rumi Protocol</title>
  <meta name="description" content="Audit reports, architectural security, testing methodology, and responsible disclosure for Rumi Protocol on the Internet Computer." />
</svelte:head>

<!-- Hero + at-a-glance findings -->
<section class="hero-wrap">
  <div class="max-w-5xl mx-auto px-6 pt-20 md:pt-24 pb-10">
    <p class="eyebrow">Security</p>
    <h1 class="page-title">Security at Rumi Protocol</h1>
    <p class="page-lead">
      Rumi Protocol is a fully on-chain CDP stablecoin running on the Internet Computer.
      Two independent security reviews, an automated external pre-audit by AVAI, and a
      thirteen-wave remediation cycle have shipped to mainnet. Every finding from both
      reviews is in one of three states: resolved with a regression test, deferred-by-design
      until SNS migration, or accepted as housekeeping with a documented watch threshold.
    </p>

    <div class="findings-grid">
      {#each findings as f}
        <div class={findingClass(f.tone)}>
          <div class="finding-num">{f.count}</div>
          <div class="finding-label">{f.sev}</div>
          <div class="finding-status">{f.tone === 'good' ? 'None reported' : 'All closed'}</div>
        </div>
      {/each}
    </div>

    <div class="program-stats">
      {#each programStats as s}
        <div class="stat">
          <div class="stat-value">{s.value}</div>
          <div class="stat-label">{s.label}</div>
        </div>
      {/each}
    </div>
  </div>
</section>

<!-- Architectural security -->
<section class="max-w-5xl mx-auto px-6 py-16 border-t" style="border-color: var(--rumi-border);">
  <h2 class="section-h2">Architectural Security</h2>
  <p class="section-sub">
    The strongest defenses are the ones built into the protocol's shape, not bolted on
    after the fact. Rumi's design eliminates entire classes of attack before any code is
    written.
  </p>

  <div class="arch-grid">
    {#each architecturalSecurity as item}
      <div class="arch-card">
        <h3 class="arch-title">{item.title}</h3>
        <p class="arch-desc">{item.desc}</p>
      </div>
    {/each}
  </div>
</section>

<!-- Testing methodology -->
<section class="max-w-5xl mx-auto px-6 py-16 border-t" style="border-color: var(--rumi-border);">
  <h2 class="section-h2">Testing Methodology</h2>
  <p class="section-sub">
    Every audit finding has at least one fence test that fails on the unfixed commit and
    passes on the fix. Pre-deploy hooks run the full unit and PocketIC integration suites
    before any mainnet install.
  </p>

  <div class="test-grid">
    <div class="test-block">
      <h3 class="test-block-title">Test surface</h3>
      <ul class="test-list">
        <li><strong>866</strong> Rust unit and integration tests across the workspace</li>
        <li><strong>36</strong> dedicated audit fence test files (one per finding family)</li>
        <li><strong>PocketIC</strong> integration tests exercise full inter-canister flows</li>
        <li><strong>Pre-deploy hook</strong> blocks mainnet installs if any test fails</li>
        <li><strong>24-hour bake-watch</strong> on every wave before the next deploys</li>
      </ul>
    </div>

    <div class="test-block">
      <h3 class="test-block-title">What gets verified</h3>
      <ul class="test-list">
        <li>Liquidation invariants: sorted-troves index, min-debt floor, ICRC-3 burn proof</li>
        <li>Stability Pool accounting: no double-deduction, balance reconciliation, deficit account</li>
        <li>Oracle behaviour: staleness gate, multi-source floor, frozen-price liquidation halt</li>
        <li>Interest accrual under concurrent harvest + treasury drain</li>
        <li>Bot cancel paths return collateral before clearing pending state</li>
        <li>ICRC transfer idempotency via `created_at_time` deduplication</li>
        <li>Pagination and cache caps on every previously-unbounded query</li>
      </ul>
    </div>
  </div>
</section>

<!-- Exploit resistance -->
<section class="max-w-5xl mx-auto px-6 py-16 border-t" style="border-color: var(--rumi-border);">
  <h2 class="section-h2">Exploit Resistance</h2>
  <p class="section-sub">
    Specific attack vectors examined during the reviews, with the structural mitigation
    that addresses each.
  </p>
  <ul class="exploit-list">
    {#each exploitResistance as item}
      <li>
        <span class="check">✓</span>
        <span>{item}</span>
      </li>
    {/each}
  </ul>
</section>

<!-- Audit reports -->
<section class="max-w-5xl mx-auto px-6 py-16 border-t" style="border-color: var(--rumi-border);">
  <h2 class="section-h2">Audit Reports</h2>
  <p class="section-sub">
    Each report is published as PDF for human reading and as Markdown for AI agents,
    indexers, and grep. Both formats contain the same findings and remediation status.
  </p>

  <div class="audit-stack">
    {#each audits as audit}
      <article class="audit-card">
        <div class="audit-head">
          <h3 class="audit-title">{audit.title}</h3>
          <span class={tagClass(audit.tagTone)}>{audit.tag}</span>
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
            Markdown <span class="btn-hint">(for AI agents)</span>
          </a>
        </div>
      </article>
    {/each}
  </div>
</section>

<!-- Transparency note -->
<section class="max-w-5xl mx-auto px-6 py-16 border-t" style="border-color: var(--rumi-border);">
  <h2 class="section-h2">Transparency</h2>
  <div class="prose-block callout">
    <p>
      The reviews above were conducted using AI-driven security analysis, not a traditional
      brand-name audit firm. The methodology is rigorous: a structured threat model, anchored
      commits, fence tests for every finding, three independent verification passes. It is
      not a substitute for a top-tier external engagement, and we treat it as the floor of
      our security posture, not the ceiling. As the protocol grows we intend to commission a
      full external audit. Until then, please do your own research and never deposit more
      than you can comfortably lose.
    </p>
  </div>
</section>

<!-- Deployed canisters -->
<section class="max-w-5xl mx-auto px-6 py-16 border-t" style="border-color: var(--rumi-border);">
  <h2 class="section-h2">Canisters in Scope</h2>
  <p class="section-sub">
    Every canister listed below is verifiable on-chain. Module hashes can be queried directly
    against each canister and matched to commits in the public source tree.
  </p>
  <div class="canister-table-wrap">
    <table class="canister-table">
      <thead>
        <tr><th>Canister</th><th>ID</th><th>Role</th></tr>
      </thead>
      <tbody>
        {#each canisters as c}
          <tr>
            <td><code>{c.name}</code></td>
            <td>
              <a href="https://dashboard.internetcomputer.org/canister/{c.id}" target="_blank" rel="noopener" class="canister-link">
                <code>{c.id}</code>
              </a>
            </td>
            <td>{c.role}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  <p class="canister-foot">
    Source code: <a href="https://github.com/RumiLabsXYZ/rumi-protocol-v2" target="_blank" rel="noopener">github.com/RumiLabsXYZ/rumi-protocol-v2</a>
  </p>
</section>

<!-- Responsible disclosure -->
<section class="max-w-5xl mx-auto px-6 py-16 border-t" style="border-color: var(--rumi-border);">
  <h2 class="section-h2">Responsible Disclosure</h2>
  <div class="prose-block">
    <p>
      If you believe you've found a vulnerability in any Rumi Protocol canister or frontend,
      please report it privately rather than disclosing publicly. We aim to acknowledge
      reports within 48 hours.
    </p>
    <ul class="contact-list">
      <li>
        <strong>Email:</strong>
        <a href="mailto:info@rumiprotocol.com">info@rumiprotocol.com</a>
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
      material. Coordinated disclosure is appreciated.
    </p>
  </div>
</section>

<style>
  /* ── Hero ── */
  .hero-wrap {
    background:
      radial-gradient(ellipse at top, color-mix(in srgb, var(--rumi-purple-accent) 8%, transparent) 0%, transparent 60%),
      transparent;
  }
  .eyebrow {
    font-size: 0.8125rem;
    font-weight: 500;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--rumi-purple-accent);
    margin-bottom: 1.25rem;
  }
  .page-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: clamp(2rem, 4.5vw, 2.75rem);
    font-weight: 700;
    letter-spacing: -0.02em;
    color: var(--rumi-text-primary);
    margin-bottom: 1.5rem;
  }
  .page-lead {
    color: var(--rumi-text-secondary);
    font-size: 1rem;
    line-height: 1.7;
    max-width: 720px;
    margin-bottom: 2.5rem;
  }

  /* ── Findings strip ── */
  .findings-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 0.75rem;
    margin-bottom: 1.5rem;
  }
  .finding-card {
    padding: 1.25rem 1rem;
    border-radius: 0.75rem;
    border: 1px solid var(--rumi-border);
    background: var(--rumi-bg-surface1);
    text-align: center;
    transition: transform 0.2s ease, border-color 0.2s ease;
  }
  .finding-card:hover { transform: translateY(-1px); }
  .finding-card.good {
    border-color: color-mix(in srgb, var(--rumi-action) 35%, var(--rumi-border));
  }
  .finding-card.closed {
    border-color: color-mix(in srgb, var(--rumi-purple-accent) 25%, var(--rumi-border));
  }
  .finding-num {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 2.25rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    line-height: 1;
    margin-bottom: 0.5rem;
  }
  .finding-card.good .finding-num { color: var(--rumi-action); }
  .finding-label {
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--rumi-text-secondary);
    margin-bottom: 0.25rem;
  }
  .finding-status {
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }

  /* ── Program stats ── */
  .program-stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 0.75rem;
    padding: 1.25rem;
    border-radius: 0.75rem;
    border: 1px solid var(--rumi-border);
    background: color-mix(in srgb, var(--rumi-bg-surface1) 60%, transparent);
  }
  .stat { text-align: left; }
  .stat-value {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1.375rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    line-height: 1;
    margin-bottom: 0.375rem;
  }
  .stat-label {
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
    line-height: 1.4;
  }

  /* ── Section heads ── */
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
    line-height: 1.65;
    margin-bottom: 2rem;
    max-width: 680px;
  }

  /* ── Architectural cards ── */
  .arch-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 1rem;
  }
  .arch-card {
    padding: 1.5rem;
    border-radius: 0.75rem;
    border: 1px solid var(--rumi-border);
    background: var(--rumi-bg-surface1);
    transition: border-color 0.2s ease, transform 0.2s ease;
  }
  .arch-card:hover {
    border-color: color-mix(in srgb, var(--rumi-purple-accent) 30%, var(--rumi-border));
  }
  .arch-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 1rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    margin-bottom: 0.625rem;
  }
  .arch-desc {
    font-size: 0.875rem;
    line-height: 1.6;
    color: var(--rumi-text-secondary);
  }

  /* ── Testing blocks ── */
  .test-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
    gap: 1rem;
  }
  .test-block {
    padding: 1.5rem;
    border-radius: 0.75rem;
    border: 1px solid var(--rumi-border);
    background: var(--rumi-bg-surface1);
  }
  .test-block-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.9375rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    margin-bottom: 0.875rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .test-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 0.625rem;
  }
  .test-list li {
    font-size: 0.875rem;
    color: var(--rumi-text-secondary);
    line-height: 1.55;
    padding-left: 1rem;
    position: relative;
  }
  .test-list li::before {
    content: '·';
    position: absolute;
    left: 0;
    color: var(--rumi-purple-accent);
    font-weight: bold;
  }
  .test-list strong {
    color: var(--rumi-text-primary);
    font-weight: 600;
  }

  /* ── Exploit list ── */
  .exploit-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
    gap: 0.625rem;
  }
  .exploit-list li {
    display: flex;
    gap: 0.75rem;
    align-items: flex-start;
    padding: 0.875rem 1rem;
    border-radius: 0.5rem;
    border: 1px solid var(--rumi-border);
    background: var(--rumi-bg-surface1);
    font-size: 0.875rem;
    color: var(--rumi-text-secondary);
    line-height: 1.55;
  }
  .check {
    color: var(--rumi-action);
    font-weight: 700;
    flex-shrink: 0;
  }

  /* ── Audit cards ── */
  .audit-stack { display: flex; flex-direction: column; gap: 1rem; }
  .audit-card {
    padding: 1.75rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    transition: border-color 0.2s ease;
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
    letter-spacing: 0.08em;
    text-transform: uppercase;
    padding: 0.25rem 0.5rem;
    border-radius: 0.25rem;
    color: var(--rumi-text-muted);
    border: 1px solid var(--rumi-border);
  }
  .audit-tag.tag-primary {
    color: var(--rumi-purple-accent);
    border-color: color-mix(in srgb, var(--rumi-purple-accent) 35%, transparent);
    background: color-mix(in srgb, var(--rumi-purple-accent) 8%, transparent);
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
    display: inline-block;
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
  }
  .btn-hint {
    color: var(--rumi-text-muted);
    font-weight: 400;
    font-size: 0.75rem;
    margin-left: 0.25rem;
  }

  /* ── Prose blocks ── */
  .prose-block { max-width: 720px; }
  .prose-block p {
    color: var(--rumi-text-secondary);
    font-size: 0.9375rem;
    line-height: 1.7;
    margin-bottom: 1rem;
  }
  .prose-block.callout {
    padding: 1.5rem;
    border-radius: 0.75rem;
    border: 1px solid color-mix(in srgb, var(--rumi-purple-accent) 25%, var(--rumi-border));
    background: color-mix(in srgb, var(--rumi-purple-accent) 4%, transparent);
    max-width: none;
  }
  .prose-block a {
    color: var(--rumi-action);
    text-decoration: none;
  }
  .prose-block a:hover { text-decoration: underline; }
  .contact-list {
    list-style: none;
    padding: 0;
    margin: 0 0 1rem 0;
  }
  .contact-list li {
    color: var(--rumi-text-secondary);
    font-size: 0.9375rem;
    line-height: 1.9;
  }

  /* ── Canister table ── */
  .canister-table-wrap {
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    overflow: hidden;
    background: var(--rumi-bg-surface1);
  }
  .canister-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.8125rem;
  }
  .canister-table thead th {
    text-align: left;
    padding: 0.75rem 1rem;
    color: var(--rumi-text-muted);
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    font-size: 0.6875rem;
    border-bottom: 1px solid var(--rumi-border);
    background: color-mix(in srgb, var(--rumi-bg-primary) 40%, transparent);
  }
  .canister-table tbody td {
    padding: 0.75rem 1rem;
    border-top: 1px solid var(--rumi-border);
    color: var(--rumi-text-secondary);
    vertical-align: top;
  }
  .canister-table tbody tr:first-child td { border-top: none; }
  .canister-table code {
    font-family: 'JetBrains Mono', ui-monospace, monospace;
    font-size: 0.78125rem;
    color: var(--rumi-text-primary);
  }
  .canister-link {
    color: inherit;
    text-decoration: none;
    border-bottom: 1px dotted var(--rumi-border);
  }
  .canister-link:hover { color: var(--rumi-action); }
  .canister-foot {
    margin-top: 0.75rem;
    font-size: 0.8125rem;
    color: var(--rumi-text-muted);
  }
  .canister-foot a {
    color: var(--rumi-action);
    text-decoration: none;
  }
  .canister-foot a:hover { text-decoration: underline; }

  /* Mobile: tighten table */
  @media (max-width: 640px) {
    .canister-table thead { display: none; }
    .canister-table, .canister-table tbody, .canister-table tr, .canister-table td {
      display: block;
      width: 100%;
    }
    .canister-table tr {
      padding: 0.875rem 0;
      border-top: 1px solid var(--rumi-border);
    }
    .canister-table tbody tr:first-child { border-top: none; }
    .canister-table td { padding: 0.25rem 1rem; border: none; }
  }
</style>
