# SP conversion and liquidation restoration - 2026-10-23

Status: final readiness PASS. All five local suites and both independent adversarial reviews pass. Production installation, activation and continuity are the next authorized steps.

Production source: `8d7c7e7587d0e9cbb693803a90cbbbb715ed85b4`.

The prior global SP reopening is already live. Normal deposits, withdrawals and receipt-backed interest are open. This release restores convenience stablecoin-to-3USD conversions and the incident-restricted ordinary/3USD liquidation paths.

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

## Authorized production sequence after final verification

Root only: fresh live capture, short SP+pool pause, paused inventories, exact backend/pool/SP upgrades, verify installed gzip hashes/public Candid/Running/settings and financial preservation. Configure pool client to the actual SP principal and query it back. Bind a fresh log extension to the original attestation, retire V1, acknowledge V2 through the actual SP, enable ingress, reopen pool then SP, sync only the reviewed frontend assets. Verify served hashes/auth/IDs and at least five-minute continuity. Exclusive dispatch markers precede each mutation; unknown outcomes require authoritative disposition and no blind retry. No rollback across newly admitted state.

Separate native-chain staging/auto controls, bot collateral allowlists, reserve redemptions and unrelated routing are outside this incident restoration. Bounded journals remain fail-closed without eviction.

## Proof limits

No funded production probe, global green CI, merged PR, aged icUSD burn fault injection or isolated nonzero-interest aged fixture is claimed. Synthetic fixture initialization, actual saved-state migration, local settlement and enabled production state are distinct proof levels.

Evidence root: `/private/tmp/rumi-sp-rails-restoration-20261023`. Release manifest, five runtime reports, migration comparisons, interface binding, UI hashes, review reports and exact-once execution receipts are retained separately.

Final readiness file: `FINAL-READINESS-08.json`, SHA256 `77cf1acf72689ff872e9427fbe8a9194eabde836f0cd3a2e7b0cc97b601b06e1`. The ready source/artifact/UI/migration/review bindings were rehashed before admission. Latest live preflight confirms normal interest receipt count290 and preserves53legacy holds, plus the preexisting borrow-fee NotificationPending row forvault51; no journal is declared empty by omission.
