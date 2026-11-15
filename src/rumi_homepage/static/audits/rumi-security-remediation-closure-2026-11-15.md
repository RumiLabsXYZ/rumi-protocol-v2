# Rumi Protocol security remediation status

- **Report date:** November 15, 2026
- **Canonical source:** `f72ede8195b23299c297fb24f37ac76d7d957269`
- **Release input tree:** `18b286ab639f714cb10bbf37cdae0f795d1cad06`
- **Status:** accepted active-path remediation merged and installed; all five PR #666 dependency-refresh canisters hash-matched; frontend published; explicit historical holds preserved

## Executive verdict

The accepted active-path remediations listed in the PR #652–#662 tranche and the PR #666 wallet-session/dependency closure are merged into the canonical repository. All five PR #666 dependency-refresh candidates—backend, liquidation bot, Stability Pool, AMM, and Analytics—are installed and their exact module hashes have been read back. The wallet-session frontend is published. Before the Stability Pool retry, the earlier requests were reconciled as expired, the prior module was read back, and no install process was running.

The AMM, Analytics, Stability Pool, Treasury, protocol backend, and liquidation bot upgrades are installed. State baselines, operating modes, recovery queues, reporter authorization, and pause/resume state were checked before and after the coordinated rollout.

Historical recovery state is materially cleaner than older status reports described: the Stability Pool reports all 237 staged interest packets credited, its state invariant passes, and its pending chain-absorb and unallocated-interest inventories are empty. Current backend payout and Treasury-stable recovery pages are empty, the bot's claim and payment journals are empty, and Treasury's legacy external-deposit backfill reports complete. These are bounded query results, not proof that every possible historical obligation has been indexed.

## Evidence model

Rumi records the following as separate evidence stages:

1. a source change exists;
2. focused tests and compatibility checks pass;
3. the change is merged into the canonical repository;
4. an exact release artifact is built and identified;
5. the artifact is installed and its module hash is read back;
6. the intended runtime mode or feature is active; and
7. any historical obligation is settled using operation-specific evidence.

Evidence at one stage does not establish a later stage.

## Current production snapshot

The following hashes were read through the authenticated management interface during the coordinated November 15, 2026 rollout. This observation supersedes older board snapshots where the values differ.

| Component | Installed module SHA-256 | Current result |
| --- | --- | --- |
| AMM | `3a59c167c0df19fadf1ecfd9d01d334fc380fbfc43830fb79e0618adca7185e6` | PR #666 dependency-refresh candidate installed; pool reserves, LP supply, flags, and empty unresolved-payout inventory preserved. |
| Analytics | `704f654800cd43cd5fd0b690776e7cac1a882b39ec2b1d76760ed6833c51f290` | PR #666 dependency-refresh candidate installed; collector state, cursors, and pull schedule preserved. |
| Stability Pool | `d8385398e5b92ca26b029b6b76ab78d634f40ced65d1f0af5b991591332329e9` | PR #666 dependency-refresh candidate installed after the earlier requests expired. The pool remained open and consistent with 13 depositors, deposits `1,889,347,934,087`, interest `9,331,996,690`, all 237 recovery packets credited, and empty queried recovery queues. |
| Treasury | `785adb02c314d49244a1f2db5f1923ec9a24440dbe5a7698525b0dce1a0c3832` | Exact candidate installed; deposit count and balances preserved, backfill complete, exact backend and bot reporter set installed, and ingress resumed. |
| Protocol backend | `59a64f78e0e652d0e48413aba587d3f826c1dc25258f56ab09575c59de444111` | PR #666 dependency-refresh candidate installed; General Availability mode, zero protocol deficit, and untripped liquidation breaker preserved. |
| Liquidation bot | `941cd13a3d4024349844d888a8aa1452483aa5dc49cc5599605c7a2fccbab17a` | PR #666 dependency-refresh candidate installed; processing paused automatically, claim/payment journals and backend claim inventory were empty, and processing was explicitly resumed. |

The AMM's pre-upgrade snapshot and fuller baseline showed the same pool values later observed after installation. Its pool values, operational flags, event counts, pending claims, and unresolved payouts matched across the accepted upgrade.

### Broader review components

Additional read-only production checks at 17:03 UTC covered three components from the wider CL-01–CL-17 review that are not release targets in the PR #652–#662 tranche.

| Component | Installed module SHA-256 | Current bounded result |
| --- | --- | --- |
| 3pool | `c17b4e71312eb60cbb1dcaeeabb2efa927ee99fc9788c60a42f9bb2f56117615` | The hash matches the documented October 23 production release from source `8d7c7e7587d0e9cbb693803a90cbbbb715ed85b4`. The pool query returned LP supply `1,485,376,123,260`, balances `[1,190,494,709,789, 2,479,907,838, 2,414,895,284]`, virtual price `1,129,913,116,291,061,964`, admin fees `[40,219, 0, 0]`, zero pending claims, and 561 retained donation receipts below the warning threshold. This is current state evidence for the component implicated by CL-01; it is separate from the AMM installation above. |
| Points | `352e269ee12a35fa40342e84d8e5ac3e3f66a2e09781ee6eff6105b0d20aacf9` | The public epoch status reports no open epoch, current index 21, committed seed state, and the driver disabled. Epoch 18 is recorded closed through the delayed legacy-B recovery path; the point ledger contains 493 rows. A local release artifact exactly matches the installed hash and the strongest reviewed source candidate is PR #608 at `c173ab14`, but the source-to-build record is incomplete. The binding is therefore partial rather than cryptographically complete. |
| Cycle Sentinel | `a863dacbc673eca482919f15de62431a065680aa012a6a0af04e31e9160afad8` | The public self-recovery risk query returned no active delivery-risk record. The installed hash exactly matches the preserved production artifact built from `5f5ff064` (PR #662 head), whose recorded locked release build passed GitHub Actions run `43734270918`. No historical write-off or delivery is inferred from the empty current risk result. |

The broader finding matrix preserves those provenance limits instead of treating an empty live inventory or a newer module hash as proof that every historical obligation was settled.

## Merged remediation tranche

The canonical source includes the following independently merged work:

- PR #652: analytics overlap and retained-event cursor safety;
- PR #653: redemption-fee decay correction;
- PR #654: liquidation-bot oracle floor;
- PR #655: high-decimal conversion safety;
- PR #656: bot debt and interest alignment;
- PR #657: exact legacy ambiguous Stability Pool burn compensation;
- PR #658: zero-debt accrual-clock correction;
- PR #659: commit-time liquidation excess gate;
- PR #660: Treasury transfer-receipt accounting journal;
- PR #661: native-XRP admission, capacity reservation, and late-funding tombstones; and
- PR #662: held protocol-payout recovery with post-await authorization and state revalidation; and
- PR #666: wallet-session binding, exact raw repayment amounts, partial repay-and-close recovery, and patched Rust dependency resolution.

The current merged commit is `f72ede8195b23299c297fb24f37ac76d7d957269`. PR #666 head `a059d40ba1e6fe9bec0488102a197eafbf271e34` and the merge commit share Git tree `18b286ab639f714cb10bbf37cdae0f795d1cad06`.

## Release artifact evidence

GitHub Actions run `43734270918` built the eight raw Rust canister Wasms from reviewed head `5f5ff064d520c7eb60ea098c4112ce7ce296ba16` with Rust 1.92.0 and the checked-in lockfile. The downloaded artifact's `SHA256SUMS` verified successfully.

Those CI outputs are raw Cargo Wasms, not direct production uploads. The release process applied the repository's canister-specific Candid metadata, shrinking, optimization, production-environment marker, and compression rules. Six canisters changed in this remediation tranche and therefore have production candidates below. Points and Cycle Sentinel were included by the general CI workflow but are not changed or deployment targets in this tranche.

| Candidate | Upload SHA-256 |
| --- | --- |
| AMM | `66aa67b3be40f27c4471bfbbf99d8bd8c3cd8f189669a89943c7a3ae934d9085` |
| Analytics | `d6c59e870590964c4be3c950e460d1fee83abf610d12f6b79f627847a499f398` |
| Stability Pool | `32cf7dac324e97d8a6d6ddef93902c89dee2bec93c5b51521cf10e260ba83cce` |
| Treasury | `785adb02c314d49244a1f2db5f1923ec9a24440dbe5a7698525b0dce1a0c3832` |
| Protocol backend | `caaf7398c04d2f71a75115f4199fa5e429c30a6c8563c874e25d884b28703bd1` |
| Liquidation bot | `dadce6c52a79d815a46fbaf2f1c954495d07edf5f8a4f2ef7662cac20bd6cbb2` |

The backend candidate passed the mainnet XRPL-environment marker check. A candidate hash is build evidence only until the same hash is observed as installed.

PR #666 exact-head GitHub Actions run `43763172823` built the eight raw Rust canister Wasms from `a059d40ba1e6fe9bec0488102a197eafbf271e34`; its published `SHA256SUMS` verified locally. The deployment-format artifacts were rebuilt from the identical merged tree and produced backend `59a64f78…44111`, bot `941cd13a…ab17a`, Stability Pool `d8385398…329e9`, AMM `3a59c167…185e6`, and Analytics `704f6548…1f290`. All five hashes are installed and were read back through the authenticated management interface.

## Current recovery evidence

The November 15 production queries established the following bounded facts:

- Stability Pool interest recovery: every one of the 237 staged packets is credited; the pool reports no pending chain absorbs or unallocated interest forwards and returns `Pool state is consistent`.
- Treasury external-token deposit backfill: complete through deposit 2,829 with 60 fail-closed tombstones for legacy rows that lack exact ledger provenance.
- Backend held-payout page: empty.
- Backend Treasury-stable transfer recovery page: empty.
- Liquidation-bot claim journal: empty.
- Liquidation-bot payment journal: empty; the backend reports no active bot claims. Three historical terminal `TransferFailed` history entries remain, but none has a live backend claim and none was retried or erased during this rollout.
- AMM unresolved outbound payouts: empty.

These results do not prove that no unknown or unindexed historical obligation exists. They establish only the queried durable inventories at the observation time.

## Residual evidence and recovery holds

The original six-canister remediation release and the later five-canister PR #666 dependency refresh are complete. The following items remain explicit evidence limitations or recovery holds, not active-path remediation blockers, hidden claims of settlement, or blanket gates for unrelated protocol work.

1. keep the legacy ambiguous Stability Pool burn case held until exact authenticated archive proof exists;
2. preserve the 60 Treasury tombstones so legacy blocks without exact ledger provenance cannot later be accepted as fresh receipts;
3. retain the partial Points source-to-build provenance classification unless stronger historical build evidence is recovered; and
4. repeat bounded durable-inventory queries periodically, because an empty queue at one observation time cannot prove that no unknown historical obligation exists.

### Dependency follow-up

A fresh production-scope npm advisory query completed on November 15 with zero high or critical findings, nine moderate findings and six low findings. The reported chains are concentrated in the frontend build/static-site toolchain; this is not a claim that they are reachable in deployed static assets, and it is not a clean-audit claim.

The same review found that the Rust production graph had regressed to `bytes` 1.9.0, within the affected range for RUSTSEC-2026-0007. PR #666 updates `bytes` to 1.11.1, removes two unused state-machine dependencies, and applies compatible patched releases across the lockfile. A fresh RustSec inventory fell from 60 vulnerability matches and 35 warnings to three reviewed non-applicable matches and nine exact maintenance or yanked-package notices. The remaining matches are an inactive optional `rkyv` feature and `crypto-bigint` operations that Rumi does not use with local secrets; both are recorded as exact, expiring policy tuples. Locked Wasm-target checks pass for all nine production canisters. Backend, liquidation bot, Stability Pool, AMM, and Analytics now run those dependency updates.

### Frontend wallet-session follow-up

PR #666 updates the enabled create-vault, borrow, collateral-add, partial-withdrawal, close, repayment, and recovery follow-up paths. Mutations, vault refreshes, balance refreshes, shared stores, and local caches remain bound to the submitting wallet owner, provider, and session generation across asynchronous boundaries. Decimal input is converted directly to exact raw integer units rather than through floating-point truncation, and a confirmed financial action is not reclassified as failed merely because a later display refresh fails. The final full frontend unit suite passes 1,740 tests, including 34 focused repayment tests, and the production build plus authentication-asset verifier pass. PR #666 is merged and the asset-only publication synced 759 vault assets while preserving the asset-canister Wasm and controllers. Real extension account-switch and multi-tab behavior still requires served-wallet validation; publication alone is not that proof.

The broader ICRC-3 archive helper accepts only a bounded callback selected by a configured ledger; reviewed public paths do not let an unprivileged caller choose that ledger or archive. Under the [ICP execution model](https://docs.internetcomputer.org/references/ic-interface-spec/), replicated inter-canister execution prevents a single boundary node from forging the reply, but the design still trusts the configured ledger's selected archive rather than independently applying the [ICRC-3 certified-chain procedure](https://github.com/dfinity/ICRC-1/blob/main/standards/ICRC-3/README.md). Rumi therefore keeps sensitive direct-only recovery paths fail-closed. Certified hash-chain verification remains defense-in-depth if independently controlled archives are moved outside that trusted boundary; it is not presented as an active unauthenticated exploit.

Disabled or dormant external-chain rails remain separately gated by chain-specific finality, replay-floor, custody, oracle, admission, and queue checks before activation. Their containment is not evidence of permanent safety.

## Method and disclosure

The program combined source review, executed regression tests, Candid and stable-state checks, independent adversarial review, exact release-artifact verification, production snapshots, module-hash readback, runtime invariant queries, and operation-specific recovery inventories. The knowledge graph was used for orientation; current source and live state remained authoritative.

This public report intentionally omits exploit instructions, private paths, user identifiers, and fund-recovery mechanics. Suspected vulnerabilities should be reported through a private GitHub Security Advisory or to `info@rumiprotocol.com`.

This report does not authorize a future upgrade, feature activation, controller change, transaction, or movement of funds.
