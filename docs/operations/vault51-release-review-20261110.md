# Vault 51 release review — November 10, 2026

Reviewer and accountable release owner: **Dorian**. Source owner: **Adrian**. ROPS-09 records this handoff; SEC-08 remains the implementation/case card.

## Decision

**PASS for bounded source/artifact preparation. HOLD installation and automatic activation.** No confirmed blocking source finding remains in the reviewed scope. This review performs no customer action, custody transfer, stop/start, installation or frontend publication. It is not deployment authority or evidence of actual settlement/adoption. Merritt has accepted this exact source/artifact preparation. No repeat source review, tests or build is required. Deployment, automatic activation and financial execution remain held pending Rob’s exact release decision.

Frozen source `42478174029e6edb54e26578bebfe79a25f576bc`, reconciled packet `b16a87f8a10a978ef0e759d131f96c5a5d5ab229`. Deployed SEC12 `f1d2e77e` is an ancestor; this review freshly read its backend Running on `2ca84c209d537def37f793588e469e34b3ee7301d672e3963ce7d88803c0fd13`. All 17 recorded source identity proofs were independently compared to Git blobs. Adoption code is unchanged from reviewed `e979cd69`; SEC12 state, cancellation/watermark, vault admission and saved-request services/UI are retained. Dispatcher/API additions coexist; canonical combined bindings were regenerated. November 7 artifacts remain superseded and must not be installed.

## Substantive findings

- No confirmed blocker in target/auth scope. `adoption.rs:26–34,62–107` binds the approved claim and original vault/owner/type, rejects payout obligations and incompatible custody/configuration. It does not initiate a fresh WaterNeuron withdrawal (`249–260`).
- Actual settlement and custody, not the quoted receivable, gate adoption: `nicp_redemption.rs:417–466` obtains the exact dedicated WaterNeuron request and its canonical native-ledger receipt. `adoption.rs:266–302` rechecks that receipt and dedicated balance, bounds and journals the one sweep fee, and credits only net verified settlement. Additional unrelated escrow deposits are not credited. Native ledger/archive trust follows existing `native_icp_proof.rs:121–164`; this is not an external certified block-hash proof claim.
- `adoption.rs:198–237` verifies the exact escrow-to-default-account transfer, amount, fee, memo, timestamp and ordering. Unknown results retain the immutable tuple; the existing ledger helper pins fee, nonce/memo and created-at time. Reconciliation cannot select new spending parameters.
- `adoption.rs:111–187` requires a verified sweep before the atomic credit, accrues the old interest period through cutover and changes only collateral type/amount. Owner, vault ID, debt, accrued interest and history remain. Index-derived debt/collateral buckets and CR/TCR are refreshed by `event.rs` claim-event application. The once-only completion check prevents overwriting subsequent normal deposits/borrowings.
- `nicp_redemption.rs:495–505` excludes owner payout for the target or any adoption journal; an existing owner-payout journal instead prevents adoption. The shared vault guard serializes claim operations. The serde-default journal and private events retain terminal receipt/tombstone and replay behavior.

Release boundary: `adoption.rs:40–59` registers an hourly timer and a check ten seconds after upgrade. **Installing this backend activates the code path that can sweep actual settlement automatically.** This is not an inert installation with a later separate toggle. Any future decision must explicitly preserve the existing installation/activation/financial authority boundary; no repayment or new owner-preference prerequisite is added to Rob’s already approved automatic happy path.

## Verification and exact artifacts

Adrian's fresh composed-source checks: 23 nICP/claim/replay, four SEC12 cancellation, seven SEC11/native-XRP; canonical regeneration/comparison and backward compatibility with SEC12 passed. Existing 13 adoption and 108 SEC12 frontend checks reused because covered sources are identical. This review did not rerun those tests or start hosted CI, extra reviewers or a broad audit.

One serialized production backend/frontend build passed. Cargo 72 seconds; two jobs/two Binaryen cores; existing locked offline dependencies. Actual embedded API equals combined canonical source; all **397** public methods are exported. Sole additional query/update export is the existing internal timer executor. Gzip decompresses exactly to optimized Wasm. All **740** frontend manifest entries independently match. Mandatory auth/domain postbuild checks passed.

| Artifact | Bytes | SHA256 |
| --- | ---: | --- |
| backend-raw.wasm | 14127173 | `bf5aa9b5049a59cfb3802ec710d132393d61c3db8af7f32d61fbeaef406b80d3` |
| backend-optimized.wasm | 9813786 | `941224b5ae5878e889e8fc864887f2b22e18949d08f6a9ec700204519d07d5d8` |
| backend-optimized.wasm.gz | 2378408 | `43bbbca98df29296a7dde2f134ce15db519bb745c53c1fcf442ef97b54223f09` |

Code section: **8,880,660 bytes**, below the unchanged **10,223,616-byte project guard**. This is a project reserve, not the network limit. See [review pins](evidence/vault51-dorian-review-20261110/review-pins.json), [actual artifact verification](evidence/vault51-dorian-review-20261110/artifact-verification.json), [frontend manifest](evidence/vault51-dorian-review-20261110/frontend-manifest.json), and [source reconciliation](evidence/vault51-auto-adoption-20261107/reconciliation-20261110.json). Prepared upgrade arguments preserve mode and explicitly name automatic timer registration.

## Remaining limits and next decision

Merritt accepted source/artifact preparation on November 10. The next decision
is Rob’s authorization of the exact release below, including its automatic
financial behavior. His automatic-adoption preference is already settled; do
not request a new repayment, owner payout or preference decision. Season 2
accounting remains higher priority, and this packet adds no build/review work.

## Exact release decision for Rob — HELD

Proposed authorization: upgrade the existing mainnet backend
`tfesu-vyaaa-aaaap-qrd7a-cai`, using `rumi_identity`, to the frozen source and
artifacts above; activate the matching frontend assets on existing
`tcfua-yaaaa-aaaap-qrd7q-cai` through the additive staged publication procedure.
Do not create/reinstall canisters, change controllers/settings, or deploy the
whole stack. Use only the November 10 SEC12-composed artifacts, never the
historical November 7 upload/bundle.

**Backend installation itself registers the hourly and ten-second automatic
checks. It authorizes the eventual verified escrow-to-default-account sweep and
same-vault ICP adoption described below; it is not an inert installation with
a separate later activation toggle.** The automatic happy path needs no further
owner action after the actual payout. All installation/activation/financial
holds remain in place until this exact decision is approved.

### Frozen inputs

The source, raw/optimized/upload bytes and hashes are in the artifact table
above. Additional exact pins:

| Input | SHA256 |
| --- | --- |
| Canonical/embedded backend Candid | `43da24f0341ca8fe52837071020e910f9ff3ecdef83a04f0dbc8f6e3033334db` |
| November 10 frontend manifest, 740 files | `79e75bd3d0f6016b4d4ae968a83d79b6b558dcf35828c595aad0b6e96afac6a3` |
| Upgrade args below | `c6b21cbd1abc383aa85b6f2f821cd35c75a5a60597189e9b79ca296add4634c2` |

Pinned [upgrade args](evidence/vault51-dorian-review-20261110/upgrade-args.did)
are `Upgrade` with `mode = null`, preserving the existing mode. The description
explicitly states automatic timer registration. The upload is the already built
`backend-optimized.wasm.gz`, not a recipe rebuild. Prior deployed source/module
are `f1d2e77e2387ab6b9fc9a5b5676047b25d7175ff` /
`2ca84c209d537def37f793588e469e34b3ee7301d672e3963ce7d88803c0fd13`.
These are recorded baselines, not a claim of a new live read in this update.

### Authorized sequence if Rob approves

1. Re-read current module/settings/controllers and frontend inventory/permissions,
   then privately capture exact claim/vault/owner/debt/interest/custody, completed
   vault158 record, SEC11/SEC12 journals, retained payouts and processing/reserve
   holds. Reconcile actual drift before overwriting anything. Reuse valid source,
   build and review evidence; do not rebuild or repeat a review campaign.
2. Stage only the matching new frontend asset keys through the existing additive
   procedure; preserve every existing asset, property and auth setting. Verify
   staged bytes before backend mutation. Prepare the exact entry-point/content
   delta from that fresh inventory; never clear assets or blindly reuse chunk IDs.
3. Follow the existing [SEC11 stop/drain/snapshot preservation procedure](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/b78e8db916835ec78a7c812ef3f09e173b2c5155/docs/operations/sec11-live-release-20261107.md#L15):
   controller-confirm Stopped, fully archive the exact oldest managed snapshot
   before replacing only that slot with a fresh stopped-state backup, preserve
   the other nine entries, and download/verify the fresh backup. No standalone
   snapshot deletion or restoration is included.
4. Install the exact pinned gzip in **upgrade** mode using the pinned args,
   dispatching once. Verify installed optimized hash, API and settings while
   stopped, then start once. The upgrade registers the automatic checks; once
   scheduled they may progress a verified settlement without another manual toggle.
   Compare retained state against the captured baseline, including vault158,
   SEC11 receipts and SEC12 cancellation/replay fences.
5. Activate only the prepared frontend entry-point/content delta. Verify final
   full inventory, changed served bytes, primary domain and authentication assets.
   Preserve the asset-canister module/settings and permission lists.
6. Read back adoption state and available custody/ledger receipts. A Running
   module proves deployment only; report Waiting, verified settlement, verified
   sweep and completed collateral adoption separately. If the automatic check
   has already progressed custody, reconcile its exact immutable journal/receipts
   rather than repeating a transfer or assuming state must remain byte-equal.

Every management/asset write is dispatched once with durable evidence. Unknown
outcomes mean stop and reconcile by readback, never automatic replay. No old
writer/snapshot rollback may discard SEC11 receipts or SEC12 cancellation state;
recover forward. These are existing accepted release controls, not new tests.

### Exact automatic financial scope

Only the existing vault51 claim/request1696/burn3435761 and its bound original
owner/dedicated subaccount qualify. No new WaterNeuron withdrawal is initiated.
The recorded 1,942.82931553 ICP receivable is not treated as money received.
After actual WaterNeuron settlement, require the canonical native-ledger receipt
and dedicated balance. Sweep only the verified received amount minus its one
queried/frozen ICP fee (bounded by 10,000 e8s) into the backend default ICP account;
unrelated escrow deposits and shared float are excluded. Unknown results retain
the original immutable tuple for proof-based reconciliation, not replacement spend.

Only after exact sweep verification, accrue the old interest period through
cutover and atomically switch the same vault to ordinary ICP collateral. Preserve
owner, vault ID, full outstanding debt, accrued interest and history; refresh
collateral/debt buckets and CR/TCR and retire the old receivable/fence with its
terminal receipt. Ordinary ICP interest/liquidation parameters apply thereafter.
Owner payout and adoption are mutually exclusive; an existing payout journal
prevents adoption. Repayment, owner payout, customer messaging, additional loans,
other collateral movement and liquidation-bot configuration changes are outside
this decision. No actual settlement, sweep or adoption is claimed by preparation.


No funded async WaterNeuron/ledger canary, actual settlement, sweep or collateral adoption is established. Existing compiler/Svelte warnings and global typecheck debt are not claimed clean. The current source/packet retains demo-deferred limitations. Future writers must preserve SEC11 receipts and SEC12 cancellation journals; old-writer rollback is unsafe.

The source board ROPS-09 records source/artifact acceptance and this exact release decision as the next deliverable. Lookout connector large-board replies are truncated/undecodable; the available scoped local reader does not expose this release board. No grant, self-enrollment or blind overwrite was attempted. A verified public mirror refresh remains separate from the local source-board update.
