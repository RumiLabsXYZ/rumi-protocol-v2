# Base icUSD bridge: implementation plan

Design: `docs/superpowers/specs/2026-10-24-base-icusd-bridge-design.md`.
Branch: `codex/base-icusd-bridge`, worktree `.worktrees/base-icusd-bridge-20261024`.
Baseline: `origin/main` at `32358d23d98b6430cab0236a3245519c9322fe90`.
Board: `docs/coordination/base-icusd-bridge-kanban.md` (card ids below match it).

Authorized: planning, implementation, local tests (ephemeral keys, local anvil, PocketIC), commits,
pushing this branch, a draft PR. Not authorized: deploy, merge, live signing/broadcast, funds,
controller or role changes.

## 1. Inventory

### Reuse unchanged

| Item | Source | Use |
| --- | --- | --- |
| OZ 5.0.2 `ERC20`, `AccessControlDefaultAdminRules` | `foundry/lib/openzeppelin-contracts` (pinned in `foundry/foundry.lock`) | Base token base classes |
| forge-std 1.16.1 | `foundry/lib/forge-std` | Contract tests and deploy script |
| icUSD ledger Wasm (ICRC-1/2/3) | `src/ledger/ic-icrc1-ledger.wasm`, `src/ledger/ledger.did` | Real ledger in PocketIC tests |
| PocketIC server 7.0.0, crate 6.0.0 | repo root `pocket-ic` (gitignored), `Cargo.lock` | Integration tests; `get_canister_http` / `mock_canister_http_response` for outcall forwarding |
| EVM RPC canister `request` API | `src/rumi_protocol_backend/src/chains/evm/evm_rpc.rs:800-818` (transport pattern) | Production RPC transport |
| Pinned crates: ic-cdk 0.12, candid 0.10, ic-stable-structures 0.6, icrc-ledger-types (fork fc278709), k256 0.13, sha3, alloy-rlp 0.3, ciborium | `Cargo.lock` | Bridge canister |
| Frontend auth, actors, fee cache, Oisy resilience | `src/vault_frontend/src/lib/services/{auth,pnp,oisySigner,ledgerFeeService}.ts`, `protocol/{walletOperations,oisyResilience}.ts` | Bridge page ICP side |
| Declaration generator | `scripts/regenerate-declarations.sh` | New canister bindings (add to arrays) |

### Reuse as a pattern (re-implemented, because the source is coupled to backend `State`)

| Pattern | Source | Bridge module | Change in semantics |
| --- | --- | --- | --- |
| EIP-1559 RLP, signing hash, y-parity recovery | `chains/evm/tx.rs:185-333` | `evm/tx.rs` | Same encoding; local hash recorded per attempt |
| tECDSA address derivation | `chains/evm/tecdsa.rs:46-90` | `evm/signer.rs` | Bridge-specific purpose paths |
| Quorum read (whole-result vote, `max(floor, n/2+1)`) | `chains/evm/evm_rpc.rs:964-995, 1139-1140` | `evm/rpc.rs` | Votes on parsed fields; finalized tag + block hash |
| Ledger idempotency + outcome taxonomy + `TooOld` handling | `management.rs:456-556, 2410-2460, 2689-2737` | `ledger.rs` | Same rules |
| ICRC-3 decode and exact tuple validation, archive range check | `icrc3_proof.rs:164-241, 917-937, 1146-1337` | `ledger.rs` | Archive callbacks limited to `icrc3_get_archives` canisters |
| Stable layout with uniqueness test | `src/rumi_cycle_sentinel/src/state.rs:13-160` | `state.rs` | Same approach |
| ICRC-21 consent messages | `src/rumi_protocol_backend/src/icrc21.rs:340-790` | `icrc21.rs` | Bridge methods |
| Hand-rolled EIP-1193 flow, pending-tx lock | `src/vault_frontend/src/routes/ckusdc/+page.svelte:25-500` | `lib/services/baseBridge*.ts` | Base chain switching added (pattern from `src/conflux_espace_frontend/src/evm.ts:161-182`) |

### Changed

| File | Change |
| --- | --- |
| `Cargo.toml` | Add workspace member `src/rumi_icusd_bridge` |
| `Cargo.lock` | New package entry only (no version changes) |
| `scripts/regenerate-declarations.sh` | Add `rumi_icusd_bridge` to both arrays |
| `src/vault_frontend/src/lib/config.ts` | `CANISTER_IDS.ICUSD_BRIDGE` (empty), `BRIDGE_ENABLED`, Base network constants |
| `src/vault_frontend/src/lib/services/pnp.ts` | Add bridge canister to delegation targets when set |
| `src/vault_frontend/src/routes/+layout.svelte` | Bridge nav and footer links behind `BRIDGE_ENABLED` |
| `icp.yaml` | `rumi_icusd_bridge` canister entry (local/staging only; no mainnet mapping) |

Unchanged by design: `src/rumi_protocol_backend/**`, `foundry/src/IcUSD.sol`, `foundry/test/IcUSD.t.sol`,
`foundry/script/DeployIcUSD.s.sol`, stability pool, treasury, analytics, liquidation bot.

### New

`foundry/src/BridgedIcUSD.sol`, `foundry/test/BridgedIcUSD.t.sol`, `foundry/script/DeployBridgedIcUSD.s.sol`,
`foundry/test/DeployBridgedIcUSD.t.sol`, `foundry/DEPLOY-BASE.md`; crate `src/rumi_icusd_bridge/**`
(including `rumi_icusd_bridge.did` and `tests/`); test canister `src/rumi_icusd_bridge/tests/evm_rpc_relay/`;
`src/declarations/rumi_icusd_bridge/**` (generated); frontend `src/vault_frontend/src/routes/bridge/`,
`src/vault_frontend/src/lib/services/baseBridge*.ts` and specs; `docs/coordination/*`.

## 2. Fixed interfaces

Owned by the coordinator. Workers must not change these without a board note.

### 2.1 Contract ABI (pinned values are produced by BB-02 and asserted in Rust and TS)

```
mint(address,uint256,bytes32)                              MINTER_ROLE
bridgeOut(uint256,bytes,bytes32,bytes32) returns (bytes32)
computeTransferId(address,bytes32) view returns (bytes32)
mintConsumed(bytes32), bridgeOutUsed(bytes32), bridgeOutCount(), totalSupply(), supplyCap(),
mintPaused(), bridgeOutPaused(), VERSION(), decimals(), hasRole(bytes32,address), defaultAdmin()
pauseMint() unpauseMint() pauseBridgeOut() unpauseBridgeOut() setSupplyCap(uint256)
setMintRateLimit(uint256,uint64) setMinBridgeOut(uint256)
event BridgeMint(bytes32 indexed mintId, address indexed to, uint256 amount)
event BridgeOut(bytes32 indexed transferId, address indexed from, uint256 amount, uint64 sequence,
                bytes icpPrincipal, bytes32 icpSubaccount)
```

### 2.2 Canister modules and owners

| Module | Owner card | Responsibility |
| --- | --- | --- |
| `types.rs` | BB-03 coordinator | Candid public types |
| `state.rs` | BB-03 coordinator | Stable layout, records, config, counters |
| `accounting.rs` | BB-03 coordinator | Bucket transitions, C1/C2/C3 checks (pure) |
| `ids.rs` | BB-03 coordinator | Mint/re-mint ids, memos, subaccounts |
| `evm/abi.rs`, `evm/tx.rs`, `evm/signer.rs` | BB-04 | Calldata, log decoding, RLP, signing |
| `evm/rpc.rs`, `observer.rs` | BB-05 | RPC transport, quorum, finality, bridge-out scan |
| `evm/lane.rs` | BB-06 | Nonce lanes, attempts, replacements, finality, cancel |
| `ledger.rs`, `flows/{deposit,release}.rs` | BB-07 | ICRC pull/transfer, outcome taxonomy, ICRC-3 search, flow steps |
| `lib.rs`, `admin.rs`, `icrc21.rs`, `.did` | BB-08 coordinator | Endpoints, timer, admin, consent, Candid test |

### 2.3 Candid surface

The authoritative file is `src/rumi_icusd_bridge/rumi_icusd_bridge.did`, generated from Rust by
`export_candid!` and checked by `candid_interface_matches_checked_in_did`. Method list is in design
section 7.10. Record shapes are fixed in `types.rs` by BB-03.

## 3. Tasks

Each task lists owner (requested model), files, dependencies, acceptance, and commands. All cargo
commands run from the worktree root with `--offline`.

### BB-02 Base contract (Sonnet)
- Files: `foundry/src/BridgedIcUSD.sol`, `foundry/test/BridgedIcUSD.t.sol`,
  `foundry/script/DeployBridgedIcUSD.s.sol`, `foundry/test/DeployBridgedIcUSD.t.sol`.
- Depends on: BB-R1.
- Acceptance: all Foundry tests pass including the 14 existing ones; ABI pins table; runtime size
  below 24,576 bytes; `IcUSD.sol` byte-identical.
- Verify: `cd foundry && PATH=$HOME/.foundry/bin:$PATH forge test --offline` and
  `forge build --sizes --offline`.

### BB-03 Core types, state, accounting (coordinator)
- Files: `src/rumi_icusd_bridge/src/{types,state,accounting,ids}.rs`, stubs for other modules.
- Acceptance: crate builds for wasm32 and host; unit tests for every transition in design 7.2
  (exact e8s, boundaries, overflow, insufficient balance, double-apply rejection), invariant checks,
  stable-layout uniqueness, CBOR round-trip and missing-field decode.
- Verify: `cargo test --offline -p rumi_icusd_bridge --lib`.

### BB-04 EVM primitives (Sonnet)
- Files: `src/rumi_icusd_bridge/src/evm/{abi,tx,signer}.rs`.
- Depends on: BB-02 pins, BB-03.
- Acceptance: calldata and log decoding match the Foundry pins and a forge-produced raw `BridgeOut`
  log; RLP/signing-hash/raw-tx match `cast mktx` output for a fixed key; y-parity recovery; address
  derivation; strict decoding rejects wrong topic counts, wrong address, dynamic-offset tricks,
  oversize values, non-canonical padding.
- Verify: `cargo test --offline -p rumi_icusd_bridge --lib evm::`.

### BB-05 RPC transport, finality, observer (Sonnet)
- Files: `src/rumi_icusd_bridge/src/evm/rpc.rs`, `src/rumi_icusd_bridge/src/observer.rs`.
- Depends on: BB-03, BB-04.
- Acceptance: quorum over parsed values; finalized head + block hash agreement; `eth_call` at a
  block hash or number; log scan with sequence-completeness proof; no cursor advance on gaps,
  inconsistency, `removed: true`, wrong address or topic; bounded chunking and response caps.
- Verify: `cargo test --offline -p rumi_icusd_bridge --lib observer:: evm::rpc::`.

### BB-06 Transaction lanes (Sonnet)
- Files: `src/rumi_icusd_bridge/src/evm/lane.rs`.
- Depends on: BB-03, BB-04, BB-05.
- Acceptance: pure state machine tests for sign-reply-lost, broadcast error, receipt on an older
  attempt after replacement, reorg before finality, finalized revert, nonce consumed without a known
  attempt (halt), cancel attempt winning or losing, gas shortfall, fee caps.
- Verify: `cargo test --offline -p rumi_icusd_bridge --lib evm::lane::`.

### BB-07 Ledger and flows (Sonnet)
- Files: `src/rumi_icusd_bridge/src/ledger.rs`, `src/rumi_icusd_bridge/src/flows/{mod,deposit,release}.rs`.
- Depends on: BB-03.
- Acceptance: outcome taxonomy tests (first-attempt no-effect, ambiguous then no-effect stays unknown,
  `Duplicate`, `BadFee`, `TooOld`); ICRC-3 decode and exact tuple match; archive callback restricted
  to listed archives; bounded search with complete-window non-inclusion; flow steps update buckets
  only through `accounting.rs`.
- Verify: `cargo test --offline -p rumi_icusd_bridge --lib ledger:: flows::`.

### BB-08 Endpoints, admin, ICRC-21, Candid (coordinator)
- Files: `src/rumi_icusd_bridge/src/{lib,admin,icrc21}.rs`, `src/rumi_icusd_bridge/rumi_icusd_bridge.did`,
  `scripts/regenerate-declarations.sh`, `src/declarations/rumi_icusd_bridge/**` (generated), `icp.yaml`.
- Depends on: BB-03..BB-07.
- Acceptance: Candid equality test; anonymous and non-admin rejection tests; ICRC-21 for user methods;
  declarations regenerated by the script (not hand-edited); release Wasm builds and size reported.
- Verify: `cargo test --offline -p rumi_icusd_bridge --lib`, `cargo build --offline --release
  --target wasm32-unknown-unknown -p rumi_icusd_bridge`, `bash scripts/regenerate-declarations.sh rumi_icusd_bridge`.

### BB-09 Integration tests (Sonnet)
- Files: `src/rumi_icusd_bridge/tests/evm_rpc_relay/**` (test canister crate),
  `src/rumi_icusd_bridge/tests/bridge_e2e_pic.rs`, `src/rumi_icusd_bridge/tests/support/**`.
- Depends on: BB-02, BB-08.
- Acceptance (real ledger, real tECDSA, real anvil execution): ICP -> Base -> ICP round trip; duplicate
  `client_request_id`; lost reply after pull; reply lost after broadcast; replacement; finalized
  revert then refund; reorg before finality (anvil snapshot/revert); inconsistent provider (harness
  fault injection, labeled mocked); bridge-out to an invalid principal held and re-minted; pause and
  cap behavior; upgrade mid-flight with outstanding liabilities; C1/C2/C3 reported true at the end.
- Verify: `POCKET_IC_BIN=<repo>/pocket-ic cargo test --offline -p rumi_icusd_bridge --test bridge_e2e_pic -- --test-threads=1`.

### BB-10 Frontend Bridge page (Sonnet)
- Files: `src/vault_frontend/src/routes/bridge/**`, `src/vault_frontend/src/lib/services/baseBridge*.ts`
  and `*.spec.ts`, `src/vault_frontend/src/lib/components/bridge/**`, plus the small config, pnp and
  layout edits listed above.
- Depends on: BB-02 pins, BB-08 declarations (can start from the draft `.did`).
- Acceptance: vitest specs for both flow state machines, persistence/recovery, wrong network,
  rejected approval, account change, allowance shortfall, insufficient gas, duplicate prevention;
  `npm run check` and `npm run build` pass for vault_frontend.
- Verify: `cd src/vault_frontend && npx vitest --run src/lib/services/baseBridge` and `npm run check`.

### BB-11 Docs and runbook (Haiku for mechanical parts)
- Files: `foundry/DEPLOY-BASE.md`, runbook section in the handoff.
- Depends on: BB-02, BB-08.
- Acceptance: public-testnet runbook covers derivation, deployment, funding, binding, verification and
  rollback, all marked as held actions.

### BB-12 and BB-13 Independent adversarial reviews (Sonnet, two different reviewers)
- BB-12: accounting, authority and replay (security persona).
- BB-13: state, Candid, upgrade, frontend wallet flow (migration and frontend personas).
- Acceptance: every finding has a disposition (fixed with test, or rejected with evidence).

### BB-14 Branch, PR (coordinator)
- Push `codex/base-icusd-bridge`, open a draft PR, update the handoff.

## 4. Migration and compatibility

- New canister: no existing state to migrate. Records are CBOR with `#[serde(default)]` for future
  additive fields; the `MemoryId` layout is append-only and tested for uniqueness.
- No change to backend state, events, Candid or declarations. `git diff origin/main --
  src/rumi_protocol_backend` must be empty at PR time.
- Existing contract consumers (backend decoders for `IcUSD`) are untouched; `IcUSD.sol` is unchanged.
- Generated declarations for the new canister come only from `scripts/regenerate-declarations.sh`.

## 5. Verification gates for the PR

1. `forge test --offline` (all suites).
2. `cargo test --offline -p rumi_icusd_bridge --lib`.
3. `cargo build --offline --release --target wasm32-unknown-unknown -p rumi_icusd_bridge` and size.
4. PocketIC + anvil suite (BB-09).
5. Backend regression: backend source unchanged; `cargo test --offline -p rumi_protocol_backend --lib
   chains::` if disk allows (about 12 GiB free at start), otherwise reported as skipped with reason.
6. vitest for bridge specs, `npm run check` (needs `npm ci`).
7. Two adversarial reviews with dispositions.

## 6. Held live actions (runbook only)

Deriving production addresses on mainnet, deploying `BridgedIcUSD` to Base Sepolia or Base,
funding gas, creating or installing the canister on any network, binding the contract, assigning
roles, and enabling deposits.
