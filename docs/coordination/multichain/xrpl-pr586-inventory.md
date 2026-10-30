# BB-41: Existing XRPL icUSD PR inventory (PR #586)

- Card: BB-41 "Existing XRPL icUSD PR inventory" (Rumi Multichain icUSD board, worker W13)
- Date: 2026-10-30
- Scope: read-only inventory of an unmerged PR. Nothing here claims deployment.
- PR: RumiLabsXYZ/rumi-protocol-v2 #586, "XRPL: click-to-deposit XRP and XRPL icUSD borrow/repay (disabled by default)", branch codex/xrpl-icusd, head b02584e4 (matches the PR headRefOid), base 01615221.

## Method and limits

- `gh pr view 586` (metadata and body) and `gh api .../pulls/586/files` (file list, GET only) worked.
- `gh pr diff 586` failed with HTTP 406 (diff over 20,000 lines). The diff was read with local read-only `git show` and `git grep` against the same head commit. No checkout, fetch, or ref change was made.
- Not read in depth: observer.rs (829 lines), retention.rs, most of settlement.rs and state.rs, the verification doc, and the test bodies. Those were sampled by name only.
- Not touched: the rumi-base-* folders, .codex/, lookout-kanban, key or identity files.

## Q1. What XRPL icUSD is

- Asset type: an XRPL issued currency (IOU) on trust lines. Not an MPT. No MPToken or MPTokenIssuance usage was found in chains/xrp, xrpl_icusd, or the frontend XRPL services.
- Currency code: a 160-bit code that is ASCII "icUSD" right-padded with zeros. Constant `ICUSD_CURRENCY_HEX`, src/rumi_protocol_backend/src/chains/xrp/issued_amount.rs:524. `CurrencyCode::icusd()` at line 536.
- Issuer model: one issuer account derived by threshold Ed25519 at path [rumi, xrpl-icusd, v1, network] (src/rumi_protocol_backend/src/xrpl_icusd/types.rs:59-66; derived in xrpl_icusd/mod.rs, configure(), around line 274). No production issuer address is hard-coded. Classic-address literals appear only in test modules and testdata: issued_amount.rs around lines 918-919, issued_rpc.rs around lines 1752-1754 and 2192, and chains/xrp/testdata/xrp_issued_kat.json.
- Decimals: 8 (`E8S_DECIMALS = 8`, issued_amount.rs:27). Values are rippled STAmount-style canonical amounts: a 16-digit mantissa in [1e15, 1e16) and an exponent in [-96, 80] (issued_amount.rs header and constants).
- Request bounds: normal requests allow at most 8 decimals and 15 significant digits (`NORMAL_MAX_*` constants). Receipt parsing keeps any valid mantissa and exponent, including sub-e8s dust, so held value stays exact.
- Caps: `outstanding_cap_e8s` and `per_operation_cap_e8s`, set by a controller through configure_xrpl_icusd, default to 0 (types.rs:364-380).
- Holders need a trust line. A `TrustLineMissing` error exists, and the frontend builds the TrustSet from backend config (src/vault_frontend/src/lib/services/xrplIcusdService.ts, header lines 10-13).

## Q2. Debt-backed or escrow-backed?

Verdict: ESCROW-BACKED at the XRPL token level. Each XRPL unit is backed by canonical icUSD in a dedicated escrow subaccount on the backend's icUSD ledger. The entry paths differ in where that canonical icUSD comes from.

- borrow_to_xrpl (xrpl_icusd/borrow.rs header, lines 1-5): mints canonical icUSD into escrow (`IcJobPurpose::EscrowMint`, types.rs:401-402; `mint_job_spec`, ledger.rs:74). It records ICP vault debt through the SAME `BorrowFromVault` event. Units from this path are therefore debt-originated on ICP, then escrowed.
- issue_xrpl_from_canonical (xrpl_icusd/issuance.rs header, lines 4-5): pulls the holder's existing canonical icUSD into escrow with ICRC-2 (`IcJobPurpose::ImportPull`). The header says it creates no debt and charges no borrowing fee.
- Repayment (xrpl_icusd/repay.rs header, lines 17-29): verified XRPL return, then `EscrowBurn` (escrow to minting account, ledger.rs:87), then the debt change (`RepayCutoffBegun`, `RepaySettled`). The burn is proven before any debt change.
- Escrow identifiers: `ESCROW_DOMAIN = "rumi:xrpl-icusd:escrow:v1"` and `escrow_subaccount()` (types.rs:33-41). `IcJobPurpose` variants EscrowMint, EscrowBurn, ImportPull, CanonicalReturn (types.rs:400-411). `check_caps` (xrpl_icusd/state.rs:312).
- Separation from chain supply: the PR does not touch chains/admin.rs or MultiChainState. `chain_supplies` is not referenced in its files. XRPL accounting lives in `State.xrpl_icusd` (state.rs:4565) with its own buckets and coverage (`check_coverage`, state.rs around line 439). So the books are separate from `chain_supplies`. The borrow-path escrow is still economically debt-originated, so it should not be treated as zero-debt backing.

## Q3. Who controls issuance on XRPL

- Signing key: threshold Ed25519 (management-canister Schnorr, algorithm ed25519). Key name from `xrp_schnorr_key_name()`: `key_1` for production, `test_key_1` for the testnet fixture (chains/xrp/config.rs:31, 34, 46; ted25519.rs:88). Same key name as the XRP collateral rail and Solana, with a distinct derivation path. `check_network_for_key` ties the network to the key (xrpl_icusd/mod.rs, around lines 249-262).
- No hot key, no XRPL multisig (no SignerList or Signers usage), and no RegularKey or blackholing code in the diff.
- Issuer flags set by AccountSet steps (xrpl_icusd/settlement.rs:55-59): DefaultRipple (ASF 8), RequireDest (ASF 1), TransferRate 1e9 (par). Then AllowTrustLineClawback and/or NoFreeze, only as the explicit `IssuerPolicy` says (types.rs:273-318). There is no default, and both cannot be set. ASF values are confirmed by xrpl_icusd/tests.rs:1645-1648. Setup verifies the final flags (settlement.rs:387-460).
- `asfRequireAuth` is not set anywhere in the diff. Holders need only their own trust line, with no issuer authorization step.
- Clawback is an opt-in policy choice. The checklist says clawback and NoFreeze are irreversible (docs/releases/2026-10-18-xrpl-icusd-launch-checklist.md, lines 55-58).
- Controller-only admin endpoints (`xrpl_require_controller`, `ic_cdk::api::is_controller`, src/rumi_protocol_backend/src/main.rs around lines 9786-9792): configure_xrpl_icusd (around line 9886), set_xrpl_icusd_pauses, verify_xrpl_icusd_minting_account, run_xrpl_issuer_setup_step, get_xrpl_icusd_coverage, and the reconcile endpoints. Who the mainnet controllers are is not in the diff: unclear.
- Issuer sequencer: single-flight through `ISSUER_BUSY` (xrpl_icusd/mod.rs thread_local block). It persists the unsigned bytes, signature, and signed blob before submit (settlement.rs:11-12). `MAX_SUBMITS = 8` (settlement.rs:45). `RESUBMIT_AFTER_NS` is 20 s (settlement.rs:46).

## Q4. How it reaches XRPL

- Path: HTTPS JSON-RPC outcalls from the backend canister, with transform reducers (chains/xrp/issuer_rpc.rs, chains/xrp/issued_rpc.rs). The browser's XRPL polling is only a hint, per the PR body.
- Hard-coded public hosts (chains/xrp/network.rs:77-84). Mainnet reads: xrplcluster.com, xrpl.link, s1.ripple.com (port 51234). Testnet: s.altnet.rippletest.net:51234 and testnet.xrpl-labs.com. Fixture builds pin testnet only (network.rs, around lines 96-101).
- Reads need `READ_QUORUM = 2` identical reduced responses (chains/xrp/xrp_rpc.rs:435; issued_rpc.rs `quorum_issued_receipt`, around line 820; issuer_rpc.rs quorum functions, around lines 137-160).
- Finality: `ledger_index: "validated"` with `strict: true` (issuer_rpc.rs:62). A result must report `validated` true (issuer_rpc.rs:115; issued_rpc.rs:747, `NotValidated`). Validated non-tesSUCCESS results are not credited.
- Credit: `meta.delivered_amount` only (issued_rpc.rs header lines 12-15; lines 685-686, 765).
- Signing: XRPL Ed25519 with the 0xED-prefixed SigningPubKey (chains/xrp/sign.rs:17). Signed message is "STX\0" followed by the unsigned blob (sign.rs:12, 25-27). Signature from threshold Ed25519 `sign_message` (ted25519.rs:131).
- Submit: `submit_plan` and `submit_signed_blob` (issuer_rpc.rs:585, 623). The signed blob is persisted before submit (settlement.rs:11-12).

## Q5. Which canister and where the accounting lives

- Logic lives in the existing backend canister, rumi_protocol_backend. No new canister. The PR body says the backend is size-optimised (opt-level s, backend crate only).
- Module: src/rumi_protocol_backend/src/xrpl_icusd/ (mod.rs, types.rs, state.rs, ledger.rs, issuance.rs, borrow.rs, repay.rs, observer.rs, settlement.rs, retention.rs, and tests). Chain primitives: src/rumi_protocol_backend/src/chains/xrp/ (issued_amount.rs, issued_codec.rs, issued_rpc.rs, issuer_rpc.rs, network.rs).
- Accounting types: `XrplIcusdState` (state.rs:20; State field at state.rs:4565), `XrplIcusdConfig` (types.rs around line 342), `IcLedgerJob` and `IcJobPurpose` (types.rs around line 400), `XrplIcusdEvent` (types.rs around line 1251). The events are wrapped as `Event::XrplIcusd` (event.rs around line 1197, serde rename "xrpl_icusd").
- Disabled by default: `XrplIcusdConfig::default()` (types.rs:364-380) sets enabled false, issuance_paused true, and network, policy, and issuer_address None. Issuer setup is NotStarted, and caps are 0. User methods return Disabled until the lane is enabled, the network is set, a policy is stored, issuer setup is Ready, and the minting account is verified (`ensure_ready`, state.rs:232-252). New issuance is also blocked while issuance_paused is set (`ensure_open_for_issuance`, state.rs:255-261).
- Build gate: the `xrpl-testnet-fixture` Cargo feature is default OFF (src/rumi_protocol_backend/Cargo.toml:18). `verify_release_artifact` (network.rs, around lines 250-260) refuses artifacts that carry the fixture marker.
- Activation is six approval-gated steps in the launch checklist (lines 53-76). The diff itself activates nothing.

## Q6. Frontend wallets

- Xaman only. src/vault_frontend/src/lib/services/xrplWalletService.ts: adapter id 'xaman' (around line 348), `createXamanAdapter` (line 209), browser PKCE with the `xumm` SDK built from the public key alone (lines 203-206), key from `VITE_XAMAN_API_KEY` (around line 120).
- src/vault_frontend/src/lib/components/wallet/XrplWalletConnect.svelte: button "Connect XRP wallet (Xaman)" (line 61). Fallback is a manual address or QR path (line 50).
- Used by src/vault_frontend/src/lib/components/vault/XrplIcusdPanel.svelte (imports, lines 20-22) and src/vault_frontend/src/lib/components/borrow/XrpBorrowModal.svelte (Xaman deep link, line 363).
- src/vault_frontend/package.json line 74: `"xumm": "1.8.1"`.
- A case-insensitive search found no Crossmark, GemWallet, Ledger, or other XRPL connector. Oisy appears only as the ICP login type that gates clicks (xrpDepositFlow.ts). It is not an XRPL signer in this diff.

## Q7. Collision check against an escrow-backed XRPL bridge token

| Identifier the PR defines | Verdict | Why |
|---|---|---|
| Currency code "icUSD" with issuer (issued_amount.rs:524) | possible conflict (user-facing) | A second XRPL "icUSD" from another issuer shows the same symbol in wallets. Code is safe: the PR refuses same-symbol, other-issuer credit (issued_amount.rs:509, 667-677), and the frontend pins the issuer from backend config. A bridge needs its own code or an explicit issuer label. |
| Escrow domain "rumi:xrpl-icusd:escrow:v1" (types.rs:33-41) | possible conflict if reused | Sharing this canonical-ledger subaccount would mix balances, buckets, and coverage between the two lanes. Needs a distinct domain string. |
| Issuer path [rumi, xrpl-icusd, v1, network] (types.rs:59-66) | possible conflict if reused | Same key and path derive the same XRPL account, so the same issuer. Needs a distinct path. |
| Threshold key key_1 / test_key_1 (chains/xrp/config.rs:31-46) | no conflict | Key is shared with the XRP collateral rail and Solana, but derivation paths differ. A bridge still needs a unique path. |
| State field `xrpl_icusd`, event tag "xrpl_icusd", error `XrplIcusdError`, Candid methods (configure_xrpl_icusd, borrow_to_xrpl, observe_xrpl_repayment, and others) | possible conflict only if same canister | Names collide only if the bridge lives in the backend canister and reuses them. A separate canister has no conflict. |
| chain_supplies / MultiChainState | no conflict | The PR does not touch chains/admin.rs or MultiChainState. |
| Frontend xrplIcusdService.ts, XrplIcusdPanel.svelte | unclear | Bridge UI names are outside what I could check from the diff. |

Reserved names a bridge would also need: the escrow domain, the issuer derivation path, the currency code, the State field, and the event tag.

## Q8. PR state

- GitHub state at query time: OPEN, isDraft false, mergeable MERGEABLE, mergeStateStatus CLEAN.
- Created 2026-10-18T12:12:08Z. Last updated 2026-10-27T21:11:47Z. Last commit "docs(xrpl): record fourth post-merge verification" (2026-10-27). 36 commits, 75 changed files (+35,337 / -743).
- Body quotes: "The lane is disabled by default." "Merging does not deploy or activate anything." "Do not merge before Codex's final review." Its "Not proven yet" section lists the R10 public XRPL testnet run and a real-browser run as unproven.
- The body does not claim deployment. Its test claims were not re-run (no builds or tests, per the rules).

## Summary table

| Question | Finding | Evidence | Verdict |
|---|---|---|---|
| 1. Asset | XRPL IOU on trust lines, currency "icUSD", 8 decimals, canonical 16-digit mantissa | issued_amount.rs:27, 524; no MPT usage | XRPL IOU (not MPT) |
| 2. Backing | Escrow-backed. Borrow path mints canonical into escrow under ICP vault debt. Import path creates no debt. | borrow.rs 1-5; issuance.rs 4-5; types.rs 400-411 | Escrow-backed; borrow-origin units are debt-originated |
| 3. Issuance control | Threshold Ed25519 (key_1 in production), no hot key, no multisig. Flags: DefaultRipple, RequireDest, TransferRate, optional clawback or NoFreeze. No RequireAuth. | config.rs 31-46; types.rs 273-318; main.rs around 9786 | Threshold-controlled; controller set unclear |
| 4. RPC path | Canister HTTPS outcalls to 5 hard-coded public hosts, quorum 2, validated ledger, delivered_amount only, STX-prefixed signing | network.rs 77-84; xrp_rpc.rs 435; issuer_rpc.rs 62, 115, 623 | Documented in code |
| 5. Canister and gating | Backend module xrpl_icusd/; disabled by default; controller-only admin; fixture feature off | types.rs 364-380; state.rs 232-261; Cargo.toml 18 | Disabled by default |
| 6. Wallets | Xaman only (PKCE, xumm 1.8.1), manual address/QR fallback | xrplWalletService.ts 203-209, 348; XrplWalletConnect.svelte 61 | Xaman only |
| 7. Collision | Symbol clash (user-facing), escrow domain, issuer path, shared state and Candid names | issued_amount.rs 509, 667; types.rs 33-66 | Possible conflict (see Q7 table) |
| 8. PR state | OPEN, not draft, MERGEABLE, CLEAN; updated 2026-10-27; no deployment claimed | gh pr view 586 | Unmerged; no deployment claimed |

## Could not determine

- Mainnet controller set, and who can change the issuer key policy beyond the code's gates: unclear from the diff.
- Whether every frontend path filters XRPL icUSD by issuer: the TrustSet and config paths do; I did not audit every component.
- Bodies of observer.rs, retention.rs, settlement.rs, and most of state.rs, plus the test suites: not read in depth.
- The bridge's own names: not in scope here. The Q7 verdicts assume a bridge that reuses none of the listed identifiers is the default.
