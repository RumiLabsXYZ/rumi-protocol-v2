# Native XLM collateral implementation plan

**Status:** implementation plan only; no XLM collateral implementation or deployment is proved by this document. **Target:** native XLM held on Stellar, collateralizing icUSD debt on the Internet Computer. Rob's request authorizes implementation and deployment; this document records the evidence and prerequisites needed to carry that instruction out safely. Signing, broadcasting, and moving funds are distinct actions and require their own explicit authorization.

## Source baseline and scope

The existing native-XRP rail is the closest in-repository analogue: it derives chain-specific Ed25519 custody keys, queries a foreign chain by HTTPS outcalls, verifies deposits, and settles native-asset withdrawals. The XRP custody and vault patterns are in `src/rumi_protocol_backend/src/chains/xrp/`, `src/rumi_protocol_backend/src/state.rs` (`xrp_collateral_config`, `CustodyKind`, `XrpPendingDeposit`, `XrpClaim`) and `src/rumi_protocol_backend/src/vault.rs` (`open_xrp_vault`, `confirm_xrp_deposit`, claim settlement). XRP collateral is not an ICRC asset; the XLM implementation should keep native XLM on Stellar and mint icUSD only on the IC, following the existing foreign-collateral model.

The XRP design spec is useful for architecture, but its numeric debt cap is stale relative to current code. The current `xrp_collateral_config` sets **250,000,000,000 e8s ($2,500)** on fresh registration; the old spec's $200 figure is not the current parity target. The existing launch guardrail preserves an already registered debt ceiling rather than clamping it (`state.rs` around `enforce_xrp_launch_guardrails`). Keep that distinction explicit in XLM registration, migration, and activation design.

Current XRP also has more lifecycle behavior than the older 2026-06 design spec describes, including manual claims, settlement reconciliation, and a dedicated Stability Pool absorb flow. Before implementation, inventory the current `vault.rs`, `state.rs`, `lib.rs`, `stability_pool` source, and public Candid/UI paths at the chosen base commit. Match supported user-visible paths or document a deliberately narrower launch scope with a hard activation gate; do not infer full parity from the old spec.

## XRP risk parameters to reuse

These are the source-backed values in `src/rumi_protocol_backend/src/state.rs` near `xrp_collateral_config` (around lines 1835–1887). The XLM config should use these same values unless a reviewed risk decision changes them:

| Parameter | XRP source value | XLM implementation treatment |
| --- | --- | --- |
| Borrow threshold | 1.50 (150%) | Reuse exactly |
| Liquidation ratio | 1.33 (133%) | Reuse exactly |
| Recovery target | 1.50 × configured recovery multiplier; source documents 1.0333 ≈ 1.55 (155%) | Reuse multiplier and verify the effective target in tests |
| Liquidation bonus | 1.12 multiplier (12% collateral penalty) | Reuse exactly; test the multiplier interpretation |
| Debt ceiling | 250,000,000,000 e8s ($2,500) for a fresh registration | Reuse as the initial source default; live ceiling is a runtime setting and must be read/approved at activation |
| Minimum vault debt | 10,000,000 e8s (0.1 icUSD; matches ICP) | Reuse exactly |
| Borrowing fee and base APR | Copied from ICP config at collateral registration | Reuse the registration-time copy behavior; inspect current ICP config at implementation |
| Dynamic interest curve | `rate_curve: None`, inherits global curve | Reuse exactly unless the current ICP/XRP config has changed |
| Borrowing-fee curve | Uses global `borrowing_fee_curve` behavior | Reuse current source semantics |
| Ledger fee | 0 in the collateral accounting config | Do not use as Stellar transaction fee; charge/allocate actual XLM network fees in settlement accounting |
| Decimals | XRP uses 6 | XLM uses 7 stroops-per-lumen decimal places; checked conversions throughout |
| Minimum collateral deposit | XRP config currently 1,000,000 drops (1 XRP) net of reserve | Establish a Stellar-specific minimum after account activation/base-reserve and fee model are verified; do not copy 1 XRP numerically |
| Oracle | XRC XRP/USD; no explicit `min_xrc_sources` in this constructor | Confirm XRC supports the exact XLM/USD identifier and source floor; test stale/unavailable/fallback behavior before registration |

The older XRP design spec records the shared 150% / 133% / 155% / 12% risk shape and a $200 ceiling. For the cap, current code supersedes that historical note. The XRP config constructor plus a fresh registration test is the correct baseline, not a hand-copied value from the 2026-06 spec.

## Stellar-specific design requirements

These are design constraints to validate against current Stellar protocol/network documentation and testnet before implementation; they are not claims that an XLM rail already exists here.

1. **Network identity and key domain.** Stellar classic accounts use Ed25519 public keys and signatures. Reuse the IC threshold-Ed25519 management-canister capability only after proving signature compatibility with Stellar's transaction-envelope signing payload. Give Stellar a unique, stable derivation-chain domain (Stellar SLIP-0044 coin type is 148, but verify against current repo chain-ID conventions); it must never alias XRP, Solana, or another key path. Bind signing to the Stellar network passphrase/network ID and transaction hash, so testnet envelopes cannot be replayed on mainnet.
2. **Canonical transaction construction.** Implement and test Stellar XDR envelope encoding and signature payload hashing. The signed envelope must be the exact transaction subsequently submitted. Include bounded time bounds, explicit source account, sequence number, fee, and destination. Reject malformed StrKey addresses and unsupported address forms. Use official Stellar SDK output as an independent golden-vector source, like `tools/xrp-kat` does for XRP.
3. **Per-vault custody.** Prefer one threshold-derived Stellar account per vault for direct attribution, as XRP does, unless a reviewed design proves a pooled-account alternative. Stellar accounts must be funded/activated and keep the current network-required minimum balance. Query current reserve and fee settings at deposit confirmation and settlement; do not hardcode XRP's approximate one-token reserve or fee. Credit only spendable XLM above the required account reserve and outstanding claims/fees. Define who pays account activation and whether reserve remains locked when closing the vault.
4. **Sequence and ambiguous submit handling.** Stellar uses account sequence numbers. Serialize signing/submission per custody account, persist the exact envelope hash and sequence before submit, and reconcile transaction status and account sequence before retrying after timeout/reply loss. A network timeout must not authorize a second payment. Define bounded transaction timeouts and terminal handling for expired, failed, or indeterminate transactions.
5. **RPC/provider trust.** Specify the read/submission interfaces (Horizon, Stellar RPC, or both) and their current supported transaction-history/finality semantics. Define consensus transforms that strip volatile response data while preserving all consumed fields and normalize response status, following the XRP outcall discipline. Use independent provider agreement or explicitly record the accepted single-provider trust boundary for a capped pilot. No deposit is credited from an unvalidated or ambiguous transaction.
6. **Native asset and decimals.** Support XLM only at launch (not issued Stellar assets/SACs). Convert stroops (1e-7 XLM) and icUSD e8s with checked integer math; reject overflow, zero-credit, below-minimum, and reserve-insufficient amounts. Account reserves and fees are not collateral available to seize or pay out.
7. **Oracle.** Confirm current XRC support for XLM/USD, quote units, source count, update cadence, staleness and fallback behavior. Keep the existing fail-closed price freshness rule. If XRC cannot provide the required source quorum, keep XLM frozen; introducing a new price feed is a separately reviewed scope decision.
8. **Claims and collateral exits.** Stellar payouts should use a durable claim ledger like `XrpClaim`, not assume an IC ledger transfer. Preserve claimant identity separately from custody owner/vault; account for network fees and reserve constraints; provide status, retry, quarantine, and operator reconciliation. No generic ICRC path may accept NativeXlm.
9. **Liquidation and Stability Pool.** Ensure every enabled liquidation path has a complete XLM payout route. Manual stablecoin liquidation may create XLM claims and send stablecoin proceeds to reserves. If Stability Pool XLM absorption is enabled, require a persisted preflight/burn proof, exact seizure/payout conservation, depositor opt-in/address binding, replay protection, and one claim per recipient. Otherwise hard-reject XLM from SP before burning and do not enable collateral until an alternative liquidation route is complete. Never let a bot or generic EVM path claim custody capability it lacks.
10. **Registration guardrail.** Add XLM-specific custody kind and a launch guard analogous to XRP: enforce a production threshold-key name and required key configuration before Active status; force Frozen/Disabled on an unsafe key after upgrade; preserve live collateral settings and ceiling unless an explicit migration is approved. Register only after deposits, all exits, liquidation, claim reconciliation, frontend, and monitoring are complete.

Stellar's official docs describe account/transaction result distinctions including invalid sequence, wrong network/signature, and insufficient balance/reserve, and expose live network fee/resource settings. These are operational inputs, not constants to copy from XRP. See [Stellar transaction result codes](https://developers.stellar.org/docs/data/apis/horizon/api-reference/errors/result-codes/transactions), [Stellar resource limits and fees](https://developers.stellar.org/docs/networks/resource-limits-fees), and [Stellar accounts API](https://developers.stellar.org/docs/data/apis/horizon/api-reference/resources/accounts).

## Work packages

### WP0 — Freeze interface, threat model, and exact parity target

- At the implementation base, inventory the present XRP implementation and downstream callers, Candid, frontend, SP, timer, and deployment paths. Record exact files and current behavior; resolve whether mainnet scope includes SP absorption or intentionally keeps XLM Frozen until manual liquidation alone is proven sufficient.
- Document custody state machine: reserved/open, funded-unconfirmed, collateral credited, claim created, payment submitted, confirmed/settled, failed/expired, quarantined/reconciled.
- Specify deposit proof/finality, payment deduplication, timeouts, reserve/fee treatment, per-account sequence serialization, claimant binding, and ambiguous-response recovery.
- Freeze oracle source and network/provider trust assumptions. Determine whether current XRC can price XLM at required freshness/quorum; do not silently substitute a new feed.
- Security review the design and use exact current source references before touching stable state or Candid.

### WP1 — Stellar rail and independent vectors

- Add `src/rumi_protocol_backend/src/chains/stellar/` (or the repo's agreed chain naming) for StrKey public address handling, unique threshold Ed25519 derivation, XDR transaction construction, signature payload, submission, transaction lookup/finality, account sequence/balance/reserve queries, and transforms.
- Use bounded requests and strict parser validation. Persist/return a deterministic transaction hash. Implement provider transforms over only consumed fields and normalize status for IC consensus.
- Add checked conversion helpers for stroops, fees, reserve, and collateral quantity. Treat XLM as the native asset; no trustline is required for XLM itself.
- Produce a committed known-answer vector from the official Stellar SDK for address derivation, transaction envelope bytes, hash, and signature verification. Include wrong-network, malformed-XDR, invalid-address, and sequence/timeout cases.

### WP2 — Additive state and launch guard

- Extend `CustodyKind` with `NativeXlm` using serde-safe/defaulted evolution. Add a collision-resistant synthetic collateral principal; never treat it as a real ledger canister id.
- Add XLM pending-deposit and claim/settlement state with safe defaults for old snapshots, bounded retention/capacity, monotonic IDs, and version-compatible serialization. Add snapshot migration/round-trip tests before any rollout.
- Add XLM configuration constructor using the exact reused risk parameters above, Stellar decimals, explicit Stellar oracle config, native custody, and XLM-specific minimum deposit calculation policy.
- Add a production-key launch guard and validate config changes. Unsafe key/network configuration must keep the collateral frozen. Do not automatically raise an already-live debt ceiling on upgrades.

### WP3 — Deposit and borrow path

- Implement open-then-verify: authorize caller, reserve vault id, derive per-vault account, persist bounded pending row before returning address, and issue no collateral/debt/icUSD at open.
- Confirm deposit from a unique validated Stellar transaction/payment to the exact expected account; verify network, destination, asset = native XLM, successful operation, amount, and transaction identity. Re-fetch account minimum reserve and compute creditable spendable amount using checked math; prevent replay across vaults/users.
- Only after atomic state validation create the ordinary vault and credit XLM; borrow through existing `borrow_from_vault`/mint accounting.
- Preserve idempotent confirmation, caller guard, pending expiration/prune, and upgrade recovery. Any provider disagreement or ambiguous proof yields no credit.

### WP4 — Withdrawals, close, liquidation, and claims

- Route add/deposit ICRC endpoints away from NativeXlm before ledger calls. Add custody-aware guards to withdrawal, partial withdrawal, close, and redemption routes.
- Create durable XLM claims for every supported collateral-out event. Bind claimant, custody account owner/nonce, XLM amount, reserved network fee, transaction hash/sequence, and lifecycle state.
- Implement claim settlement with account-level sequencing, reserve/fee checks, pre-submit persistence, exact-envelope threshold signing, one submit attempt, and confirm-before-retry reconciliation. Quarantine conflicting sequence/balance/transaction evidence; provide an authorized resolution path with auditable evidence.
- Cover manual liquidations and all callers of generic liquidator/bot endpoints. If an endpoint cannot return or surface the created claim and retry handle, keep it unavailable to XLM.
- Decide and implement Stability Pool treatment. Either implement the complete opt-in + burn-proof + preflight + claim conservation path, or reject native XLM absorption before burn and retain a hard activation restriction if that leaves liquidation coverage incomplete.
- Prove conservation: aggregate claims plus paid fees/reserve adjustments equal collateral removed; no duplicate payment after retry/restart.

### WP5 — Frontend, Candid, and operator observability

- Add user flows for open address, deposit confirmation, available collateral, withdraw/close, external Stellar payout address, claim status, retry, and unresolved/ambiguous settlement. Never show “received” until chain confirmation.
- Add optional Stellar memo only if the chosen account model needs it; per-vault direct accounts should not require memo-based attribution. Validate public keys/addresses client-side and again server-side.
- Add XLM claim/pending status to account and operator views, with bounded pagination and explicit incomplete-history metadata.
- Update Rust public types and regenerate `.did`, generated frontend bindings, and client/service types from canonical generators. Do not hand-edit generated Candid declarations.
- Add alarms/metrics for stale prices, deposit backlog, unresolved claims, provider disagreement, reserve shortfall, and sequence conflicts. Keep metrics free of secrets.

### WP6 — Testnet and release evidence

- Unit tests: vectors, conversions, reserves/fees, transaction signing/hash, transforms/parser consensus, deposit replay, stale/invalid network, reserve-deduction boundaries, and sequence serialization.
- State tests: old snapshot decode, new state round trip, safe defaults, migration behavior, claim/intent invariants, and upgrade/restart reconciliation.
- Backend integration/PocketIC tests with deterministic mock Stellar RPC: successful deposit and borrow, wrong account/network, replay, provider disagreement, submit-reply-lost, transaction found/not-found, sequence advanced, fee charged on failure, reserve depletion, and retry after restart.
- E2E on Stellar testnet with low-value isolated accounts: deposit → verify → mint → repay/withdraw or liquidate → claim → signed payment → final transaction confirmation. Keep testnet proof separate from mainnet readiness.
- Run the repository's canonical formatting/build/test/CI gates for changed crates and affected frontend. Regenerate Candid and check interface parity. Run adversarial verification and independent security review due to custody/signing, off-chain collateral, stable-state, and money-accounting stakes.
- Pin source revision, toolchain, test logs, generated artifact hashes, optimized Wasm size/hash, and exact CI head. Resolve all findings before activation proposal.

### WP7 — Mainnet deploy and activation

Mainnet is the requested target after WP0–WP6. Do not treat code merge, CI, deployment, key availability, collateral registration, or successful testnet traffic as equivalent evidence.

1. Pre-deploy packet: reviewed source revision; green exact-head required CI; independent audit disposition; source-matched optimized Wasm and size/hash; regenerated Candid; backup/recovery and rollback plan; monitoring/incident owner; current Stellar reserve/fee/provider/oracle settings; controller/key/network identity checks.
2. Apply Rob's deployment authorization to the verified Rumi backend canister and exact reviewed module. Verify installed module hash and exposed API against the pinned build; record pre/post canister version and controllers.
3. Register XLM in Frozen/Disabled state first. Read back the full config and verify the exact 150%/133%/155%/12% risk values, $2,500 initial ceiling, fee/interest curves, oracle source/quorum/freshness, minimum debt, decimals, custody kind, and min-deposit/reserve logic. Check XLM is excluded from unsupported ICRC, bot, redemption, and SP routes.
4. Prove production threshold key derivation and verification without moving user funds; verify unique XLM derivation domain and network binding. Confirm operator recovery and claim reconciliation paths.
5. Once the preconditions are met, enable the configured collateral under Rob's request to enable native XLM. Record the exact runtime config before and after activation. A live pilot deposit and payout involve funds movement and need separate authorization; until then, verify admission and accounting with deterministic and testnet evidence.
6. Increase the cap or user access only through a later reviewed decision. Pause/freeze on oracle stale/quorum loss, provider disagreement, unresolved submit ambiguity, reserve shortfall, sequence conflict, invariant mismatch, or monitoring failure.

## Acceptance gates and evidence boundaries

- **Implemented:** source plus deterministic tests; says nothing about deployed code.
- **Release ready:** exact-head CI, generated interface parity, optimized artifact identity/size, security disposition, and testnet lifecycle proof.
- **Deployed:** canister target/version and module hash match the approved artifact; still does not prove XLM registration or collateral admission.
- **Configured:** readback proves collateral parameters, key/network/oracle, limits, and status; Frozen/Disabled is not Active.
- **Activated:** Rob's request plus runtime readback shows intended status/cap; does not prove a deposit, signature, broadcast, finality, or settlement.
- **Deposit/settlement:** independently reconcile Stellar transaction/finality, account sequence/balance/reserve, IC vault collateral/debt, claim lifecycle, and icUSD supply. Testnet success is not mainnet transaction evidence.

## Open questions to close in WP0

1. Does current XRC support XLM/USD with adequate source quorum and freshness? If not, keep XLM Frozen pending a separately reviewed oracle decision.
2. Which Stellar public API is canonical for queries and submission (Horizon vs Stellar RPC), and what exact validation/finality proof does it expose for classic XLM payments?
3. Which account model is selected (per-vault account vs pooled account), who funds initial activation/minimum reserve, and how is locked reserve treated at vault close?
4. Is Stability Pool XLM absorption part of the first mainnet activation scope? If not, which fully supported liquidation path justifies enabling XLM and how does the launch guard enforce the limitation?
5. What production key name/network deployment identity is approved, and how will production signing be verified without creating an unapproved transaction?
6. Which human/operator will own incident response? The source-backed $2,500 debt ceiling is the requested XRP parity default; verify the live XRP ceiling separately before activation.

## Repository references

- XRP design history (use architectural decisions, not stale $200 cap): `docs/superpowers/specs/2026-06-16-native-xrp-collateral-design.md`, especially lines 34–70, 91–123, 127–153.
- Current XRP risk config and launch guard: `src/rumi_protocol_backend/src/state.rs`, approximately lines 1835–1967.
- XRP pending-deposit and custody data: `src/rumi_protocol_backend/src/state.rs`, approximately lines 1804–1832 and 1970–2044.
- XRP open/confirm deposit path: `src/rumi_protocol_backend/src/vault.rs`, approximately lines 3439–3708.
- XRP signing and chain rail: `src/rumi_protocol_backend/src/chains/xrp/ted25519.rs`, `address.rs`, `codec.rs`, `sign.rs`, `xrp_rpc.rs`, and `adapter.rs`.
- Current SP native-XRP contract and payout requirements: `docs/superpowers/specs/2026-06-27-xrp-liquidation-payout-and-sp-opt-in.md` and `docs/superpowers/plans/2026-06-27-xrp-liquidation-payout-and-sp-opt-in.md`; verify current source because those plans may also be stale.
- Stellar network-dependent references: [transaction result codes](https://developers.stellar.org/docs/data/apis/horizon/api-reference/errors/result-codes/transactions), [resource limits and fees](https://developers.stellar.org/docs/networks/resource-limits-fees), and [accounts API](https://developers.stellar.org/docs/data/apis/horizon/api-reference/resources/accounts).
