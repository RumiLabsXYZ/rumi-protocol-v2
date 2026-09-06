# Native SOL Collateral — Design

Status: spec
Date: 2026-07-24
Branch: `feat/native-sol-collateral`

Add native SOL (on Solana mainnet-beta) as a CDP collateral type, mirroring the
native-XRP rail. icUSD stays IC-native. SOL is custodied at per-vault threshold
Ed25519 addresses. Collateral leaves the protocol only through the claim model.
Stability Pool depositors opt in to SOL liquidations by supplying a Solana
payout address.

## 0. Why this is mostly integration work, not crypto work

The Solana cryptographic primitives already exist in this repo and are unit
tested: `chains/solana/{ted25519,sol_rpc,tx}.rs` (~1,400 lines + ~1,100 lines of
tests) built on the real `solana-message` / `solana-transaction` /
`solana-instruction` / `solana-pubkey` crates, plus `bs58`, `base64` and
`ed25519-dalek`, all already in `Cargo.toml` and already resolving on
`wasm32-unknown-unknown`.

What those modules currently serve is a *different product*: the dormant
`chains::solana` rail, which mints icUSD **as an SPL token on Solana** via
`ChainAdapter` / `MultiChainState` / `SettlementQueue`. That model is not what we
want and its SPL-mint machinery (`mint_to_ix`, `create_ata_idempotent_ix`,
`build_mint_message*`, the supply gate in `deposit_watch.rs`) is explicitly out
of scope here.

What we want is the `chains::xrp` model: a `CollateralConfig` row with
`custody_kind = NativeSol` keyed under a synthetic principal, per-vault custody
addresses, open-then-verify deposits, and claim-based payouts, all plugged into
the ordinary IC-native `Vault` machinery.

So the work is: **a new `chains::sol` module for the CDP-specific pieces, reusing
the existing pure Solana primitives, plus the same integration surface XRP has in
`state.rs` / `vault.rs` / `main.rs` / `stability_pool` / frontend.**

## 1. Module layout

New module `src/rumi_protocol_backend/src/chains/sol/`, a structural sibling of
`chains/xrp/`:

| File | Responsibility |
|---|---|
| `mod.rs` | Module doc + re-exports |
| `config.rs` | `SOL_CHAIN_ID`, decimals, prod/test Schnorr key names, cluster coupling |
| `address.rs` | base58 decode + **on-curve** validation, `is_valid_sol_address` |
| `ted25519.rs` | `sol_custody_derivation_path`, `sol_settlement_derivation_path`, `sol_nonce_derivation_path`, derive + sign using the state-driven key name |
| `rpc.rs` | Cluster-aware SOL RPC: `get_balance`, `get_rent_exempt_minimum`, `get_durable_nonce`, `send_transaction`, `get_transaction`, `get_slot` |
| `adapter.rs` | `build_withdrawal_transfer` (solvency/fee/reserve validation) + `sign_sol_payment_from` |
| `tests_*.rs` | Unit tests per module, KAT-locked (§10) |

Reused as-is from `chains::solana::tx` (pure, cluster- and key-agnostic):
`serialize_legacy_message`, `assemble_wire_tx_multi`, `order_signatures_by_signer`,
`system_transfer_instruction`, `advance_nonce_instruction`,
`build_transfer_message_with_nonce`, `build_create_nonce_account_message`,
`first_signature_base58`, `NONCE_ACCOUNT_RENT_LAMPORTS`, `encode_compact_u16`.

Not used: everything SPL/ATA/mint, `chains::solana::settlement`,
`chains::solana::deposit_watch`, `chains::solana::adapter`.

### 1.1 Derivation-path collision (security)

`chains::solana::ted25519::custody_derivation_path` already produces
`[501u32_le, user_bytes, vault_id_le]` for the dormant chain-vault rail, whose
`vault_id` comes from a **different counter** (`multi_chain`) than the CDP vault
counter. Reusing that path shape would let two unrelated vaults derive the same
custody address.

`chains::sol` therefore inserts an explicit role label, the same technique XRP
uses for its settlement path:

```
sol_custody_derivation_path(user, vault_id)
  = [SOL_CHAIN_ID_le, b"collateral", user.as_slice(), vault_id_le]   // 4 elements
```

Four elements can never equal the dormant rail's three, regardless of chain id.
`SOL_CHAIN_ID` stays `ChainId(501)` (SLIP-44 correct). A unit test asserts
non-collision against `chains::solana::ted25519::custody_derivation_path` for
identical `(user, vault_id)`.

## 2. Key management and cluster coupling

XRP's most important safety property is that the Schnorr key name is **runtime
State**, and RPC network selection is **derived from it**, so custody derivation
and the network being read can never disagree.

`chains::solana` today has neither: `config::solana_schnorr_key_name()` is a
hardcoded `"test_key_1"` and `sol_rpc.rs` hardcodes `SolanaCluster::Devnet`
inside its private `json_request`. That is fine for a dormant devnet rail but
unusable for custody of real collateral.

We add, mirroring XRP exactly:

- `State.sol_schnorr_key_name: String`, `#[serde(default = "default_sol_schnorr_key_name")]` → `"test_key_1"`.
- `SOL_PRODUCTION_SCHNORR_KEY_NAME = "key_1"`, `SOL_TEST_SCHNORR_KEY_NAME = "test_key_1"`.
- `is_sol_production_key_name(name)` drives **both** `key_id()` and cluster
  selection: production key ⇒ `SolanaCluster::Mainnet`, otherwise
  `SolanaCluster::Devnet`. One function, one decision, no way to desync.
- `set_sol_schnorr_key_name` (developer-only) **refuses to change once any SOL
  state exists** (pending deposits, claims, or a registered SOL collateral) —
  same guard as `validate_xrp_schnorr_key_change`.
- `enforce_sol_launch_guardrails(state)` runs on every `post_upgrade`: if the key
  name is not the production key and the SOL collateral is not already
  `Frozen`/`Deprecated`, force `Frozen`. Fail-closed, idempotent.

The existing dormant `chains::solana` rail is left on its hardcoded devnet
constants and is **not** migrated to the new state field. Touching it would
change parked behavior for no benefit here, and its own key/cluster gap is
recorded as pre-existing (§12).

## 3. Collateral registration and parameters

Keyed under a synthetic principal, exactly like XRP:

```rust
pub fn sol_collateral_principal() -> Principal {
    Principal::from_slice(b"rumi-sol-native")  // 15 bytes, cannot collide with a real canister id
}
```

Parameters — per instruction, identical to ckETH's live mainnet values (which are
in turn identical to XRP's, confirmed by direct `get_collateral_config` query on
`tfesu-vyaaa-aaaap-qrd7a-cai`):

| Field | Value | Rationale |
|---|---|---|
| `liquidation_ratio` | 1.20 | = ckETH = XRP |
| `borrow_threshold_ratio` | 1.35 | = ckETH = XRP |
| `liquidation_bonus` | 1.075 | = ckETH = XRP |
| `borrowing_fee` | 0.002 | = ckETH = XRP |
| `interest_rate_apr` | 0.015 | = ckETH = XRP |
| `redemption_fee_floor` / `ceiling` | 0.005 / 0.05 | = ckETH = XRP |
| `redemption_tier` | 3 | = ckETH = XRP; volatile non-ICP asset |
| `recovery_target_cr` | `1.35 × recovery_cr_multiplier` | computed, not stored |
| `decimals` | **9** | lamports |
| `min_collateral_deposit` | 20_000_000 lamports (0.02 SOL) | ≈ ckETH's 0.001 ETH in USD terms |
| `min_vault_debt` | 0.1 icUSD | = XRP |
| `ledger_fee` | 0 | Solana fees are paid by the rail at settle time, not here |
| `debt_ceiling` | 250_000_000_000 (2,500 icUSD) | **launch value**, matching XRP's launch default rather than ckETH's 10,000 — raisable live via `set_collateral_debt_ceiling`. A brand-new custody rail earns its ceiling. |
| `price_source` | `Xrc { base_asset: "SOL", quote_asset: "USD" }` | same generic XRC path as every other collateral |
| `min_xrc_sources` | `None` → global floor 3 | SOL has deep coverage; no ckXAUT-style override needed |
| `custody_kind` | `Some(NativeSol)` | |
| `symbol` | `Some("SOL")` | no ledger to query `icrc1_symbol` from |
| `rate_curve` | `None` → global default | = XRP |

`register_sol_collateral()` is developer-gated, refuses unless the production
Schnorr key is configured, refuses double-registration, snapshots ICP's live
`borrowing_fee`/`interest_rate_apr`... **no** — unlike XRP it hardcodes 0.002 /
0.015 to match ckETH. XRP's "copy ICP's live values" produced silent drift the
moment ICP's fee changed; we do not repeat that.

Registration must land **after** the claim-based payout paths exist, or a SOL
vault could borrow icUSD and not be liquidatable. Same ordering invariant XRP
documented.

## 4. Deposit flow (open-then-verify)

No timers, no polling, no signature scanning. Two user-driven calls, mirroring
XRP.

**`open_sol_vault() -> SolVaultOpenInfo`**
1. `GuardPrincipal` reentrancy guard.
2. Require production Schnorr key; require SOL collateral registered and open-accepting.
3. Bounds: max 10 pending opens per caller, max 10,000 global.
4. Fetch `rent_exempt_minimum` live (§4.1).
5. Reserve `vault_id` from the shared vault counter (cannot collide with ICP vaults).
6. Derive custody address from `sol_custody_derivation_path(caller, vault_id)`.
7. Insert `SolPendingDeposit { owner, custody_address, derivation_nonce: vault_id, opened_at_ns, rent_exempt_lamports }`.
8. Return `{ vault_id, custody_address, rent_exempt_lamports }`.

**User sends SOL to `custody_address`** — plain transfer, no memo. A transfer to a
non-existent address creates the account automatically, so no `CreateAccount` is
needed.

**`confirm_sol_deposit(vault_id) -> u64`** (owner-only)
1. `sol_rpc::get_balance(custody_address)` at commitment **`finalized`** (not
   `confirmed`) — this is custody, we accept the extra latency for
   non-reversibility.
2. Re-fetch `rent_exempt_minimum` live.
3. Credit `balance_lamports - rent_exempt_lamports`, floored at
   `min_collateral_deposit`. Same balance-based crediting as XRP
   (`sol_credit_amount`), same tradeoff: we credit the account's net balance, not
   a specific transaction's amount.
4. Atomically re-check the pending entry still exists, remove it, `record_open_vault`.

Borrowing icUSD afterwards is an ordinary `borrow_from_vault` call. There is no
SOL-specific mint path.

`cancel_sol_pending_open` (owner) / `sweep_sol_pending_open` (developer) clean up
abandoned unfunded opens, both re-verifying the account is still unfunded and
both gated on a 10-minute minimum age so an in-flight deposit cannot be cancelled
out from under the user.

### 4.1 The reserve: rent-exempt minimum

XRP nets off the XRPL base reserve. The Solana analogue is the rent-exempt
minimum for a 0-byte system account (890,880 lamports ≈ $0.13 at $150/SOL).

We keep the custody account permanently rent-exempt rather than sweeping it to
zero, so the address stays alive and observable across a vault's whole life. The
value is fetched live via `getMinimumBalanceForRentExemption(0)` rather than
hardcoded: it is a network-wide constant, so it reaches `Equality` consensus
across providers exactly like XRP's `server_state` reserve read.

`getMinimumBalanceForRentExemption` does not exist in `chains::solana::sol_rpc`
today and is added in `chains::sol::rpc`.

Note this is materially cheaper than XRP's stranded 1 XRP (≈$3) per vault.

Consequence, deliberate and mirroring XRP: a fully withdrawn SOL vault stays
**open**, because the rent-exempt reserve remains locked at the custody address.
The alternative (sweeping the account to zero on the final claim so Solana
deallocates it, returning the full balance) would require the settlement path to
permit `amount == balance` as a special case, adding a failure mode to the most
custody-sensitive code in the rail to recover ≈$0.13. Not worth it at launch.
The frontend says so explicitly, as it already does for XRP.

### 4.2 No top-ups

`add_margin` / `add_margin_with_deposit` are rejected for native-SOL vaults, as
they are for XRP. Balance-based crediting would make top-ups implementable, but
it introduces a double-credit surface (credited-vs-observed accounting drift) for
no launch benefit. Additional collateral means a new vault. Mirrors XRP; the
frontend hides the Deposit button accordingly.

## 5. Payout flow (claim model + durable nonce)

Collateral never leaves via an ICRC transfer. Every out-flow — owner withdraw,
close, liquidator reward, protocol fee, SP payout — becomes a `SolClaim`, settled
later by a separate signed transfer.

```rust
pub struct SolClaim {
    pub claimant: Principal,
    pub lamports: u64,
    pub custody_owner: Principal,
    pub custody_nonce: u64,           // reproduces the custody derivation path
    pub created_at_ns: u64,
    #[serde(default)] pub settlement: Option<SolSettlement>,
    #[serde(default)] pub quarantine_reason: Option<String>,
}

pub struct SolSettlement {
    pub signature: String,            // base58 tx signature, computed LOCALLY
    pub nonce_value: String,          // base58 durable-nonce blockhash consumed
    pub destination: String,
    pub submitted_at_ns: u64,
}
```

### 5.1 Why durable nonce, not `getLatestBlockhash`

A recent blockhash changes every slot, so independent RPC providers essentially
never agree and `Equality` consensus is chronically `Inconsistent`. Both this
repo's `sol_rpc.rs` doc comment and the musicalchairs playbook record this as a
hard blocker.

We therefore use a **durable nonce account**, as `chains::solana::tx` already
implements. Every payout transaction is:

```
[ advance_nonce_account(nonce_account, authority = settlement_key),
  system_transfer(custody_address -> destination, lamports) ]
```

signed by **two** keys: the settlement key (fee payer, first signer, nonce
authority) and the per-vault custody key.

This gives three properties we want:

1. **Deterministic, non-expiring transactions.** No blockhash race.
2. **Fee payer is the protocol settlement wallet, not the user's custody
   account.** The user's collateral is not shaved by network fees, and the
   custody account needs no fee buffer beyond rent exemption.
3. **A hard, ledger-enforced anti-double-pay primitive.** A durable nonce is
   single-use: once the nonce advances, the signed transaction is permanently
   dead. Recording `nonce_value` with the settlement makes replay analysis exact
   (§5.3) — cleaner than XRP's sequence-number inference.

Cost: all settlements serialize through one nonce account, and the settlement
wallet must hold SOL for fees. Both are acceptable — the serialization mirrors
the single-in-flight-tx property XRP gets from its account Sequence, and
`chains::solana::hardening::hot_wallet_ok` already exists for the funding gate.

The nonce account is created once by a developer-gated
`sol_bootstrap_nonce_account`, funded with `NONCE_ACCOUNT_RENT_LAMPORTS`
(1,447,680). This is a launch prerequisite, checked by
`register_sol_collateral`.

### 5.2 `settle_sol_claim(claim_id, destination)`

Claimant-only. No destination tag — Solana has no analogue for native SOL
(exchanges use unique deposit addresses), so the XRP `_with_tag` variant has no
counterpart.

1. `GuardPrincipal` keyed on the **caller principal** (not on `claim_id`: two
   different claimants settling two different claims both pass this guard
   concurrently. Cross-claimant serialization is a separate mechanism, step
   3a below).
2. Refuse if the claim is quarantined.
3. Validate `destination` as a well-formed, **on-curve** Solana address (§7).
3a. **Acquire the protocol-wide `SolSettlementInflightGuard`** (security review
    fix, 2026-07-24; see below).
4. **Idempotency before signing** (§5.3).
5. Aggregate solvency across all unresolved claims on the same custody address:
   `balance >= Σ(unresolved claims) + rent_exempt_minimum`.
6. Reconcile sibling in-flight claims on the same custody address; refuse if one
   is unconfirmed and its nonce is still current.
7. Read the current durable nonce; build, sign (2 signers), compute the signature
   locally via `first_signature_base58`.
8. **Persist `SolSettlement` before submitting.**
9. `send_transaction`, single attempt, never retried blindly.

#### Protocol-wide settlement serialization (security review fix, 2026-07-24)

`GuardPrincipal` (caller-keyed) and the per-custody-address lock used elsewhere
in this rail do not, between them, serialize two DIFFERENT claimants settling
two DIFFERENT claims. Every `SolClaim` shares ONE durable-nonce account, so a
Stability Pool absorption fanning out into up to `MAX_SOL_SP_PAYOUT_ALLOCATIONS`
claims (§6) previously let all of them read the same live nonce, sign, and
submit concurrently. Solana itself only ever lands one of those transactions
(nonce uniqueness rules out a double-pay), but the LOSING claims' retries could
race each other too: whichever loser's retry runs first finds the winner's
`Confirmed` settlement, uses it to clear its own ambiguity, and (correctly)
removes the winner's now-finalized claim as part of the same reconciliation
pass. Any OTHER loser that retries after that point no longer has that evidence
available and gets quarantined, needing manual `admin_resolve_sol_claim`. A
routine liquidation with many opted-in depositors could therefore strand most
of them behind admin action from a single race window, not because of any
fund-loss risk, but because the canister's own bookkeeping did not serialize
to match the reality already imposed at the Solana layer (one nonce account,
one live transaction at a time).

The fix is a canister-wide, self-healing, transient (heap, not persisted)
re-entrancy guard (`SolSettlementInflightGuard` in `guard.rs`), held across the
whole danger zone: read the live nonce, reconcile siblings, sign, persist the
`SolSettlement`, submit. At most one `settle_sol_claim` call may be inside that
zone at a time, across every claim and every vault. A second concurrent caller
is turned away immediately with a retryable error, before it ever reads the
nonce or signs anything, rather than being allowed to race and later land in
quarantine. It self-heals after `chains::solana::hardening::INFLIGHT_STALE_NS`
(10 minutes) using that module's existing `inflight_should_acquire` predicate,
so a settlement that traps mid-flight (whose `Drop` never runs, since a trap in
a post-`await` continuation does not run destructors on the IC) cannot wedge
the rail forever.

### 5.3 Idempotency and quarantine

On a repeat `settle_sol_claim` where a prior `SolSettlement` exists, read
`get_transaction(prev.signature)` and the live nonce:

| tx status | live nonce vs recorded | action |
|---|---|---|
| `Confirmed` | — | claim paid; remove claim, return prior signature. **No second transfer.** |
| `Failed` | — | charge fee against remaining lamports, clear settlement, allow re-sign |
| `NotFound` | nonce **unchanged** | transaction never landed and cannot have landed (nonce not consumed) → safe to re-sign |
| `NotFound` | nonce **advanced** | ambiguous: the nonce was consumed by something. **Quarantine.** |

The last row is the SOL analogue of XRP's F-03 sequence-divergence quarantine. A
quarantined claim refuses all further signing until a developer resolves it with
`admin_resolve_sol_claim(ConfirmPaid | ReleaseForRetry)` after off-chain
reconciliation against a Solana explorer.

Because the nonce is shared across all claims, an advanced nonce most often means
*another* claim consumed it legitimately. Step 6's sibling reconciliation runs
first precisely so that the common case is resolved without quarantining.

## 6. Stability Pool opt-in

Reuses the existing native-payout machinery rather than adding a parallel one.
The SP already has `native_payout_addresses: Option<BTreeMap<Principal, String>>`
keyed by collateral principal, and `collateral_requires_payout_address()` is the
single gate.

Changes:
- `collateral_requires_payout_address` matches SOL's synthetic principal in
  addition to XRP's.
- `opt_in_native_collateral(collateral_type, payout_address)` works unchanged for
  SOL. The `_with_tag` variant is XRP-only; SOL callers use the tagless form.
- Rename-free: `NativeXrpPendingPayout` is generalized to a
  collateral-tagged `NativePendingPayout` (it already carries `collateral_type`),
  with the XRP-named endpoints kept as-is for wire compatibility and SOL-specific
  `get_my_native_sol_payouts` / `ack_native_sol_payout_settled` added alongside.

Absorption mirrors `stability_pool_liquidate_xrp_vault` exactly: preflight with a
TTL-reserved snapshot, SP computes pro-rata opt-in-only allocations client-side
(sorted by principal bytes, dust to first), backend validates the caller is the
registered SP, matches the preflight, re-verifies collateral/debt have not fallen
below the snapshot, then creates one `SolClaim` per allocation **plus** a
developer-settleable claim for the protocol fee cut (XRP's B-1 fix — without it
the fee is stranded). Replay-keyed on `(proof.ledger_kind, proof.block_index)`
with a request fingerprint.

Depositor SOL amounts are never written into `DepositPosition.collateral_gains`;
that is the ICRC path. SOL uses claims exclusively.

## 7. Address validation

Solana addresses are plain base58 of a 32-byte Ed25519 public key — no version
byte, no checksum. That makes a typo far more likely to decode "successfully"
than an XRPL address, so validation must be stricter, not looser.

Backend `chains::sol::address::is_valid_sol_address` requires:
1. base58 decodes to exactly 32 bytes, **and**
2. the point is **on the Ed25519 curve**.

The on-curve check matters: an off-curve 32-byte value is a PDA (program-derived
address) with no private key. Sending collateral to one destroys it
irrecoverably. `solana-pubkey` is already a dependency with the `curve25519`
feature enabled, so `Pubkey::is_on_curve()` is available at no new cost.

This closes a real gap in both reference implementations: `chains::solana::
ted25519::decode_solana_address` and musicalchairs' `isPlausibleSolanaAddress`
both check length only.

Frontend performs the same length/base58 check synchronously (no curve check —
that needs crypto the browser would have to load, and it would burn the
user-gesture window needed to open the Oisy signer popup, the same reasoning
documented in `XrpVaultPanel.svelte`). The backend curve check is the real trust
boundary. Unlike XRP, the validator is threaded through **all three** entry
points (claim settlement, SP opt-in, manual-liquidation payout) — XRP only wired
it into claim settlement, which is an inconsistency worth not copying.

## 8. State and upgrade safety

Backend `State` is serialized whole as CBOR via ciborium/serde
(`storage.rs::save_state_to_stable`), so `#[serde(default)]` genuinely works for
added fields — unlike the raw-Candid `Encode!`/`Decode!` pattern that caused the
2026-05-18 AMM state wipe. No versioned `SolStateV<N>` wrapper is needed.

New `State` fields, all `#[serde(default)]`:
- `sol_schnorr_key_name: String` → `"test_key_1"`
- `sol_pending_deposits: BTreeMap<u64, SolPendingDeposit>`
- `sol_claims: BTreeMap<u64, SolClaim>`
- `next_sol_claim_id: u64`
- `sol_nonce_account: Option<String>`
- `sp_sol_absorb_preflights` / `sp_sol_absorb_results_by_proof`

`CustodyKind` gains a `NativeSol` variant. Because
`CollateralConfig::is_native_xrp()` hardcodes `== NativeXrp`, every one of the
~19 branch sites is converted from a boolean check to an explicit `match` on
`custody()` so the compiler enforces exhaustiveness when a third custody kind is
added. This is the single highest-value refactor in the change: it turns "did we
remember to handle SOL here?" from a review question into a build error.

Each new field gets the existing old-snapshot decode test pattern: CBOR-encode a
state, strip the new key from the `ciborium::Value::Map`, assert it decodes to
its default.

**Known inherited gap:** `post_upgrade`'s event-replay fallback does not
reconstruct claim state (claims are not `Event`s), same as XRP. `storage.rs`
traps rather than silently falling back on a present-but-corrupt snapshot, which
is what makes this safe. Documented, not fixed here.

## 9. Endpoints

Backend, mirroring the XRP surface one-for-one minus the tag variant:

| Endpoint | Kind | Gate |
|---|---|---|
| `open_sol_vault() -> Result<SolVaultOpenInfo, ProtocolError>` | update | any |
| `confirm_sol_deposit(vault_id) -> Result<u64, ProtocolError>` | update | owner |
| `settle_sol_claim(claim_id, destination) -> Result<String, ProtocolError>` | update | claimant |
| `cancel_sol_pending_open(vault_id)` | update | owner |
| `sweep_sol_pending_open(vault_id)` | update | developer |
| `register_sol_collateral()` | update | developer |
| `sol_bootstrap_nonce_account()` | update | developer |
| `set_sol_schnorr_key_name(name)` / `get_sol_schnorr_key_name()` | update / query | developer / any |
| `sol_settlement_address()`, `sol_custody_address(user, nonce)`, `sol_balance(addr)` | update | developer |
| `get_my_sol_pending_deposits()`, `get_my_sol_claims()` | query | any, self-scoped |
| `get_sol_pending_deposits()`, `get_sol_claims()`, `get_sol_quarantined_claims()` | query | developer |
| `admin_quarantine_sol_claim(id, reason)`, `admin_resolve_sol_claim(id, resolution)` | update | developer |
| `stability_pool_preflight_sol_absorb(...)`, `stability_pool_liquidate_sol_vault(...)`, `stability_pool_sol_claim_outstanding(...)` | update | SP / non-anonymous |

No `transform` queries are needed: SOL RPC goes through the SOL RPC canister as
inter-canister calls, which does its own multi-provider consensus. XRP needs
transforms only because it makes raw HTTPS outcalls.

Native SOL is excluded from automated (bot) liquidation and from redemption
priority at launch, exactly as XRP is. Manual and SP liquidation are supported.

## 10. Testing

- **Unit tests** per `chains::sol` module, mirroring `chains/xrp`'s layout:
  derivation-path distinctness (including the collision test in §1.1), address
  validation incl. an off-curve PDA rejection case, credit-amount boundaries,
  transfer-message byte exactness, settlement solvency/fee guards.
- **KAT fixture** `chains/sol/testdata/sol_kat.json` generated by a new
  `tools/sol-kat/` Node project using real `@solana/web3.js`, covering address
  encoding, legacy message serialization, durable-nonce transfer message bytes,
  and wire-tx assembly. Mirrors `tools/xrp-kat/`. This is what makes the
  hand-assembled wire format trustworthy.
- **PocketIC e2e** `tests/sol_native_e2e_pic.rs` with a mocked SOL RPC canister
  (intercepting by JSON-RPC method name, the way `xrp_native_e2e_pic.rs`
  intercepts rippled): happy path open→fund→confirm→borrow→repay→close→settle;
  liquidation-is-claim-based; and a Candid contract-shape assertion.
- **Idempotency tests** for every row of the §5.3 table, including the quarantine
  case.
- **Frontend vitest** mirroring the nine existing XRP spec files.

## 11. Frontend

Mirrors the XRP surface: `solVaultService.ts`, `solPayoutHelpers.ts`,
`nativeSolBorrowFlow.ts`, `SolBorrowModal.svelte`, `SolVaultPanel.svelte`,
`SolPendingDepositBanner.svelte`, `SolPayoutRouting.svelte`, plus SOL branches in
`VaultCard.svelte`, `ManualLiquidations.svelte`, `routes/+page.svelte`,
`collateralStore.ts`, `EarnInfoCard.svelte`, and `types.ts`
(`custodyKind: 'IcrcLedger' | 'NativeXrp' | 'NativeSol'`).

Deviations from XRP, deliberate:
- No destination-tag input anywhere.
- QR uses the `solana:<address>?amount=<sol>` Solana Pay URI.
- Add `sol-logo.svg` to `static/` and register `SOL` in `TokenBadge.svelte`'s
  `logos` map. XRP was never given one; we do both.
- The banner gets its own `--rumi-sol-recovery-height` CSS var; the layout
  padding math in `+layout.svelte` / `PositionStrip.svelte` must **sum** both
  vars, not replace.
- Docs: neither XRP nor SOL has any `/docs` copy today. Add a native-collateral
  docs page covering the custody model, the reserve, and claim settlement, for
  both assets.

## 12. Pre-activation gates

Registration is the last step, not the first. Before `register_sol_collateral` is
called on mainnet:

1. `set_sol_schnorr_key_name("key_1")` — which flips RPC to mainnet-beta in the
   same breath (§2).
2. `sol_bootstrap_nonce_account()` succeeded and the nonce account is
   `Initialized` on mainnet.
3. Settlement wallet funded above `SOLANA_HOT_WALLET_MIN_LAMPORTS`.
4. KAT vectors regenerated from `@solana/web3.js` and matching byte-for-byte.
5. All claim-based out-paths (§5) merged and tested — the XRP ordering invariant.
6. Security review of the settlement state machine (§5.3) specifically.
7. **Verify `chains::sol::rpc::SOL_RPC_PRINCIPAL`** (currently the literal
   `tghme-zyaaa-aaaar-qarca-cai`) against the live SOL RPC canister id before
   calling `register_sol_collateral` on mainnet. The source already carries a
   "VERIFY against the live repo before mainnet" comment on that constant;
   this gate makes it a checked step rather than a comment someone can miss.

The dormant `chains::solana` rail is configured for devnet and is not migrated by
this change (§2). Notes on its current configuration, and what would have to
change before it could be activated, are kept out-of-tree in
`.claude/security-docs/` since this repository is public.

**Deliberate decision: `chains::sol::rpc::sol_rpc_principal()` has no operator
override.** The dormant `chains::solana` rail's equivalent
(`chains::solana::sol_rpc::sol_rpc_principal()`) DOES support one
(`State.sol_rpc_principal_override`, settable by a developer-only endpoint, so
PocketIC / staging can point it at a mock). This rail's `chains::sol::rpc`
deliberately does not carry the same override. Every custody-relevant read on
this rail (balance checks, transaction status, the durable nonce itself) goes
through that one function, so a freely-settable override would let a
compromised developer principal repoint every custody read at a lying RPC
canister with a single call. Without an override, doing the same requires a
full wasm upgrade, which is subject to the pre-deploy test hook
(`.claude/hooks/pre-deploy-test.sh`). The failure mode of a stale hardcoded id
is fail-closed (outcalls simply error) rather than fail-open, which is the
safer direction to fail in for a custody-relevant address.

## 13. Out of scope

- Redemptions against native-SOL collateral (excluded from redemption priority,
  same as XRP).
- Automated bot liquidation of SOL vaults.
- Collateral top-ups on existing SOL vaults (§4.2).
- SPL tokens as collateral. Native SOL only.
- Migrating the dormant `chains::solana` icUSD-on-Solana rail.
