# DON-06c independent final donation audit

## Verdict

**PASS for the exact v4 report's bounded conclusions and payment inventory. Confirmed blockers in these final artifacts: none.**

This PASS establishes a certified ledger inventory, the seven confirmed production repairs, later aggregate reserve reconciliation, and an accurately disclosed early-history limitation. It does **not** establish that every original legacy credit succeeded, that all 1,999 payments have individual receipts, or that the user's full historical no-missed-interest assurance has been proved. The first 92 credit histories remain open at the proof level stated in v4. No additional positive repair amount is identified.

Root assigned DON-06c before this review and remains the sole production executor. I did not read another final review or contact the other final reviewer. I used source-investigator reports as inputs and independently reproduced the material arithmetic and raw-data joins. Only this assigned report was written. No production call, source edit, build, financial mutation, snapshot, or export was performed by this reviewer.

## Exact reviewed artifacts

All relative evidence paths below are under `/private/tmp/rumi-3usd-donation-reconciliation-20261023`.

| Artifact | SHA256 |
| --- | --- |
| `reconciliation-report-v4.md` | `f680b656d68150c84ad13a02a0f56a260960462bf88f393a081c29b141e7f67a` |
| `every-donation-reconciliation-v4.csv` | `9cf8a7ef1632719bfea84ee5946d346be5c7f4054650f01246284549b72bf7f8` |
| `pool/DON-02e-v3-timing-and-proof-boundary.md` | `82d1cd571b0a62942fbe27fa2d1ab356f9f59b3b1351be78faa9b3f2bb04af9d` |
| Original seven-credit manifest | `30edc54ad35151a4dba7a7ff37d9d17f0bf2a7a4630a3d4a58162bdf65d011a6` |
| Executed v2 repair script | `f15f94a666d0ab4fc4bd8127858a018e0262340de380e400cd085e33bc552627` |
| Production repair verification | `ff33f53f8503be0c010f7487ea246b43e9403f4073ec304044c99e639986637b` |

I applied `final-audit-rubric-v1.md`, adversarial-verification, rust-canister-engineering, and the loaded Rumi security/accounting personas. The relevant worktree is `/Users/robertripley/.codex/worktrees/sp-reopening-20261023/rumi-protocol-v2`; deployed source anchor is `8d7c7e75`. No new source or interface is proposed by the repair.

## 1. Exhaustive actual ledger payments

I independently reconstructed the representation-independent ICRC-3 SHA256 chain from every raw page, including Nat/Int LEB128, blob/text, arrays, and key-hash-sorted maps. IDs are exactly 0..42696, with 42696 valid parent links and a parentless genesis. The frozen last hash is `e6db178175f30f42020b09cf34b1bc2f1e7c07ae838029900b7cd08c42327679`.

Selecting `op=mint` to the pool owner's exact principal bytes yields exactly 1,999 mints totaling **14,504,987,166 raw = 145.04987166 icUSD**. Every one uses the default account; there are no extra pool-owner mints to a nondefault subaccount. Allowances/approvals were not counted as cash.

I independently checked the overlapping supplemental blocks and both new parent links through 42698. The resulting hash is `c12dc6cf26ae37bab77eec28af88ec2de53a494bb16a2201e1be58458c61e5e9`. I reran the saved official SDK 6.1.1 verifier in memory with evidence writes removed, without disabling signature, range, or time checks. Mainnet BLS/root-key/delegation/range verification passes; the witness matches certified_data and certifies that exact index/hash. Certificate freshness is also checked against its original query interval, rather than merely accepting an old certificate today. This binds the full frozen ledger prefix to the certified tip.

## 2. Reserve intervals and modern receipts

I traversed the actual old pool stable-memory log partitions and compared every value byte/hash and fresh Candid decode against the exports: 49 V1 swaps, 18 V1 liquidity entries, 324 V2 swaps, 110 V2 liquidity entries, and six admin entries: **507 raw events**. From the V2 entries I independently reconstructed 434 intervals: 430 exact and four short. All non-icUSD residuals are zero. The four short windows have unique credited subsets:

| Interval | Missing source blocks | Raw deficit |
| --- | --- | ---: |
| 424 | 40815, 40820 | 10346987 |
| 425 | 40839 | 5280076 |
| 426 | 40855, 40858 | 10232628 |
| 427 | 40870, 40873 | 10203850 |
| Total | seven | 36063541 |

All four ending outputs are ckUSDT. Their output/admin fee uncertainty cannot produce an icUSD deficit. The interval mint total is 10481534654; measured credit residuals total 10445471113. No later compensating excess appears. All pool icUSD burns precede the opening V2 anchor.

I explicitly challenged the closing anchor: old Memory0 SlimState has stale reserve values. The final report now correctly binds the closing reserves to the actual saved-snapshot predecessor-hook and final-candidate local query replies, which both hash to `9bcadcaa9728ce6154da4a3e9130ceaea01e02cddcb28eeb7086040344b82a4c`. It does not invent a separate reserve cell or claim that Memory0 directly contains those balances.

I independently extracted actual Memory38's 93 raw receipt key/value pairs across 18 B-tree nodes; the virtual-memory hash is `fdf84cba1e1bd5eff5b4d856f9f056e3a95982c7859166d5869afccdb0f2fb25`. I read all 1,680 private backend journal rows from the actual backup: 420 each Started/Updated/Accepted/Completed. Exactly 93 pool jobs match the receipts, each with one of every terminal transition. All 93 join to canonical ledger block, destination, amount, nonce/memo, and created-at-time. Total: 481920698 raw. The seven newer normal donations separately match positive logs and exact +36176230 reserve growth; they are distinct from the seven repaired payments. Current 107 key bytes were not exported, and the report does not claim otherwise.

## 3. Early 92: successful reconstruction, retained proof boundary

The canonical first 92 arrivals sum to 3987276282 raw. I verified the ledger-discovered stablecoin indices, the saved index-history boundary, and all 49 ckUSDT/48 ckUSDC early selected tuples against actual ledger/archive blocks: amounts, default accounts, timestamps, and outgoing fees agree. This is index-selected tuple evidence, not a certified full ckstable-chain claim.

The signed icUSD amount net independently reproduces 138184896685, with 1500000 separately paid outbound ledger fees. The historical model equation is exact:

`138184896685 - 10000000 - 13096399 - 435799 - 24223 = 138161340264`.

I inspected the pinned historical math and independently reran its six swap and one ROC replays in memory. All seven outputs match their canonical ledger amounts exactly; pool fees are 435799 and 24223. I confirmed the 23 underlying early burns have the same 22 timestamp-group multiplicities as 23 SP-side LP success-burn records, including the two-at-one-time group. The historical source emits those LP records in the confirmed-success branch, not the restoration branch.

I independently classified all 55 early nonic outputs: 45 unique retained V1 swap outputs, three retained V1 ROC outputs, four initial proportional-exit legs, and three unlogged ROC legs. I checked the six hardcoded early swap input/output caller accounts match, rather than relying on timing alone. I re-extracted VP26 directly from actual stable Memories14/15: time1773928338, supply290050751743, VP1049205595523870340. Actual saved configuration independently contains the March17→19 ramp tuple. I reproduced all 128 observed callback masks and the expanded 1,024 timing/completion cases. Only the fully booked observed-input case has a matching native icUSD reserve, 89397705201; all tested alternatives are strictly bracketed.

Those results do **not** authenticate an exhaustive original call inventory. A fully failed proportional exit before March13 could debit books/LP while producing no underlying transfer; its original LP burn history was subsequently replaced by synthetic remints. That expands possible book vectors beyond the observed-call masks. I therefore reject the earlier unconditional candidate-completeness and universal early-credit-exclusion claim. The overriding supplement and v4 report now disclose this precise limitation, label these rows `EARLY_ARRIVAL_VERIFIED_CREDIT_MODEL_ONLY`, and do not turn the 560374 candidate headroom into independent proof that all 92 callbacks succeeded. No speculative new credit is admitted. This correction is necessary for this PASS.

The initial 0.10 icUSD partial-pull issue is kept separate from interest yield; the failed original block is not assigned between the two equal initial pulls. No refund/ownership recovery or residual sweep is implied.

## 4. Actual seven-credit repair and terminality

I independently inventoried all 25 recorded v2 requests. Exactly seven are financial updates, all to the expected pool's existing `receive_donation_v2`. Every exact nonce/source block/token0/amount matches the manifest and its exclusive pre-dispatch marker. All seven raw/typed replies are Ok, all command receipts exit zero, and matching controller logs are consecutive3588..3594. There are no unknown-outcome files or extra financial requests. The v1 directory contains only four identity-command files with a CLI parse rejection, before any financial dispatch.

Independent before/after reconstruction gives:

| Quantity | Before | After |
| --- | ---: | ---: |
| icUSD reserve book | 1050977906725 | 1051013970266 |
| Physical icUSD | 1051033626687 | 1051033626687 |
| Receipt count | 100 | 107 |
| 3USD supply | 1485147981057 | 1485147981057 |
| VP | 1125659911286418330 | 1125684141892726645 |

The book increment and surplus decrease are exactly36063541 raw. Other books, admin fees, settings, and module are unchanged. The residual is19616202 raw and was not credited. All126 payout rows are unique/settled; claims and conversion fence were empty. The LP ledger is complete at2489 blocks, with last activity October8, before all seven October14 candidates.

The existing endpoint authorizes backend/admin or controller and keys receipts by authenticated caller plus nonce. Controller repair receipts are not backend receipts. I checked the old backend successful stop receipt and Stopped status, complete journal absence of the seven nonces, current empty unfiltered pending maps, and the current source's absence-return/fresh-nonce behavior. [Official management-canister documentation](https://docs.internetcomputer.org/references/management-canister/#stop_canister) confirms Stopped follows completion of outstanding responses. This establishes normal original-retry exclusion within the stated trust boundary. It is not a fence against deliberate future code replacement or restoring an old snapshot.

The exact deployed-Wasm local fixture is a 95-call fresh fixture, covering exact credits/replay, payload conflict, authorization, and insufficient backing. It is not a production state upgrade or post-restart replay test. No source/schema/install change was proposed for this repair.

## 5. Final inventory, continuity, and limits

I rehashed all three final targets and the v3/v2/nested early evidence manifests; all checked hashes match. The v4 CSV has exactly1,999 unique source blocks, correct amounts/timestamps, and these disjoint categories:

- 92 arrival-verified/credit-model-only:3987276282 raw.
- 1,800 later aggregate matches:9963550415 raw.
- 93 exact persisted receipt/journal joins:481920698 raw.
- Seven newer normal log/book-growth matches:36176230 raw.
- Seven repaired credits:36063541 raw.

At closing-live-03, completed00:55:35UTC, the pool status/books/supply/VP still equal the repair after-state and count remains107. All four captured modules remain Running/unchanged; mint and stable-transfer journals are empty. All three physical balances match the earlier baseline: icUSD1051033626687, ckUSDT3653601532, ckUSDC2563511903. This later read is distinct from measuring physical icUSD immediately before/after the repair.

The report preserves the rejected stop/export attempt and failed CLI version, does not claim a new deployment, does not classify the residual as profit, and distinguishes the earlier projected APY capture from actual payments. I did not obtain a new APY measurement. Earlier LP allocations and the first92 original credit histories are not declared proven. An older backup/original operation history is the concrete remaining evidence need, not another endpoint or financial mutation.

Read-only inventory:66 registered worktrees,2% free, zero eligible removals/65 protected or blocked. No worktree was created or removed. No memory/persona file was changed. Durable proposed lesson: an exact aggregate match and a bounded observed callback model must not be presented as exhaustive historical receipt proof when a migration erased book-only operation history.
