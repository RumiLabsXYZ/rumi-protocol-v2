# Rumi Protocol security remediation status

- **Report date:** November 15, 2026
- **Canonical source:** `dbcc69bcda00c34721f3b3ffb809d887ff93e175`
- **Release input tree:** `baba52eb113ed5c53f9a74cb99204fc3a111f29d`
- **Status:** remediation source merged; production rollout partially complete

## Executive verdict

The accepted active-path remediations listed in the PR #652–#662 tranche below are merged into the canonical repository. The broader review program is **not yet closed**, because the complete finding inventory is still being reconciled and merge and test evidence do not prove that every production canister is running the remediated code.

The AMM upgrade is installed and was verified against an exact pre-upgrade snapshot and state baseline. Analytics remains healthy on its preceding module after its upgrade command was interrupted before installation. The Stability Pool, Treasury, protocol backend, and liquidation bot also still require their coordinated production upgrades and post-upgrade readbacks.

Historical recovery state is materially cleaner than older status reports described: the Stability Pool reports all 237 staged interest packets credited, its state invariant passes, and its pending chain-absorb and unallocated-interest inventories are empty. Current backend payout and Treasury-stable recovery pages are empty, the bot's claim and payment journals are empty, and Treasury's legacy external-deposit backfill reports complete. These are bounded query results, not substitutes for installing the remaining fixes.

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

The following hashes were read through the authenticated management interface between 16:30 and 16:47 UTC on November 15, 2026. This observation supersedes older board snapshots where the values differ.

| Component | Installed module SHA-256 | Current result |
| --- | --- | --- |
| AMM | `66aa67b3be40f27c4471bfbbf99d8bd8c3cd8f189669a89943c7a3ae934d9085` | Remediation installed; pre/post pool values, flags, event counts, pending claims, and unresolved payouts matched. |
| Analytics | `cbb3654eea3f6e5d7c4ae8fdd958319ecc7ba5a26b2ebcdb526247417b7234db` | Running and answering health queries on the preceding module; upgrade still pending. |
| Stability Pool | `ff4988d75e7e97c24130691187ec49ca0304c54505d65cab98c46bb72024f47f` | Running and unpaused on the preceding module; state invariant passed and recovery inventories queried empty. |
| Treasury | `0d7a7cf1f4701af0f8e3c9f661d2f671f59422c68350181dc3d9db4666b8d840` | Running on the preceding module; external-deposit backfill complete, but the new reporter interface is not installed. |
| Protocol backend | `75d2cdc48fdcd444b117d0b1cb3a4630a69cc3a38e9daa90792553aea4a44d29` | Running in General Availability on the preceding module; current payout and Treasury-stable recovery pages queried empty. |
| Liquidation bot | `f27db02758f4f6bbf61659347cea38a45984ac2aa700c9191bd4ad0d45aa9669` | Running and processing on the preceding module; current claim and payment journals queried empty. |

The AMM's pre-upgrade snapshot and fuller baseline showed the same pool values later observed after installation. Its pool values, operational flags, event counts, pending claims, and unresolved payouts matched across the accepted upgrade.

### Broader review components

Additional read-only production checks at 17:03 UTC covered three components from the wider CL-01–CL-17 review that are not release targets in the PR #652–#662 tranche.

| Component | Installed module SHA-256 | Current bounded result |
| --- | --- | --- |
| 3pool | `c17b4e71312eb60cbb1dcaeeabb2efa927ee99fc9788c60a42f9bb2f56117615` | The hash matches the documented October 23 production release from source `8d7c7e7587d0e9cbb693803a90cbbbb715ed85b4`. The pool query returned LP supply `1,485,376,123,260`, balances `[1,190,494,709,789, 2,479,907,838, 2,414,895,284]`, virtual price `1,129,913,116,291,061,964`, admin fees `[40,219, 0, 0]`, zero pending claims, and 561 retained donation receipts below the warning threshold. This is current state evidence for the component implicated by CL-01; it is separate from the AMM installation above. |
| Points | `352e269ee12a35fa40342e84d8e5ac3e3f66a2e09781ee6eff6105b0d20aacf9` | The public epoch status reports no open epoch, current index 21, committed seed state, and the driver disabled. Epoch 18 is recorded closed through the delayed legacy-B recovery path; the point ledger contains 493 rows. The exact source commit for the currently installed bytes has not yet been reconstructed, so these runtime facts do not by themselves prove source-byte identity. |
| Cycle Sentinel | `a863dacbc673eca482919f15de62431a065680aa012a6a0af04e31e9160afad8` | The public self-recovery risk query returned no active delivery-risk record. Canonical source contains the signer-gated unresolved write-off introduced by PR #501, but the exact source commit for the currently installed bytes has not yet been reconstructed. No historical write-off or delivery is inferred from the empty current risk result. |

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
- PR #662: held protocol-payout recovery with post-await authorization and state revalidation.

The final merged commit is `dbcc69bcda00c34721f3b3ffb809d887ff93e175`. The final reviewed PR head and the merge commit share Git tree `baba52eb113ed5c53f9a74cb99204fc3a111f29d`.

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

## Current recovery evidence

The November 15 production queries established the following bounded facts:

- Stability Pool interest recovery: every one of the 237 staged packets is credited; the pool reports no pending chain absorbs or unallocated interest forwards and returns `Pool state is consistent`.
- Treasury external-token deposit backfill: complete with zero ambiguous legacy blocks.
- Backend held-payout page: empty.
- Backend Treasury-stable transfer recovery page: empty.
- Liquidation-bot claim journal: empty.
- Liquidation-bot payment journal: empty.
- AMM unresolved outbound payouts: empty.

These results do not prove that no unknown or unindexed historical obligation exists. They establish only the queried durable inventories at the observation time.

## Remaining release follow-through

The following items remain under this program's user-approved objective to deploy the merged remediation tranche and publish a complete, evidence-separated security report. They are not blanket gates for unrelated protocol work.

1. install and read back the analytics candidate required to deploy merged PR #652 after its AMM dependency;
2. deploy the Stability Pool side of merged PR #657 and verify state, recovery inventories, and the 237 interest receipts remain intact;
3. deploy merged PR #660 to Treasury, preserve its deposit and balance state, confirm its backfill remains complete, configure and read back the exact backend and bot reporters, and only then enable the new receipt-dependent flow;
4. deploy the backend portions of the merged tranche with a meaningful recorded upgrade description while preserving General Availability mode;
5. deploy the liquidation-bot portions of merged PRs #654, #656, and #660, inspect its durable journals, and verify processing resumes only after the coordinated Treasury/backend state is ready;
6. satisfy the program's installation-verification criterion by comparing every installed module hash with its candidate hash and repeating the bounded runtime queries; and
7. satisfy the requested full-review report by publishing a final finding-by-finding matrix that separates source closure, installation, activation, and historical settlement.

Disabled or dormant external-chain rails remain separately gated by chain-specific finality, replay-floor, custody, oracle, admission, and queue checks before activation. Their containment is not evidence of permanent safety.

## Method and disclosure

The program combined source review, executed regression tests, Candid and stable-state checks, independent adversarial review, exact release-artifact verification, production snapshots, module-hash readback, runtime invariant queries, and operation-specific recovery inventories. The knowledge graph was used for orientation; current source and live state remained authoritative.

This public report intentionally omits exploit instructions, private paths, user identifiers, and fund-recovery mechanics. Suspected vulnerabilities should be reported through a private GitHub Security Advisory or to `info@rumiprotocol.com`.

This report does not authorize a future upgrade, feature activation, controller change, transaction, or movement of funds.
