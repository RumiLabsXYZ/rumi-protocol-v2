# Vault 51 automatic collateral adoption — release candidate

Adrian owns the bounded implementation. Rob explicitly approved automatic
adoption after the actual WaterNeuron ICP payout, retaining vault 51, owner,
full outstanding debt, accrued interest and history. Repayment and owner payout
are not prerequisites. This is a concrete preparation packet; **not installed,
not activated, and no custody transfer executed**. Dorian's existing release
procedure and Bram's SEC-08 evidence/PRs 612–613 are reused; vault 158 is unchanged.

## Exact candidate

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
cannot submit a fresh nICP withdrawal. The current read-only observation still
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

## Verification

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

## Existing release procedure and remaining boundary

Use the existing backend `tfesu-vyaaa-aaaap-qrd7a-cai`, frontend
`tcfua-yaaaa-aaaap-qrd7q-cai`, mainnet and `rumi_identity`; never substitute a
new canister, old main composition, reinstall or stale-state restore. Re-read
module/settings/controllers, claim51/debt/interest/custody, the completed vault158
record, SEC11 receipts/maps, other retained payouts and processing/reserve holds.
Unexpected divergence requires review of its actual cause, not silently resetting it.

Follow Dorian's accepted stop/drain and snapshot-preservation procedure from the
[SEC11 release](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/b78e8db916835ec78a7c812ef3f09e173b2c5155/docs/operations/sec11-live-release-20261107.md): confirm Stopped, fully archive the
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
