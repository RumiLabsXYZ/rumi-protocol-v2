# 3USD actual interest donation reconciliation

## Result

The frozen ledger contains **1,999 icUSD interest mints to the pool, totaling 145.04987166 icUSD**. Every payment is listed in `every-donation-reconciliation-v4.csv`, with its exact amount, ledger block, timestamp, memo/nonce where present, and credit evidence category.

Seven October 14 payments reached the pool but were missing from its recorded reserves. **All seven were repaired on October 24 at 00:31 UTC, totaling 0.36063541 icUSD.** No new icUSD was minted and no tokens were transferred. The existing deployed donation endpoint credited funds already present.

Later legacy reserve intervals reconcile in aggregate. The earliest 92 payments, totaling 39.87276282 icUSD, fit a reconstructed historical accounting model, but their individual credits are not fully proven because part of the pre-March-13 operation history is unavailable. No additional missed amount is positively identified, so no speculative credit was applied.

This is a verified ledger payment inventory, seven confirmed repairs, and a bounded historical accounting review. It is not proof that every legacy notification succeeded or that no depositor ever missed interest. Original per-call ACKs, complete pre-migration operation history, and certified historical installed-source lineage are unavailable. An earlier backup or original operator/callback logs could strengthen that part of the audit.
Independent review dispositions are retained in the operator's private evidence store; this report does not approve itself. The early-92 proof limitation above supersedes any broader earlier wording.

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

The closing balances were anchored to the actual saved pool snapshot. Of 434 chronological reserve intervals after April 10, 430 match exactly and four are short. Each short interval identifies one unique credited subset, and together they are the seven amounts repaired above. Later intervals show no compensating excess.

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


Installed pool module remains `c17b4e71312eb60cbb1dcaeeabb2efa927ee99fc9788c60a42f9bb2f56117615`. No source change or new installation was required. There was no new production stop, state export, transfer or mint.

The durable correction receipts belong to the authenticated controller caller and are distinct from backend-scoped receipts. Review concluded that no retained or pending original operation can duplicate these corrections.

The complete captured LP ledger has2,489 blocks, with its last change on October8. LP allocation therefore did not change between the October14 missed credits and this repair. No historical LP reallocation was performed. All107 current receipt key bytes were not exported, and a post-restart repair replay was not performed. The repair script measured physical icUSD before/after; a separate closing read at00:55:35UTC recaptured all three physical token balances, confirmed the same books/supply/VP/107receipts, unchanged installed modules and empty mint/transfer journals. That later read also confirms the previously captured fiat stable physical balances remained unchanged.

## Earliest 92: observed model and verification limit

The earliest 92 arrivals are verified against the icUSD ledger and the stablecoin ledgers' own histories, and an observed-transaction accounting model reconciles them to the first V2 reserve. Because part of the pre-March-13 operation history was not retained, the model cannot independently prove that every early credit succeeded, and no speculative credit was applied. A separate 0.10 icUSD initial-deposit claim is not an interest donation and was not paid or added to yield.









## Remaining funds and APY

After the seven corrections, physical icUSD minus recorded reserves and admin fees is **0.19616202 icUSD**. This residual was not swept into depositor yield; the separate0.10 initial-deposit claim and historical fee/accounting differences must retain their proper ownership treatment. Earlier baseline ckUSDT/ckUSDC surpluses were also untouched. No further missed interest amount is positively identified. The earliest92 credit-history limitation remains open; an older backup or operation log was requested without holding up the confirmed repairs.

The displayed2.63% APY is a projection of current annualized borrower interest and fees divided by pool assets. It is not a historical receipt total. The separate earlier APY capture reproduced2.6289092% from about434.05 icUSD projected annual income over16,726.12 pool assets. Repairing historical credits raises the value of existing3USD; it does not make the projected APY equal the borrowing APR on one vault.

## Evidence

Primary evidence is retained in the operator's private evidence store, with a final file hash inventory. Heavy raw evidence is not published; the readable report and complete payment CSV are the reviewable repository artifacts.
