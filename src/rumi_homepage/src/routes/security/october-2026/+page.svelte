<script>
  const report = {
    date: 'October 12, 2026',
    datetime: '2026-10-12',
    source: 'protocol code through PR #563 · live observations dated per row',
    snapshot: '3pool, Points, and AMM upgrades with module readback · October 12, 2026'
  };

  const states = [
    { name: 'Merged source', meaning: 'The change is present on the repository main branch.' },
    { name: 'Installed', meaning: 'The canister reports the module hash shown in this snapshot.' },
    { name: 'Paused', meaning: 'The live system reports its emergency pause as active.' },
    { name: 'Not installed', meaning: 'The mainnet canister still reports its earlier module.' }
  ];

  const changes = [
    {
      pr: '#563',
      title: 'Treasury withdrawal request binding',
      summary: 'Merged into main on October 13 UTC. Every new Treasury withdrawal ledger memo now includes its request ID, including when the controller provides a custom memo. The controller memo limit is 24 UTF-8 bytes so the complete transfer memo fits within 32 bytes; the Treasury form uses the same limit. Focused Treasury tests and a frontend build passed. An October 13 UTC readback still showed Treasury on its earlier module, so this change has not been installed and does not settle any held withdrawal.',
      state: 'Merged · Treasury not installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/563'
    },
    {
      pr: '#561',
      title: 'Treasury withdrawal intent recovery',
      summary: 'Merged into main on October 13 UTC. The source records a durable, request-bound withdrawal intent before transfer and restores the reserved amount to Treasury’s internal balance once for typed no-effect outcomes. Ambiguous outcomes remain held without automatic replay. After an upgrade, the controller must re-query the journal; that status alone does not prove whether a ledger transfer settled. Controller-only status queries expose the journal, but no endpoint yet accepts positive ledger evidence to resolve a held intent. Treasury still reports Running with its earlier module SHA-256 4163a25f7e1d5119768cf16d6c75b6c98925682b0a89e99ced5cfe5aa0107300, so this change is not installed.',
      state: 'Merged · Treasury not installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/561'
    },
    {
      pr: '#557',
      title: 'Bounded 3USD reserve inventory',
      summary: 'Merged into main on October 13 UTC. The source adds developer/admin-gated, paginated views of retained backend reserve records and Stability Pool reserve-absorb intents and completed IDs so a later cutover can be reconciled against recorded state. The pages are not an atomic snapshot; a complete multi-page inventory requires quiescence or a versioned snapshot. A focused CI check passed. October 13 UTC readbacks still found the earlier backend and Stability Pool modules; the new inventory methods are not installed. The existing backend reported V2 reserve ingress disabled and an empty legacy pending-refund list, which does not establish that all historical obligations are settled.',
      state: 'Merged · not installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/557'
    },
    {
      pr: '#548',
      title: 'Latest-bar ICP oracle request',
      summary: 'Merged source requests the latest available ICP/USD bar rather than a fixed one-minute-old bar, while retaining the 120-second source-age gate. The October 13 UTC backend module readback still matched the earlier PR #527 release, so this oracle change is not installed. A later accepted price can clear an oracle-triggered ReadOnly mode; that observed recovery is not proof that stale provider samples cannot recur.',
      state: 'Merged · not installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/548'
    },
    {
      pr: 'Live',
      title: 'Backend and bot readback with capped borrowing',
      summary: 'An October 12 module readback found the backend still at SHA-256 e325ce0d9ee4b5dcd1e68caafc6c69e58ec0d8cae46efd2930dfaed4f97ad352 and the liquidation bot still at 184fe54ee091935f4e5dd5331c58a2743ea4cfa80f6c55a18db13556cc8e19a7. The backend still reported a 22,000 icUSD protocol-wide new-mint cap. An October 11 readback found debt below that cap; remaining capacity was not recalculated for this update. These installed modules predate PRs #531 and #532. This does not prove a completed user borrow, settle historical operations, or establish that every finding is closed. Treasury still reported its earlier module.',
      state: 'Installed · capped borrowing',
      tone: 'installed',
      href: null
    },
    {
      pr: '#535',
      title: 'Archived 3pool payout recovery',
      summary: 'Merged and installed on October 12 as part of the 3pool module SHA-256 74a81602f61ee08023bc1a6746f4961aebcdb8c7f98b0019c9eefbb63a3998ae. Archived payout receipts can be reconciled only through exact evidence linked to the configured ledger; uncertain claims remain held. A hash-gated PocketIC regression used Wasm bytes matching the installed predecessor and final candidate. A fresh pre-upgrade snapshot was taken while stopped, then the canister restarted on the new hash. Immediate readback found unchanged LP supply, reserves, virtual price, and admin fees, with zero pending claims. The admin inventory contained five confirmed swap outputs and five closed, non-dispatched refund reservations; this does not prove every historical ledger obligation or settle a new payout.',
      state: 'Installed · readback passed',
      tone: 'installed',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/535'
    },
    {
      pr: '#540/#541',
      title: 'Global payout inventory and exact upgrade pin',
      summary: 'The installed 3pool module adds a bounded admin-only page over its retained payout entitlements. The final artifact was rebuilt twice with the same SHA-256, pinned in the exact-installed-predecessor PocketIC upgrade test, and checked against the authored Candid service. The complete first live inventory page held five confirmed swap outputs and five closed, non-dispatched refund reservations; there was no next cursor. Caller-visible receipts and external-ledger history remain separate evidence.',
      state: 'Installed · inventory available',
      tone: 'installed',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/540'
    },
    {
      pr: '#553',
      title: 'Archived 3pool input recovery',
      summary: 'Merged and installed on October 12 as 3pool module SHA-256 d1bc983879dc4a385ec87a398168f798802027b428bbef0f1c8382435ed96c6e. The source adds ledger-anchored proof for archived input transfers during refund reconciliation. An opt-in exact-Wasm PocketIC test in PR #555 passed an upgrade from the installed predecessor with populated pool state. The canister was paused and stopped, a fresh pre-upgrade snapshot was taken, and the reviewed Wasm was installed before restart. Post-upgrade status reported the exact new hash; the pool was unpaused and healthy, with LP supply 1,485,147,981,057, admin fees [40,219, 0, 0], and zero pending claims. While paused, four inventory pages returned 62 unique retained payout records, all internally marked “settled.” Those statuses do not independently prove every external ledger outcome. No new funded swap or deposit was exercised.',
      state: 'Installed · readback passed',
      tone: 'installed',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/553'
    },
    {
      pr: '#507',
      title: 'AMM transfer and payout recovery',
      summary: 'Merged and installed on October 12 as module SHA-256 1f3fddd44fb7832585c9a45d0b46ef607805381531c71f71a8d517a7eddabf6a. The source journals inbound pulls and outbound payouts before ledger calls and holds uncertain outcomes. A hash-gated PocketIC test in PR #538 recovered one official-ledger native ICP payout after a callback trap without paying twice; it does not prove every reply-loss path. The canister was stopped, a fresh snapshot was verified, and the reviewed Wasm was installed before restart. Immediate readback matched the pre-upgrade pool, LP, rewards, events, claims, maintenance setting, and three checked ledger balances. The new unresolved-payout journal was empty. Swap and deposit routes remain held, while exits and reward claims remain callable. The ICP pool account holds 540,000 raw units above its tracked reserve; the icUSD reward account holds 900,021 raw units whose historical obligations remain unattributed. Installation did not settle those balances or restore trading.',
      state: 'Installed · recovery available · trading held',
      tone: 'installed',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/507'
    },
    {
      pr: '#531',
      title: 'Reserve-redemption recovery journal',
      summary: 'Merged into main on October 11. The source records each redemption leg and its exact receipt or refund obligation before continuing. This is a source merge only: the running backend predates this change, and live redemption recovery has not been demonstrated.',
      state: 'Merged · not installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/531'
    },
    {
      pr: '#532',
      title: 'Interest and stablecoin transfer journals',
      summary: 'Merged into main on October 11. The backend now journals interest mints and Treasury-directed stablecoin transfers; Stability Pool interest receipts and 3pool donation receipts are also recorded for exact retry. Local source-matched tests and a backend build passed. The later 3pool upload was built from main after this merge, but the October 12 installation belongs to the separately listed PRs #540 and #541. The running backend and Stability Pool still predate this PR. Cross-canister failure and upgrade recovery remain unverified in production.',
      state: 'Merged · peer installs pending',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/532'
    },
    {
      pr: '#545',
      title: 'Stability Pool deposit-intent recovery',
      summary: 'Merged into main on October 12. The source records a caller-bound deposit intent before an external transfer and requires linked ledger evidence before crediting a recovered deposit. The merged PR includes focused PocketIC coverage and Candid compatibility checks. A post-merge readback at 21:31 UTC still showed the earlier Stability Pool module SHA-256 54245b779feb30c4da7ac3f60e9cb62afb99f1accd2ea5affc7adda8a28e4754 and emergency_paused=true. The merged fix has not been installed, and source checks do not resolve any historical deposit or refund obligation.',
      state: 'Merged · Stability Pool paused · not installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/545'
    },
    {
      pr: '#527',
      title: 'Treasury interest mint and deposit receipts',
      summary: 'The source change journals Treasury interest mints and exact deposit receipts. The later backend upgrade is separately recorded above. Treasury still reports its earlier module, so this row does not claim its new code is installed or that held interest has been reconciled.',
      state: 'Merged · Treasury not installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/527'
    },
    {
      pr: '#450',
      title: 'Points release and historical reconciliation',
      summary: 'Merged and deployed to the Points canister and frontend. The October 8 Points module reported SHA-256 dd9b8d5475069c23084ed08aff414faf81dda94081c62b7c4a111d232d7d6277; the later October 12 redaction upgrade below superseded that module. The original 401 ledger rows are preserved; 16 tagged rows add an approximate historical uplift of 2,151.74119482 points, bringing the ledger to 417 rows. This does not recreate discarded historical snapshot selection. Epoch 18 remains open and held with A complete, B incomplete, and the driver off. No epoch close or reward release is shown. Flat 4× is scheduled for epoch 19 and is not active yet.',
      state: 'Installed October 8 · superseded',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/450'
    },
    {
      pr: '2d1d3793',
      title: 'Points snapshot-time redaction',
      summary: 'The October 12 pre-upgrade readback found that public status exposed B’s scheduled time before capture. A fresh Points-only snapshot was taken while the canister was stopped; it was restarted, then upgraded to the reviewed Wasm. The running Points module now reports SHA-256 b0b168e4513a62df86344035615fbd4564b57bcd2b919542948814d1ad7e5b32. Immediate readback found B incomplete and its public time absent, epoch 18 still held, the driver disabled, the ledger unchanged at 417 rows, and the fiat-stable policy unchanged. This does not resolve the held epoch or award its points.',
      state: 'Installed · public B time withheld',
      tone: 'installed',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/commit/2d1d37931471a104cc8aa44b47b13fbb6680b33c'
    },
    {
      pr: '#466',
      title: '3pool upgrade and state preservation',
      summary: 'Merged into main and installed on October 8. At that time, the rumi_3pool canister (fohh4-yyaaa-aaaap-qtkpa-cai) reported module SHA-256 b6e5e2eab401c8e1551a33acc134369a92744efb769ed4e34a7feb9923a7a1af. The October 8 readback recorded unchanged LP supply, reserves, admin fees, and virtual price, with zero pending claims. PR #483 later superseded this installation; see its October 9 readback below.',
      state: 'Installed October 8 · superseded',
      tone: 'installed',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/466'
    },
    {
      pr: '#483',
      title: '3pool upgrade verification and state preservation',
      summary: 'Merged on October 9. The rumi_3pool canister (fohh4-yyaaa-aaaap-qtkpa-cai) reported module SHA-256 9797bf2454923ef41d6ef6a003435e0430aaa2b2b413734c6d9e6e921889dfc0 before the October 12 upgrade. A clean-source rebuild matched that predecessor module, and its immediate October 9 state readback passed. The October 12 installation described above superseded it.',
      state: 'Installed October 9 · superseded',
      tone: 'installed',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/483'
    },
    {
      pr: '#501',
      title: 'Cycle Sentinel recovery safeguard',
      summary: 'Installed on October 10. Cycle Sentinel (joh3a-5aaaa-aaaap-quy6a-cai) was restarted and reports module SHA-256 b18228fab536e290478472b7e4a54268ef64d046e8e2a21dd0cc1936206b48aa. A pre-upgrade snapshot is retained. This installation does not establish that any historical quarantined operation was reconciled; the available local controller identity is not an application signer.',
      state: 'Installed · recovery pending',
      tone: 'installed',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/501'
    },
    {
      pr: '#504',
      title: '3USD account boundary and AMM custody',
      summary: 'Source merged on October 10. The AMM installation at SHA-256 832485e330faa18fd3f7b2d05b3dca670698227e420e255a128b0333c2979fb2 was superseded by the October 12 recovery upgrade above. The single 3USD/ICP pool retained its reserves and LP supply; its 3USD ledger balance matched the tracked reserve at the latest readback. AMM maintenance remains active. The October 12 3pool upgrade includes the account guard; backend V1/V2 cutover and historical account compatibility remain under review.',
      state: 'AMM installation superseded · 3pool guard installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/504'
    },
    {
      pr: '#459',
      title: 'Stability Pool containment',
      summary: 'An October 12 query confirmed that the rumi_stability_pool canister (tmhzi-dqaaa-aaaap-qrd6q-cai) remained emergency-paused, with module SHA-256 54245b779feb30c4da7ac3f60e9cb62afb99f1accd2ea5affc7adda8a28e4754; the newer source is not installed there. The October 8 pause readback recorded 13 depositors. Aggregate deposits are omitted because later totals differ from that snapshot and have not been fully analyzed.',
      state: 'Paused · newer source not installed',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/459'
    },
    {
      pr: '#455',
      title: 'Pause liquidation bot after upgrades',
      summary: 'This merged source change makes the bot start paused after an upgrade. The separate later live readback above confirms a running upgraded bot; it does not show that PR #455 itself installed a canister or that every recovery path has completed.',
      state: 'Merged · live install recorded separately',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/455'
    },
    {
      pr: '#456',
      title: 'P08 account migration',
      summary: 'The earlier October snapshot recorded an old backend module. The separate later live readback above superseded that observation. The wider 3USD account cutover and other affected canisters require their own activation and migration evidence.',
      state: 'Backend installed · cutover open',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/456'
    },
    {
      pr: '#457',
      title: 'P08 populated-state upgrade proof',
      summary: 'The source and upgrade evidence are merged. A later backend upgrade superseded the earlier not-installed snapshot; that does not by itself prove the wider account migration or live recovery.',
      state: 'Backend installed · migration open',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/457'
    },
    {
      pr: '#458',
      title: 'BOT-10 recovery guard',
      summary: 'Source is merged. Later backend and bot upgrades superseded the earlier old-module readbacks. Claim and payment journal outcomes still need exact reconciliation.',
      state: 'Installed · recovery open',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/458'
    },
    {
      pr: '#460',
      title: 'CL-02 collateral-return proof',
      summary: 'Source is merged. A later backend upgrade superseded the earlier old-module readback. Historical claim outcomes and receipts remain separate evidence.',
      state: 'Backend installed · receipts open',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/460'
    },
    {
      pr: '#468',
      title: 'Backend release compatibility',
      summary: 'The backend compatibility change is merged into main. A later backend module installation superseded the earlier not-installed observation; the current installed hash is recorded above.',
      state: 'Backend installed',
      tone: 'installed',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/468'
    },
    {
      pr: '#511',
      title: 'Borrow-mint recovery and temporary borrowing hold',
      summary: 'Receipt-based borrow recovery is merged in source and the later backend upgrade superseded the old-module observation. The October 10 cap of one raw unit was a temporary hold; a later October 11 readback showed a 22,000 icUSD protocol-wide cap and the backend running. Historical mint recovery remains a separate obligation.',
      state: 'Backend installed · borrowing capped',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/511'
    },
    {
      pr: '#521',
      title: 'Borrowing-fee receipt and Treasury indexing',
      summary: 'The backend fee outbox and Treasury physical-block index are merged in source. The later backend upgrade superseded the old backend readback, but Treasury (tlg74-oiaaa-aaaap-qrd6a-cai) still reports its earlier module SHA-256 4163a25f7e1d5119768cf16d6c75b6c98925682b0a89e99ced5cfe5aa0107300. Its index migration and held interest reconciliation are not complete; borrowing is currently available only within the temporary cap.',
      state: 'Backend installed · Treasury pending',
      tone: 'held',
      href: 'https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/521'
    }
  ];

  const limits = [
    {
      system: 'AMM',
      status: 'Maintenance mode active',
      detail: 'The October 12 AMM upgrade installed the PR #507 recovery source, with maintenance mode still active. New swaps and liquidity deposits remain held; liquidity exits and reward claims remain callable. Exact payout recovery is available for journaled transfers, but the upgrade did not settle a historical liability or explain the pre-existing ledger balances.'
    },
    {
      system: 'Points',
      status: 'Epoch 18 held · epoch 19 scheduled',
      detail: 'The October 8 release evidence confirms the historical adjustments. The October 12 Points upgrade now withholds the incomplete B snapshot time from public status. An immediate administrative read shows A complete, B incomplete, epoch 18 still open and held, and the driver disabled. The ledger remains at 417 rows, but its total does not prove an epoch 18 reward release. Future flat 4× accrual is scheduled for epoch 19, not active.'
    },
    {
      system: 'Vault borrowing',
      status: 'Temporary cap configured',
      detail: 'An October 12 query confirmed the backend still reports a protocol-wide new-mint cap of 22,000 icUSD. An October 11 readback found existing debt below that limit, so borrowing admission had resumed within the remaining capacity at that time. Remaining capacity was not recalculated for this update; individual collateral ceilings and normal checks still apply. This does not prove a completed user borrow or settle historical obligations.'
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
    content="A dated Rumi Protocol security follow-up report separating merged source, installed modules, capped borrowing, and remaining operational holds."
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
            This October 12 live baseline with October 13 source updates through PR #563 separates merged source from mainnet installation. October 12
            readbacks confirmed the 3pool modules from PRs #540, #541, and #553; the latest
            upgrade preserved LP supply and had zero pending claims. The Points upgrade now withholds the unfinished B snapshot
            time, while epoch 18 remains held with its driver off. The AMM recovery upgrade now
            runs with its pre-upgrade pool and ledger state preserved; trading and deposits remain held.
            The October 12 readback found upgraded backend and bot modules. Treasury and Stability
            Pool remained on earlier modules, and the Stability Pool pause stayed active.
            Borrowing had resumed under a temporary cap at the October 11 readback, and that
            cap still reported 22,000 icUSD on October 12. PR #531 remains source-only;
            the backend and Stability Pool changes in PR #532 remain uninstalled. PR #545's
            Stability Pool deposit-intent fix is merged but also remains uninstalled. PR #553's
            archived 3pool input-recovery module was installed after a fresh snapshot and post-upgrade readback.
          </p>
        </div>
        <aside class="report-stamp" aria-label={report.snapshot}>
          <span class="stamp-label">Publication state</span>
          <strong>{report.snapshot}</strong>
          <p>The October 12 upgrades installed the 3pool recovery modules through PR #553, the Points timing-redaction fix, and the AMM recovery upgrade. Their module hashes and readbacks are listed below. The Stability Pool remained paused and the AMM stayed in maintenance mode, with swaps and deposits held. The borrowing cap remained 22,000 icUSD at the latest October 12 backend readback; remaining debt capacity was last checked on October 11. This page combines time-scoped records; it is not live telemetry.</p>
          <a href="/security">View security archive <span aria-hidden="true">→</span></a>
        </aside>
      </div>
      <div class="rule-caption"><span>Scope · {report.source} · October 13, 2026 UTC</span><span>October 12 live snapshot and later source addenda</span></div>
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
        <p>The live readbacks below are dated October 12 unless a row states otherwise. October 13 source addenda are marked separately. The 3pool release includes PR #535 recovery source, the PR #540/#541 artifact, and the now-installed PR #553 input-recovery module. The AMM recovery source from PR #507 is also installed, while PRs #545, #548, and #557 remain merged source only. A source merge and a live installation remain separate states.</p>
      </div>
      <div class="change-list">
        {#each changes as item}
          <article class="change-row">
            <div class="change-ref"><span>{item.pr}</span><span class="change-rule" aria-hidden="true"></span></div>
            <div class="change-main">
              <h3>{item.title}</h3>
              <p>{item.summary}</p>
              {#if item.href}
                <a href={item.href} target="_blank" rel="noopener noreferrer">Review {item.pr} public source record <span aria-hidden="true">↗</span></a>
              {/if}
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
        This page records an October 12 snapshot; it does not publish private audit material or
        establish that every finding is closed.
      </p>
    </div>
  </section>

  <footer class="page-width page-foot">
    <span>Live snapshot date: <time datetime={report.datetime}>{report.date}</time></span>
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
  .report-stamp p { margin: .65rem 0 1rem; color: var(--report-muted); font-size: .82rem; line-height: 1.6; overflow-wrap: anywhere; }
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
    :global(.top-bar) { box-sizing: border-box; min-width: 0; }
    .page-width { box-sizing: border-box; width: calc(100% - 2rem); max-width: 610px; }
    .hero-grid, .limits-inner { grid-template-columns: 1fr; gap: 1.75rem; }
    .hero-grid > *, .limits-inner > * { min-width: 0; }
    .report-stamp { box-sizing: border-box; width: 100%; max-width: 480px; overflow-wrap: anywhere; }
    .lede, .report-stamp p, .report-stamp strong { overflow-wrap: anywhere; }
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
