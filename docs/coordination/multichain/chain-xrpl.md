# BB-37: XRPL groundwork (escrow-backed icUSD on PR #586)

Status: planning document, read-only. Card BB-37 (worker W19), 2026-10-30. Nothing here deploys, enables, funds, signs, merges or edits PR #586.
Labels: **F** fact with a source, **R** recommendation, **D** open decision. PR #586 is open and unmerged; citations are `git show b02584e4:<path>` (head "docs(xrpl): record fourth post-merge verification"). Path prefixes: `x/` = `src/rumi_protocol_backend/src/xrpl_icusd/`, `c/` = `src/rumi_protocol_backend/src/chains/xrp/`. Hosts and addresses are deliberately not copied (see `c/network.rs:77-84` and the accepted profile).
Settled for planning (design section 12): D2 and D7 (issued-currency escrow and import seam, MPT deferred, 15-significant-digit caps on each operation and on total outstanding, inexact amounts rejected or held exactly), D9 (synthetic test ledgers). D6 (controllers, custody, authority activation) stays open.

## 1. How the multichain design maps onto PR #586

Confirmed (F):
- **Lane and default.** `XrplIcusdState` (`x/state.rs:20`, held as `State.xrpl_icusd`) is disabled by default: not enabled, issuance paused, both caps 0, no policy, no issuer (`x/types.rs:364-384`). User methods need `ensure_ready` (`x/state.rs:232-252`); new issuance also needs `ensure_open_for_issuance` (`:255-261`).
- **Escrow.** `(backend, sha256("rumi:xrpl-icusd:escrow:v1"))` (`x/types.rs:33-49`), one pooled account for borrow and import origins. The ledger minting account must be the backend default account and differ from escrow (`x/ledger.rs:43-65`).
- **Issuer.** Threshold Ed25519 at path `[rumi, xrpl-icusd, v1, <network name>]` (`x/types.rs:59-67`; derived in `x/mod.rs:265-280`). The last element is the text "mainnet" or "testnet" (`c/network.rs:52-58`), not the numeric XRPL network id. Key `key_1` (mainnet) or `test_key_1` (fixture build only).
- **No-debt import.** `issue_xrpl_from_canonical` (`main.rs:9805-9808`, `x/mod.rs:444-516`) calls `create_import` (`x/issuance.rs:50-111`): an ICRC-2 pull into escrow (`ImportPull`, phase `EscrowPullPending`), then `advance_import` (`:124-149`) creates the delivery claim only after the pull block is fetched back and matched; the issuer Payment and `D -> O` follow. No debt, no fee (`:1-8`).
- **Debt-backed borrow and repay.** Borrow mints into escrow and records the normal `BorrowFromVault` debt (`x/borrow.rs:1-22`, `x/ledger.rs:74`). Repay is verified return, then `EscrowBurn` (escrow to minting account, `x/ledger.rs:87`), then the debt change (`x/repay.rs:1-26, 720-856`). Debt-backed and no-debt stay separate kinds (`x/types.rs:668-709`).

Corrections to design section 1.4 and 2 (F):
- **`CanonicalReturn` is claim resolution, not a return of delivered tokens.** `plan_resolution` accepts only claims in `DebtCommitted` or `IssuePrepared` with no signed attempt (`x/issuance.rs:180-195, 210-213`); a delivered operation is terminal and can never use it. The only other user, held repayment (`x/repay.rs:1129-1177, 1250-1299`), needs a repayment intent. Its machinery (escrow to owner, fee reserve, proof, `x/issuance.rs:221-256, 276-367`) is what the missing return should reuse.
- **The gap is sharper than stated.** `create_intent` requires a native-XRP vault owned by the caller with debt (`x/repay.rs:186-216`), so a holder without one cannot obtain a tag. An untagged or unknown-tag payment is folded into the sender's aggregate and refunded on XRPL with no credit anywhere (`x/repay.rs:537-569`). The observer also runs only while a repayment intent is open or coverage disagrees (`x/state.rs:707-730`), so an external return can sit unseen.
- **Buckets** (`x/state.rs:118-132, 177-226`): `D` = backed claims not yet issued (Borrow from `DebtCommitted`, Import from `IssuePrepared`; a pull still pending is NOT in `D`, `:171-173`), with `in_flight` for signed issuance. `O` = one stored counter. `R` = repayment `applied_e8s` while `ReturnVerified` or `EscrowBurnPending`. `H` = repayment `held`, per-sender unattributed aggregates, quarantined receipts, refunds in `RefundPending` or `Held`. Refinements: (a) `RF_d` is `H` until the refund Payment is signed, then `D` (with `in_flight`), then `O` at validation (`:202-217`); (b) there is no `U_d` term: an unknown ledger job keeps its amount in `D` or `R` until proven, and a lost-but-executed transfer appears as a coverage `Shortfall` (`:439-450`) that blocks new issuer attempts (`x/settlement.rs:980-984, 1026-1035`); (c) `O` has no origin split, so the design's `imported` versus `debt_originated` tag exists per operation only until delivery. **R:** report cumulative delivered by kind and never claim a per-token origin or a debt-free outstanding; only the import flow carries "no new debt". (d) `O` lags the ledger by the observer lag, so design 2.4 should cross-check `issued_outstanding_X` against the coverage snapshot (`snapshot_e8s`, `x/state.rs:453-481`).

## 2. The missing no-vault return, specified (R, follow-up after #586 merges)

Names below are recommendations. This is a new operation kind beside repayment, never an edit to #586.

**2.1 Start (ICP side).** `prepare_xrpl_return { amount_e8s, payer, network, request_nonce (32 bytes), to_subaccount (optional) }`, authenticated callers only. It mirrors `create_intent` (`x/repay.rs:137-216`) without the vault:
- Lane `ensure_ready`, not `ensure_open_for_issuance` (a pause of new issuance never blocks returns, as for repayment, `:153-155`). Network equals the configured one. Payer is a valid classic address and not the issuer. Destination is the caller's principal plus the optional subaccount, bound for life; reject the backend principal, management canister and anonymous (a transfer to the backend default account is the ledger minting account, so it would burn, `x/ledger.rs:43-45`).
- `0 < amount <= per_operation_cap <= 999,999,999,999,999 e8s` and `amount <= O`. Refuse with `CoverageUnavailable` when the canonical-return fee surplus is not funded (`x/issuance.rs:221-256`). Per-owner active-operation limit and 10 s throttle apply (`x/types.rs:22`, `x/mod.rs:73`).
- A never-reused destination tag from the same monotonic counter as repayments (`x/state.rs:733-740`), indexed in `tag_index`. The same `request_nonce` returns the same operation and never a second tag; the nonce comparison must include payer, amount, network and destination subaccount (`x/state.rs:280-309`, `x/repay.rs:59-75`). Unused intents expire after 7 days and the tag is never reassigned (`x/types.rs:1216`, `x/repay.rs:306-328`).
- The reply carries the exact Payment to sign, shaped like `payment_json` (`x/repay.rs:80-100`): issued currency to the issuer, `DestinationTag`, `Amount` exact, no `Paths`, no `SendMax`, no `Flags`.

**2.2 Accepted payment.** One consumer for the wallet hint (`observe_xrpl_return`, a lookup key only) and the paged issuer-history scan, deduplicated on `(network, issuer, uppercase tx hash)` (`x/repay.rs:348-354, 431-443`, `x/observer.rs:1-22`). Credit only when all hold:
1. At least 2 providers return the identical reduced receipt (`c/xrp_rpc.rs:435`, `c/issued_rpc.rs:820-860`); `validated` true and `tesSUCCESS` (`c/issued_rpc.rs:747-752`); a Payment to the issuer from another account (`x/repay.rs:392-404`).
2. Credit uses `meta.delivered_amount` only, exact icUSD asset, positive (`c/issued_rpc.rs:765-772`); the nominal `Amount` is evidence.
3. The sender's trust-line change equals the delivered amount leaving it (`x/repay.rs:405-420`); otherwise the value leaves `O` into quarantine and nobody is credited (`:490-523`).
4. Eligible: no `tfPartialPayment`, no `Paths` or `SendMax`, delivered equals nominal (`c/issued_rpc.rs:695-709`).
5. The tag maps to an open Return intent on this network and the sender equals the bound payer (the check at `x/repay.rs:525-535`, adapted to the new kind). The issuer sets RequireDest (`x/settlement.rs:56`), so XRPL itself should reject untagged payments (XRPL behavior, not re-verified here).
6. `O` covers the amount (`x/repay.rs:479-486`).

**2.3 Effect.** `O -= delivered`; the value moves to the intent's `held` (`H`), escrow untouched. Credited = whole e8s of `min(held, authorized amount)`, shown as `R` while its payout is pending (design `PR_d`, a reporting choice that leaves coverage unchanged). Sub-e8s and any excess stay held exactly (`x/types.rs:114-267`) and the owner resolves them with the existing refund or canonical actions (`x/repay.rs:1100-1182`). A delivered amount finer than 1e-40 is logged and quarantined (`:456-470`).

**2.4 Refunded on XRPL instead of credited.** Wrong, unknown, consumed or expired tag; sender not the bound payer; anomalous payment (partial, path, `SendMax`, delivered not equal nominal); payment arriving after expiry. These fold into the verified sender's aggregate and are refunded by an issuer Payment when at least `min_auto_refund_e8s` (default 0.01 icUSD, `x/types.rs:1132`, `x/retention.rs:267-372`); smaller amounts are retained exactly and not auto-refunded. A refund waits for the sender's trust line and room (`x/settlement.rs:1056-1066`).

**2.5 Payout.** An IC ledger job `CanonicalReturn`, escrow subaccount to the bound account, for the credited e8s. The ledger fee is charged on top from the operator-funded surplus, so the holder receives the full amount (`x/issuance.rs:218-256`; **D:** keep that policy or charge the holder). The job is persisted (nonce, `created_at_time`, memo, accounts, amount) before its first await (`x/ledger.rs:7-17`, `x/state.rs:513-545`); success is `Ok` or `Duplicate` plus the block fetched back and matched (`x/types.rs:483-486`). Only then does the credited amount leave `R`/`H` (pattern at `x/issuance.rs:327-367`). `JobSpec.to` is already a full `Account`, so the bound subaccount needs no new ledger code.

**2.6 Unknown outcomes are the same attempt** (design 6.2). Ledger job: resend the identical fields; after `TooOld` only the archive-following ICRC-3 scan from `ledger_tip_before` may resolve it; never a new nonce (`x/ledger.rs:15-17`). Inbound XRPL payment: `NotFound` or `NotValidated` is retried and nothing is credited before validation (`x/observer.rs:228-231`). Refund Payment: unsigned bytes and `Sequence` persisted before signing, signature and blob before submit, identical blob resubmitted (`MAX_SUBMITS 8`, 20 s), the lease held until a validated outcome or a complete-history non-inclusion proof after `LastLedgerSequence` (`x/settlement.rs:1-19, 45-46`, `x/types.rs:515-552`); a signed ambiguity blocks claim resolution (`x/issuance.rs:180-184`).

**2.7 Separation from repayment (F structure, R tests).** The Return kind has no vault id (`x/types.rs:712-719`): no vault exclusion, no accrual freeze, no `RepayCutoffBegun` or `RepaySettled`, no `EscrowBurn`, and `settle_return_against_debt` returns `NotApplicable` for other kinds (`x/repay.rs:729-741`). A Repayment tag cannot be consumed as a Return and the reverse. The tag fixes the meaning at signing and cannot be relabeled after payment, so the page must name the purpose beside the tag. No new `XrplIcusdEvent` should be needed because operations are snapshot state and events carry only vault effects (`x/state.rs:1-7`, `x/types.rs:1247-1250`; verify when implementing). Touch points: `x/types.rs`, `x/state.rs` (`open_intent_exists`, `add_op_liability`), `x/repay.rs` (expiry, attribution), `x/observer.rs:453` (kind check), `x/mod.rs`, `main.rs`, Candid, frontend.

## 3. Amounts and caps (15 significant digits)

What #586 enforces today (F):
- Entry points take `amount_e8s: u64` (`x/types.rs:926-961`). The conversion `ExactIssuedAmount::try_from_e8s` accepts up to 16 significant digits and fails with `TooPrecise` beyond that; it never rounds (`c/issued_amount.rs:305-307`).
- **The 15-digit validator is not wired into the lane.** `validate_normal_request` and `parse_normal_request` (positive, at most 8 decimals, at most 15 digits, `c/issued_amount.rs:241-264`) have no caller outside tests (grep of `b02584e4` finds only `:243` and test lines 830-855). The frontend `parseE8s` accepts up to `u64` (`xrplIcusdService.ts:46-52`). The 15-digit bound therefore holds only if a controller sets caps at or below 999,999,999,999,999.
- Per-operation cap: `check_caps` (`x/state.rs:311-322`) for import and borrow (`x/mod.rs:475`, `x/borrow.rs:178, 263`); repayment intents check it separately (`x/repay.rs:180`). Total outstanding: `ceil(O + D + R + H + reserved borrow net + new)` against `outstanding_cap_e8s` (`x/state.rs:323-337`). Both caps default 0, which refuses everything (`x/types.rs:375-376`).
- Inbound dust is kept exactly in a 1e-40 accumulator and finer values are refused and held (`x/types.rs:114-186`); canonical payout pays whole e8s and leaves the remainder held (`x/repay.rs:1162-1173`).

Missing for D7 (R, all follow-up, none an edit to #586):
1. Config accepts any `u64` for both caps (`x/mod.rs:218-223`). Clamp both to 999,999,999,999,999 e8s in `apply_config_update` and again as a constant inside `check_caps` and `create_intent`.
2. Pending imports are not counted. An Import in `EscrowPullPending` is neither in `D` (`x/state.rs:171-173`) nor a reservation, so concurrent imports can each pass `check_caps` and together exceed the total cap (from my reading, not tested). Count `EscrowPullPending` amounts, and re-check at `advance_import`.
3. No ceiling on the XRPL side. Coverage compares the issuer snapshot only with `O` (`x/state.rs:453-481`); add "snapshot total at most the clamp" as a coverage condition.
4. The holder TrustSet limit defaults to the outstanding cap (`xrplIcusdService.ts:176-181`), so an unclamped cap would write a limit longer than 15 digits. A holder-set limit is not a protocol bound (D7 sub-question stays open); the cap is.
5. Add the clamp to the Return intent and to the frontend parser before the amount is shown.

## 4. Holder opt-in (trust line)

- The holder signs one `TrustSet` with `LimitAmount { currency (160-bit hex), issuer, value }` and no flags, built only from backend config plus the live wallet connection, refusing a wrong network (`xrplIcusdService.ts:158-183`; wallet allows only Payment and TrustSet, `xrplWalletService.ts:26-46`). The issuer cannot create it, and `asfRequireAuth` is not set, so no issuer approval step exists (inventory Q3).
- Delivery admission needs the line to exist, not be issuer-frozen, and have room for the amount (`x/settlement.rs:1123-1157`). If it is missing or too small, delivery waits (`:1056-1066`) and the owner may take `ReceiveCanonical` while no signed attempt is ambiguous (`x/issuance.rs:180-213`). This supports the page sentence that escrowed icUSD stays safe and returnable, with that condition.
- Reserve (sources-native.md section 3 rows 6 and 7, live ledger values on 2026-10-30, which validators can change): 1 XRP base plus 0.2 XRP per owned object, base fee 10 drops. A trust line is one owned object. The "first two lines free" remark in the facts file is unverified, so the page must not promise it.
- The page must (frontend-wallets.md section 4.5): show "Step 1: enable icUSD on your XRPL account" before any transfer; say it is one wallet-signed transaction that adds a balance entry and needs XRP reserve; confirm the line by a canister-mediated read, not the wallet's word; name Xaman (the only wallet in #586, `xumm` 1.8.1, manual address or QR fallback); show issuer and network from backend config, never from user input. For the Return flow it must also show the tag and purpose, the exact amount, the bound ICP account, and that a wrong tag is refunded, not redirected.
- Wallet: Xaman only (inventory Q6). Crossmark and GemWallet are future adapter work, not a groundwork gate.

## 5. RPC, finality, signing

- Path: HTTPS outcalls to rippled JSON-RPC from the backend (`c/issued_rpc.rs:1-20`, `c/issuer_rpc.rs`). Reducers pin the status, drop headers and keep only consumed fields so replicas converge. Read quorum is 2 identical reduced responses (`c/xrp_rpc.rs:435`). Mainnet has three read hosts and testnet has two (`c/network.rs:77-84`).
- **Non-443 port (F plus open).** One mainnet host and one testnet host use port 51234. #586 itself treats the mainnet one as possibly unreachable and requires a spare 443 provider (`c/xrp_rpc.rs:1131-1140`). Testnet has exactly 2 hosts, so a quorum of 2 needs the non-443 one: if outcalls to that port fail, the testnet lane cannot read at all. Whether IC outcalls accept it is still not established (sources-native.md open question 3). R: probe it in a separate, approved step; do not assume.
- Finality: `ledger_index: "validated"` with `strict: true` (`c/issuer_rpc.rs:62`), `validated` true (`:115`, `c/issued_rpc.rs:747`), `tesSUCCESS` only, `delivered_amount` only. History is paged with fixed ledger bounds; each provider's `complete_ledgers` must cover the interval or the pass stays put with a typed `HistoryGap` (`x/observer.rs:14-22`).
- Signing: threshold Ed25519 over `STX\0` plus the unsigned serialization, no pre-hash; public key `0xED` plus 32 bytes; local `TXN\0` SHA-512Half for the hash (`c/sign.rs:1-41`, `c/ted25519.rs:122-155`). The attach is 30B cycles per signature (published about 26B), signing only on real operations. Cycle cost per outcall is unmeasured here; measure on staging before quoting.

## 6. Authority and trust limits (stated plainly)

- One signer. The issuer key is derived from the backend canister's own key, so whoever can upgrade the backend can make it sign any issuer transaction. Controller-only endpoints gate configuration (`main.rs:9786-9792`, 9886-10008); the mainnet controller set is not in the diff. **D6 stays open; no controller set is invented here.**
- The source builds only `Payment` and `AccountSet` for the issuer (`x/settlement.rs:80-121`; a `TrustSet` codec exists for tests, `c/issued_codec.rs:92-101`). No `SetRegularKey`, `SignerListSet`, `AccountSet` disable-master, blackhole or `RequireAuth` appears in `b02584e4` (grep of the backend source). The master key stays enabled and nothing is blackholed.
- `IssuerPolicy { allow_clawback, no_freeze }` has no default, cannot change once setup starts, and the two cannot both be set (`x/types.rs:273-292`). Clawback needs an empty owner directory and is irreversible; NoFreeze is irreversible (launch checklist lines 55-58). Neither is used by any code path in #586, so enabling clawback grants a permission that still needs new code and a controller-approved upgrade to exercise. Choosing a policy is a decision before setup; local fixtures use false and false, "not a production decision". The page must disclose whichever is chosen.
- A single key means a lost, destroyed or unreachable canister key makes the issuer permanently inaccessible: no new issuance, no refunds, and the ICP escrow is equally stuck. An issuing account that keeps issuing cannot be blackholed (sources-native.md section 3 row 7).
- Trust in public XRPL providers is bounded by the quorum of 2, not removed. Disabled is not paused: the lane is `enabled = false` by default, and a pause of issuance or settlement leaves existing operations reconciling (`x/mod.rs:696-698`).

## 7. Implementation constraints and the merge dependency

- **Merge dependency.** XRPL work cannot start until #586 merges or is explicitly stacked on its head. It is 36 commits and 75 files (inventory Q8), its body says "Do not merge before Codex's final review", and the follow-up touches its core types, observer and Candid. R: wait for the merge; do not build on a moving branch. Nothing in this document comments on, edits or checks out #586.
- **Backend Wasm size.** The code section after the production build steps was 9,648,104 bytes (77% of the 12 MiB IC limit of 12,582,912) after merging main `01615221` (`docs/verification/2026-10-18-xrpl-icusd-verification.md`, 139-153). It was 11,826,498 (94%) earlier and the report says headroom shrinks as main grows (`:115-117`). The project keeps a 256 KiB executable-code reserve (`Cargo.toml` comment, `icp.yaml:617`; my reading: a practical ceiling near 12,320,768). PocketIC 7 caps the code section at 10 MiB (10,485,760), leaving about 837,656 bytes for the follow-up before the PocketIC suites need a special build again (`:40-45, 137`). Budget the follow-up against that smaller figure.
- **Build gate.** The release recipe runs `ic-wasm shrink`, `optimize O1`, `gzip`, then `scripts/verify-xrpl-icusd-artifact.sh --require-mainnet`, which rejects a fixture artifact (`icp.yaml:608-625`). A testnet demo (D9) uses the default-off `xrpl-testnet-fixture` feature, which pairs `test_key_1` with XRPL testnet only (`Cargo.toml` feature comment, `c/network.rs:1-18`), plus a test ICRC ledger whose minting account is the backend default (`x/ledger.rs:46-65`). No real canonical icUSD is escrowed.
- **Candid and state.** A new `OperationKindTag::Return` is an output-variant extension that the compatibility check may flag; review it, do not bypass it, and ship the regenerated frontend declarations first (the launch checklist treats the Event variant the same way, item 2). A stored `Return` operation also makes the snapshot undecodable by a pre-follow-up Wasm, so rollback after first use is unsupported (add a snapshot round-trip test). The backend is upgraded, never reinstalled.
- **Operational bounds** stay as in #586: 10 active operations per owner, 10,000 globally, 30-day retention of terminal operations, dedup compaction behind the scanned frontier, snapshot about 46 MB against a 64 MiB budget (launch checklist 102-111).

## 8. Profile alignment (`src/rumi_bridge_destinations/profiles/xrpl.json`; not edited)

Matches: `xrpl_iou`, `backend_module`, `existing_work = pr586`, `xrpl_decimal_15` with `max_e8s` and `max_outstanding_e8s` both 999999999999999, escrow label, signing path shape and key names, `xrpl_validated` finality, quorum 2, Xaman only, all claims at defaults, `status = disabled`, `instance_ref = null`, and the two required blockers `xrpl-pr586-unmerged` and `xrpl-return-intent-missing` on all three networks (validator V7 requires at least these).
Mismatches and gaps for the coordinator:
1. **Devnet.** The profile has `xrpl-devnet` (identity 2, a devnet host). #586's `XrplNetwork` has only Mainnet and Testnet (`c/network.rs:46-50`) and no devnet hosts, so nothing in #586 can serve it. Needs a blocker such as `xrpl-devnet-unsupported-by-pr586` (code) or removal of the record.
2. **Derivation element.** The profile template `{network_identity}` follows design 5.1 and V6, where the identity of an XRPL network is its numeric id (0, 1, 2); #586 uses the text "mainnet" or "testnet" (`x/types.rs:59-67`). Clarify the template so it does not imply the numeric id.
3. **Testnet port severity.** The profile gives all networks the same `xrpl-outcall-port-unknown` blocker, but testnet has no spare provider (2 hosts, quorum 2, one non-443) while mainnet has a spare. Say so in the testnet blocker text.
4. **Additional blockers to consider (R, V7 accepts extras):** `xrpl-caps-not-hard-bounded` (code: sections 3 items 1 to 3), `xrpl-issuer-policy-undecided` (decision: section 6), `xrpl-backend-wasm-headroom` (external: section 7).
5. **Design drift.** Design 9.2 `amount_bounds` omits `max_outstanding_e8s`, `domains` and `open_decisions`, which the profile and validator already carry. `domains.return_id` is null; the follow-up defines it as the dedup key plus tag (section 2.2).
6. The holder-trust-line prerequisite carries no reserve fact (section 4); optional.

## 9. Future focused tests and ordered steps (future implementation, not groundwork gates)

Tests (each small, written with the step that needs it):
- Cross-kind negatives (design section 4): a Return-tag payment is not accepted by the repayment path and the reverse; a Return never changes any vault, exclusion, freeze or debt event.
- Attribution matrix: unknown, consumed and expired tag, wrong payer, partial, path, `SendMax`, delivered not equal nominal, not validated, non-`tesSUCCESS`: all end refunded or no-effect, never credited.
- Dedup: one transaction via hint and scan applies once, including below the dedup floor after compaction.
- Payout: lost reply and resend of identical fields, `Duplicate`, `TooOld` then scan; credit and `O` unchanged until the block is proven.
- Amounts: cap above 999,999,999,999,999 rejected at config; 1,000,000,000,000,000 rejected; sub-e8s remainder held; concurrent imports cannot exceed the total cap.
- Snapshot round trip, Candid check reviewed, and an artifact size check against 10 MiB.

| # | Step | Size | Depends on |
| --- | --- | --- | --- |
| 0 | #586 merged (or an agreed stack on its head) | external | Codex review, Miles |
| 1 | Cap clamps, pending-import counting, snapshot ceiling, parser parity (section 3) | small | 0 |
| 2 | Return state, `prepare_xrpl_return`, nonce and tag rules, expiry | medium | 0 |
| 3 | Observer attribution, `observe_xrpl_return`, `open_intent_exists`, buckets | medium | 2 |
| 4 | Canonical payout and held and refund resolution reuse | medium | 3 |
| 5 | Candid, declarations, frontend Return card and Step 1 opt-in card | medium | 4 |
| 6 | PocketIC end-to-end plus negatives, watching the 10 MiB cap | medium | 4 |
| 7 | Testnet fixture run on a synthetic ledger, port probe | small code, approvals needed | 6, Miles |
| 8 | Decisions: D6 controllers, issuer policy, fee-surplus policy | none (decisions) | Rob, Miles |

## 10. Not grounded in source

- Whether IC outcalls accept non-443 ports; the mainnet controller set; per-outcall cycle cost; that XRPL rejects untagged payments to a RequireDest issuer (protocol knowledge, no facts-file source).
- `observer.rs` pass internals and `retention.rs` refund drain were read for the points cited, not exhaustively; PR test bodies and the frontend components were not audited. No test, build or probe was run.
- Concurrent-import cap overshoot (section 3 item 2) is read from the code, not reproduced.
