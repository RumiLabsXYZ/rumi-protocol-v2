# Solana Integration Design (Rumi Protocol)

Status: approved design, pre-implementation
Date: 2026-06-01
Author: brainstormed with Rob
Target depth: devnet-mature, mainnet-ready (stop before the irreversible mainnet cutover)

## 1. Summary

Add Solana as a second foreign-chain CDP to Rumi, mirroring the existing Monad
integration. A user locks native SOL as collateral at a canister-derived custody
address; the backend mints icUSD as an SPL token on Solana (the canister's
threshold-Ed25519 settlement address is the SPL mint authority). Burns and
withdrawals reverse the flow. icUSD on Solana is an SPL token, the direct
analogue of the `IcUSD.sol` ERC-20 minted on Monad.

The work targets the same milestone Monad reached and then stopped at: a
devnet-mature, staging-deployed integration with all bones in place, so a future
mainnet launch is a runbook (deploy the SPL mint on mainnet-beta, fund the pool,
swap the signing key, flip endpoints) rather than a build.

## 2. Goals and non-goals

### Goals
- Open / deposit / mint / withdraw / close a Solana CDP on devnet.
- Deposit detection (balance poll), SPL mint settlement, SOL withdrawal settlement.
- Burn detection via notify-then-verify (`submit_burn_proof` + `getTransaction`),
  with an SPL-supply backstop gate. Poll-scan built but disabled by default.
- Full PocketIC integration test plus per-submodule unit tests.
- Staging deploy with Solana timers OFF (near-zero idle cycle burn).

### Non-goals (explicitly out of scope)
- The irreversible mainnet cutover (mainnet-beta SPL mint, `key_1`, fund pool,
  endpoint/CSP flip). Documented as a runbook, not executed.
- SIWS (Sign-In-With-Solana) and a `siws_provider` canister. Auth stays on IC
  (Internet Identity / Oisy); the user supplies their own Solana address as a
  validated mint/withdrawal target. SIWS can be added later without reworking
  the CDP core.
- A full vault UI. We ship a `solanaService.ts` analogue (mirroring
  `monadBurnService.ts`), not a finished screen. Monad's UI is unwired too.
- Unifying the foreign-chain CDP with the ICP-native `Vault` model (Phase 2).

## 3. Background: what we reuse

The `chains/` layer was built multi-chain from day one. `chains/adapter.rs:2-3`
states each foreign chain "implements this trait in its own module
(`chains::monad`, `chains::solana`, ...)". Reused as-is:

- `chains/config.rs` — `ChainId`, `ChainConfigV2`, `RegisterChainArg`,
  `UpdateChainConfigArg`. `GasStrategy::SolanaPriorityFee { lamports_per_cu_ceiling }`
  already exists. `chain_native_decimals` already documents "9 for Solana SOL".
- `chains/supply.rs` — `apply_supply_delta`, the supply invariant, the migration
  template.
- `chains/settlement_queue.rs` — `SettlementOp`, `SettlementOpKind`,
  `SettlementQueueV1` (FIFO, one-in-flight). `Mint` and `NativeWithdrawal`
  variants are reused (recipient is a `String`, works for base58).
- `chains/multi_chain_state.rs` — `MultiChainStateV3`. Every field maps to Solana
  (see Section 7).
- `chains/admin.rs` — `register_chain`, `set_chain_config`, developer gating.
- The vault state machine (`open/withdraw/close_chain_vault_in_state`,
  `ChainVaultV1`, `ChainVaultStatus`) — reused after generalization (Section 5).

`chains/monad/` is the per-file template for `chains/solana/`.

## 4. Key external facts

- **SOL RPC canister.** DFINITY's Solana analogue of the EVM RPC canister.
  Mainnet principal MUST be pulled from the live repo at deploy time. Two
  candidates seen during research: `tghme-zyaaa-aaaar-qarca-cai` (fiduciary
  subnet, per the repo) and `2xib7-jqaaa-aaaar-qai6q-cai` (older docs). Verify
  before wiring. The playbook discipline applies: pull the live candid, never
  hand-type from memory.
- **Typed Rust crates exist but are unusable here:** `sol_rpc_client 6.0.0` /
  `sol_rpc_types 3.1.2` require **ic-cdk 0.20**, a hard conflict with the
  project's deliberate **ic-cdk 0.12** pin. This is the same wall Monad hit
  (`evm_rpc_client` -> ic-cdk 0.19), which is why Monad hand-rolls its EVM RPC.
  DECISION (2026-06-01, Rob): hand-roll the Solana seam on ic-cdk 0.12, mirroring
  Monad, rather than undertake a large, high-risk ic-cdk 0.20 migration for a
  build we are not taking live. Consequence: the playbook's decode-safety
  discipline (#1-#4: mirror exact candid widths, tolerate inconsistent sends,
  mutate only after a good decode, treat decode-fail as "maybe sent") applies in
  full, because we own the candid types now. The typed methods and consensus
  pattern remain the reference for what to mirror by hand.
- **Threshold Ed25519** via the management canister's `sign_with_schnorr`
  (algorithm `Ed25519`). Key names: `test_key_1` (testing, on mainnet) and
  `key_1` (production). Ed25519 has NO local dfx dev key.
- **PocketIC can provision an Ed25519 test key**, so integration tests can
  exercise the real signing path (with a graceful-degrade fallback like the
  Monad supply-gate test).
- **Solana address = base58 of the 32-byte Ed25519 public key.** No hashing
  (simpler than EVM's keccak + last-20-bytes).
- **Costs (published per-call, not measured here):** SOL RPC `request` ~1-5B
  cycles; `sign_with_schnorr` (Ed25519) ~26B cycles. Signing dominates; we sign
  only on real ops, never on a timer.
- **No SPL helpers** in the SOL RPC canister. The `MintTo` / ATA-create /
  `Transfer` / `AdvanceNonceAccount` instructions and message serialization are
  built by us (Section 6, `tx.rs`).

## 5. Architecture: chain-agnostic generalization

Two touch-points in the current code are Monad-hardcoded. Both get generalized
(approved fork 1). This changes no Monad behavior.

### 5.1 Vault helpers hoisted and parameterized
`open/withdraw/close_chain_vault_in_state` currently live in
`monad/chain_vault.rs` and embed `tecdsa::is_valid_evm_address` and the `"MON"`
price key. Hoist them to a chain-agnostic module (`chains/vault.rs`), injecting
the chain-specific pieces resolved by `ChainId`:

- **Address validator:** `is_valid_evm_address` (Monad) vs `is_valid_solana_address`
  (base58, 32 bytes) (Solana).
- **Native-asset price symbol:** `"MON"` vs `"SOL"` for the `manual_prices` key.
- **Native decimals:** read from `chain_configs[chain].chain_native_decimals`.

### 5.2 Decimals-aware collateral (approved fork 2)
Store foreign-chain collateral in the chain's NATIVE base units, not a forced
e18 scale:

- SOL collateral is stored as lamports (1 SOL = `1_000_000_000`). No conversion,
  no e18 normalization, no `0.000000001` landmine.
- `ChainVaultV1.collateral_amount_e18` is renamed to `collateral_amount_native`
  with `#[serde(alias = "collateral_amount_e18")]` so existing snapshots decode
  unchanged. For MON (18 decimals) the native unit is wei, which equals the
  former e18 value, so the data is identical and no value transform is needed.
- `collateral_ratio_e4` gains a `native_decimals: u8` parameter and divides by
  `10^native_decimals` instead of a hardcoded `1e18`. Monad passes 18 (behavior
  preserved), Solana passes 9.
- Follow the versioned-snapshot discipline: `ChainVaultV2` (renamed field) and a
  `MultiChainStateV4` value-type bump for `chain_vaults`, each new/renamed field
  carrying `#[serde(default)]` / `#[serde(alias)]`. Prove the decode with a
  round-trip test mirroring `tests_multi_chain_state_v2`. (Avoids the 2026-05-18
  AMM-style state-wipe class.)

### 5.3 Timer dispatch by chain
`setup_timers()` currently calls `monad::deposit_watch::observer_tick()` and
`monad::settlement::settlement_tick()` directly. Generalize each tick to iterate
registered, ENABLED chains and dispatch to the right adapter by `ChainId`. This
keeps the timer count flat (one observer timer, one settlement timer, total) as
chains are added. Each chain has an enable flag so Solana stays dark until we
turn it on (Section 11).

### 5.4 Module layout
```
chains/
  adapter.rs            (reuse; trait is already chain-neutral)
  config.rs             (reuse)
  vault.rs              (NEW: hoisted, generalized vault state machine)
  admin.rs              (reuse)
  supply.rs             (reuse)
  settlement_queue.rs   (reuse)
  multi_chain_state.rs  (V4 bump for ChainVaultV2 value type)
  monad/                (unchanged behavior; vault helpers move out to vault.rs)
  solana/               (NEW, mirrors monad/ file-for-file)
    mod.rs
    config.rs           (chain id, SOL/icUSD decimals, RPC endpoints, key name)
    sol_rpc.rs          (sol_rpc_client/sol_rpc_types wrapper, consensus-aware)
    ted25519.rs         (sign_with_schnorr Ed25519 + base58 derivation)
    tx.rs               (Solana message + MintTo/ATA/Transfer/AdvanceNonce)
    adapter.rs          (SolanaAdapter: ChainAdapter impl)
    deposit_watch.rs    (observer: balance poll + supply-gate burn check)
    settlement.rs       (worker: durable-nonce sign + sendTransaction + confirm)
    burn_proof.rs       (notify-then-verify via getTransaction)
    hardening.rs        (hot-wallet SOL gate, stuck-tx, commitment checks)
    tests_*.rs          (per-submodule unit tests)
```

## 6. Component design (new Solana submodules)

### 6.1 `sol_rpc.rs`
Hand-rolled wrapper over raw inter-canister calls to the SOL RPC canister
(mirror `monad/evm_rpc.rs`): `ic_cdk::api::call::call_with_payment128` with
hand-defined candid request/response types mirroring the live candid; JSON-RPC
payloads parsed with `serde_json`. Strict consensus on all reads (demand
agreement); lenient on `sendTransaction` (first provider Ok wins, because a
signed Solana tx signature is deterministic from the signed bytes). Methods
needed:
- `get_balance(addr, commitment)` — custody/settlement balances (deposit detect,
  gas gate).
- `get_account_info(mint)` — read the SPL mint account, decode its `supply`
  field for the backstop gate; read recipient ATA existence.
- `get_latest_blockhash()` — only for bootstrap/diagnostics (settlement uses the
  durable nonce, not a recent blockhash).
- `get_signatures_for_address(addr, until)` — poll-scan path only (disabled by
  default).
- `get_transaction(sig)` — burn-proof verification.
- `send_transaction(raw_tx)` — broadcast.

### 6.2 `ted25519.rs`
Mirrors `tecdsa.rs` but for Ed25519. ic-cdk 0.12 has no
`management_canister::schnorr` module (only `ecdsa`), so we call the management
canister directly via `call_with_payment128` with hand-defined
`SchnorrPublicKeyArgument` / `SignWithSchnorrArgument` candid structs (algorithm
`Ed25519`):
- `derive_solana_address(path)` — `schnorr_public_key` call, then base58-encode
  the 32-byte pubkey (no hashing).
- Settlement (mint-authority) derivation path: `[chain_id_le, b"settlement"]`,
  one address per chain, cached per upgrade.
- Custody derivation path: `[chain_id_le, principal_bytes, nonce_le]`, one per
  vault. Deposits land here; never signed.
- `is_valid_solana_address(s)` — base58 decode to exactly 32 bytes.

### 6.3 `tx.rs` (the largest net-new piece)
Build and sign Solana transactions. Instruction builders:
- `AdvanceNonceAccount` (durable nonce, always the first instruction).
- SPL `MintTo` to the recipient's associated token account.
- Associated-token-account create-if-absent (checked via `get_account_info`).
- System `Transfer` (SOL withdrawal).
Plus: legacy message assembly (header, account keys, durable-nonce as the
"recent blockhash", instructions), serialize the message, sign the message bytes
with `sign_with_schnorr`, assemble the wire transaction (signatures + message).
Build instructions via the lightweight pure-Solana primitive crates
(`solana-instruction`, `solana-pubkey`, `solana-message`, `spl-token`,
`spl-associated-token-account`). VERIFIED 2026-06-01 (M1 Task 3 spike):
`solana-pubkey 4.2` + `solana-instruction 3.4` (and transitive
`solana-address`/`solana-program-error`/`solana-sanitize`) compile to
wasm32-unknown-unknown and coexist with candid 0.10 / ic-cdk 0.12 with no
conflict. `solana-message` / `spl-token` / `spl-associated-token-account` to be
confirmed at M2 start (very likely fine since the foundation compiles).

### 6.4 `adapter.rs` (`SolanaAdapter`)
Implements the six `ChainAdapter` methods, wiring `sol_rpc`, `ted25519`, `tx`:
- `verify_deposit` — `get_transaction` / balance confirm at `finalized`.
- `sign_mint` — resolve SPL mint from `chain_contracts[solana]`, derive
  settlement authority, build durable-nonce + (ATA-create?) + `MintTo`, sign.
- `sign_withdrawal` — durable-nonce + System `Transfer` of lamports, sign.
- `sign_burn` — reserved (`NotImplemented`); burns are user-initiated.
- `fetch_finality` — `get_slot(finalized)`; commitment level replaces block depth.
- `observe_event` — delegated to the observer.

### 6.5 `deposit_watch.rs` (observer)
Mirrors Monad's two-watch tick:
- **Deposit watch (unconditional):** for each `AwaitingDeposit` vault,
  `get_balance(custody, finalized)`; if it covers the declared collateral, flip
  to `MintPending` and enqueue the `Mint` op (reusing
  `verify_deposit_and_enqueue_mint_in_state`).
- **Burn supply-gate (gated):** read the SPL mint `supply` via
  `get_account_info`; if `onchain == recorded` skip (we are the sole minter, no
  burn possible); if `onchain < recorded`, a burn happened. Primary recovery is
  notify-then-verify; the poll-scan (`get_signatures_for_address` over the mint)
  is built but disabled by default.

### 6.6 `settlement.rs` (worker)
Mirrors Monad's one-op-per-tick FIFO drain with one-in-flight enforcement:
- **Submit:** build the durable-nonce-led tx, sign (~26B cycles), broadcast via
  `send_transaction`, mark `Inflight`. Refresh / advance state ONLY after a
  confirmed-good decode (decode-fail is treated as "maybe sent").
- **Confirm:** `get_transaction(sig)` at `finalized`; on success run
  `confirm_mint_in_state` (move `pending_mint_e8s` to `debt_e8s`, flip to `Open`,
  increment `chain_supplies`). Stuck-tx handling is a seam (Section 10).
- **Durable nonce:** bootstrapped once at setup (idempotent). Every settlement
  tx leads with `AdvanceNonceAccount`, so build to broadcast survives the
  multi-second signing gap without blockhash expiry.

### 6.7 `burn_proof.rs`
`submit_burn_proof(chain_id, tx_sig)` -> `get_transaction(sig)` at finality ->
verify it contains an SPL `Burn` of the icUSD mint by the expected account ->
dedup on `"{tx_sig}:{instr_index}"` -> `apply_burn_to_state` (decrement
`debt_e8s` + `chain_supplies`) -> emit `ChainBurnObserved`. Returns
`TemporarilyUnavailable` (retryable) if not yet final.

### 6.8 `hardening.rs`
Pure predicates: hot-wallet SOL gate (settlement address needs lamports for fees
+ rent for ATAs / the nonce account), stuck-tx threshold, commitment-level
checks. Solana has no EVM-style reorgs; the reorg fields stay dormant (we read
at `finalized`).

## 7. State

All `MultiChainStateV3` fields map to Solana with no new top-level field for the
primary flow:

| Field | Solana meaning |
|---|---|
| `chain_configs` | Solana `ChainConfigV2` entry |
| `chain_supplies` | Solana icUSD SPL supply (e8s) |
| `settlement_queues` | Solana outbound ops |
| `chain_vaults` | Solana vaults (value type -> `ChainVaultV2`, Section 5.2) |
| `chain_contracts` | Solana icUSD SPL **mint address** (base58) |
| `manual_prices` | `(solana_chain_id, "SOL") -> price_e8` |
| `last_observed_block` | finalized-slot high-water mark |
| `hot_wallet_balance_e18` | settlement-address balance for the gas gate, in native base units (lamports for Solana); the `_e18` field name is a legacy wart |
| `reorg_halted` / `reorg_suspect_streak` | dormant for Solana (commitment-level finality) |
| `processed_burn_keys` | dedup, key = `"{tx_sig}:{instr_index}"`, outer key = slot |

State changes:
- `ChainVaultV2` (renamed `collateral_amount_native` + serde alias) and the
  `MultiChainStateV4` value-type bump for `chain_vaults`.
- Optional `MultiChainStateV4` field: a per-address signature cursor
  (`BTreeMap<(ChainId, String), String>`) for the disabled-by-default poll-scan.
  Cheap, `#[serde(default)]`, added for Monad-parity of the emergency toggle.

icUSD is 8 decimals on every chain, so `chain_supplies` and `debt_e8s` stay in
e8s everywhere. Only the native collateral asset differs in decimals (handled in
5.2). The Phase-1b foreign-chain supply invariant
(`sum(chain_supplies) == sum(chain_vault.debt_e8s)`) holds across both chains.

## 8. Data flows

- **Open -> deposit -> mint:** `open_solana_vault` derives a custody address and
  records an `AwaitingDeposit` vault (no mint enqueued). User sends SOL to the
  custody address. Observer detects the balance at finality, enqueues `Mint`.
  Settlement signs `MintTo` (creating the recipient ATA if needed) and
  broadcasts. On confirmation, `pending_mint_e8s -> debt_e8s`, vault -> `Open`,
  `chain_supplies += amount`.
- **Withdraw / close:** CR-checked (decimals-aware); enqueue `NativeWithdrawal`;
  settlement signs a System `Transfer` of lamports. Reserve-at-enqueue and
  restore-on-revert as in Monad.
- **Burn (repay):** user executes an SPL `Burn` of icUSD on Solana, then the
  dApp calls `submit_burn_proof`. Backend verifies via `get_transaction` and
  applies the burn. The supply-gate observer is the backstop.

## 9. Error handling and safety

- **Decode safety (we own it now):** because we hand-roll the candid types, the
  playbook's decode-trap class is live. Mirror exact int widths (lamports = u64,
  slots = the live width), tolerate inconsistent sends, and treat a
  `sendTransaction` decode-failure as "maybe sent" (reconcile, never
  blind-retry; the durable nonce makes a resend idempotent only after a
  confirmed-good decode). Pull the live SOL RPC candid before hand-typing.
- **At-least-once + no-mutation-on-rejection:** preserved from the Monad helpers
  (enqueue first, mutate after; cursor/state advance only after success).
- **Supply invariant halt:** reused; a `SupplyInvariant` error halts the chain's
  workers (no silent divergence).
- **Address validation at the boundary:** base58 mint/withdrawal targets
  validated before they can reach `tx.rs` (which would otherwise panic deep on
  the worker path).
- **No SP-style retries** on settlement failures (per project rule); a failed op
  surfaces, it does not auto-retry into a loop.

## 10. Known seams (built but deferred, matching Monad's stopping point)
- Stuck-tx replacement: detection wired; the actual re-sign/replace is a seam
  (Solana's analogue is re-signing the same durable-nonce tx, which is naturally
  idempotent). Mirrors Monad's Task-11 seam.
- Poll-scan burn detection: built, disabled by default.
- Relayer robustness for orphaned burns (user closes tab before finality):
  operator/manual recovery for now, same as Monad.

## 11. Cycle-burn posture

Solana observer and settlement are DISABLED by default (per-chain enable flag
off), so idle burn is ~0 until we turn Solana on for testing. When enabled,
intervals default to 300s (floored to >=30s).

| State | Estimate (derived from published per-call costs, not measured) |
|---|---|
| Idle (default, Solana off) | ~0 |
| Observer ON @ 300s, devnet, few vaults | ~1.5-3T/day (balance + account-info reads) |
| Per mint/withdraw settled | ~30-35B (one `sign_with_schnorr` ~26B + a few reads) |

Discipline: invoke the cycles-management skill before any deploy; re-confirm
against the live cycle-cost table; never sign on a timer; default intervals at
300s (floored to >=30s), tunable via setters; one timer pair total across all
chains.

## 12. Testing strategy

- **Unit (per submodule, mirror `tests_*.rs`):** base58 derivation vectors,
  instruction encoders (`MintTo`, ATA, `Transfer`, `AdvanceNonceAccount`),
  message serialization, deposit/burn state transitions, decimals-aware CR math,
  supply invariant, vault state machine.
- **PocketIC integration (mirror `phase1b_supply_gate_pic.rs`):** mock SOL RPC
  canister + provision an Ed25519 test key; prove the supply gate (skip on
  match, scan on drop), deposit -> mint, withdraw, burn-proof. Graceful degrade
  if the key cannot be provisioned in the local PocketIC build.
- **State migration:** round-trip decode test for `ChainVaultV1 -> V2` and
  `MultiChainStateV3 -> V4` (the field rename + value-type bump must not wipe
  Monad vaults).
- **SPL side:** a small check of the icUSD mint's authority + decimals config
  (the SPL analogue of the Foundry `IcUSD.t.sol`).

## 13. Milestones (stop before the irreversible mainnet flip)

- **M1 - read-only seam (devnet):** derive settlement + a custody address, read
  balances and the mint account via `sol_rpc_client`, bootstrap the durable
  nonce. De-risks the unfamiliar SOL-RPC + Ed25519 path AND verifies
  `solana-program` wasm32 build compatibility first.
- **M2 - deposits + mint + withdraw (devnet):** vault open, balance-poll
  detection, SPL `MintTo` settlement, SOL withdrawal, PocketIC tests.
- **M3-prep - burn + backstop + staging:** `submit_burn_proof` via
  `get_transaction`, SPL-supply backstop gate, staging deploy with Solana timers
  OFF. STOP here (exactly where Monad stopped).

## 14. Open items to verify during implementation
- SOL RPC canister principal (pull live; resolve the two candidates).
- RESOLVED: typed sol_rpc crates need ic-cdk 0.20 (conflict); seam is hand-rolled
  on ic-cdk 0.12. Reconsider an ic-cdk upgrade only if we later go to mainnet and
  want the typed decode safety.
- Which lightweight `solana-*` primitive crates compile to wasm32-unknown-unknown
  without pulling ic-cdk (for `tx.rs`), or whether to hand-encode instructions.
- Whether the fork's `ic-management-canister-types` carries Schnorr types, else
  hand-define the candid structs in `ted25519.rs`.
- PocketIC Ed25519 key provisioning in this repo's pinned PocketIC build.
- Devnet RPC endpoints with adequate rate limits.
- SPL token program choice: classic SPL Token (recommended for simplicity and
  compatibility, mirroring the simple ERC-20) vs Token-2022.

## 15. Mainnet runbook (documented, not executed)
Deploy the icUSD SPL mint on mainnet-beta with the `key_1`-derived settlement
address as mint authority; fund the settlement pool with SOL (fees + rent);
bootstrap the mainnet durable nonce; register the Solana chain with mainnet
endpoints; set the mint address; reset observer cursors; flip the frontend RPC
endpoint AND the asset-canister CSP `connect-src` together; remove devnet
warnings; enable the Solana timers. Operator-gated; irreversible; runbooked.
