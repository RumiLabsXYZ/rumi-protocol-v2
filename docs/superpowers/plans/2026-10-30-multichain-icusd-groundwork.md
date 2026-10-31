# Multichain icUSD: integrated implementation plan (groundwork close-out)

Card BB-40, 2026-10-30. Coordinator: Claude (Opus 5.5). Supervisor: Miles, for Rob. Branch `codex/base-icusd-bridge`.

This plan turns the accepted groundwork into queued future work. **Nothing in it is started, deployed, signed, funded or
activated.** Every queued card waits until Rob or Miles opens the implementation phase. When it opens, the same staffing rules
apply: Claude Sonnet workers for substantive code, Claude Haiku for narrow mechanical tasks, at most two Sonnet lanes plus one
Haiku task at once, one build executor, one owner per file. Every destination profile stays `disabled` until a separate,
explicit activation authorization for that network.

## 1. What groundwork delivered (proof level: source and local-test)

| Piece | Where | Evidence |
| --- | --- | --- |
| Shared escrow design for all nine chains | `docs/superpowers/specs/2026-10-30-multichain-icusd-groundwork-design.md` | Accepted with Miles' corrections (custodied returns, holder burns, Sui epoch expiry, XRPL wiring gaps) |
| Research and per-chain notes | `docs/coordination/multichain/` (`reuse-inventory`, `sources-evm`, `sources-native`, `xrpl-pr586-inventory`, `chain-evm`, `chain-solana`, `chain-xrpl`, `chain-sui`, `frontend-wallets`) | Each accepted after coordinator source checks |
| Disabled destination profiles and validator | `src/rumi_bridge_destinations/` (types, rules V1 to V11, nine profiles, 19 network records after the XRPL devnet record was removed, dry run) | One build lane: 63 tests passed, dry run exit 0 with 0 violations (final refresh); Miles reran the first dry run independently |
| Board, priorities and Lookout | `docs/coordination/base-icusd-bridge-kanban.md` and siblings | Published changed-only with no-op readbacks |

Not proven by groundwork: any execution on a new chain, browser-wallet acceptance (the Base browser test BB-27 still waits on
Rob), production activation, cycle costs, live provider behavior, and every item named "implementation missing".

## 2. Decisions in force (design section 12)

Settled for planning (Miles, under Rob's authority, no live action): D1 in-place EVM generalization on a working branch, no
deployed upgrade; D1a legacy Base-named methods on new EVM instances; D1b additive native RPC variants where useful; D2 and D7
PR #586's issued currency with 15-digit per-operation and total caps, MPT deferred; D3 Solana memo plus durable nonce; D4 Sui
Move gate with a separate UpgradeCap authority; D5 identical `BridgedIcUSD` bytecode; D8 Monad escrow-backed labels and exact
binding; D9 synthetic test ledgers. **Open: D6** (controllers, production custody, authority activation), queued as BB-61.

Rules every card keeps: the reviewed Base APIs, Candid, stable layout and live signing domains stay unchanged (a `base` slug
must reproduce them byte for byte); generated declarations are regenerated through the canonical command, never hand-edited;
unknown outcomes are reconciled as the same attempt; no ICP payout before a destination burn is final; inexact amounts are
rejected, never rounded; no operator hot keys.

## 3. Ranked subprojects and queued cards

Ranks follow the board's priority list. A card's command is the focused check it must pass when built; none is run now.

### 3.1 EVM family (rank 3): Ethereum, Arbitrum, Robinhood Chain, HyperEVM, Monad

Source: `docs/coordination/multichain/chain-evm.md` (changes C1 to C11, steps S1 to S8, tests T1 to T8, config CFG-1 to 7).

| Card | Scope | Files | Depends on | Focused command | Size |
| --- | --- | --- | --- | --- | --- |
| BB-42 | Slug templates for domains, escrow label, key root and salt; chain table with Base rows only; Base domain and id regression check (C1, C2, C8, T1) | `src/rumi_icusd_bridge/src/ids.rs`, `state.rs` (or a new `chains.rs`), `lib.rs` | BB-40 | `cargo test -p rumi_icusd_bridge --lib ids::tests` | S |
| BB-43 | Table-driven init and RPC validation; head-rule dispatcher (finalized tag with quorum, Monad Verified head at finalized minus 3, Blocked); existing finalized-head, init and ICRC-21 tests unchanged (C3, C4, C7, C9, T2 to T8) | `state.rs`, `evm/rpc.rs`, `observer.rs`, `icrc21.rs`, `views.rs` | BB-42 | `cargo test -p rumi_icusd_bridge --lib state::tests::init_validation` and `--lib evm::rpc::tests` | M |
| BB-44 | Additive native RPC-service variants (D1b), declarations regenerated canonically (C5, C6) | `types.rs`, `evm/rpc.rs`, `rumi_icusd_bridge.did`, generated declarations | BB-43 | `cargo test -p rumi_icusd_bridge --lib candid_interface_matches_checked_in_did` | S |
| BB-45 | Per-chain defaults (gas caps, gas limits, log chunk sizes, Monad reserve) and a sibling Foundry deploy script with its own allow-list (C10, C11, CFG-2 to 7); the existing Base script is not edited. **Delivers actual configurable EVM source support:** opens the canister install gate for the Ethereum, Arbitrum and Monad rows (mainnet and testnet), so their init, RPC validation and head-rule paths run in local tests. Robinhood's gate opens only with its BB-46 tag probe record (head rule stays `Blocked` until then); HyperEVM's only with archive-capable independent providers or a reviewed observer redesign (`hyperevm-historical-state`). Catalog profiles stay `disabled`; real installs and activation stay held. Ethereum's realistic providers come from BB-44's native variants (not a source gate). The finalized-tag reads outside the head-rule dispatcher (refund mint check, final-nonce proofs) are routed in BB-43 itself (Miles' review), before this card opens Monad | `chains.rs`, `state.rs`, `admin.rs`, `foundry/script/` (new), `foundry/test/` | BB-43 | `forge test --match-contract <new deploy test>` and `cargo test -p rumi_icusd_bridge --lib admin::tests` | M |
| BB-46 | Per-chain activation evidence, held for separate authorization: Robinhood keyless providers and `finalized` tag probe (a passing record flips the Robinhood row's head rule and install gate in this card), Monad fast-tag consensus, HyperEVM exit (archive-capable independent providers or a reviewed observer redesign; a redesign is new source work with its own card) | profile notes and evidence only | BB-43, separate probe authorization | read-only probe records (none run now) | L |

### 3.2 Shared reporting and frontend (rank 4)

| Card | Scope | Files | Depends on | Focused command | Size |
| --- | --- | --- | --- | --- | --- |
| BB-58 | Aggregate reporting: per-destination audit queries summed outside any canister with index sets `D` and `X`, custody and strays counted once; analytics subtracts escrow from circulating supply | `src/rumi_analytics/src/timers.rs` (around 188-199), a read-only aggregator | BB-42 | the analytics crate's focused unit tests for the new formula | M |
| BB-59 | Frontend destination selection: destination view, one `assertCanSign` before any wallet call, disabled destinations unselectable, destination key on new intents with legacy Base records bound explicitly to Base, EVM chain rows | `src/vault_frontend/src/lib/services/baseBridge/`, `routes/bridge/` | BB-43 | the frontend's focused vitest files for the changed services | M |
| BB-60 | Profile consumption and upkeep: a TypeScript reader that keeps amounts as decimal strings, and profile updates as each implementation lands (status stays disabled) | `src/rumi_bridge_destinations/profiles/`, a small frontend reader | BB-59 | `cargo test --offline -p rumi_bridge_destinations` plus the reader's vitest | S |

### 3.3 Solana (rank 5)

Source: `docs/coordination/multichain/chain-solana.md` (sections 2 to 4, 10 and 12).

| Card | Scope | Files | Depends on | Focused command | Size |
| --- | --- | --- | --- | --- | --- |
| BB-47 | Solana instance skeleton by copy (ledger saga, accounting with `RC`, `BU` and stray buckets, admin, ICRC-21, stable layout) and message builders for mint, return, burn and refund with golden vectors | a new Solana bridge canister (name proposed in BB-47), patterns from `chains/solana/tx.rs` | none (after BB-58 by scheduling preference only) | the new crate's unit tests for builders and accounting | M |
| BB-48 | SOL RPC mirror and consensus settings; durable-nonce pool with persist-before-sign and same-attempt reconciliation (a memo is discovery only; adoption needs the exact stored message); history-availability probe tooling | the new canister, `src/sol_rpc_mock` | BB-47 | the new crate's reconcile tests, including the advance-only adversarial case | L |
| BB-49 | Mint, return, burn and refund lanes: strict return verifier (exact top-level `TransferChecked` plus memo), strays refunded, `RC` to `BU` to `PR` gating with no payout before a final burn, custody audit, unattributed holder-burn seam; also the Solana supply and custody reader for the BB-58 collector (moved from BB-58, which keeps the shared collector); carried from BB-47 (Miles, 2026-10-31): the persisted canonical-ledger saga for pull, refund and release (exact tuple and operation id saved before effects, same-attempt reconciliation after unknown outcomes, bounded archive proofs, caller and allowance binding, upgrade and retry persistence, fees disjoint, no payout before a final burn), refund ids extended with a stable transfer locator (outer plus optional inner instruction index) and each refund claim bound to its observed source account and amount (amount and recipient stay out of the id; a vector shows two transfers in one transaction get distinct ids and a retry keeps its id), and custody wiring where an unexplained deficit is never an OK check | the new canister | BB-48 | the new crate's lane tests plus the holder-burn adversarial case; PocketIC Schnorr seams as capability only | L |
| BB-50 | Solana Wallet Standard connector, address validator and return flow | `src/vault_frontend/` | BB-49, BB-59 | the frontend's focused vitest | L |

### 3.4 XRPL follow-ups to PR #586 (rank 6; local work may run on a stacked or rebased #586 branch, no merge required for it)

Source: `docs/coordination/multichain/chain-xrpl.md`. No edit to #586 itself.

| Card | Scope | Files (on #586's module) | Depends on | Focused command | Size |
| --- | --- | --- | --- | --- | --- |
| BB-55 | Wire the 15-digit validator into import and return requests; clamp both caps at 999,999,999,999,999 e8s; count pending imports against the total cap; snapshot ceiling; frontend parser parity | `xrpl_icusd/mod.rs`, `state.rs`, `chains/xrp/issued_amount.rs`, the XRPL frontend service | PR #586 merged, or a stacked or rebased branch on its head for local work | the backend's focused `xrpl_icusd` unit tests | S |
| BB-56 | No-vault return: `prepare_xrpl_return` intent (destination account, payer, amount, never-reused tag), observer attribution on validated `tesSUCCESS` payments using `delivered_amount`, escrow-to-owner payout; never touches a vault or debt; origin split for outstanding supply | `xrpl_icusd/repay.rs`, `observer.rs`, `issuance.rs`, `state.rs` | BB-55 | the backend's focused `xrpl_icusd` tests | M |
| BB-57 | Candid and regenerated declarations, frontend return card and opt-in card, PocketIC end-to-end with negatives, keeping the backend Wasm under PocketIC 7's 10 MiB cap (9,648,104 bytes on #586's head); also the XRPL lane reader (`E_X` and issued outstanding) for the BB-58 collector (moved from BB-58) | backend `.did`, declarations, frontend | BB-56 | the XRPL PocketIC suite only, serially | M |

### 3.5 Sui (rank 7)

Source: `docs/coordination/multichain/chain-sui.md` (sections 2 to 4 and 8).

| Card | Scope | Files | Depends on | Focused command | Size |
| --- | --- | --- | --- | --- | --- |
| BB-51 | Move package `rumi_icusd_gate` (init with supply cap 0, mint with consumed mint ids, recipient-bound `bridge_out`, limits, version guard) and Move unit tests; an external Move review is deferred to pre-real-value work, not a groundwork or demo gate | a new Move package directory (path proposed in BB-51) | none (after BB-58 by scheduling preference only) | `sui move test` (Sui CLI installed only when implementation is authorized) | M |
| BB-52 | Sui instance skeleton by copy; roles, paths and mint-id domain; BCS transaction builder with both digests; signer over the Ed25519 pattern; GraphQL mirror with per-provider quorum | a new Sui bridge canister | BB-51 | the new crate's golden-vector and transform tests | L |
| BB-53 | Publish and bind checks; mint lane (persist-before-sign; epoch expiry plus a missing digest never authorizes replacement or refund; delivery reconciled from the consumed mint-id record); return observer; audit query; also the Sui supply reader for the BB-58 collector (moved from BB-58) | the new canister | BB-52 (D6, BB-61, only for live authority steps; local lane and observer source does not wait) | the new crate's lane tests | L |
| BB-54 | Sui Wallet Standard connector and return flow | `src/vault_frontend/` | BB-53, BB-59 | the frontend's focused vitest | L |

### 3.6 Authority decision (rank 8)

| Card | Scope | Owner | Depends on |
| --- | --- | --- | --- |
| BB-61 | D6: controllers, production custody (EVM admin, Solana mint authority, Sui UpgradeCap, XRPL issuer policy) and authority activation per network | Rob, with Miles | none; gates live activation, not source work |

## 4. Activation prerequisites (stay disabled until all hold)

Source support and activation are separate. BB-43 adds the non-Base EVM rows with the install gate closed as an interim
state; BB-45 opens it at source level for the supported rows (above). The catalog `disabled` status, real installs and
activation below are unaffected by that.

Each profile in `src/rumi_bridge_destinations/profiles/` lists its blockers; the validator rejects any enabled status. The
main ones: HyperEVM historical state; Monad Verified head; Robinhood `finalized` tag and keyless providers; Solana guard,
return custody, unattributed-burn policy, history availability and SOL RPC id re-verification; Sui gate module, RPC provider
contract and epoch-expiry non-execution proof; XRPL PR #586 merge, return intent and cap wiring; D6 for every network. A
separate authorization is needed for each real-network probe, deployment, signing step, funding and activation.

## 5. Groundwork verification

- `cargo test --offline -p rumi_bridge_destinations` and the dry run, by the one build lane (results in the handoff).
- `python3 docs/coordination/base-icusd-bridge-lookout-derive.py`, the offline priority validator and the Lookout changed-only
  publish with a no-op readback.
- No bridge, backend, Foundry or frontend runtime file changed since `c82f2880` (only the new library crate and its workspace
  line). The reviewed Base runtime, Candid and declarations are untouched.

## 6. Proposed durable lessons (proposal only; no persona, catalog or memory write)

1. **Finality-rule enums need value checks.** A profile that names the right finality rule but carries `latest` or `processed`
   as its parameter would pass an enum-only check. Pointer: `src/rumi_bridge_destinations/src/validate.rs` (the V5 family and
   finality checks, with the `v5_evm_tag_latest_rejected` and `v5_solana_commitment_processed_rejected` tests). Candidate home:
   the `rust-canister-engineering` review checklist (boundedness and validation) or the chain-integration personas.
2. **Shared JSON amount bounds use canonical decimal strings.** `u64::MAX` as a bare JSON number is exact in Rust but rounds
   in JavaScript `JSON.parse`. Pointer: `src/rumi_bridge_destinations/src/types.rs` (amount bounds as strings) and the
   `amount_u64_max_round_trips_exactly` test. Candidate home: `rust-canister-engineering` (Candid and generated artifacts) or
   the frontend release personas.
3. **A head-rule dispatcher must cover every raw finality-tag state proof (Miles, 2026-10-30).** When settlement finality
   becomes a per-chain rule, audit every state read that proves something final, not only the obvious head callers: refund
   checks and nonce recovery read the raw `finalized` tag too. BB-43's first pass routed the observer and lane head but missed
   `request_refund`'s mint-consumed check and the two final-nonce proofs (`fetch_nonces`, `clear_lane_halt_with`), fixed in
   `685984b9`. Pointer: a narrow search for `BlockTag::Finalized|finalized_head`, and the request-body tests in
   `flows/deposit.rs` and `evm/lane.rs`. Candidate home: the `rust-canister-engineering` review checklist (caller and
   settlement boundaries) or the chain-integration personas.
