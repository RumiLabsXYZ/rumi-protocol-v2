# Base icUSD bridge: Base Sepolia deployment packet

The single readable plan for one isolated Base Sepolia test of the bridge. It supersedes the
per-step HELD wording of `foundry/DEPLOY-BASE.md` for this run only.

## 0. Authority, scope, executor

- **Authority:** Rob, 2026-10-24 (relayed and verified by Miles): fix the three PR #591
  follow-ups, then get the bridge ready for Base Sepolia testing, "deploy whenever you're ready".
- **In scope:** a NEW synthetic test icUSD ledger and a NEW dedicated `rumi_icusd_bridge` canister
  on ICP, and a NEW `BridgedIcUSD` contract on Base Sepolia (chain id 84532) only, with test assets,
  operating gas and escrow kept apart from everything else.
- **Out of scope (unchanged holds):** the production icUSD ledger `t6bor-paaaa-aaaap-qrd5q-cai`, every
  existing Rumi canister, frontend and configuration, Base mainnet (8453), real user icUSD,
  production controllers, roles or governance, merging PR #591, buying ICP, cycles or tokens.
- **Executor:** Miles is the ONE public executor, after validating this packet. Nobody else
  creates, installs, signs, broadcasts or funds anything for this run.
- **HELD inside the run:** provisioning a test-only Base Sepolia deployer signer (an approval
  review refused creating one; Rob's explicit answer is pending). Steps F, I.4 and J.2 need it.
  Everything before F can run without it.

## 1. Candidate and artifacts

| Item | Value |
| --- | --- |
| Code candidate | `4169feb8` (branch `codex/base-icusd-bridge`; later commits are docs only) |
| Bridge Wasm (cargo release, clean tree) | sha256 `a0049b9ae79a81084121fdad43249dadc413e87404d1e7462db4cd41607ae157`, 3,853,783 B |
| Bridge install artifact (candid metadata, then shrink) | sha256 `0b324387251dbb8842d3671ab00a70a5b6ccf5fffd2a47727639e11edc6f929a`, 3,225,447 B |
| Test ledger Wasm `src/ledger/ic-icrc1-ledger.wasm` | sha256 `cb0c3233ebb137f606d7928e90bfd907fe932a941f11b41c23cf0d6cf9c64802` |
| `BridgedIcUSD` creation bytecode (forge build) | sha256 `b780d317624e9a82defd5ca4bddc3ed8993e024bf931ad146ff095ba728267a4`, 10,016 B |
| `BridgedIcUSD` runtime bytecode | sha256 `0dd0612c45e19fa72ca2dd6b2a4e8234b9964b39f81dcdc5b4a7b865cceb4ec8`, 8,266 B; contract source unchanged since `e4213be1` |

Reproduce from a clean checkout of the candidate (repo root), then compare every hash:

```bash
cargo build --offline --release --target wasm32-unknown-unknown -p rumi_icusd_bridge
mkdir -p <RUN_DIR>
ic-wasm target/wasm32-unknown-unknown/release/rumi_icusd_bridge.wasm -o <RUN_DIR>/rumi_icusd_bridge.wasm metadata candid:service -f src/rumi_icusd_bridge/rumi_icusd_bridge.did -v public
ic-wasm <RUN_DIR>/rumi_icusd_bridge.wasm -o <RUN_DIR>/rumi_icusd_bridge.wasm shrink --keep-name-section
shasum -a 256 target/wasm32-unknown-unknown/release/rumi_icusd_bridge.wasm <RUN_DIR>/rumi_icusd_bridge.wasm src/ledger/ic-icrc1-ledger.wasm
(cd foundry && forge build --offline)
```

`<RUN_DIR>` is an absolute path outside the repository (for example a `base-bridge-run` folder in
the operator's home); icp-cli does not expand `$HOME` inside `icp.yaml`.

## 2. Targets

| Target | Value |
| --- | --- |
| ICP network | `ic` |
| Subnet for both new canisters | `fuqsr-in2lc-zbcjj-ydmcw-pzq7h-4xm2z-pto4i-dcyee-5z4rz-x63ji-nae` (13-node application subnet, one of the cycles minting canister's 27 default subnets, hosts `test_key_1`; creation fee 500B each) |
| Threshold ECDSA key | `test_key_1` (10B cycles per signature; `ecdsa_public_key` is free). Bridge init accepts it for 84532 only, never for 8453 |
| EVM RPC canister | `7hfb6-caaaa-aaaar-qadga-cai` (34-node subnet `pzp6e`); Custom providers required for Base Sepolia |
| Base chain | Base Sepolia, chain id 84532 (`0x14a34`) |
| Providers | `https://base-sepolia-rpc.publicnode.com`, `https://base-sepolia.gateway.tenderly.co`, `https://84532.rpc.thirdweb.com`; `min_quorum` 2 (bridge maximum is 5 providers) |

Provider choice: all six public endpoints probed agree on the finalized block and hash, but
`eth_getLogs` span limits differ (publicnode and Tenderly at least 10,000 blocks, thirdweb 1,000,
sepolia.base.org 200, drpc 100). With these three, every 1,000-block log chunk is answered by all
three, so a release scan of up to an hour of 200 ms blocks is about 18 chunks instead of about 180.

## 3. Identities, roles and signing domains

| Role | Holder |
| --- | --- |
| Controller of both new canisters | `rumi_identity`, principal `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae` (only controller; check after creation) |
| Bridge admin (`admins`) | the same principal (testnet-only simplification: one operator holds controller and admin) |
| Test ledger minting account | `rumi_identity` default account (transfers from it are mints) |
| Tester on ICP | `rumi_identity` subaccount `0x00...01` (31 zero bytes, then `01`); no new ICP identity or key |
| Bridge minter and admin on Base | threshold-ECDSA addresses derived by the new bridge canister (`get_base_addresses`); derivation path `[b"rumi-icusd-base-bridge", b"v1", purpose, chain_id]`, so they are bound to the new canister's principal and never equal any live address |
| Contract roles | `DEFAULT_ADMIN_ROLE` = bridge admin address (one-day admin delay), `MINTER_ROLE` = bridge minter address, `PAUSER_ROLE` = admin and minter, no guardian; the deployer keeps no role (the deploy script asserts it) |
| Deployer and tester on Base | one test-only Base Sepolia account: deploys the contract, funds the minter and admin, receives the test mint and calls `bridgeOut`. **HELD** (see 0) |

Domains pinned in source: mint ids `rumi.icusd.base.mint.v1`, re-mints `rumi.icusd.base.remint.v1`,
bridge-out transfer ids `rumi.icusd.base.bridge-out.v1` (contract-computed with chain id,
contract, sender and nonce), contract `VERSION` `rumi-base-icusd-bridge-v1`.

`rumi_identity` raw principal bytes (for `bridgeOut`'s `icpPrincipal`):
`0xcc7286c72720b32eb57c906efec1552955def045b321da65af59db1402` (29 bytes).

## 4. Synthetic test ledger

Name "Test icUSD (Base bridge run)", symbol `TICUSD`, 8 decimals, fee 10,000 e8s, ICRC-2 on.
Initial balance: 1,000 TICUSD (100,000,000,000 e8s) to the tester subaccount. Archive options:
1,000 blocks per archive, trigger at 2,000 (never reached by this run), archive controller
`rumi_identity`. No value: never accepted by or connected to any production canister.

## 5. Budget and bounds

Cycles (`rumi_identity` cycles-ledger balance read by Miles: 3,050,950,044,313; 0 ICP):

| Line | Cycles |
| --- | --- |
| Test ledger `--cycles` | 600,000,000,000 (500B creation fee, 100B operating) |
| Test bridge `--cycles` | 1,900,000,000,000 (500B creation fee, 1.4T operating) |
| Cycles-ledger fees | 2 x 100,000,000 |
| Total | 2,500,200,000,000 |
| Remaining reserve | 550,750,044,313 (at least 0.5T kept) |

Bridge burn estimate (from measured EVM RPC `requestCost` on the 34-node subnet, plus the
documented 340M collateral per provider call; not measured spend): about 0.74T per day while bound
and idle, about 0.4T for one round trip, 50B for signatures, so a bounded window of about six hours
is about 0.7T. The 30-minute supply reconciliation is the largest fixed cost and is not tunable;
stopping the bridge canister after the test ends all burn.

Observer configuration applied right after install (validated bounds in source):
`logs_chunk_blocks` 1,000; `logs_max_response_bytes` 16,384 (finalized blocks measured at
2.2 to 2.5 KB); `default_max_response_bytes` 2,048 (receipts fall back to the larger cap);
`active_interval_secs` 60; `idle_interval_secs` 1,800; `cycles_per_rpc_call` 2,000,000,000
(excess is refunded).

Bridge limits (inside the contract limits): min deposit 1, max deposit 10, max outstanding 100,
mint and release windows 100 per 86,400 s (all icUSD); 5 active per caller, 500 global.

Base Sepolia ETH (faucet or existing test ETH only): deployer about 0.01 (deploy estimate 2,795,263
gas), minter at least 0.0011 and admin at least 0.0007 (`gas_limit x max_fee + min_gas_reserve`
at the bridge defaults); send minter 0.003 and admin 0.002 for headroom.

## 6. Run directory

`<RUN_DIR>/icp.yaml` (outside the repo; the repo's root `icp.yaml` is never used for this run):

```yaml
canisters:
  - name: test-icusd-ledger
    build:
      steps:
        - type: pre-built
          path: <REPO>/src/ledger/ic-icrc1-ledger.wasm
          sha256: cb0c3233ebb137f606d7928e90bfd907fe932a941f11b41c23cf0d6cf9c64802
  - name: rumi-icusd-bridge-test
    build:
      steps:
        - type: pre-built
          path: <RUN_DIR>/rumi_icusd_bridge.wasm
          sha256: 0b324387251dbb8842d3671ab00a70a5b6ccf5fffd2a47727639e11edc6f929a
environments:
  - name: base-sepolia-test
    network: ic
    canisters: [test-icusd-ledger, rumi-icusd-bridge-test]
```

All ICP commands below run from `<RUN_DIR>` with the installed `icp` 1.3.0 (not `npx -y`, which
may download) and `-e base-sepolia-test --identity rumi_identity`. Never use `-e ic` from the repo
root. `SUB` is the tester subaccount blob
`"\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\00\01"`.

## 7. Steps, in order

Each step lists its expected result. Stop at the first mismatch (section 9).

**A. Preflight (read-only).** `icp cycles balance -n ic` for `rumi_identity` is at least
2,500,200,000,000 plus the reserve; the candidate hashes match section 1; `git status` is clean.

**B. Create the two canisters** (spends 2.5T cycles; irreversible):

```bash
icp canister create test-icusd-ledger -e base-sepolia-test --identity rumi_identity --subnet fuqsr-in2lc-zbcjj-ydmcw-pzq7h-4xm2z-pto4i-dcyee-5z4rz-x63ji-nae --controller fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae --cycles 600000000000
icp canister create rumi-icusd-bridge-test -e base-sepolia-test --identity rumi_identity --subnet fuqsr-in2lc-zbcjj-ydmcw-pzq7h-4xm2z-pto4i-dcyee-5z4rz-x63ji-nae --controller fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae --cycles 1900000000000
icp canister settings show test-icusd-ledger -e base-sepolia-test --identity rumi_identity
icp canister settings show rumi-icusd-bridge-test -e base-sepolia-test --identity rumi_identity
```

Expected: two new ids on fuqsr, controllers exactly `rumi_identity`, balances about 100B and 1.4T.

**C. Install the test ledger:**

```bash
icp canister install test-icusd-ledger -e base-sepolia-test --identity rumi_identity --mode install --wasm <REPO>/src/ledger/ic-icrc1-ledger.wasm --args '(variant { Init = record { minting_account = record { owner = principal "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae"; subaccount = null }; transfer_fee = 10_000 : nat; decimals = opt (8 : nat8); token_symbol = "TICUSD"; token_name = "Test icUSD (Base bridge run)"; metadata = vec {}; initial_balances = vec { record { record { owner = principal "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae"; subaccount = opt blob "<SUB>" }; 100_000_000_000 : nat } }; feature_flags = opt record { icrc2 = true }; archive_options = record { num_blocks_to_archive = 1_000 : nat64; trigger_threshold = 2_000 : nat64; controller_id = principal "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae" } } })'
icp canister call test-icusd-ledger icrc1_balance_of '(record { owner = principal "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae"; subaccount = opt blob "<SUB>" })' --query -e base-sepolia-test --identity rumi_identity
```

Expected: `100_000_000_000 : nat`; `icrc1_symbol` `TICUSD`, `icrc1_decimals` 8 (both `--query`).

**D. Install the bridge** (deposits start paused; mints, releases and observer unpaused but idle
until a contract is bound), then apply the observer configuration at once:

```bash
icp canister install rumi-icusd-bridge-test -e base-sepolia-test --identity rumi_identity --mode install --wasm <RUN_DIR>/rumi_icusd_bridge.wasm --args '(variant { Init = record { icusd_ledger = principal "<TEST_LEDGER_ID>"; evm_rpc = principal "7hfb6-caaaa-aaaar-qadga-cai"; ecdsa_key_name = "test_key_1"; base_chain_id = 84532 : nat64; admins = vec { principal "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae" }; fee_recipient = null; rpc_services = vec { variant { Custom = record { url = "https://base-sepolia-rpc.publicnode.com" } }; variant { Custom = record { url = "https://base-sepolia.gateway.tenderly.co" } }; variant { Custom = record { url = "https://84532.rpc.thirdweb.com" } } }; min_quorum = 2 : nat8 } })'
icp canister call rumi-icusd-bridge-test admin_set_observer_config '(record { logs_chunk_blocks = 1_000 : nat64; logs_max_response_bytes = 16_384 : nat64; default_max_response_bytes = 2_048 : nat64; idle_interval_secs = 1_800 : nat64; active_interval_secs = 60 : nat64; cycles_per_rpc_call = 2_000_000_000 : nat })' -e base-sepolia-test --identity rumi_identity
icp canister call rumi-icusd-bridge-test get_bridge_info '()' --query -e base-sepolia-test --identity rumi_identity
icp canister status rumi-icusd-bridge-test -e base-sepolia-test --identity rumi_identity
```

Expected: `(variant { Ok })` for the observer call; `get_bridge_info` shows chain 84532, three
providers, quorum 2, `deposits = true`, no contract, no halt; the module hash in `status` equals
`0b324387...929a`.

**E. Derive the Base addresses** (no signing, no Base transaction):

```bash
icp canister call rumi-icusd-bridge-test get_base_addresses '()' -e base-sepolia-test --identity rumi_identity
```

Record `<MINTER_ADDR>` and `<ADMIN_ADDR>`.

**F. HELD (deployer signer): fund the bridge addresses and deploy the contract.** With the
test-only deployer account `<DEPLOYER>` (keystore name `<DEPLOYER_KEYSTORE>`, never a raw key):

```bash
export PATH="$HOME/.foundry/bin:$PATH"
cast send <MINTER_ADDR> --value 0.003ether --rpc-url https://base-sepolia-rpc.publicnode.com --account <DEPLOYER_KEYSTORE>
cast send <ADMIN_ADDR> --value 0.002ether --rpc-url https://base-sepolia-rpc.publicnode.com --account <DEPLOYER_KEYSTORE>
cd <REPO>/foundry
export EXPECTED_CHAIN_ID=84532 BRIDGE_ADMIN_DELAY_SECONDS=86400 BRIDGE_SUPPLY_CAP_E8S=100000000000 BRIDGE_MINT_WINDOW_LIMIT_E8S=50000000000 BRIDGE_MINT_WINDOW_SECONDS=86400 BRIDGE_MIN_BRIDGE_OUT_E8S=100000000
export BRIDGE_ADMIN_ADDR=<ADMIN_ADDR> BRIDGE_MINTER_ADDR=<MINTER_ADDR>
unset BRIDGE_GUARDIAN_ADDR ALLOW_BASE_MAINNET
forge script script/DeployBridgedIcUSD.s.sol:DeployBridgedIcUSD --rpc-url https://base-sepolia-rpc.publicnode.com --account <DEPLOYER_KEYSTORE>
forge script script/DeployBridgedIcUSD.s.sol:DeployBridgedIcUSD --rpc-url https://base-sepolia-rpc.publicnode.com --account <DEPLOYER_KEYSTORE> --broadcast
```

Expected: the dry run passes its post-deploy checks; the broadcast prints
`BridgedIcUSD deployed at: <CONTRACT_ADDR>`. Wait about 20 minutes for Base finality before G.

**G. Configure, bind, unpause:**

```bash
icp canister call rumi-icusd-bridge-test admin_set_limits '(record { min_deposit_e8s = 100_000_000 : nat64; max_deposit_e8s = 1_000_000_000 : nat64; max_outstanding_e8s = 10_000_000_000 : nat64; mint_window_limit_e8s = 10_000_000_000 : nat64; release_window_limit_e8s = 10_000_000_000 : nat64; window_seconds = 86_400 : nat64; max_active_per_caller = 5 : nat32; max_active_global = 500 : nat32 })' -e base-sepolia-test --identity rumi_identity
icp canister call rumi-icusd-bridge-test admin_bind_contract '("<CONTRACT_ADDR>")' -e base-sepolia-test --identity rumi_identity
icp canister call rumi-icusd-bridge-test admin_set_pauses '(record { deposits = false; mints = false; releases = false; observer = false })' -e base-sepolia-test --identity rumi_identity
```

Expected: `Ok` each; bind reads `VERSION`, decimals 8, `totalSupply` 0, `totalMinted` 0,
`bridgeOutCount` 0 and the roles at the finalized head, and refuses on any mismatch.

**H. ICP to Base (5 TICUSD):**

```bash
icp canister call test-icusd-ledger icrc2_approve '(record { from_subaccount = opt blob "<SUB>"; spender = record { owner = principal "<TEST_BRIDGE_ID>"; subaccount = null }; amount = 500_010_000 : nat; expected_allowance = null; expires_at = null; fee = null; memo = null; created_at_time = null })' -e base-sepolia-test --identity rumi_identity
icp canister call rumi-icusd-bridge-test bridge_to_base '(record { amount_e8s = 500_000_000 : nat64; base_recipient = "<DEPLOYER>"; from_subaccount = opt blob "<SUB>"; client_request_id = blob "sepolia-run-0001"; expected_deposit_fee_e8s = 0 : nat64 })' -e base-sepolia-test --identity rumi_identity
icp canister call rumi-icusd-bridge-test get_deposit '(<DEPOSIT_ID>)' --query -e base-sepolia-test --identity rumi_identity
cast call <CONTRACT_ADDR> "balanceOf(address)(uint256)" <DEPLOYER> --rpc-url https://base-sepolia-rpc.publicnode.com
icp canister call rumi-icusd-bridge-test get_supply_audit '()' --query -e base-sepolia-test --identity rumi_identity
```

Expected: the deposit moves PullPending, Escrowed, MintInFlight, Delivered (about 20 minutes);
`balanceOf` is 500,000,000; the audit shows escrow 500,000,000, `coverage_ok` and
`base_supply_within_bound` true. Resubmitting the same request returns the same deposit.

**I. Base to ICP (2 TICUSD), HELD signer for I.2:**

1. `cast call <CONTRACT_ADDR> "computeTransferId(address,bytes32)(bytes32)" <DEPLOYER> 0x<NONCE> --rpc-url https://base-sepolia-rpc.publicnode.com` with a fresh random 32-byte `<NONCE>`; record both.
2. `cast send <CONTRACT_ADDR> "bridgeOut(uint256,bytes,bytes32,bytes32)" 200000000 0xcc7286c72720b32eb57c906efec1552955def045b321da65af59db1402 0x0000000000000000000000000000000000000000000000000000000000000001 0x<NONCE> --rpc-url https://base-sepolia-rpc.publicnode.com --account <DEPLOYER_KEYSTORE>`
3. After about 20 minutes: `icp canister call rumi-icusd-bridge-test get_release '("<TRANSFER_ID>")' --query -e base-sepolia-test --identity rumi_identity` reaches `Released` (the next idle probe can add up to 30 minutes).
4. The tester subaccount balance rises by 199,990,000 (2 TICUSD minus the 10,000 ledger fee).

**J. Close the window:**

```bash
icp canister call rumi-icusd-bridge-test admin_set_pauses '(record { deposits = true; mints = true; releases = true; observer = true })' -e base-sepolia-test --identity rumi_identity
icp canister stop rumi-icusd-bridge-test -e base-sepolia-test --identity rumi_identity
```

Stopping keeps all state for later readback and ends the RPC burn. Never reinstall the bridge;
upgrades only (stop, `--mode upgrade` with `(variant { Upgrade = record { admins = null } })`,
start).

## 8. Expected changes

Two new canisters on fuqsr (about 2.5T cycles moved from `rumi_identity` into them), one new
contract on Base Sepolia, faucet ETH spent by the test deployer, synthetic TICUSD moved between the
tester subaccount, the bridge escrow and Base. Nothing else changes: no existing canister, no
production ledger, no production frontend, no repository configuration.

## 9. Stop and recovery conditions

Stop, record and report (do not retry around the condition) when:

- any artifact hash, module hash, controller set, chain id or contract `VERSION` differs from this
  packet;
- `get_bridge_info` shows a bridge halt or a lane halt (never clear one without a coordinator
  review);
- any outcome is ambiguous (`PullUnknown`, `TransferUnknown`, a refund or sweep left unresolved):
  wait for the bridge's own reconciliation; never resubmit with a new request id, never refund or
  re-mint by hand;
- the observer's `last_error` shows the same provider failure on three passes (for example an
  endpoint the IC cannot reach): replace providers with `admin_set_rpc_services` (controller only;
  it verifies the chain id by quorum before storing; at most 5) or stop;
- `lanes[].awaiting_gas` is true: top up the minter or admin from the deployer, or stop;
- the bridge's cycles fall below 800B, or `rumi_identity` would fall below 0.5T: pause and stop.

## 10. Evidence (keep proof levels apart)

Source and local tests (PR #591) | installation (module hashes) | signing (derived addresses, signed
transactions) | broadcast (transaction hashes) | receipt (status 1, `gasUsed` under 80% of the mint
limit) | finality (finalized block at or after the receipt block) | settlement (deposit
`Delivered`, release `Released`, ledger balances, `get_supply_audit`, the full audit log via
`get_audit_log` with `--query`). Exact ids, transactions and funding records go in the executor's
private run record; only sanitized board metadata goes to Lookout.

## 11. Optional local UI check (isolated copy, never committed)

The command-line round trip does not need it. In a throwaway copy of `src/vault_frontend`, set
`CANISTER_IDS.ICUSD_LEDGER` to `<TEST_LEDGER_ID>` and `CANISTER_IDS.ICUSD_BRIDGE` to
`<TEST_BRIDGE_ID>` in `src/lib/config.ts` (both, because the bridge page refuses a ledger that does
not match `get_bridge_info`), keep the host `https://icp0.io` (network `ic`), and run the dev server
locally. The production `config.ts` keeps `ICUSD_BRIDGE` empty and is not changed, built or
deployed.

## 12. Watch items

- IC outcall reachability of the three provider hosts is unverified until the first live request.
- The EVM RPC canister marks the `request` method the bridge uses as deprecated; it is present today.
- Whether `requestCost` already includes the 340M collateral is unresolved; the budget assumes not.
- Base Sepolia blocks are 200 ms, so finality is about 19 minutes and log scans are block-heavy.
