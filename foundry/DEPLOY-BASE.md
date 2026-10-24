# Base icUSD bridge: public-testnet runbook (Base Sepolia)

**Read first.** Every step marked **HELD** needs its own explicit authorization from the protocol owner before anyone runs it. Nothing in this document has been executed. The run uses a separate test icUSD ledger, so no real funds are involved.

Status: draft, 2026-10-24. Checked against the deploy script, `rumi_icusd_bridge.did`, `types.rs` and the bridge source.

## 1. What this run proves and what it does not

It proves three things local tests cannot: real Base Sepolia finality (the `finalized` head and its block hash), real EVM RPC providers (three public Base Sepolia endpoints through the EVM RPC canister `7hfb6-caaaa-aaaar-qadga-cai`, 2 of 3 must agree), and real threshold signing on IC mainnet with `test_key_1`.

It does not prove mainnet readiness. Local tests already cover the accounting, lane and contract logic. Section 14 lists the mainnet differences, each needing its own approval.

## 2. Prerequisites

1. Foundry on PATH, with `foundry/lib` installed (`forge install`) for offline builds:
   ```bash
   export PATH="$HOME/.foundry/bin:$PATH"
   ```
2. icp-cli pinned to 1.3.0. Every `$ICP` below means this:
   ```bash
   ICP="npx -y @icp-sdk/icp-cli@1.3.0"
   ```
3. `ic-wasm`, `didc`, `cast` and `shasum` on PATH. `rumi_identity` must control both new canisters. The tester uses a separate identity (`<tester identity>`), because the ICP pull comes from the caller's own account.
4. Cycles (estimates, re-measure before the run): about 26.15 billion per `sign_with_ecdsa` on `key_1` (the backend design notes record `26,153,846,153` for ic-cdk 0.12.2; the `test_key_1` cost is not measured). About 2 billion per EVM RPC provider call (the bridge default `cycles_per_rpc_call`). Plan for at least 5 trillion on the bridge canister for a test run, as a planning figure.
5. Deployer key: a fresh Base Sepolia account used only for the deploy, held in a foundry keystore, never in an environment variable or a file. The script refuses a deployer that is the admin, minter or guardian.
   ```bash
   cast wallet import base-sepolia-deployer --interactive
   ```
6. Choose `<ic-env>`, the icp-cli environment for the new canisters. It must target network `ic` and must not be `mainnet-live`. Confirm a subnet with `test_key_1` enabled (not checked here).

Note: `icp.yaml` has no bridge entry, so confirm with `--help` how icp-cli 1.3.0 handles a canister outside the manifest. The icp-cli flags below follow this repo's usage; confirm each before a HELD step.

## 3. Build and record artifacts

Do not run any HELD step from a dirty git tree. Artifact hashes must match a commit.

1. Offline Foundry build:
   ```bash
   cd foundry && forge build --offline --sizes
   ```
2. Canister Wasm, from the repo root:
   ```bash
   cargo build --release --target wasm32-unknown-unknown --locked -p rumi_icusd_bridge
   ```
3. Candid metadata (output goes to `$HOME/base-bridge-run`, not the repo):
   ```bash
   mkdir -p "$HOME/base-bridge-run" && ic-wasm target/wasm32-unknown-unknown/release/rumi_icusd_bridge.wasm -o "$HOME/base-bridge-run/rumi_icusd_bridge.wasm" metadata candid:service -f src/rumi_icusd_bridge/rumi_icusd_bridge.did -v public
   ```
4. Shrink:
   ```bash
   ic-wasm "$HOME/base-bridge-run/rumi_icusd_bridge.wasm" -o "$HOME/base-bridge-run/rumi_icusd_bridge.wasm" shrink --keep-name-section
   ```
5. Hash the artifacts, then record the commit with `git rev-parse HEAD`:
   ```bash
   shasum -a 256 "$HOME/base-bridge-run/rumi_icusd_bridge.wasm" src/ledger/ic-icrc1-ledger.wasm foundry/out/BridgedIcUSD.sol/BridgedIcUSD.json
   ```

## 4. HELD: create the two canisters on IC mainnet

Creation is irreversible and costs cycles.

1. Test icUSD ledger:
   ```bash
   $ICP canister create test-icusd-ledger --environment <ic-env> --identity rumi_identity --cycles 1000000000000
   ```
2. Bridge canister (5 trillion cycles for a test run):
   ```bash
   $ICP canister create rumi-icusd-bridge-test --environment <ic-env> --identity rumi_identity --cycles 5000000000000
   ```

## 5. HELD: install the test ledger

Install once. The operator is the minting account. The tester gets 1,000 test icUSD (100000000000 e8s). Fee 10,000 e8s, decimals 8, ICRC-2 on. The init shape follows `src/ledger/ledger.did`. The tester balance should then read `100_000_000_000 : nat`.

1. Install:
   ```bash
   $ICP canister install test-icusd-ledger --environment <ic-env> --identity rumi_identity --mode install --wasm src/ledger/ic-icrc1-ledger.wasm --args '(variant { Init = record { minting_account = record { owner = principal "<operator>" }; transfer_fee = 10_000 : nat; decimals = opt (8 : nat8); token_symbol = "TICUSD"; token_name = "Test icUSD (Base bridge run)"; metadata = vec {}; initial_balances = vec { record { record { owner = principal "<tester>"; subaccount = null }; 100_000_000_000 : nat } }; feature_flags = opt record { icrc2 = true }; archive_options = record { num_blocks_to_archive = 1_000 : nat64; trigger_threshold = 2_000 : nat64; controller_id = principal "<operator>" } } })'
   ```

## 6. HELD: install the bridge (deposits start paused)

Install once, never reinstall (section 12). Init pauses deposits only. Mints, releases and the observer start unpaused, but no mint is possible until a contract is bound. `fee_recipient = null` means fee sweeps are refused. Check `pauses` in `get_bridge_info`.

```bash
$ICP canister install rumi-icusd-bridge-test --environment <ic-env> --identity rumi_identity --mode install --wasm "$HOME/base-bridge-run/rumi_icusd_bridge.wasm" --args '(variant { Init = record { icusd_ledger = principal "<test ledger id>"; evm_rpc = principal "7hfb6-caaaa-aaaar-qadga-cai"; ecdsa_key_name = "test_key_1"; base_chain_id = 84532 : nat64; admins = vec { principal "<operator>" }; fee_recipient = null; rpc_services = vec { variant { Custom = record { url = "https://sepolia.base.org" } }; variant { Custom = record { url = "https://base-sepolia-rpc.publicnode.com" } }; variant { Custom = record { url = "https://base-sepolia.drpc.org" } } }; min_quorum = 2 : nat8 } })'
```

## 7. HELD: derive the Base addresses

Note: `get_base_addresses` derives the minter and admin public keys (`ecdsa_public_key`) and caches the addresses. It does not sign anything and sends no transaction to Base.

```bash
$ICP canister call rumi-icusd-bridge-test get_base_addresses '()' --environment <ic-env> --identity rumi_identity
```
Record `minter` and `admin` for `BRIDGE_MINTER_ADDR` and `BRIDGE_ADMIN_ADDR` in step 9.

## 8. HELD funding: Base Sepolia test ETH

Send 0.05 test ETH each to the minter (mint gas plus the Base L1 data fee), the admin (contract admin actions) and the deployer (deploy gas). These are suggestions; re-check against current testnet fees. Gas is operator funding and never comes from escrow. Verify with `cast balance <address> --rpc-url https://sepolia.base.org`.

## 9. HELD broadcast: deploy BridgedIcUSD

Run variables (non-secret; the key stays in the keystore). Supply cap 1,000 icUSD, mint window 500 icUSD per 86,400 seconds, minimum bridge-out 1 icUSD, admin delay one day, no guardian.

```bash
export EXPECTED_CHAIN_ID=84532 BRIDGE_ADMIN_DELAY_SECONDS=86400 BRIDGE_SUPPLY_CAP_E8S=100000000000 BRIDGE_MINT_WINDOW_LIMIT_E8S=50000000000 BRIDGE_MINT_WINDOW_SECONDS=86400 BRIDGE_MIN_BRIDGE_OUT_E8S=100000000
export BRIDGE_ADMIN_ADDR=<admin from step 7> BRIDGE_MINTER_ADDR=<minter from step 7>
unset BRIDGE_GUARDIAN_ADDR ALLOW_BASE_MAINNET
```

Dry run (simulation only, no `--broadcast`):
```bash
cd foundry && forge script script/DeployBridgedIcUSD.s.sol:DeployBridgedIcUSD --rpc-url https://sepolia.base.org --account base-sepolia-deployer
```
HELD broadcast (same command plus `--broadcast`):
```bash
cd foundry && forge script script/DeployBridgedIcUSD.s.sol:DeployBridgedIcUSD --rpc-url https://sepolia.base.org --account base-sepolia-deployer --broadcast
```
Record the `BridgedIcUSD deployed at:` address and deploy tx hash from `foundry/broadcast/DeployBridgedIcUSD.s.sol/84532/run-latest.json`. The script re-checks the deployed state and reverts on a mismatch, but the contract stays deployed, so never bind it after a failed check. Wait about 20 minutes for Base L1 batch finality before step 10, because binding reads the finalized head.

## 10. HELD: configure and bind

0. Read-only checks first. Bridge read methods are queries: call them with `--query`. The bridge refuses a query name sent as an update call (its ingress filter only admits update methods), and `icp canister call` sends an update unless `--query` is given. The ledger must report 8 decimals and the expected symbol (the bridge does not verify ledger metadata itself), and `get_bridge_info` must show `mint_gas_limit` of at least 250,000 (default 300,000) and `admin_gas_limit` of at least 80,000 (default 100,000). The most expensive mint measured on anvil uses 152,390 gas.
   ```bash
   $ICP canister call <ledger id> icrc1_decimals '()' --query --environment <ic-env>
   $ICP canister call <ledger id> icrc1_symbol '()' --query --environment <ic-env>
   $ICP canister call rumi-icusd-bridge-test get_bridge_info '()' --query --environment <ic-env>
   ```
1. Set limits inside the contract limits: `max_outstanding_e8s` at most the supply cap, `mint_window_limit_e8s` at most the contract window limit, `window_seconds` at least the contract window, and `min_deposit_e8s` at least the contract minimum bridge-out.
   ```bash
   $ICP canister call rumi-icusd-bridge-test admin_set_limits '(record { min_deposit_e8s = 100_000_000 : nat64; max_deposit_e8s = 1_000_000_000 : nat64; max_outstanding_e8s = 10_000_000_000 : nat64; mint_window_limit_e8s = 10_000_000_000 : nat64; release_window_limit_e8s = 10_000_000_000 : nat64; window_seconds = 86_400 : nat64; max_active_per_caller = 5 : nat32; max_active_global = 500 : nat32 })' --environment <ic-env> --identity rumi_identity
   ```
2. Bind, once only. It refuses unless no contract is bound, all accounting buckets are zero, and no deposit or release is active. At the finalized head it reads `VERSION` (`rumi-base-icusd-bridge-v1`), `decimals` (8), `totalSupply` (0) and `bridgeOutCount` (0). The minter must hold `MINTER_ROLE`, the admin must not, and `defaultAdmin()` must be the admin. It refuses if the step 1 limits exceed the contract's.
   ```bash
   $ICP canister call rumi-icusd-bridge-test admin_bind_contract '("<BridgedIcUSD address from step 9>")' --environment <ic-env> --identity rumi_identity
   ```
3. Unpause deposits. `admin_set_pauses` replaces all four flags, so send all four. It is refused before binding and while halted.
   ```bash
   $ICP canister call rumi-icusd-bridge-test admin_set_pauses '(record { deposits = false; mints = false; releases = false; observer = false })' --environment <ic-env> --identity rumi_identity
   ```

## 11. HELD verification (each sub-step moves test tokens or changes state)

Run each sub-step only after its own approval.

**A. ICP to Base**

1. Approve 5 icUSD plus the 10,000 e8s ledger fee, as the tester:
   ```bash
   $ICP canister call test-icusd-ledger icrc2_approve '(record { spender = record { owner = principal "<test bridge id>" }; amount = 500_010_000 : nat })' --environment <ic-env> --identity <tester identity>
   ```
2. Request the deposit. Pass `fees.deposit_fee_e8s` from `get_bridge_info` as `expected_deposit_fee_e8s`. The client request ID is 16 bytes.
   ```bash
   $ICP canister call rumi-icusd-bridge-test bridge_to_base '(record { amount_e8s = 500_000_000 : nat64; base_recipient = "<tester Base address>"; from_subaccount = null; client_request_id = blob "test-run-0000001"; expected_deposit_fee_e8s = 0 : nat64 })' --environment <ic-env> --identity <tester identity>
   ```
3. Poll with the `id` from step 2 until the status is `Delivered`:
   ```bash
   $ICP canister call rumi-icusd-bridge-test get_deposit '(<deposit id>)' --query --environment <ic-env> --identity <tester identity>
   ```
4. Check Base. The balance should be 500,000,000 minus the deposit fee:
   ```bash
   cast call <contract address> "balanceOf(address)(uint256)" <tester Base address> --rpc-url https://sepolia.base.org
   ```
   ```bash
   cast call <contract address> "totalSupply()(uint256)" --rpc-url https://sepolia.base.org
   ```
5. Check coverage: in `get_supply_audit`, `coverage_ok` and `base_supply_within_bound` must both be `opt true`.
   ```bash
   $ICP canister call rumi-icusd-bridge-test get_supply_audit '()' --query --environment <ic-env> --identity rumi_identity
   ```
6. Duplicate check: repeat step 2 with identical arguments. The same deposit ID comes back and the ledger balance does not change. A different amount under the same ID must return `RequestIdConflict`.

**B. Base to ICP**

1. Create a random 32-byte nonce and compute the transfer ID before sending. Save both first:
   ```bash
   openssl rand -hex 32
   ```
   ```bash
   cast call <contract address> "computeTransferId(address,bytes32)(bytes32)" <tester Base address> 0x<nonce> --rpc-url https://sepolia.base.org
   ```
2. Bridge out 2 icUSD from the tester's Base test key. `icpPrincipal` is the raw principal bytes as hex, not the text form. The subaccount is 32 zero bytes (the default account):
   ```bash
   cast send <contract address> "bridgeOut(uint256,bytes,bytes32,bytes32)" 200000000 0x<raw principal hex> 0x0000000000000000000000000000000000000000000000000000000000000000 0x<nonce> --rpc-url https://sepolia.base.org --account <tester Base keystore>
   ```
3. Optional UI hint (it never settles anything):
   ```bash
   $ICP canister call rumi-icusd-bridge-test notify_base_bridge_out '("<bridgeOut tx hash>")' --environment <ic-env> --identity <tester identity>
   ```
4. Wait about 20 minutes for finality, then poll until the status is `Released`:
   ```bash
   $ICP canister call rumi-icusd-bridge-test get_release '("<transfer id>")' --query --environment <ic-env> --identity rumi_identity
   ```
5. The tester's ICP balance should rise by 199,990,000 (200,000,000 minus the 10,000 ledger fee, with release fee 0):
   ```bash
   $ICP canister call test-icusd-ledger icrc1_balance_of '(record { owner = principal "<tester>"; subaccount = null })' --environment <ic-env> --identity rumi_identity
   ```

**C. Negative checks**

1. Deposits paused: set `deposits = true` (other flags unchanged). A new `bridge_to_base` must return `Paused`. Then set `deposits = false` again.
2. Mint hold and retry: pause Base mints, then make deposit B with a new request ID. It reports `MintHeld` with a reason, because the mint preflight simulation reverts.
   ```bash
   $ICP canister call rumi-icusd-bridge-test admin_submit_contract_action '(variant { PauseMint })' --environment <ic-env> --identity rumi_identity
   ```
   Unpause with `variant { UnpauseMint }` (same shape), then retry B:
   ```bash
   $ICP canister call rumi-icusd-bridge-test admin_retry_deposit '(<deposit B id>)' --environment <ic-env> --identity rumi_identity
   ```
3. Refund: pause mints again, make deposit C (it reports `MintHeld`), then request its refund. Expect `refund_amount_e8s` of 499,990,000 (500,000,000 minus the ledger fee) back to the tester's ICP account. Unpause mints afterwards:
   ```bash
   $ICP canister call rumi-icusd-bridge-test request_deposit_refund '(<deposit C id>)' --environment <ic-env> --identity <tester identity>
   ```

Note: design section 7.10 names `admin_retry_operation` and `list_releases_for_account`. The `.did` has `admin_retry_deposit` and `list_releases_for_owner`, which this runbook uses.

## 12. HELD stop and rollback

1. Pause everything. Escrow and liabilities stay in place, signed attempts are kept, and reconciliation keeps running:
   ```bash
   $ICP canister call rumi-icusd-bridge-test admin_set_pauses '(record { deposits = true; mints = true; releases = true; observer = true })' --environment <ic-env> --identity rumi_identity
   ```
2. Pause the Base contract for mints and bridge-outs:
   ```bash
   $ICP canister call rumi-icusd-bridge-test admin_submit_contract_action '(variant { PauseMint })' --environment <ic-env> --identity rumi_identity
   ```
   ```bash
   $ICP canister call rumi-icusd-bridge-test admin_submit_contract_action '(variant { PauseBridgeOut })' --environment <ic-env> --identity rumi_identity
   ```
3. If the bridge is halted, `admin_clear_halt` only after the section 13 checks pass again.
4. Upgrades only. Never reinstall, because a reinstall wipes the liability records. Stop, upgrade, then start, so no call is in flight:
   ```bash
   $ICP canister stop rumi-icusd-bridge-test --environment <ic-env> --identity rumi_identity
   ```
   ```bash
   $ICP canister install rumi-icusd-bridge-test --environment <ic-env> --identity rumi_identity --mode upgrade --wasm "$HOME/base-bridge-run/rumi_icusd_bridge.wasm" --args '(variant { Upgrade = record { admins = null } })'
   ```
   ```bash
   $ICP canister start rumi-icusd-bridge-test --environment <ic-env> --identity rumi_identity
   ```

## 13. Evidence to capture

- Commit, clean-tree result, `shasum -a 256` output, canister IDs, environment, controllers and admins.
- Base contract address, deploy tx hash, minter and admin addresses, funding tx hashes, and every `bridgeOut` tx hash.
- `get_bridge_info` and `get_supply_audit` output before and after each stage, saved to files, plus deposit and release views.
- The full audit log, paged with `get_audit_log(start, limit)` until a page comes back empty.

## 14. Differences for a future mainnet run (each needs separate approval)

- Key `key_1`. Init validation rejects `test_key_1` for chain 8453 (accepted only for 84532 and local anvil).
- `base_chain_id = 8453`, with `ALLOW_BASE_MAINNET=true` (the script requires exactly `true`).
- `BaseMainnet` RPC providers (Alchemy, Ankr, BlockPi, PublicNode, Llama) through the EVM RPC canister, with `min_quorum >= 2`.
- The real icUSD ledger `t6bor-paaaa-aaaap-qrd5q-cai` instead of a test ledger, with conservative caps.
- Controller and admin authority: who holds controller status, and whether admin is a governed principal rather than `rumi_identity`.
- The pre-deploy test hook (`.claude/hooks/pre-deploy-test.sh`) applies to mainnet deploys.
- Repeat the "first mint after the contract mint window has elapsed" check on Base Sepolia (wait one contract mint window after deploying, then bridge to a recipient whose address has no zero bytes) and confirm the receipt's `gasUsed` stays below 80% of `mint_gas_limit`.
- Watch `lanes[].awaiting_gas` and the observer's deferral fields in `get_bridge_info`; a funded minter is a precondition for every delivery, and the page tells users when deliveries wait for gas.
