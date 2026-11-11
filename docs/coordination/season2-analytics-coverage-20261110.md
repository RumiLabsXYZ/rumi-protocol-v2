# Season 2 analytics coverage: checked facts and unfinished reconstruction

Status: evidence note, not a credit proposal. No participant-specific public query was repeated. Public analytics interface metadata was read anonymously on November 10: canister dtlu2-uqaaa-aaaap-qugcq-cai; Candid text SHA-256 c35e66a15ded99cd81bae4a51f1fdf627f8c2c833a1f5f632d7732e97571e65d. Source references below describe the checked-in implementation at base 5ecd5196, not a deployed-Wasm attestation.

## What was checked

| Evidence | Actual content | What it does not establish |
| --- | --- | --- |
| Saved get_vault_series and get_vault_series-earliest-0 responses; current public Candid | Daily protocol totals and per-collateral count, collateral, debt and CR statistics | Per-owner/per-vault debt at each historical time |
| Saved get_stability_series and earliest response | Pool-wide balances, depositor count, interest received and liquidation count | Per-user interest allocation |
| Saved backend get_protocol_snapshots: 917 rows in the month export plus tail | Hourly protocol/per-collateral totals, not individual vault rows; gaps exist | Continuous participant debt history |
| Current get_address_value_series interface and checked-in implementation | Per-principal chart reconstructed from normalized events and current wallet balances; response has approximate_sources | Exact per-vault accrued-interest history; this cohort endpoint was not queried under the existing denial |
| Analytics stable normalized EVT_VAULTS/EVT_STABILITY logs | Retained event rows with internal accessors | Public raw vault/SP export: current Candid and HTTP route table expose no such endpoint |
| Full retained source SP events, IDs 0..2547, plus saved positions | Complete observed source-event interval; interest amounts and opt-in/out changes | Interest event collateral type or recipient amounts, which are not fields of InterestReceived |

## Specific source evidence

- [DailyVaultSnapshotRow / DailyStabilityRow](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/5ecd5196/src/rumi_analytics/src/storage/mod.rs#L314-L343) are aggregate schemas. [get_vault_series](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/5ecd5196/src/rumi_analytics/src/queries/historical.rs#L23-L33) pages that daily log.
- [get_address_value_series](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/5ecd5196/src/rumi_analytics/src/queries/address_value.rs#L133-L208) loads normalized logs and marks vault equity approximate; [VaultPositionState](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/5ecd5196/src/rumi_analytics/src/queries/address_value.rs#L394-L416) explicitly omits accrued interest and per-vault redemption adjustments. This chart implementation cannot be treated as an exact debt snapshot archive.
- [Normalized event schemas](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/5ecd5196/src/rumi_analytics/src/storage/events.rs#L82-L143) retain owner/vault IDs and debt action amounts, including a DustForgiven kind. The internal stable logs/accessors exist, but [HTTP routes](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/5ecd5196/src/rumi_analytics/src/http/mod.rs#L13-L34) expose aggregate series, not those raw rows. Retained but not publicly exported is different from absent.
- [SP analytics tailing](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/5ecd5196/src/rumi_analytics/src/tailing/stability_pool.rs#L135-L190) records deposits/withdrawals/claims but skips interest, opt-in/out and liquidation events. [Source InterestReceived](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/5ecd5196/src/stability_pool/src/types.rs#L1290-L1293) contains token ledger and amount, while the distribution function takes collateral_type. Analytics normalized history therefore does not provide collateral-specific user interest allocation.
- [Backend ProtocolSnapshot](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/5ecd5196/src/rumi_protocol_backend/src/lib.rs#L2337-L2353) is also aggregate. Those hourly rows cannot simply be assigned to individual owners.

## Corrected legacy-case finding

The earlier claim that fourteen older vault balances were zero-current was wrong. Independent SDK decoding of the saved cursor-558710 49-vault page shows all fourteen still present with positive debt. The later liquidated vault is separate. No identity is published here.

The proposed 20 points was the total debt component for that one participant, not 20 extra. It rounded a provisional 19.01356466 model to 20, adding 0.98643534. That optional compensation proposal is withdrawn; it was neither a protocol requirement nor an approved award.

A complete local event scan now locates the later vault's opening and borrow, and its successful source SP liquidation. The old positions' recorded actions are before November 1, not proved post-November writeoffs. All 16,021 backend timestamp entries in the examined November interval were present, nonzero and nonregressing. The replay mismatch is not evidence that this history is lost.

## What remains unfinished

1. Reconcile the actual participant debt timeline using correct typed endpoints and joined source events, including the SP retirement that is not mirrored as a backend liquidation event. Do not invent a legacy compensation case from owner-level aggregation.
2. Determine whether per-vault interest can be reconstructed from the retained source inputs/versioned behavior. The analytics chart omits it; that fact alone does not prove reconstruction from other retained inputs impossible.
3. Check whether backend notification/receipt or ledger provenance can restore SP collateral-specific allocation. The normalized analytics log does not supply it. A missing public field is not proof that every underlying receipt was lost; no held update/recovery method has been invoked.
4. Keep the lack of actual random A/B captures separate from balance reconstruction. Choosing a retrospective scoring convention is a policy question, not evidence that balances cannot be reconstructed.

Until these are resolved, no estimate or compensation is submitted for acceptance. Continue independent timing/init preparation, preserve S1 and denied-query boundaries, and keep all eligible November 1 continuity explicit.

## Further retrieved evidence, not a missing-history claim

A previously unqueried public source method, get_liquidation_history(opt 100), returned 61 pool-wide records without participant arguments. Its typed record for the later loan identifies only icUSD in stables_consumed, at 202,479,288 raw units. This resolves the event summary's token ambiguity using an existing historical record. The checked-in icUSD path uses realized debt without a stable-token surcharge (liquidation.rs:6471–6485); exact retirement evidence should be bound to this record/path or its underlying receipt, not assumed from a mixed-token aggregate. This is another reason to complete retrieval before declaring data missing.

The public backend INFO log was also read without participant arguments. Its retained buffer contained 579 entries spanning Nov 9 03:52 UTC–Nov 10 19:43 UTC, including 35 interest-related messages. That buffer does not cover the whole requested interval. This observed span is not proof that older logs/receipts were never retained elsewhere.

## Former proposal's numerical effect, not an award

For transparency, the withdrawn proposal's comparisons were:

- Debt upper versus its lower comparator after the late Nov 1 checkpoint: 143.52669719 points.
- SP icUSD upper versus lower envelopes: 136.71617410 points.
- Rounding the special-case model from 19.01356466 to 20: 0.98643534 point.
- Sum of those three differences: 281.22930663 points.
- An additional 14,140.47611671 debt-point difference came from counting the early Nov 1 interval versus a formal zero lower bound before the first checkpoint. That zero was an uncertainty bound, not an acceptable uncredited gap.

These are comparisons within the former estimates, not measured excess over exact earned points. No hypothetical random A/B award exists to compare. They do not validate the underlying estimates, and the corrected old-vault observation removes the rationale for the special rounding proposal.

## Bounded reconstruction experiment

One offline diagnostic replay completed through cursor 558710 using all retained backend events, 40 observed vault snapshots from the late November 1 checkpoint, and the recorded icUSD Stability Pool payment. It did not reconcile the saved endpoint: 10 of 49 current vault debts matched, 39 differed, and the replay retained two additional vaults. The test completed in 301 seconds; successful execution is not successful accounting reconciliation. No second pass, participant query, production code change or live mutation was performed.

The anchor is around 16:51 UTC rather than midnight, omits internal accrual clocks, and spans eight seconds containing one backend event. The injected debt payment does not reconstruct the associated collateral change. These known limitations keep this diagnostic from establishing exact historical balances; they do not establish that retained history is absent. The earlier first 16 hours 51 minutes also remain unresolved by this experiment. No diagnostic output is an approved award.

## Concrete replay correction and remaining retained inputs

A source-supported offline correction restored recorded price updates to replay state. The production replay had treated PriceUpdate as a no-op, despite live dynamic interest depending on cached prices. At the saved anchor a representative vault used stale ICP price 2.126862129 in the baseline versus the retained update 2.51173936. Its modeled APR was 0.0503897587 versus 0.0292049922; the first following accrual added seven raw debt units in the baseline and four after correction. This is a modeled step after the anchor, not an observed first live fault.

At the same bracketed cursor 558710, the endpoint comparison improved from 10/49 matching vault debts to **41/49** (39 mismatches to eight). Two selected cohort vaults matched exactly after correction. The separately observed 202,479,288-raw icUSD payment then reduced the later loan to zero without forcing the ending balance; the earlier baseline had left a residual. The typed holdout was independently checked against its raw Candid response for every debt-relevant field; the wrapper's before/after cursors were both 558710.

The correction stays in an isolated test-only diagnostic; the shipping Points branch changes no backend production code. It does not establish exact historical accounting: 28 of 40 observed debts still differed before the late-anchor replacement (36 before the price correction), and internal accrual clocks/private side effects are not in that public anchor. The earliest proven disagreement is therefore already at the saved late Nov1 anchor; no intermediate observation identifies its first live cause.

Two of the eight endpoint debt residuals belong to the observed cohort. One has retained repay/withdraw actions and a collateral difference; another has borrow actions and a 29-raw-unit debt difference. Live paths accrue individual vaults before these operations, while replay arms do not. Exact retained pre-operation clocks/state or an operation receipt carrying the accrual cutoff are the next inputs needed to prove that candidate cause; a later event timestamp alone does not establish pre-await time. The other six residuals are outside the participant anchor and need their contemporaneous state/clocks and liquidation/private-journal provenance; retained SP records already identify earlier liquidation candidates for two of them. One replay-only vault also needs its retained close/liquidation provenance. None is permission to force an endpoint or declare history missing.

Prospective scoring release is now independent of this reconstruction. Later reviewed historical credits remain confined to [November 1,T0); the earlier estimates and compensation remain withdrawn.

## November 11 source-only worker and memo-clock validation

After direct human approval of the exact six ordinary source files, the native Claude.ai subscription worker completed a bounded source-only task. Its accepted local test commit is bc6510a8: 267 added lines within the existing cfg(test) module, with no production source, Candid, dependency or release artifact change. Codex independently inspected the diff and ran all six focused synthetic tests successfully. No raw participant evidence was sent to Claude.

The tests independently demonstrate omitted pre-operation single-vault accrual for borrow, repayment and partial withdrawal. A known pre-operation cutoff corrects those synthetic examples; a later recorded event timestamp is not a valid general substitute. Harvest changes accrued-interest allocation, while the borrowed debt already includes interest. The single-vault fixture's synthetic all-vault checkpoint helper is not a production replay correction.

Codex also checked the nonce source: next_op_nonce_at places the issuance time in the upper 64 bits; the transfer helper encodes the nonce in a 16-byte memo and derives created_at_time from those bits. Four actual operations now have indexed public ledger records privately bound to their source events by operation kind, amount, actor/default account and memo/transaction timestamp. The reads used global block indexes, no participant-address arguments, and no Claude export. Source-version and same-timestamp execution ordering still require care; this is not a complete historical execution attestation.

One private endpoint-only comparison is running with those recovered cutoffs applied to the specific vaults. It does not force endings, introduce awards or treat synthetic timeline output as an accounting transcript. The first 16 hours 51 minutes before the saved late-Nov1 anchor remain outside this experiment. A separate retained redemption-claim object provides concrete next evidence for the remaining collateral/valuation case; it is not silently injected into this clock comparison.

The second source-only task added two focused replay tests for the partial-withdrawal dust branch; both pass under Codex's independent run (local commit2893db49). Current source clears small debt/accrued-interest before the transfer without emitting a forgiveness event in that branch; successful replay only removes collateral. A failed transfer may leave no public event for that mutation. The native-XRP early-return path is different and must not be assigned this rule. This identifies a concrete candidate behavior to reconstruct with validated pre-operation state and deployed version, not proof of a particular historical residual.

The first memo-clock endpoint pass stopped on a diagnostic adapter error: an initial borrow's memo time came before its zero-debt public opening was available in the replay registry. The saved opening payload was independently checked as zero debt, for which single-vault accrual is a no-op. A guarded handling of that case is in the corrected private pass; no ending balance is forced and no residual reduction is claimed before completion. Discarded pre-window observer logging was removed without changing canonical replay state transitions.

The corrected memo-clock pass completed at the frozen cursor558710: global debt mismatches **8→7**, matches42/49, and validated-cohort debt mismatches **2→1**. All four memo checkpoints are accounted for (one initial zero-debt opening is a verified no-op; three accrue the specific vault). The borrowing residual is now exact without fitting its endpoint. The retained redemption-claim timeline/valuation is the next concrete input for the remaining cohort case. Six global residuals remain outside that cohort anchor, and the replay-only vault remains. Native diagnostic execution passed in276.24seconds; this is endpoint evidence, not approval of a historical award convention or a complete November1 timeline.
