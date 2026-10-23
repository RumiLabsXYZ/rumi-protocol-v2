# SP conversion and liquidation restoration - 2026-10-23

Status: DEPLOYED AND REOPENED. Backend, pool and SP candidate08 upgrades succeeded; pool opened at19:29:11UTC and SP resume completed at19:29:26UTC on2026-10-23. Frontend assets are published and all726 served HTTPS body hashes match the reviewed delivery. Production continuity checks passed.

Production source: `8d7c7e7587d0e9cbb693803a90cbbbb715ed85b4`.

Normal deposits, withdrawals, receipt-backed interest, stablecoin-to-3USD conversions and the incident-restricted ordinary/3USD liquidation paths are open. The actual SP is configured as the pool receipt client, V2 reserve ingress is enabled and client-ready, and historical V1 ingress is retired after reconciliation.

## Real blockers fixed

- Persist exact caller-funded conversion receipts, LP minimum, fee budget and refunds; preserve unknown transfer tuples and positive aged receipt recovery.
- Save the configured SP client's pool receipt before preflight awaits. Failed preflight has a durable no-effect result that fences delayed originals. Public unfunded requests retain no-row preflight. New stable memory46 defaults unset; existing pool admin must configure the actual SP client before reopening.
- Ordinary prepare cancellation binds the full request and frozen payment ledger. An absent prepare receives an immutable cancellation tombstone; delayed prepare cannot replace it. Started payment and payout phases cannot be cancelled.
- 3USD V2 retires exact Unseen/AdmissionPending requests under its existing admission/key guards. Submitted, confirmed, refund and settled operations retain their original recovery identities.
- Preserve wide-u128 pending payout keys across actual upgrades; use native ICP history for native collateral proof and bounded authenticated archive callbacks for ICRC-3 ledgers.
- Recover exact wallet approvals with explicit Oisy consent and click-time signer entry.

## Exact release files

Installed module hashes are the gzip hashes below. Executable sections remain below the network12MiB limit minus the agreed256KiB reserve.

| Canister | Gzip SHA256 | Executable code bytes |
| --- | --- | ---: |
| rumi_protocol_backend | `59e5a416454271b42dfb6e75c5e842567c9458eca39688cbd52b68d0ee994483` | 8790198 |
| rumi_3pool | `c17b4e71312eb60cbb1dcaeeabb2efa927ee99fc9788c60a42f9bb2f56117615` | 2999717 |
| stability_pool | `827e272ab4a72459e9eee99a24d3e1994083ddf135825b9ec2a9d92e7f6db2ca` | 4922952 |

## Verification obtained

- Scoped units: SP289, backend1313, pool185, fixture6 passed; two preexisting ignored tests remain.
- Source/public Candid and actual deployed-backend compatibility:18checks passed; existing SP input metadata drift is explicitly preserved.
- Actual saved-state migration: SP247steps, pool25steps;13positions,287snapshot receipts,237sealed recovery packets,46vaults,53legacy holds,2489blocks,16LP balances,126payout identities,93donations and15swap receipts preserved. Backend179legacy fields differ only by two documented Sunset oracle timestamp invalidations. New journals are empty. Pool new memories44/45/46 are empty.
- Exact-artifact local runtimes: standard130checks/452calls; agedconversion51/281; agedbackend115/522; agedpool42/191. These include lost replies, two-day aging, normal pending upgrades, wrong-proof rejection, positive proof adoption, exact balances/debt/fees and replay idempotence.
- Frontend20conversion and21wallet-binding tests, offline build and auth/custom-domain checks pass. All534build inputs and74generated declaration files are bound. Typecheck retains30existingerrors66warnings with identical fingerprints and no introduced diagnostics.
- Final frontend delivery:729files, treeSHA256 `4a5843eefd53aa934df00addf52d1adc9b8b37a99c9ff67b05b68407c4834076`.
- Two fresh independent adversarial reviews O and P returned final PASS with no unresolved blocker after validating all five runtime suites.

The additional no-admission runtime passed79checks/703typedcalls. All five suites total417checks/2149calls. It uses real pool preflight rejection/refund and executable delayed replay, a real stopped-backend never-admitted prepare followed by normal owner repayment/closure, and V2 no-effect retirement. The AdmissionPending setup is explicitly synthetic and changes only one no-dispatch journal, with every other State field/virtual memory compared; an actual normal pending upgrade follows. No fabricated transfer proof or production initializer change is used.

## Historical reserves and protected recovery

The separate historical cutover reconciliation is in [sp-rails-3usd-cutover-reconciliation-20261023.md](sp-rails-3usd-cutover-reconciliation-20261023.md).

Exact attestation SHA256: `735beb8ca505532c75b1c322188435bd170358aed7dd1672adf286b0981650a2`.

Preserve3USD book8219310823raw, physical8471101833raw, hashed balance0 and unallocated excess251791010raw. All2489pool blocks and144backend liquidation events were reconciled. Do not credit, sweep, refund, spend or reclassify the excess. Keep all237completed recovery packets and53legacyNone holds; never replay recovery or call finish again.

## Completed production rollout

Root executed all11 actions once, with an exclusive dispatch marker before each request: pauseSP, pausePool, backend upgrade, pool upgrade, SP upgrade, configure actualSP receipt client, attest historicalV1 reconciliation, actualSP V2 acknowledgment, enableV2 ingress, openPool and resumeSP. Every receipt exited0; no unknown outcome or blind retry occurred. The historical state moved Quarantined→Reconciled at attestation and Reconciled→Retired atomically with V2 enablement.

Installed gzip hashes/public Candid, Running state, management settings and controls were verified. The frontend sync was submitted once after source/build/delivery binding; the asset module, management settings and Commit/ManagePermissions/Prepare permission sets remain unchanged. All726 published assets were paginated and their metadata and actual HTTPS body hashes match; `/stability-pool` serves the reviewed index with correct production principals. The three `.ic-assets.json` recipe sidecars are correctly excluded from the729-file delivery publication inventory.

## Live preservation and continuity

- Paused upgrade comparison:40checks passed, including all46 recorded vault debts/owners/collateral and all13SP positions unchanged across installation.
- Reopened and later continuity:84checks passed. Every continuity request began at least370.571seconds after the actual resume receipt, and capture completed19:36:12UTC. All13SP positions, backing,290processed receipts,237completed recovery packets,53legacyNone holds,126settled payout identities and preexisting pending journals remain exact.
- The one preexisting borrow-fee NotificationPending row forvault51 is preserved; no claim that all journals are empty is made.
- Pool and SP are open; actualSP client is set; reserve ingress is enabled/client-ready; V1 is Retired; new V2 inventories and the pool active receipt fence are empty. Reserve book, physical balances, excess and2489-block historical tip remain unchanged.
- Stored vault debts correctly continue to accrue interest. A separate194-check classification lists every vault's exact debt change:36positive debts grew by0.01141290icUSD at the first read and0.01892083icUSD cumulatively by the continuity read. All other46vault public fields remain exact. Captured AccrueInterest events530911 at19:27:44UTC and530915 at19:32:44UTC bracket these changes. The complete2000-event tail contains1587price updates,412interest accruals and the authorized upgrade; no borrow, repayment or liquidation events. TimerB records accrual, updates stored debt, then harvests interest for distribution. This classifies observed growth against the deployed path; it does not independently reproduce each dynamicAPR calculation.
- No new SP interest receipt arrived in this observation window. The later treasury snapshot records0.29694829icUSD pending distribution; that later snapshot is not equated to either earlier debt delta.

## Controls still retained

| Control | Current state and reason |
| --- | --- |
| Old V1 reserve ingress | Retired after historical reconciliation; normal3USD liquidation uses enabledV2. This is not an outstanding reopening blocker. |
| Separate legacy receipt-swap / authorized redeem-and-burn endpoints | Remain disabled in production. Their old implementations lack bounded ambiguous-leg recovery / persisted burn identity (`src/rumi_3pool/src/lib.rs:1271`, `:1288`, `:5070`). The restored convenience conversion and V2 liquidation paths use their reviewed replacement protocols; these old endpoints are not a reopening dependency. |
| Foreign/native-chain SP automation | Disabled, with no configured chain pools in the captured SP inventory. Enabling additional chain rails requires their own configured pools and settlement evidence; it is outside restoration of the existing rails. |
| Bot collateral allowlist | Existing ICP-only policy retained. This limits that bot's collateral selection; it does not re-pause the restored ordinary SP receipt path. No accepted finding requires expanding the allowlist to reopen these rails. |
| Reserve redemptions | Existing disabled setting retained. This separate reserve-redemption feature is not the restored conversion/liquidation ingress. |
| Legacy recovery holds |53legacyNone holds remain quarantined; completed237packets are preserved. Reopening never replays recovery or spends unallocated reserve excess. |

Bounded receipt journals still fail closed at capacity without eviction. No outstanding blocker remains for the incident-restricted conversion and ordinary/3USD liquidation paths restored here.

## Proof limits

No funded production probe, global green CI, merged PR, aged icUSD burn fault injection or isolated nonzero-interest aged fixture is claimed. Synthetic fixture initialization, actual saved-state migration, local settlement and enabled production state are distinct proof levels.

Evidence root: `/private/tmp/rumi-sp-rails-restoration-20261023`. A compact durable copy is retained in the primary checkout at `.recovery-artifacts/rumi-sp-rails-restoration-20261023/`. Release manifest, five runtime reports, migration comparisons, interface binding, UI hashes, review reports, exact-once execution receipts and actual live/served verification are retained separately.

Production proof files: `paused-install-preservation-08.json`, `LIVE-RESTORATION-08.json`, `interest-accrual-continuity-08.json`, `frontend-production-sync08-02/verification.json`, and `frontend-served08-03/verification.json`. Source reviewers O/P passed before deployment; separate post-production O/P audits verify operations and served evidence. PR590 records the implementation; no merge or aggregate-green CI claim is made.

Final readiness file: `FINAL-READINESS-08.json`, SHA256 `77cf1acf72689ff872e9427fbe8a9194eabde836f0cd3a2e7b0cc97b601b06e1`. The ready source/artifact/UI/migration/review bindings were rehashed before admission. Latest live preflight confirms normal interest receipt count290 and preserves53legacy holds, plus the preexisting borrow-fee NotificationPending row forvault51; no journal is declared empty by omission.
