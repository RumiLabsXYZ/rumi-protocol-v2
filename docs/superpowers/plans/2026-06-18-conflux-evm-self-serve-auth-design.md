# M2: EVM-native self-serve authentication (Design)

> **Status:** Design / spec. The task-by-task implementation plan is a separate
> `2026-06-18-conflux-evm-self-serve-auth-plan.md` (produced via
> superpowers:writing-plans after this design is approved). This document defines
> *what* we build and *why*; the plan defines *how*, task by task.
>
> **Scope banner:** This is the **M2** layer on top of the **M1** native-collateral
> CDP rail (merged to `main` via PR #248 + #251). M1 proved the full
> deposit→mint→burn→withdraw loop on Conflux eSpace testnet under **developer-gated**
> operator control. M2 makes it **self-serve**: a MetaMask user signs an EIP-712
> intent, the canister verifies the signature and acts, and the vault is owned by a
> synthetic IC principal derived from the user's EVM address. The IC caller is
> irrelevant. The `chains/` rail stays **testnet-only and dev-gated to enable** (the
> `chains/mod.rs` experimental banner stays); M2 adds no mainnet capability.

---

## Goal

Let a user with only an EVM wallet (MetaMask on Conflux eSpace chain 71, or Monad
chain 10143) open / borrow against / withdraw from / close a native-collateral
icUSD vault by **signing an EIP-712 intent in their wallet** — without holding an
IC principal, without an operator driving the call, and without trusting whatever
relayer or anonymous agent forwards the call to the canister. Authenticity comes
from the EVM signature, never from the IC caller principal.

## Decisions settled in brainstorming (2026-06-18)

| Fork | Decision | Rationale |
| --- | --- | --- |
| Scope | **Backend + tests only** | The frontend (design-doc Component 5) is a separate effort. M2 here is the auth layer + the supporting contract change. |
| Methods | **open / borrow / withdraw / close** (4 `_evm` methods) | Rob's call: borrow is included even though M1 has no borrow-more path. |
| Borrow ⇒ contract change | **Bundle the IcUSD.sol mint idempotency redesign into M2** | Borrow-more is a *second* on-chain mint per vault, which the M1 mint-once-per-`vault_id` guard hard-reverts. Idempotency moves to per-*op*. |
| Auth | **EIP-712 typed-data intent, verified on the canister** | Reuses the backend's existing secp256k1 recovery; no on-chain verifying contract; an untrusted relayer holds no authority. |
| Ownership | **Synthetic IC principal deterministically derived from the EVM address** | Keeps the entire M1 custody/vault machinery (`owner: Principal`) unchanged. |
| `mint_recipient` | **Forced `== owner_evm`** (field kept in the struct) | Free-form buys ~zero capability over mint-to-self+transfer and adds an irrecoverable mis-send footgun. Relaxable later without a signature break. |
| Withdraw/close collateral dest | **Forced `== owner_evm`** | Same no-footgun rule. The user can transfer CFX afterward. |
| Nonce | **Per-owner monotonic, keyed by the synthetic principal** (so per-(owner,chain)) | One trust assumption shared with ownership; cross-chain replay already blocked by the domain `chainId`. |
| Anti-spam backstop | **Per-owner cap + TTL-GC of stale `AwaitingDeposit`** (no hard global cap) | A hard global cap with no GC self-DoSes. The GC bounds unfunded state without that failure mode. |
| State migration | **Additive `#[serde(default)]` fields on the EXISTING structs** (no `V5`/`ChainVaultV2` bump) | `State`/`multi_chain` is ciborium/CBOR (verified in `storage.rs`), so additive serde-default adds are upgrade-safe. Keeps the merge with the concurrent interest-accrual branch purely additive. |

---

## Non-goals (out of scope for this spec)

1. **Frontend** (MetaMask + viem SvelteKit page). Separate effort.
2. **Mainnet** and everything it gates (production tECDSA keys, independent RPC
   providers, automated CFX oracle, the chains-rail security review).
3. **Liquidation** of chain vaults (the next design spec; the rail stays
   forward-compatible).
4. **Local BIP32 custody derivation** (the real fix for the per-open tECDSA-derive
   cycle cost — see §5). Noted as a follow-up; M2 keeps the per-open management call.
5. **ERC-20 collateral**, stability-pool-from-chain, and the other M1 deferred items.

---

## Verified preconditions (from the merged M1 code on `origin/main`)

- **State is ciborium/CBOR, not Candid.** `storage.rs::save_state_to_stable` /
  `load_state_from_stable` round-trip the whole `State` through
  `ciborium::ser/de`. `state.rs` carries dedicated tests proving the
  serde-default in-place migration. ⇒ Adding `#[serde(default)]` fields to
  `ChainVaultV1` and `MultiChainStateV4` is upgrade-safe; **no** version-struct
  bump is needed (and we deliberately avoid one to minimize merge conflict with
  the concurrent interest-accrual branch, which also edits these structs).
- **Crypto is all present.** `chains/evm/tx.rs::recover_y_parity` does the k256
  ecrecover round-trip (`VerifyingKey::recover_from_prehash` + the existing
  `tecdsa::evm_address_from_pubkey`); `sha3::Keccak256` is imported in `tx.rs` and
  `tecdsa.rs`. EIP-712 digest + signer recovery is a small new module reusing
  these — **no new crate**.
- **Three in-state helpers** (`chains/vault.rs`): `open_chain_vault_in_state`,
  `withdraw_collateral_in_state`, `close_chain_vault_in_state`. **No `borrow`
  helper exists** — the M1 rail mints the full declared debt once at
  deposit-verification.
- **Dev-gated endpoints are the template** (`main.rs`): `open_chain_vault`
  (async; tECDSA custody derive), `withdraw_chain_collateral` / `close_chain_vault`
  (**synchronous** with a hard "must stay sync" invariant — audit FLAG-16: the
  reserve-at-enqueue must be atomic within one message, no `.await`).
- **`inspect_message` (main.rs) silently drops anonymous ingress** for every
  method except two consent-message reads. The 4 `_evm` methods must be added to
  its accept-list so anonymous callers reach them (it is a cycle-saving
  pre-filter, **not** a security boundary).
- **On-chain idempotency** (`foundry/src/IcUSD.sol`): `mint(to, amount, vault_id)`
  with `mapping(uint64=>bool) minted; require(!minted[vault_id])` — one mint per
  vault, forever. The settlement queue assigns every op a **unique-per-chain
  `op_id`** (`settlement_queue.rs`); the confirm path fetches the receipt for the
  op's **own** `tx_hash`, so multiple Mint events per vault never confuse it.

---

## Component A — EIP-712 intent + signer recovery (`chains/evm/eip712.rs`, new)

The canister is the verifier; there is no on-chain verifying contract.

### Domain (binds chain + deployment)

```
EIP712Domain(string name, string version, uint256 chainId, address verifyingContract)
  name              = "Rumi icUSD CDP"
  version           = "1"
  chainId           = the collateral chain id (71 Conflux testnet / 10143 Monad)
  verifyingContract = chain_contracts[chain]   // the deployed IcUSD address
```

- `chainId` blocks chain 71 ↔ 10143 replay.
- `verifyingContract` is the IcUSD address, which differs per chain **and** per
  deployment (staging kvg63 vs mainnet have different IcUSD contracts) ⇒ blocks
  cross-deployment replay.
- If `chain_contracts[chain]` is unset, the intent is rejected (no domain to bind).

`domainSeparator = keccak256(abi.encode(DOMAIN_TYPEHASH, keccak256(name),
keccak256(version), chainId, verifyingContract))`, where `DOMAIN_TYPEHASH =
keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")`.

### Intent (one struct; `action` discriminates)

```
VaultIntent(uint8 action, uint64 chainId, address owner, uint64 vaultId,
            uint256 collateralWei, uint256 debtE8s, address recipient,
            uint256 nonce, uint256 deadline)

  action: 0 = Open, 1 = Borrow, 2 = WithdrawCollateral, 3 = Close
```

| action | vaultId | collateralWei | debtE8s | recipient |
| --- | --- | --- | --- | --- |
| Open | 0 | declared collateral | initial debt | == owner |
| Borrow | target | 0 | additional debt | == owner |
| Withdraw | target | amount to release | 0 | == owner (collateral dest) |
| Close | target | 0 | 0 | == owner (collateral dest) |

- `recipient` is **enforced `== owner` for every action** in M2 (no mis-send
  footgun). The field is kept so relaxing to free-form later is a validation
  change, not a typehash/signature break.
- `chainId` must equal the domain `chainId` (redundant guard).
- `now > deadline` ⇒ reject (a signature has a bounded first-use window).

`VAULT_INTENT_TYPEHASH = keccak256("VaultIntent(uint8 action,uint64 chainId,address owner,uint64 vaultId,uint256 collateralWei,uint256 debtE8s,address recipient,uint256 nonce,uint256 deadline)")`.
`hashStruct = keccak256(abi.encode(TYPEHASH, action, chainId, owner, vaultId, collateralWei, debtE8s, recipient, nonce, deadline))`
(each field a 32-byte word; `uint*` left-padded big-endian, `address` left-padded to 32 bytes).
`digest = keccak256(0x19 ‖ 0x01 ‖ domainSeparator ‖ hashStruct)`.

### Signer recovery (reuse — no new crate)

```
recover_evm_address(digest: [u8;32], sig65: [u8;65]) -> Result<String, _>
  r = sig65[0..32]; s = sig65[32..64]; v = sig65[64]
  parity = match v { 27|28 => v-27, 0|1 => v, _ => reject }
  vk = VerifyingKey::recover_from_prehash(digest, Signature::from_scalars(r,s), RecoveryId::new(parity==1,false))
  return evm_address_from_pubkey(vk.to_encoded_point(false))   // existing helper
```

This is the same round-trip `tx.rs::recover_y_parity` already performs. Require
`recovered.to_lowercase() == intent.owner.to_lowercase()`.

---

## Component B — Synthetic principal (vault ownership)

```
synthetic_owner(chain, evm_addr_20) =
    Principal::from_slice(
        keccak256(b"rumi.evm.owner.v1:" ‖ chain.0.to_le_bytes() ‖ evm_addr_20_raw)[0..28]
        ‖ [0x01]
    )                                        // 29 bytes total
```

Properties:
- **Deterministic** in `(chain, evm_addr)`.
- **Opaque class** (the trailing `0x01` type tag). A self-authenticating II/wallet
  principal ends in `0x02` and is 29 bytes, so this principal can **never** equal a
  real user principal. (It is far longer than any real canister id, and is never
  used as a caller identity or authenticated against, so a theoretical canister-id
  collision is harmless.)
- Used **only** as an internal owner key. Custody stays
  `custody_derivation_path(chain, synthetic_owner, vault_id)`, so the entire M1 rail
  is byte-unchanged.
- The 28-byte keccak truncation gives a 2⁻¹¹² collision bound. The **same**
  principal keys the nonce map, so ownership and replay share one trust assumption
  rather than introducing a second.

`evm_addr_20_raw` is the 20 raw address bytes parsed from the recovered hex
(canonical; avoids hex-case ambiguity).

---

## Component C — Per-owner nonce (replay guard)

New persisted field `evm_owner_nonces: BTreeMap<Principal, u64>` on
`MultiChainStateV4` (`#[serde(default)]`), **keyed by the synthetic principal**
(which embeds the chain ⇒ nonces are automatically per-(owner, chain)).

Rule: `expected = evm_owner_nonces.get(&synthetic).copied().unwrap_or(0)`; reject if
`intent.nonce != expected`; **increment on success**.

### Async-vs-sync nonce handling (saga safety)

Two different spend semantics, each justified by whether there is an `.await`:

- **Sync methods (borrow / withdraw / close) — spend on SUCCESS:** signature
  recovery is pure compute, so verify → nonce check → op → increment-nonce all run
  in **one synchronous message**. No `.await`, no race. The increment happens only
  after the op succeeds, so a failed CR check does not burn a nonce. This preserves
  the FLAG-16 "withdraw/close must stay synchronous" invariant (the reserve-at-enqueue
  stays atomic).
- **Async open — spend on ATTEMPT (pre-await):** the custody derive needs the
  `vault_id` (the derivation path is `(chain, synthetic, vault_id)`), so `vault_id`
  must be reserved **before** the await — and for race-safety the nonce is checked +
  incremented in the **same pre-await `mutate_state`**:
  1. **Pre-await atomic `mutate_state`:** verify sig + recover owner + deadline
     (pure), `nonce == expected`, **increment nonce**, per-owner cap, **reserve
     `vault_id`** from `chain_vault_id_counter`.
  2. `await derive_evm_address((chain, synthetic, vault_id))`.
  3. **Post-await `mutate_state`:** insert the vault (owned by `synthetic`, with the
     reserved `vault_id` + derived custody + `owner_evm`).

  A same-nonce double-submit is rejected at the pre-await nonce check (step 1), so the
  loser never reaches the derive — no wasted derive, no duplicate vault. The tradeoff:
  if the derive fails (transient management-canister error), the nonce + vault_id are
  already spent; the user re-signs with `nonce + 1`. Acceptable — derive failures are
  rare and a spent counter is harmless.

---

## Component D — Anti-spam / cycle-drain

**Honest stance:** an anonymous-callable open that performs a tECDSA derive is
inherently cycle-drainable by a Sybil (unlimited throwaway EVM keys), and a
per-owner cap cannot bound that. M2 bounds **state**, orders cheap checks before the
expensive derive, and accepts the residual derive-cycle cost as a **testnet** risk.

- **Per-owner cap:** ≤ `MAX_VAULTS_PER_OWNER` (const, 25) non-terminal vaults
  (`AwaitingDeposit | MintPending | Open | Closing`) per synthetic owner. Checked
  pre-derive and re-checked in the post-await mutate.
- **TTL-GC (the backstop):** a timer prunes `AwaitingDeposit` vaults older than
  `AWAITING_DEPOSIT_TTL_NS` (const, ~24h, via the vault's `opened_at_ns`). Bounds
  total unfunded state without the self-DoS of a hard global cap. Runs in the
  existing observer/cleanup tick; pruning an `AwaitingDeposit` vault is safe (no
  collateral confirmed, no mint enqueued). Funded vaults are never GC'd.
- **Cheap-before-expensive ordering:** sig recovery + nonce + cap checks all happen
  before the tECDSA derive in `open_chain_vault_evm`.
- **The expensive path stays deposit-gated:** `sign_with_ecdsa` + EVM broadcast fire
  only after a real on-chain deposit is observed at finality (unchanged from M1).
- **Master switch unchanged:** the `_evm` methods are inert until an admin
  `register_chain`s + sets a manual price + enables timers (all dev-gated). On a
  chain that is not registered, every `_evm` method fails fast (`UnknownChain`).
- **`inspect_message`** must whitelist the 4 `_evm` method names so anonymous
  ingress reaches them.
- **Follow-up (the real fix, out of scope):** derive custody addresses **locally**
  via BIP32 from a cached root tECDSA pubkey, eliminating the per-open management
  call entirely.

---

## Component E — Borrow + IcUSD.sol per-op idempotency

### Contract (`foundry/src/IcUSD.sol`)

```solidity
mapping(uint64 => bool) public mintedOps;            // was: minted[vault_id]
function mint(address to, uint256 amount, uint64 vault_id, uint64 op_id)
    external onlyRole(MINTER_ROLE)
{
    require(!mintedOps[op_id], "op already minted");  // op_id is unique per chain
    mintedOps[op_id] = true;
    _mint(to, amount);
    emit Mint(uint256(vault_id), to, amount);         // EVENT UNCHANGED
}
```

- Idempotency moves from per-`vault_id` to per-`op_id` (the settlement queue's
  unique-per-chain op id). A second mint to the **same vault** (a borrow) with a
  **different** `op_id` succeeds; a resubmit with the **same** `op_id` still reverts
  (no double-mint after a transient RPC error).
- **The `Mint` event is byte-identical**, so the backend log decoder, burn-watch,
  and confirm paths are untouched. `vault_id` stays load-bearing for the event and
  for `burn(amount, vault_id)` (repay).

### Backend mint calldata + settlement

- `tx::encode_mint_calldata` → new selector `mint(address,uint256,uint64,uint64)`;
  append the `op_id` ABI word.
- Thread the settlement op's `op_id` (already assigned at enqueue) through
  `build_tx_plan` into the calldata. The genesis (open) mint uses the same per-op
  guard — consistent.

### `borrow_chain_vault_in_state` (new helper in `chains/vault.rs`)

Mirrors the open helper's structure, parameterized on the same `(address_validator,
price_symbol)` seams:
1. Require `status == Open` and `pending_mint_e8s == 0` (no stacked borrows).
2. CR-check `(collateral_amount_native, debt_e8s + additional_e8s)` ≥ `min_cr_e4`.
3. Set `pending_mint_e8s = additional_e8s`.
4. Enqueue a `Mint { recipient, amount_e8s = additional_e8s, vault_id }` op
   (off-chain dedup key `mint-{chain}-{vault}-{now_ns}`, mirroring the withdraw key).

`confirm_mint_in_state` is unchanged: on the observed mint it moves
`pending_mint_e8s → debt_e8s` and `apply_supply_delta(+additional)`. The supply
invariant `sum(chain_supplies) == total_chain_vault_debt` holds across borrow
(both sides += additional).

### Foundry tests

Update the mint() tests: new selector; **second mint, same vault, different op_id ⇒
succeeds**; **same op_id ⇒ reverts**. The `Mint`/`Burn` event-pinning tests are
unchanged (the event did not change).

### Operator step (not done by this change)

Redeploy `IcUSD.sol` to eSpace testnet with the new ABI and re-point
`set_chain_contract`. Prepped here; run by the operator.

---

## Component F — The four `_evm` methods (`main.rs`, beside the dev-gated ones)

The dev-gated `open_chain_vault` / `withdraw_chain_collateral` / `close_chain_vault`
**stay** (operator + test use). The new methods accept **anonymous** ingress; all
authority is the EVM signature.

```
open_chain_vault_evm(intent, sig65)            -> Result<u64, ProtocolError>   // async
borrow_chain_vault_evm(intent, sig65)          -> Result<(), ProtocolError>    // sync
withdraw_chain_collateral_evm(intent, sig65)   -> Result<(), ProtocolError>    // sync
close_chain_vault_evm(intent, sig65)           -> Result<(), ProtocolError>    // sync
```

Common verification (a shared helper):
1. Resolve `chain = intent.chainId`; require it registered and `chain_contracts`
   set; build the domain.
2. `digest = eip712_digest(domain, intent)`; `signer = recover_evm_address(digest, sig)`;
   require `signer == intent.owner`.
3. Require `intent.chainId == chain` and `now <= intent.deadline`.
4. `synthetic = synthetic_owner(chain, signer)`.
5. Nonce check (+increment on success per §C).

Then per method (the Open nonce/`vault_id` ordering follows §C: spend-on-attempt
pre-await; the sync methods spend-on-success):
- **Open:** require `recipient == owner`; in the pre-await mutate do the nonce
  check+increment + per-owner cap + reserve `vault_id`; derive custody from `(chain,
  synthetic, vault_id)`; then `open_chain_vault_in_state(.. owner = synthetic ..,
  custody, collateralWei, debtE8s, mint_recipient = owner_evm ..)` with
  `owner_evm = Some(lowercase signer)` stored on the vault. Returns the `vault_id`.
- **Borrow:** require `recipient == owner`; load the vault, require `vault.owner ==
  synthetic` (and `vault.owner_evm == signer`); `borrow_chain_vault_in_state(vault_id,
  debtE8s, ..)`.
- **Withdraw:** require `recipient == owner`; ownership check as above;
  `withdraw_collateral_in_state(vault_id, collateralWei, dest = owner_evm, ..)`.
- **Close:** ownership check; `close_chain_vault_in_state(vault_id, dest = owner_evm, ..)`.

Ownership authorization = re-recover the signer from the intent and match the stored
`owner_evm` (lowercased); defense-in-depth, also assert `synthetic == vault.owner`.

---

## Component G — State migration (coordinated with the interest-accrual session)

Two additive `#[serde(default)]` fields on the **existing** structs (no version
bump):
- `ChainVaultV1.owner_evm: Option<String>` — `None` for dev/Monad/Solana vaults;
  `Some(lowercase addr)` for EVM-self-serve vaults.
- `MultiChainStateV4.evm_owner_nonces: BTreeMap<Principal, u64>`.

The GC needs no stored counter (it scans by `opened_at_ns`, bounded by the
per-owner cap × owners and the TTL). An **upgrade test** snapshots an M1 state
(registered chain + vaults + supplies), encodes via the same ciborium path
`storage.rs` uses, decodes into the new struct, and asserts the vaults/supplies
survive and the new fields default. If the interest-accrual branch merges first,
rebase and resolve the (additive) conflicts in these two structs.

---

## Testing strategy (TDD)

- **Unit (Rust):**
  - EIP-712: a golden `domainSeparator` + `hashStruct` + `digest` vector; a known
    secp256k1 key signs a known intent and `recover_evm_address` returns the
    expected address; a tampered field changes the digest / fails recovery.
  - `synthetic_owner`: determinism; opaque-class (`0x01` tag); never equal to a
    constructed self-authenticating principal; distinct per chain.
  - Nonce: monotonic accept; replay reject; per-(owner,chain) isolation.
  - Per-owner cap: the (N+1)th non-terminal open rejects.
  - Borrow: CR boundary; `pending != 0` rejects; supply invariant across confirm.
  - Mint idempotency (Rust calldata): selector + op_id word.
- **Upgrade (Rust):** the §G round-trip.
- **PocketIC e2e** (extend `tests/conflux_espace_happy_path_pic.rs`): an
  **anonymous** caller submits in-test-signed intents → open → deposit → mint →
  **borrow → mint** → repay (burn) → withdraw → close, asserting synthetic
  ownership, supply invariant, idempotency, and **replay / wrong-signer rejection**.
  Run with an **absolute** `POCKET_IC_BIN=$(pwd)/pocket-ic`; rebuild the canister
  wasm the test `include_bytes!`s after code changes.
- **Foundry:** the updated mint tests.

Definition of done: the 4 `_evm` methods work end to end; replayed nonce + mismatched
signature are rejected; recovery + EIP-712 unit-tested with known vectors; the
upgrade test shows existing vaults migrate cleanly; all chains lib tests + the
existing conflux/monad PocketIC tests stay green.

---

## Files touched (map)

- **New:** `chains/evm/eip712.rs` (domain/struct hashing + `recover_evm_address` +
  `synthetic_owner`); `chains/evm/tests_eip712.rs`.
- **Modify (additive):** `chains/monad/chain_vault.rs` / `chains/vault.rs`
  (`owner_evm` field; `borrow_chain_vault_in_state`); `chains/multi_chain_state.rs`
  (`evm_owner_nonces` field); `chains/evm/tx.rs` (mint calldata selector + op_id);
  `chains/evm/settlement.rs` (thread op_id); `main.rs` (4 `_evm` methods + shared
  verify helper + `inspect_message` accept-list + the TTL-GC tick);
  `rumi_protocol_backend.did` (intent record + 4 methods).
- **Solidity:** `foundry/src/IcUSD.sol` (per-op idempotency + new mint signature) +
  its Foundry tests.
- **Tests:** new unit + upgrade suites; extended Conflux PocketIC e2e.

---

## Safety / constraints honored

- **Authority isolation:** the EVM signature is the only authority on `_evm`
  methods; the IC caller (relayer / anonymous agent) holds none.
- **Replay:** per-owner monotonic nonce (per-(owner,chain) via the synthetic key) +
  domain `chainId`/`verifyingContract` binding (cross-chain + cross-deployment) +
  `deadline`.
- **Reentrancy / saga:** sync methods have no await; the async open does the
  authoritative nonce check+increment + vault insert in a single post-await
  `mutate_state`, with cheap pre-await checks to short-circuit spam.
- **Cycle-drain:** cheap-before-expensive ordering; deposit-gated mint; per-owner
  cap + TTL-GC bound state; residual per-open derive cost accepted on testnet with
  local-derivation as the noted follow-up.
- **Idempotency:** per-op on-chain guard (`mintedOps[op_id]`); off-chain settlement
  dedup keys; the supply invariant self-check unchanged.
- **State-wipe discipline:** additive `#[serde(default)]` fields on ciborium-encoded
  structs (verified safe), with an upgrade round-trip test.
- **Testnet + dev-gated to enable:** the `chains/mod.rs` banner stays; no mainnet
  capability is added.
