# Solana M2 (Adapter: deposits / mint / withdraw) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. This plan is self-contained: you start with NO prior conversation context. Read Section 0 (Context) first, then the spec and playbook it references.

**Goal:** Make a Solana CDP fully functional on devnet: open a vault (SOL collateral), detect the deposit, mint icUSD as an SPL token (threshold-Ed25519-signed), withdraw SOL, and confirm settlements. End-to-end signing/broadcast proven in PocketIC.

**Architecture:** A `SolanaAdapter` implementing the existing `ChainAdapter` trait, mirroring `chains::monad` method-for-method. Transactions are built with the lightweight `solana-message`/`solana-transaction`/`solana-system-interface` crates plus HAND-ENCODED SPL MintTo + ATA-create instructions, signed with threshold Ed25519 (`ted25519::sign_message`, already built), and broadcast via the hand-rolled SOL RPC `jsonRequest` seam (already built). Durable nonce accounts make build-sign-broadcast deterministic across async gaps.

**Tech Stack:** Rust, ic-cdk 0.12, candid 0.10, `solana-message` 4 / `solana-transaction` 4 / `solana-system-interface` 3 / `solana-pubkey` 4 / `solana-instruction` 3 (all wasm32-verified, in Cargo.toml), `bs58`, `serde_json`, PocketIC 6.0.

---

## 0. CONTEXT (read before anything)

**Branch + worktree.** All prior Solana work is committed on the `feat/solana-integration` branch. Work in a git worktree for that branch under `.worktrees/` (project convention; `.worktrees/` is gitignored). Do NOT work in the main checkout (another session uses it). Use `git -C <worktree>` / `cargo --manifest-path <worktree>/Cargo.toml` (a bare `cd` in a compound command does NOT persist between tool calls — this caused a real cwd-confusion incident; always pass explicit paths).

**What already exists (committed, tested):**
- `chains/solana/config.rs` — `SOLANA_CHAIN_ID = ChainId(501)`, `SOL_NATIVE_DECIMALS = 9`, `SOLANA_ICUSD_DECIMALS = 8`, `solana_schnorr_key_name() = "test_key_1"`, `solana_default_register_arg()`.
- `chains/solana/ted25519.rs` — `derive_solana_address(path) -> (pubkey, base58)`, `sign_message(message, path) -> 64-byte sig` (threshold Ed25519 via hand-rolled `sign_with_schnorr`, ~30B cycles), `settlement_derivation_path(chain)`, `custody_derivation_path(chain, user, nonce)`, `is_valid_solana_address`, `solana_address_from_pubkey`.
- `chains/solana/sol_rpc.rs` — hand-rolled SOL RPC over `jsonRequest` (returns text, parsed with serde_json): `get_balance(pubkey) -> u64 lamports`, `get_mint_supply(mint) -> u64`, the candid types (`RpcSources`, `RpcConfig`, `MultiRequestResult`, etc.), `json_request(payload)` (private), strict-consensus reads. `sol_rpc_principal()` honors `State::sol_rpc_override()` for mocks.
- `main.rs` dev-gated endpoints: `solana_settlement_address`, `solana_get_balance`, `solana_get_mint_supply`, `set_sol_rpc_principal`.
- Foundation: `collateral_ratio_e4(collateral_native, native_decimals, price_e8, debt_e8s)` is decimals-aware; the `ChainVault` collateral field is `collateral_amount_native` (Rust) / `collateral_amount_e18` (wire, via `#[serde(rename)]`).

**Critical decisions / gotchas (do not relitigate):**
- **Hand-roll the seam on ic-cdk 0.12.** Do NOT add `sol_rpc_client`/`sol_rpc_types` (need ic-cdk 0.20) or `spl-token`/`spl-associated-token-account` (pull spl-token-2022 confidential-transfer crypto, wasm-risky). Hand-encode SPL MintTo + ATA-create (Section: tx.rs).
- **serde is pinned** to 1.0.217 / serde_json 1.0.135 in Cargo.lock (the IC fork's `ic-types` breaks on newer serde). Do NOT `cargo update` serde/serde_json. See the Cargo.toml comment.
- **inspect_message rejects anonymous update callers** at ingress. Test dev-gates with a non-anonymous, non-developer principal, NOT `Principal::anonymous()`.
- **Decode the rich `ProtocolError` as `candid::Reserved`** in PocketIC test mirrors (it won't subtype-decode into a minimal local enum).
- **PocketIC: use an absolute `POCKET_IC_BIN`** (`/abs/path/to/pocket-ic`); cargo runs tests with cwd = the package dir, so `./pocket-ic` is not found.
- **PocketIC provisions threshold Ed25519** (`test_key_1`), so signing IS testable end-to-end (proven in M1).
- **Candid regen:** after adding endpoints, `RUMI_REGEN_DID=1 cargo test ... --bin rumi_protocol_backend check_candid_interface_compatibility` rewrites the `.did`; then run it again without the flag to confirm.
- **Cycle burn (Rob cares):** add NO always-on timers. The observer/settlement run only when enabled per-chain (default OFF), like Monad's `burn_watch_poll_enabled`. A signature costs ~26B cycles, so sign only on real ops.

**References (read these):**
- `docs/superpowers/specs/2026-06-01-solana-integration-design.md` — the design (Sections 5, 6, 8, 9 especially).
- `docs/icp-solana-integration-playbook.md` — the seam field guide. Apply #2 (decode-trap-after-broadcast), #4 (lenient sends / strict reads), #5 (deposit loop), #7 (durable nonce).
- `chains/monad/{adapter,deposit_watch,settlement,tx,burn_proof,hardening}.rs` — the per-file templates to mirror.
- DFINITY `basic_solana` example (github.com/dfinity/sol-rpc-canister, examples/basic_solana) — canonical message-build + sign + send reference. Confirm exact `solana-message`/`solana-transaction` method names against it / docs.rs.

**Verify-as-you-go:** the exact `solana-message` / `solana-transaction` method signatures (e.g. `Message::new_with_blockhash`, `Message::serialize`, `Transaction { signatures, message }`) must be confirmed against the crate docs at implementation time. Where this plan shows such calls, treat them as the intended shape and adjust to the real API.

**Build/test commands (always with explicit paths):**
- `cargo test --manifest-path <WT>/Cargo.toml -p rumi_protocol_backend --lib <filter>`
- `cargo build --manifest-path <WT>/Cargo.toml -p rumi_protocol_backend --target wasm32-unknown-unknown --release`
- `POCKET_IC_BIN=<abs>/pocket-ic cargo test --manifest-path <WT>/Cargo.toml -p rumi_protocol_backend --test <name>` (rebuild the wasm first; PocketIC tests `include_bytes!` it)

---

## File Structure
- Create: `chains/solana/tx.rs` — message build, SPL/ATA/System/nonce instruction encoding, sign, wire assembly.
- Create: `chains/solana/adapter.rs` — `SolanaAdapter: ChainAdapter`.
- Create: `chains/solana/deposit_watch.rs` — observer (deposit poll + supply gate).
- Create: `chains/solana/settlement.rs` — settlement worker.
- Create: `chains/solana/hardening.rs` — hot-wallet SOL gate, stuck-tx, commitment helpers (mirror monad).
- Create: `chains/solana/tests_*.rs` per module.
- Create: `chains/vault.rs` — hoisted, generalized vault state machine (F3).
- Modify: `chains/solana/{mod,config,sol_rpc}.rs` — add `sendTransaction`, `getLatestBlockhash`, `getAccountInfo`-raw (nonce read), `getTransaction`; new mod decls; nonce + mint config.
- Modify: `chains/monad/chain_vault.rs` — re-export from `chains::vault` (F3) keeping Monad behavior.
- Modify: `main.rs` — Solana vault endpoints + timer dispatch; `rumi_protocol_backend.did` (regen).
- Create: `tests/solana_m2_pic.rs` — deposit -> mint -> withdraw integration (mock SOL RPC + Ed25519).

---

## Task 1: Signing pipeline proof — `tx.rs` System Transfer + sign + a PocketIC verify test

Prove the canister can build, threshold-sign, and serialize a valid Solana transaction BEFORE adding SPL/nonce complexity.

**Files:** Create `chains/solana/tx.rs`, `chains/solana/tests_tx.rs`; modify `chains/solana/mod.rs`; modify `chains/solana/sol_rpc.rs` (add `send_transaction`); modify `main.rs` (dev endpoint); create `tests/solana_m2_sign_pic.rs`.

- [ ] **Step 1 (pure helpers, TDD):** In `tx.rs`, add a pure `assemble_wire_tx(signature: [u8;64], message_bytes: &[u8]) -> Vec<u8>` that prepends the compact-array of signatures (count=1 as compact-u16 = single byte 0x01, then the 64 sig bytes) to the serialized message. Write `tests_tx.rs` asserting the output length = 1 + 64 + message_bytes.len() and the first byte is 1. (This is the legacy wire format: `[compact-u16 sig count][sigs][message]`.) Run the test (red, then green).

- [ ] **Step 2 (message build):** Add `build_transfer_message(from: &Pubkey, to: &Pubkey, lamports: u64, recent_blockhash: Hash) -> Message` using `solana_system_interface::instruction::transfer(from, to, lamports)` and `solana_message::Message::new_with_blockhash(&[ix], Some(from), &recent_blockhash)`. Add a pure test: build with dummy pubkeys + a dummy blockhash, assert the message has 1 instruction and the fee payer (account_keys[0]) == from. (Confirm exact API names against the crate.)

- [ ] **Step 3 (sign + assemble):** Add `async fn sign_transfer(from_path, from_pubkey, to, lamports, blockhash) -> Result<Vec<u8>, String>`: build message, `message.serialize()`, `ted25519::sign_message(msg_bytes, from_path)`, `assemble_wire_tx(sig, &msg_bytes)`. Returns wire bytes.

- [ ] **Step 4 (broadcast helper):** In `sol_rpc.rs` add `send_transaction(wire_tx: &[u8]) -> Result<String /*signature*/, String>`: base64-encode the wire tx (add the `base64` crate, default-features=false), build the `sendTransaction` JSON-RPC payload (`params: [b64, {"encoding":"base64","skipPreflight":false}]`), call `json_request`. LENIENT consensus on sends (playbook #4): if you later expose the Multi result, accept the first Ok. Parse the returned signature string.

- [ ] **Step 5 (dev endpoint + PocketIC proof):** Add dev-gated `solana_sign_test_transfer(to: String, lamports: u64) -> Result<Vec<u8>, ProtocolError>` (derive settlement address+path, dummy blockhash, `sign_transfer`, return wire bytes). In `tests/solana_m2_sign_pic.rs` (mirror `tests/solana_m1_seam_pic.rs` harness): call it as the developer; deserialize the bytes with `solana_transaction::Transaction` (via bincode) and assert `tx.verify().is_ok()` (the threshold Ed25519 signature is valid for the message). Graceful-degrade if PocketIC lacks the key. Regen the `.did`. Build wasm, run the PocketIC test.

- [ ] **Step 6:** Commit: `feat(solana): tx signing pipeline (build+sign+assemble System Transfer) + PocketIC verify`.

## Task 2: `tx.rs` — hand-encoded SPL MintTo + Associated Token Account

**Files:** modify `chains/solana/tx.rs`, `chains/solana/tests_tx.rs`.

- [ ] **Step 1 (constants + ATA derive, TDD):** Add program-id constants (base58, parse to `Pubkey`): SPL Token `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`, ATA program `ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL`, System `11111111111111111111111111111111`. Add `derive_ata(owner: &Pubkey, mint: &Pubkey) -> Pubkey` = `Pubkey::find_program_address(&[owner.as_ref(), TOKEN_PROGRAM.as_ref(), mint.as_ref()], &ATA_PROGRAM).0`. Test against a known (owner, mint, ata) vector from the Solana docs/CLI.

- [ ] **Step 2 (MintTo, TDD):** `mint_to_ix(mint, dest_ata, authority, amount: u64) -> Instruction`: program = TOKEN_PROGRAM; accounts = `[AccountMeta::new(mint, false), AccountMeta::new(dest_ata, false), AccountMeta::new_readonly(authority, true)]`; data = `[7u8]` ++ `amount.to_le_bytes()` (MintTo discriminant = 7). Test: assert data == `[7, ...amount LE]` (9 bytes) and account count/flags.

- [ ] **Step 3 (ATA create-idempotent, TDD):** `create_ata_idempotent_ix(funder, owner, mint) -> Instruction`: program = ATA_PROGRAM; ata = `derive_ata(owner, mint)`; accounts = `[new(funder, true), new(ata, false), readonly(owner, false), readonly(mint, false), readonly(SYSTEM, false), readonly(TOKEN_PROGRAM, false)]`; data = `[1u8]` (CreateIdempotent discriminant = 1). Test the data byte + account layout.

- [ ] **Step 4 (mint message builder):** `build_mint_message(authority, mint, recipient_owner, amount, blockhash) -> Message` with instructions `[create_ata_idempotent_ix(authority, recipient_owner, mint), mint_to_ix(mint, derive_ata(recipient_owner, mint), authority, amount)]`, fee payer = authority. Pure test: 2 instructions, correct programs.

- [ ] **Step 5:** Commit: `feat(solana): hand-encoded SPL MintTo + ATA-create instructions`.

## Task 3: `tx.rs` — durable nonce (advance + bootstrap)

Durable nonce makes build->sign(slow)->broadcast deterministic (playbook #7). The settlement address owns one nonce account.

**Files:** modify `chains/solana/tx.rs`, `chains/solana/sol_rpc.rs`, `chains/solana/config.rs`, tests.

- [ ] **Step 1 (nonce account address):** Derive the nonce account as a deterministic address. Simplest: a second threshold-Ed25519 derivation path `[chain_id_le, b"nonce"]` (so it has no private key off-canister and the canister "owns" it as authority). Add `nonce_derivation_path(chain)` to `ted25519.rs`. (Alternative: a PDA; the derived-keypair-less account is simpler here.)

- [ ] **Step 2 (read nonce value):** In `sol_rpc.rs` add `get_durable_nonce(nonce_pubkey) -> Result<Hash, String>`: `getAccountInfo` (base64) on the nonce account, parse the nonce account layout (version u32, state u32, authority [32], **nonce blockhash [32] at offset 40**, fee calculator). Return the 32-byte blockhash as `Hash`. Pure test: decode a constructed 80-byte nonce account buffer.

- [ ] **Step 3 (advance-nonce-led messages):** Add `build_*_message_with_nonce(...)` variants whose recent_blockhash = the durable nonce and whose FIRST instruction is `solana_system_interface::instruction::advance_nonce_account(&nonce_pubkey, &authority)`. Apply to both transfer and mint builders. Pure test: first instruction targets the System program + nonce account.

- [ ] **Step 4 (bootstrap, idempotent):** Add `async fn bootstrap_nonce_account(chain, blockhash_override: Option<Hash>) -> Result<(), String>`: if `get_durable_nonce` succeeds, return Ok (already bootstrapped). Else build a create+initialize-nonce tx (`solana_system_interface::instruction::create_nonce_account(payer, nonce, authority, lamports_for_rent)`), sign with the settlement key (+ the nonce key for the create), broadcast. Dev-gated endpoint `solana_bootstrap_nonce(opt text)`. Operator runs it once on devnet. **PLAYBOOK #4 (load-bearing):** `getLatestBlockhash` is a per-slot value that the sol-rpc canister cannot reach multi-provider consensus on (chronic `#Inconsistent`), so the consensus auto-fetch (`None`) fails on real clusters and only works in PocketIC. On devnet/mainnet the operator passes one fresh finalized blockhash as the override (fetched and handed in within ~60s before it expires). Proven by `tests/solana_bootstrap_pic.rs` (None fails under modeled `#Inconsistent`, override succeeds).

- [ ] **Step 5:** Commit: `feat(solana): durable nonce (advance + idempotent bootstrap)`.

## Task 4: `SolanaAdapter` (ChainAdapter impl)

**Files:** create `chains/solana/adapter.rs`, `tests_adapter.rs`; modify `mod.rs`. Mirror `chains/monad/adapter.rs`.

- [ ] **Step 1:** `SolanaAdapter { chain_id }`. Implement `ChainAdapter`:
  - `sign_mint(instr)`: resolve SPL mint from `chain_contracts[chain]`; derive settlement addr+path; read durable nonce; `build_mint_message_with_nonce(authority=settlement, mint, recipient=instr.recipient (base58), amount=instr.amount_e8s, nonce)`; sign; return `SignedMint { raw_tx, tx_hash: "" }`.
  - `sign_withdrawal(req)`: SOL transfer of `req.amount_e8s` lamports (note: amount carried in native lamports for Solana) to `req.recipient`; nonce-led; sign.
  - `verify_deposit(sig)`: `getTransaction` at finalized; Ok if confirmed.
  - `fetch_finality()`: `getSlot(finalized)` -> `FinalitySnapshot`.
  - `sign_burn` -> `NotImplemented` (user-initiated). `observe_event` -> delegated (empty).
- [ ] **Step 2:** Tests for the pure parts (address validation at boundary). Async covered by the M2 PocketIC test.
- [ ] **Step 3:** Commit.

## Task 5: F3 — hoist + generalize the vault state machine

**Files:** create `chains/vault.rs`; modify `chains/monad/chain_vault.rs`, `chains/mod.rs`, `main.rs`, and import sites.

- [ ] **Step 1:** Move `ChainVault`/`ChainVaultStatus`/`OpenVaultError`/`WithdrawError`/`collateral_ratio_e4`/`open_chain_vault_in_state`/`withdraw_collateral_in_state`/`close_chain_vault_in_state`/`verify_deposit_and_enqueue_mint_in_state` to `chains/vault.rs`. Parameterize the open/withdraw/close helpers with: an address-validator `fn(&str)->bool`, a `price_symbol: &str`, and read `native_decimals` from `chain_configs[chain]`. Replace the hardcoded `is_valid_evm_address` + `"MON"`.
- [ ] **Step 2:** `chains/monad/chain_vault.rs` re-exports the moved items (or update imports across monad + tests to `chains::vault`). Keep `MONAD_MIN_CR_E4` in monad config. Add `SOLANA_MIN_CR_E4` to solana config.
- [ ] **Step 3:** Update `main.rs` Monad endpoints to call the generalized helpers with `(is_valid_evm_address, "MON")`. Run the FULL Monad lib + PocketIC suites — behavior MUST be unchanged.
- [ ] **Step 4:** Commit.

## Task 6: Solana vault endpoints + candid

**Files:** modify `main.rs`, `rumi_protocol_backend.did`.

- [ ] **Step 1:** Add dev/user endpoints `open_solana_vault(collateral_lamports, debt_e8s, mint_recipient_base58) -> Result<ChainVault>`, `withdraw_solana_collateral`, `close_solana_vault`, calling the generalized `chains::vault` helpers with `(is_valid_solana_address, "SOL")` and `SOLANA_CHAIN_ID`. Custody address = `derive_solana_address(custody_derivation_path(chain, caller, nonce))`. Set a `manual_prices[(SOLANA_CHAIN_ID,"SOL")]` admin setter if not already generic.
- [ ] **Step 2:** Regen `.did`; confirm candid compat. Commit.

## Task 7: Solana observer (`deposit_watch.rs`)

**Files:** create `chains/solana/deposit_watch.rs`, `tests_deposit_watch.rs`. Mirror `chains/monad/deposit_watch.rs`.

- [ ] **Step 1 (deposit watch):** `run_observer(chain)`: for each `AwaitingDeposit` vault, `sol_rpc::get_balance(custody, finalized)`; if >= declared collateral (lamports), `verify_deposit_and_enqueue_mint_in_state`. Per-chain re-entrancy guard (mirror Monad). Pure state-transition tests.
- [ ] **Step 2 (supply gate):** read SPL mint `supply` via `get_mint_supply`; if `onchain == recorded` skip burn handling; if `onchain < recorded`, a burn happened (primary recovery is notify-then-verify in M3; here just log + flag). Tests.
- [ ] **Step 3:** Commit.

## Task 8: Solana settlement (`settlement.rs`) + timer dispatch

**Files:** create `chains/solana/settlement.rs`, `tests_settlement.rs`; modify `main.rs` (register_observer_timer / register_settlement_timer dispatch by chain-kind). Mirror `chains/monad/settlement.rs`.

- [ ] **Step 1:** `run_settlement(chain)`: FIFO drain, one-in-flight. Submit: read nonce, `SolanaAdapter::sign_mint`/`sign_withdrawal`, `sol_rpc::send_transaction`, mark Inflight. Confirm: `getTransaction(sig)` at finalized; on success `confirm_mint_in_state`. Mutate durable state ONLY after a good decode (playbook #2). Re-entrancy guard.
- [ ] **Step 2 (timer dispatch):** generalize the observer/settlement ticks so each registered+ENABLED chain dispatches to its kind's `run_observer`/`run_settlement`. Add a per-chain enable flag (default OFF) so Solana stays dark until enabled (cycle burn). Determine chain-kind by `ChainId` (501 = Solana) or a `ChainKind` you add to config.
- [ ] **Step 3:** Commit.

## Task 9: PocketIC integration test (deposit -> mint -> withdraw)

**Files:** create `tests/solana_m2_pic.rs`; create/extend a `sol_rpc_mock` canister (mirror `src/monad_rpc_mock`) answering getBalance / getAccountInfo / sendTransaction / getTransaction with scripted values.

- [ ] **Step 1:** Mock SOL RPC canister with setters (`set_balance`, `set_mint_supply`, `set_tx_confirmed`). Install backend + mock; `set_sol_rpc_principal(mock)`; register Solana chain; set SPL mint; set SOL price; provision Ed25519 key.
- [ ] **Step 2:** Open vault -> set custody balance -> tick observer -> assert Mint enqueued -> tick settlement -> assert mint signed+sent+confirmed -> vault Open, `chain_supplies` incremented. Then withdraw -> assert SOL transfer settled. Graceful-degrade if no Ed25519 key.
- [ ] **Step 3:** Commit.

## Task 10: Staging deploy + devnet runbook (operator-gated)

- [ ] Invoke the cycles-management skill; confirm headroom. Solana timers stay OFF by default. Deploy backend (upgrade only) to staging. Register Solana chain (devnet); bootstrap the nonce via `solana_bootstrap_nonce(opt "<fresh finalized blockhash>")` (PLAYBOOK #4: the blockhash override is REQUIRED on real devnet because `getLatestBlockhash` chronically returns `#Inconsistent`; the no-override/`None` path will fail there. Fetch a fresh finalized blockhash, e.g. `solana blockhash` or a single-provider getLatestBlockhash, and call within ~60s before it expires); deploy the icUSD SPL mint on devnet with the settlement address as mint authority (8 decimals), `set_chain_contract`. Exercise open->deposit->mint->withdraw on real devnet. Document results. (Operator-run; not agent-executed.) See also `docs/icp-solana-integration-playbook.md` #4 and the cutover runbook #24.

---

## Self-Review (run before handing off)
- **Spec coverage:** deposits+mint (Tasks 1-4,6-9), withdraw (Tasks 1,4,6,8,9), durable nonce (Task 3), generalization (Task 5), disabled-by-default timers (Task 8), tests (each task + Task 9), staging (Task 10). Burns (notify-then-verify) are M3-prep, intentionally deferred.
- **Decisions honored:** hand-rolled seam, hand-encoded SPL, serde pin, no always-on timers, decimals-aware native units.
- **Ordering:** the signing pipeline (Task 1) is proven before SPL/nonce; F3 (Task 5) precedes the vault endpoints (Task 6).

## Execution
Execute task-by-task with TDD + frequent commits on `feat/solana-integration` (in a worktree). After all tasks, use superpowers:finishing-a-development-branch. Stop and ask if a Solana wire-format detail can't be verified against the crate/example.
