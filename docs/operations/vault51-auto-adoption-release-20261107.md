# Vault 51 automatic collateral adoption — release candidate

## November 10 reconciliation — current source only

**The November 7 binaries and frontend bundle below are superseded and must not
be installed over SEC12. No replacement production build has been run.**

The verified successor is SEC12 source `f1d2e77e2387ab6b9fc9a5b5676047b25d7175ff`,
backend module `2ca84c209d537def37f793588e469e34b3ee7301d672e3963ce7d88803c0fd13`,
recorded in [Dorian's deployed result](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/0f00344208bbf9f46184956fafaceb3fa9961ccb/docs/operations/sec12-release-20261108/RESULT.md).
This is retained November 8/9 release evidence, not a fresh November 10 live read.
Reconciled source `42478174029e6edb54e26578bebfe79a25f576bc` merges that deployed lineage without rewriting history.

Actual overlap: additive private-event dispatcher arms and public entry points
in `event.rs`/`main.rs`, plus Candid/declarations. There were no source conflicts.
Bindings are regenerated from combined Rust rather than accepting their textual
auto-merge as proof. SEC12 state/watermark/cancellation, vault admission checks,
saved-request/Continue/Cancel UI and protocol service files remain byte-identical
to SEC12. Adoption logic and edited adoption UI remain byte-identical to the
November 7 tested source. No new owner preference or lifecycle change is introduced.

Fresh checks passed: 23 adoption/claim/replay tests, four SEC12 cancellation tests,
seven SEC11/native-XRP tests, canonical Candid regeneration/comparison and
backward compatibility against SEC12. Thirteen adoption and 108 SEC12 frontend
tests are reused only for byte-identical covered source; no new bundle is claimed.

Validation results and reconciled source pin are recorded in the companion
`evidence/vault51-auto-adoption-20261107/reconciliation-20261110.json`. The
November 7 builds/export checks below remain historical only. The next expensive
backend/frontend build is to be scheduled with Dorian under the existing measured
allocation; current fresh inventory/artifacts/exports must be pinned then. Dorian
owns independent release review. No deployment, activation or financial action is
authorized by this reconciliation packet.

The accepted preservation rule now also includes the SEC12 cancellation journal
and replay watermark: [deployed result, final preservation paragraph](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/0f00344208bbf9f46184956fafaceb3fa9961ccb/docs/operations/sec12-release-20261108/RESULT.md#L15).
Do not use an older SEC11-only writer as rollback.

Adrian owns the bounded implementation. Rob explicitly approved automatic
adoption after the actual WaterNeuron ICP payout, retaining vault 51, owner,
full outstanding debt, accrued interest and history. Repayment and owner payout
are not prerequisites. This is a concrete preparation packet; **not installed,
not activated, and no custody transfer executed**. Dorian's existing release
procedure and Bram's SEC-08 evidence/PRs 612–613 are reused; vault 158 is unchanged.

## Historical November 7 candidate

Build source `e979cd699874c0d866db18cdd22fd42b2b72c427` on
`codex/vault51-auto-icp-adoption`, based on the actual SEC11 deployed composition
`0eff84c93742ef6679a2ef09b1bd72e34b511665`. Backend remained Running on
`b81583ae81421b88d3bd64bf27976b338b2b09748e9dd6b863849b40ac6fd736` at preflight.
The existing SEC11 state, origin/receipt maps and private journal handling are
preserved. State/Cargo/dependencies and XRP deposit source have no diff from
that base. Arbitrary main or an old snapshot is not a valid rollback.

| Artifact | Bytes | SHA256 |
| --- | ---: | --- |
| Raw backend Wasm | 14,075,845 | fddfe58c7d90e3afb54fc0a5c8f0aa81be44e9779348cb12b3ccb9a1b5620821 |
| Optimized backend Wasm | 9,774,310 | 3b9d4e40b9e005e38db43c3f20ce8db00f6826373e74399800f62836451ff48c |
| Deterministic gzip upload | 2,370,770 | 7e099a2cd01ec3f78315125c3aaa716356c908d9fcf14983edcf8e9a2fbdc681 |

Executable code section: **8,843,099 bytes**, below the unchanged project guard
10,223,616 bytes. This guard is not the ICP network's executable-code ceiling.
Canonical metadata/shrink/O1/gzip packaging passed, and gzip decodes exactly to
the pinned optimized bytes. All **396** application exports match the actual
embedded Candid, with no missing/unexpected application method. Endpoint checking
excludes only lifecycle/timer exports and the pre-existing Rust binary `main`
entry point (unchanged at the deployed source); these are not public API methods.
The embedded Candid equals the canonical source and remains backward-compatible
with the deployed API. No existing claim-stage variant changed.

Frontend build: 740 files / 10,908,754 bytes; every file is pinned in the
[frontend manifest](evidence/vault51-auto-adoption-20261107/frontend-manifest.json).
The actual compiled bundle contains the new method and automatic-adoption
wording. The frontend is built, not published.

## Trigger and financial behavior

A timer registered once per lifecycle checks the exact owner/vault51/request1696/
burn3435761/subaccount hourly, plus one check ten seconds after upgrade. It
cannot submit a fresh nICP withdrawal. The November 7 read-only observation
shows WaitingDissolvement, zero ICP in escrow, no settlement and no payout.
The amount 1,942.82931553 ICP is a recorded receivable, not money received.

On a verified WaterNeuron settlement, retain the claim fence, verify the referenced
canonical native ICP block and dedicated balance, then pin a sweep of
`received - actual ICP fee` from that dedicated subaccount to the backend's normal
default ICP account. The one sweep fee is queried and frozen in the journal;
its selected budget is at most 10,000 e8s. Changed/oversized fees, mismatches or
unknown outcomes hold that original tuple; no shared float or replacement
transfer is authorized. Extra unrelated subaccount deposits are not credited.

After canonical proof of the exact sweep, accrue under the old collateral rules
through cutover and atomically change only vault 51's collateral type/amount.
Preserve its owner, ID, liabilities, interest and public history. Reindex the
collateral/debt buckets and CR/TCR, retire the old receivable's valuation/fence
and keep its terminal receipt/tombstone. Subsequent operations use ordinary ICP
parameters, not special nICP treatment. A replay cannot credit twice or overwrite
later deposits/borrowings. Adoption and owner payout are mutually exclusive;
any pre-existing payout journal is preserved and prevents automatic adoption.

An optional serde-defaulted adoption journal preserves old claims. No new
claim-stage variants or state-map replacement are introduced. The owner/operator
reconciliation method can verify the exact sweep after a lost/expired reply; it
cannot spend tokens or change delivery parameters. Happy-path adoption requires
no additional owner action at arrival. A held ambiguity still requires proof.

## Historical November 7 verification

- Two initial backend regressions failed on the old code for the intended missing
  adoption/fence behavior, then passed.
- 23 focused nICP/claim/state/replay tests passed, including exact receipt tuple,
  fee conservation/payout exclusion, once-only credit and later activity, and
  old CBOR decode/new journal roundtrip.
- Seven SEC11/native-XRP preservation tests passed.
- 13 frontend claim/CR tests passed, including no duplicate claim valuation and
  no terminal display without a receipt. All three edited Svelte components compile.
- Canonical self-check passed in regeneration and comparison modes; canonical
  bindings were regenerated. `didc check` passed against the deployed interface.
- One changed-source locked production backend build passed (1m03s).
- Frontend production build and mandatory auth-origin/custom-domain checks passed.

These are focused deterministic tests and build/export proofs. No full async
WaterNeuron/ledger funded canary, real settlement or live adoption is claimed.
Pre-existing compiler/Svelte warnings remain. A clean project-wide typecheck is
not claimed. Final independent source/artifact review remains with the existing release
owners. No new review team or hosted CI campaign was started for this release-
branch preparation. Any later PR must retain its enforced repository checks;
no CI pass is claimed here.

## Retained release procedure — replacement artifacts required

This procedure must use a new SEC12-composed artifact and current inventory,
not the historical November 7 upload or frontend manifest.

Use the existing backend `tfesu-vyaaa-aaaap-qrd7a-cai`, frontend
`tcfua-yaaaa-aaaap-qrd7q-cai`, mainnet and `rumi_identity`; never substitute a
new canister, old main composition, reinstall or stale-state restore. Re-read
module/settings/controllers, claim51/debt/interest/custody, the completed vault158
record, SEC11 receipts/maps, other retained payouts and processing/reserve holds.
Unexpected divergence requires review of its actual cause, not silently resetting it.

Follow Dorian's accepted stop/drain and snapshot-preservation procedure from the
[SEC11 release](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/b78e8db916835ec78a7c812ef3f09e173b2c5155/docs/operations/sec11-live-release-20261107.md#L15): confirm Stopped, fully archive the
exact oldest managed snapshot before replacing only that slot, preserve the other
nine, verify/download the fresh stopped-state backup, then install the exact
pinned gzip in upgrade mode using the pinned args. Dispatch each management
write once; unknown outcomes stop for readback. Confirm Running, exact installed
hash/API/settings and retained state, including old/new claim decode and SEC11
journals. Do not restore an older schema over later state.

Publish the matching frontend through the existing additive staged-assets/entry-
point procedure, preserving every current asset/property/auth setting. Refresh
current inventory before preparing that concrete mutation. Do not clear assets,
reuse old chunk IDs blindly or repeat an uncertain commit. Verify final served
bytes, actual inventory and primary-domain auth assets.

Production installation/activation has not been executed in this implementation/
preparation assignment. Rob's automatic-adoption direction is already recorded;
no new owner repayment or financial-approval step should be added to the future
verified happy path. The existing SEC11 state-preservation/no-downgrade rule and
normal exact-artifact release review remain in force.

See the [implementation scope](../coordination/vault51-auto-adoption-2026-11-07.md),
[artifact manifest](evidence/vault51-auto-adoption-20261107/artifact-manifest.json),
[release pins](evidence/vault51-auto-adoption-20261107/release-pins.json), and
[exports](evidence/vault51-auto-adoption-20261107/export-verification.json).

Accepted state-preservation source: [SEC11 release lines 17 and 77](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/b78e8db916835ec78a7c812ef3f09e173b2c5155/docs/operations/sec11-live-release-20261107.md#L77).
The stop/drain/snapshot procedure is [line 15](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/b78e8db916835ec78a7c812ef3f09e173b2c5155/docs/operations/sec11-live-release-20261107.md#L15).
These are retained release controls, not newly invented optional work gates.
