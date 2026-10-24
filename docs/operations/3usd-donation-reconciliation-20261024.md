# 3USD actual interest donation reconciliation

## Result

The frozen ledger contains **1,999 icUSD interest mints to the pool, totaling 145.04987166 icUSD**. Every payment is listed in `every-donation-reconciliation-v4.csv`, with its exact amount, ledger block, timestamp, memo/nonce where present, and credit evidence category.

Seven October 14 payments reached the pool but were missing from its recorded reserves. **All seven were repaired on October 24 at 00:31 UTC, totaling 0.36063541 icUSD.** No new icUSD was minted and no tokens were transferred. The existing deployed donation endpoint credited funds already present.

Later legacy reserve intervals reconcile in aggregate. The earliest92 payments, totaling39.87276282 icUSD, also fit a reconstructed historical accounting model, whose exact equation closes at zero. **Their individual credits are not fully proven.** The March13 migration replaced the earlier LP log. An old exit could debit reserves and LP before every payout failed, leaving no ledger transfer and no surviving original LP record. Such a book-only operation expands the possible historical states beyond the observed callback models. No additional missed amount is positively identified, so no speculative credit was applied.

This is a verified ledger payment inventory, seven confirmed repairs, and a bounded historical accounting review. It is not proof that every legacy notification succeeded or that no depositor ever missed interest. Original per-call ACKs, complete pre-migration operation history, and certified historical installed-source lineage are unavailable. An earlier backup or original operator/callback logs could strengthen that part of the audit.
Independent review disposition is recorded separately in `FINAL-AUDIT-READINESS-v1.json`; this report does not approve itself. For the earliest92, `DON-02e-v3-timing-and-proof-boundary.md` expressly supersedes v3 candidate-completeness and unconditional closure wording. The early payment arrivals are verified; their model-consistent credits remain a proof limitation. Old repair manifests/reviews retain their original, narrower historical scope.

## Coverage and proof levels

| Payment category | Count | icUSD total | Credit evidence |
| --- | ---: | ---: | --- |
| Early March–April payments | 92 | 39.87276282 | Arrivals verified; credit model fits aggregate records, with unresolved original history |
| Later legacy payments | 1,800 | 99.63550415 | Reserve intervals reconcile in aggregate |
| Modern persisted backend receipts | 93 | 4.81920698 | Exact stored receipt, terminal backend journal, ledger tuple |
| Newer normal donations | 7 | 0.36176230 | Exact positive logs and reserve growth; unique ledger amount/time join |
| Missed payments repaired | 7 | 0.36063541 | Seven exact successful correction calls, matching logs and conservation |
| **Total** | **1,999** | **145.04987166** | All rows explicitly classified |

The icUSD ledger capture is contiguous over blocks0..42696:42,697 blocks and42,696 verified parent hash links, with no gaps or duplicate IDs. Supplemental blocks42697..42698 extend that chain. The official SDK6.1.1 verifies the mainnet BLS signature, delegation and canister range, certificate time, witness root and certified tip42698, binding the entire frozen capture to that tip. Its hash is `c12dc6cf26ae37bab77eec28af88ec2de53a494bb16a2201e1be58458c61e5e9`.

Pool: `fohh4-yyaaa-aaaap-qtkpa-cai`, default subaccount. icUSD ledger: `t6bor-paaaa-aaaap-qrd5q-cai`,8 decimals. Approvals are excluded from monetary flows. The frozen total is a historical cutoff, not a claim about future payments.

## The seven corrections

| Original mint block | icUSD credited |
| --- | ---: |
| 40815 | 0.05114190 |
| 40820 | 0.05232797 |
| 40839 | 0.05280076 |
| 40855 | 0.05117783 |
| 40858 | 0.05114845 |
| 40870 | 0.05101043 |
| 40873 | 0.05102807 |
| **Total** | **0.36063541** |

The closing anchor comes from the actual saved pool snapshot, exercised through its predecessor upgrade hook and the final candidate; both local query replies have SHA256 `9bcadcaa9728ce6154da4a3e9130ceaea01e02cddcb28eeb7086040344b82a4c`. It is not a claim that stale Memory0 SlimState directly contains those closing balances. There are434 chronological reserve-anchor intervals after April10.430 match exactly; four are short. Each short interval has one unique credited subset, identifying these seven missing amounts. All four ending swaps output ckUSDT, so uncertainty about their output fee cannot explain an icUSD gap. Later intervals contain no compensating excess. No pool icUSD burn occurs after March22.

The old93 stored receipts and terminal backend journals join exactly to their ledger source blocks, nonces, amounts, memos and created-at-times. Seven newer normal donations are different from the seven missed payments; they explain the normal receipt count93→100 and exact reserve growth0.36176230 icUSD.

## Production result

| Measurement | Before | After |
| --- | ---: | ---: |
| Recorded icUSD reserves | 10,509.77906725 | 10,510.13970266 |
| Physical icUSD | 10,510.33626687 | 10,510.33626687 |
| Donation receipt count | 100 | 107 |
| 3USD supply | 14,851.47981057 | 14,851.47981057 |
| Virtual price | 1.125659911286418330 | 1.125684141892726645 |

All seven replies are `Ok`, with matching consecutive pool logs3588..3594. Other reserve books, admin fees, module/settings and controllers are unchanged. Claims remain zero; the conversion fence and backend pending interest mint map are empty. Both independent preflight reviewers and both post-execution reviewers passed the exact seven-credit repair. The95-call local fixture used the exact deployed pool Wasm and checked replay, conflict, unauthorized caller and insufficient backing behavior.

The first executor version rejected an unsupported CLI identity flag before any financial call. Its four identity files are preserved. Both reviewers checked the narrow CLI correction; v2 then dispatched each credit once, with an exclusive marker and saved reply. No financial outcome is unknown, and no financial request was retried.

Installed pool module remains `c17b4e71312eb60cbb1dcaeeabb2efa927ee99fc9788c60a42f9bb2f56117615`. No source change or new installation was required. There was no new production stop, state export, transfer or mint. A proposed new stop/bulk snapshot export was rejected by automatic approval review before execution and was not bypassed.

The durable correction receipts belong to the authenticated controller caller. They are not backend-scoped receipts. The earlier completed backend stop drained old callbacks; the complete retained backend journal has none of the seven candidate nonces, current pending maps are empty, and current source cannot recreate absent old retries through normal admission. That concrete terminality proof admits these corrections without inventing a cross-caller fence. Deliberate future code changes or restoration of an old snapshot remain outside that proof.

The complete captured LP ledger has2,489 blocks, with its last change on October8. LP allocation therefore did not change between the October14 missed credits and this repair. No historical LP reallocation was performed. All107 current receipt key bytes were not exported, and a post-restart repair replay was not performed. The repair script measured physical icUSD before/after; a separate closing read at00:55:35UTC recaptured all three physical token balances, confirmed the same books/supply/VP/107receipts, unchanged installed modules and empty mint/transfer journals. That later read also confirms the previously captured fiat stable physical balances remained unchanged.

## Earliest92: observed model and verification limit

Using native raw icUSD units:

```
138,184,896,685  signed ledger amount flows, including all92 mints and23 burns
-    10,000,000  one uncredited partial initial deposit
-    13,096,399  recorded historical output pool fees
-       435,799  six earlier swap fees, exact source output replay
-        24,223  earlier one-coin withdrawal fee, exact source output replay
=138,161,340,264 observed first V2 icUSD reserve
```

The amount-flow sum does not yet subtract1,500,000 raw outbound ledger fees. Actual physical balance is138,183,396,685. Old source separately debited the ledger fee while its reserve book deducted output plus pool fee. Admin40,219 is already part of the pool-fee debit in this reserve equation; subtracting it again would double-count it. It is subtracted once when computing physical surplus outside liabilities.

Within the observed-transaction model, a candidate bound uses only five recorded fee rows that predate the fee-fix source, totaling12,996,047; it excludes the later April8 fee. After those fees and the partial deposit,560,374 raw units remain, below the smallest whole early donation9,500,234. The reviewed donation source credits its full amount or none, with one normal notification after each successful mint and no early retained retry/dedup-success path. The23 actual ledger burns have matching success-side LP burn logs, excluding a settled-burn/book-rollback offset under the reviewed source.

The review found and addressed another possible offset: old swaps transferred output before updating books, so a lost output reply could leave both sides unbooked. V3 classifies all55 early nonic output transfers and enumerates128 observed-call book states covering six early swap callbacks and the initial stablecoin pair. A timing supplement expands this to1,024 cases including three early add-completion choices. A saved March19 VP record after the known A500 ramp admits an integer icUSD reserve only for the fully booked case; the other observed-call alternatives are bracketed and rejected by exact integer source math. This callback check does not assume donation credits or a historical20bps ROC fee. The raw saved VP is1.049205595523870340, supply2,900.50751743, and unique inferred icUSD reserve893.97705201. Later swaps have retained success events. That result supports the observed-transaction reconstruction. It does not make the candidate inventory exhaustive: an erased pre-migration book-only failed exit could change the nonic book vectors, and the retained records do not exclude every such alternative. The numerical bound therefore cannot independently establish that every early credit succeeded.

The exact output/fee replays are confirmatory. Historical fee changes before event logging were not exhaustively retained; neither those replays nor the corroborating VP anchors manufacture original per-call ACKs. DON-02e v3 and its expressly overriding proof-boundary supplement record the observed case space, source pins and unresolved historical traces. The supplement takes precedence over broader v3 statements.

Canonical ckUSDT/ckUSDC ledgers identified their own index principals through ICRC106. Those index ledgers were verified, account pagination reached each reported oldest transaction, and all49/48 early transfers were checked against actual ledger/archive blocks. This is index-selected tuple verification, not certification of the entire ckstable chains. Exact integer replays reproduce six early swaps and one one-coin withdrawal. The saved amplification configuration pins the March17→19 ramp; the specific March19 VP callback check supports the observed callback model, while the other inferred VP anchors are corroborating evidence. V3 does not claim an exact unconditional reconstruction of all three historical reserve books.

The initial failed-deposit amount is0.10 icUSD. There were two initial icUSD pulls but only one corresponding ckUSDT/ckUSDC pair, while source committed reserves only after all three pulls succeeded. The retained callbacks do not identify which of the two icUSD pulls belonged to the failed attempt. This is a separate user-fund claim, not an interest donation, and it was not paid or added to yield.

## Remaining funds and APY

After the seven corrections, physical icUSD minus recorded reserves and admin fees is **0.19616202 icUSD**. This residual was not swept into depositor yield; the separate0.10 initial-deposit claim and historical fee/accounting differences must retain their proper ownership treatment. Earlier baseline ckUSDT/ckUSDC surpluses were also untouched. No further missed interest amount is positively identified. The earliest92 credit-history limitation remains open; an older backup or operation log was requested without holding up the confirmed repairs.

The displayed2.63% APY is a projection of current annualized borrower interest and fees divided by pool assets. It is not a historical receipt total. The separate earlier APY capture reproduced2.6289092% from about434.05 icUSD projected annual income over16,726.12 pool assets. Repairing historical credits raises the value of existing3USD; it does not make the projected APY equal the borrowing APR on one vault.

## Evidence

Primary evidence root: `.recovery-artifacts/rumi-3usd-donation-reconciliation-20261023` in the main checkout, with a final file hash inventory. Raw responses, failed attempts, original manifests, source pins, exact local runtime, all independent reviews and production dispatch receipts are retained. Heavy raw evidence remains ignored; the readable report and complete payment CSV are the reviewable repository artifacts.
