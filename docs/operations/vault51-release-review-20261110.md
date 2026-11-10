# Vault 51 release review — November 10, 2026

Reviewer and accountable release owner: **Dorian**. Source owner: **Adrian**. ROPS-09 records this handoff; SEC-08 remains the implementation/case card.

## Decision

**PASS for bounded source/artifact preparation. HOLD installation and automatic activation.** No confirmed blocking source finding remains in the reviewed scope. This review performs no customer action, custody transfer, stop/start, installation or frontend publication. It is not deployment authority or evidence of actual settlement/adoption. Merritt can assess this exact candidate without another source-research campaign.

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

The next deliverable is Merritt's decision on this exact preparation, then any separately authorized release action through the existing stop/drain/snapshot preservation and additive frontend stage/activate procedure. At that future checkpoint, re-read live module/settings/controller, exact claim/vault/owner/debt/interest/custody, vault158 record, SEC11/SEC12 journals, current snapshot inventory and frontend assets/auth. Current byte/state drift must be reconciled, not overwritten. Those are the already accepted release controls, not a new test campaign. No replacement executable/publisher that could mutate production was dispatched here.

No funded async WaterNeuron/ledger canary, actual settlement, sweep or collateral adoption is established. Existing compiler/Svelte warnings and global typecheck debt are not claimed clean. The current source/packet retains demo-deferred limitations. Future writers must preserve SEC11 receipts and SEC12 cancellation journals; old-writer rollback is unsafe.

The source board ROPS-09 is updated with owner/next deliverable. Lookout connector large-board replies are truncated/undecodable; the available scoped local reader does not expose this release board. No grant, self-enrollment or blind overwrite was attempted. A verified public mirror refresh remains separate from the local source-board update.
