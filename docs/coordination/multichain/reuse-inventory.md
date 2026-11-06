# BB-30 Cross-chain reuse inventory

- Card: BB-30 "Cross-chain reuse inventory" (Rumi Multichain icUSD board)
- Date: 2026-10-30
- Worktree HEAD: c82f2880 (branch codex/base-icusd-bridge)
- Scope: source inventory only; nothing here claims deployment, live status, or test results.
- Method: read-only grep and file reads. No build, test, canister call, network write, or git state change.
- Paths are repo-relative. Line numbers are from this HEAD.
- Verdict column: reuse (use as is or as a pattern), change (needs edits for an escrow icUSD representation on another chain), or missing (no source exists, or the existing source is debt-backed and must not be used).

## 0. Headline findings

1. Only `foundry/src/BridgedIcUSD.sol` fits the escrow model. `foundry/src/IcUSD.sol` is debt-backed (its `burn` repays vault debt, IcUSD.sol:64-67) and must not become the bridge token.
2. The bridge canister is single-destination: one `chain_id` (state.rs:195), one contract binding (state.rs:209), two nonce lanes (ids.rs:138-142), one observer cursor.
3. Hard collision: bridge release indexes are keyed by the per-contract BridgeOut `sequence` (state.rs:1262-1270). `flows/release.rs:65-68` halts when a sequence maps to two transfer ids, so a second contract would trip it.
4. The backend EVM config knows only Monad and Conflux (chains/evm/mod.rs:78-100). No Base, Ethereum, Arbitrum, HyperEVM, or Robinhood row exists there.
5. The two EVM sides disagree on finality: the backend avoids the `finalized` tag (chains/evm/mod.rs:58) while the bridge reads `finalized` through quorum (src/rumi_icusd_bridge/src/evm/rpc.rs:745).
6. Non-EVM source exists only for Solana and XRPL (backend). There is no Sui code, and the frontend has no Phantom, Sui, or XRPL wallet connector.
7. Reuse is by copy, not by crate. The bridge does not depend on the backend (src/rumi_icusd_bridge/Cargo.toml), so the EVM tx, tECDSA, and RPC logic is duplicated.
8. Native SOL collateral is gone from this HEAD. Commit c029d666 (revert of PR #322) removed `chains/sol/`. `chains/solana/` (a separate, earlier module) remains.

## 1. Foundry token contracts (foundry/)

| Item | File:line | What it does | Chain-generic or chain-specific | Verdict |
| --- | --- | --- | --- | --- |
| Bridge token identity | foundry/src/BridgedIcUSD.sol:1-52, 53 | Escrow-backed ERC-20 "Rumi icUSD" for the bridge. Header says there is no repayment burn. | Mechanics generic; header and names Base | reuse (template) |
| Debt-backed token | foundry/src/IcUSD.sol:13, 64-67 | Monad icUSD. `burn` repays vault `target_vault_id` and decrements debt. | Debt-specific | missing (escrow token needed; do not reuse as bridge token) |
| Decimals | BridgedIcUSD.sol:191-193; IcUSD.sol:44-46 | 8 decimals in both (1 unit = 1 e8s) | Generic | reuse |
| Authority roles | BridgedIcUSD.sol:148-189 (constructor); 57 (MINTER_ROLE) | MINTER_ROLE for the tECDSA minter; DEFAULT_ADMIN_ROLE for a separate tECDSA admin; optional pause-only guardian; deployer gets nothing | Generic (ICP-side bridge model) | reuse |
| Debt-token authority | IcUSD.sol:39-42 | Constructor grants DEFAULT_ADMIN_ROLE and MINTER_ROLE only. No pause, no cap. | Debt-specific | missing for bridge (no supply cap or pause) |
| Mint | BridgedIcUSD.sol:202-230 | `mint(to, amount, mintId)` onlyRole(MINTER_ROLE); `mintConsumed` idempotency; supply cap check (210); fixed-window rate limit; `totalMinted` += amount (227) | Generic | reuse |
| Mint (debt token) | IcUSD.sol:37, 54-59 | `mint(to, amount, vault_id, op_id)`; per-op idempotency via `mintedOps` | Debt-specific | missing for bridge |
| Burn and redemption | BridgedIcUSD.sol:243-262; computeTransferId 269-270 | `bridgeOut` is the only burn. transferId = keccak(domain, chainid, this, from, clientNonce). Principal 1-29 bytes; anonymous 0x04 rejected. | Generic mechanics; ICP principal rules are ICP-side | reuse |
| Pause and admin config | BridgedIcUSD.sol:278-322 | pauseMint and pauseBridgeOut (PAUSER_ROLE); unpause and setters need admin; setSupplyCap (305), setMintRateLimit, setMinBridgeOut | Generic | reuse |
| BridgeOut sequence | BridgedIcUSD.sol:97, 262 | `bridgeOutCount` is per contract; first event carries sequence 1 | Per-contract (collides if indexed globally, see 2b) | change (bridge-side indexing) |
| Chain-named constants | BridgedIcUSD.sol:55 (VERSION "rumi-base-icusd-bridge-v1"), :61 (BRIDGE_OUT_DOMAIN "rumi.icusd.base.bridge-out.v1") | Base-named strings baked into the token | Chain-specific naming | change (only if a per-chain token is wanted) |
| Deploy script allow-list | foundry/script/DeployBridgedIcUSD.s.sol:27, 34-35, 60 | Allowed chain ids 84532, 31337, 8453; 8453 needs ALLOW_BASE_MAINNET=true | Base-specific | change |
| Foundry tests | foundry/test/BridgedIcUSD.t.sol (handler at :1441), foundry/test/DeployBridgedIcUSD.t.sol | Token and deploy-script tests | Base-token tests | reuse (extend per chain) |
| Monad runbook and README | foundry/DEPLOY.md, foundry/README.md | IcUSD (debt) testnet deploy notes | Debt-specific | missing (reference only) |
| Toolchain | foundry/foundry.toml:5 (solc 0.8.24); foundry/remappings.txt (OpenZeppelin) | Shared compiler and library pins | Generic | reuse |

## 2. Base bridge canister (src/rumi_icusd_bridge/)

### 2a. Module inventory

| Module | File:line | Purpose | Chain-generic or chain-specific | Verdict |
| --- | --- | --- | --- | --- |
| Entry points | src/rumi_icusd_bridge/src/lib.rs:1-6, 41, 93, 143, 163 | Canister init (instance salt at 41); Candid methods bridge_to_base (93), notify_base_bridge_out (143), get_base_addresses (163) | Base-named API | change (method and arg names) |
| Environment shim | src/rumi_icusd_bridge/src/env.rs:1-77 | IC system calls, plus host stubs for unit tests | Generic | reuse |
| Public types | src/rumi_icusd_bridge/src/types.rs:22-40, 51-66, 131-136, 240 | InitArgs.base_chain_id (30); L2MainnetService (51-58); RpcServiceConfig = BaseMainnet or Custom (60-66); BridgeToBaseArgs.base_recipient (136); BaseTxStage (240) | Partly Base-specific | change |
| Stable state | src/rumi_icusd_bridge/src/state.rs:43-58, 179-257 | Append-only MemoryIds (43-58); ContractBinding (179); BridgeConfig (191); init validation (240-257) with chain allow-list (235-247) | Singletons; Base allow-list | change |
| Lane and core state | src/rumi_icusd_bridge/src/state.rs:384-390, 505-533 | LaneState: next_nonce (388) and one unincluded tx (390). BridgeCore: config, buckets, windows, observer, base_supply_snapshot (533) are all singletons | Singleton | change |
| Escrow accounting | src/rumi_icusd_bridge/src/accounting.rs:1-20, 31, 134-135, 237-280 | Disjoint liability buckets (O, PM, fees, unknown outflow); coverage check (237); base_exposure (134); check_base_supply (268) | Bucket math generic; field names Base | change (rename or per destination) |
| Admin | src/rumi_icusd_bridge/src/admin.rs:41-257 | Pauses, limits, fees, gas, observer settings, RPC replacement (235), admin list (257). Admins cannot move escrow (header 1-6). | Generic | reuse |
| Timer and reconcile | src/rumi_icusd_bridge/src/timer.rs:1-8, 215, 246-292, 339 | One self-rescheduling tick (215); supply reconcile (246); base_supply_snapshot (339) | Mostly generic; one Base supply check | change |
| Observer | src/rumi_icusd_bridge/src/observer.rs:261-297, 623, 838, 910 | bind_contract (261) requires totalSupply 0 and bridgeOutCount 0 (295, 297); count-monotonic check (623); finalized scan run_pass (838); totalSupply and C2/C3 check (910) | Contract and finality specific | change |
| EVM ABI codec | src/rumi_icusd_bridge/src/evm/abi.rs:1-8, 103-136, 150-160, 315-352 | BridgedIcUSD topic0 (103-105) and selectors (114-136); calldata encoder (150-160); strict decoders decode_bridge_out (315), decode_bridge_mint (352) | Codec generic; constants per contract | reuse (codec); change (constants) |
| EVM RPC transport | src/rumi_icusd_bridge/src/evm/rpc.rs:1-8, 53-59, 119-137, 217-223, 283-307, 742-745, 838, 902 | Calls the EVM RPC canister (call_with_payment128 at 307); per-provider parse and quorum (header); BlockTag incl. Finalized (57); enum mirror (119-137); to_rpc_service maps only BaseMainnet and Custom (217-223); finalized_head uses "finalized" (745) | Transport generic; mapping Base-only | reuse (transport); change (mapping) |
| Signer | src/rumi_icusd_bridge/src/evm/signer.rs:1-8, 51-52, 76, 137-145 | tECDSA key per lane; derivation path includes chain id (76; ids.rs:155); refuses a tx whose chain id differs (139-141) | Generic | reuse |
| EIP-1559 tx | src/rumi_icusd_bridge/src/evm/tx.rs:1-8, 27, 74-99 | Type-2 encoder, signing hash, y-parity recovery. No legacy tx. | Generic for EIP-1559 chains | reuse |
| Lane lifecycle | src/rumi_icusd_bridge/src/evm/lane.rs:1-8, 78, 160, 430, 1262, 1311, 1416 | Lane runner (78); MAX_UNFINALIZED = 8 (160); ensure_nonce (1262); start_mint (1311); create_mint_tx_with (1416) | Lifecycle generic; lanes are two singletons (ids.rs:138-142) | change (per-chain lanes) |
| Deposits (ICP to Base) | src/rumi_icusd_bridge/src/flows/deposit.rs:1-10, 51, 234, 270, 376, 535, 616 | admit (51); bridge_to_base (234); pull reconcile (270); mint finalization (376); refunds (535); fee sweeps (616) | ICP side generic; mint Base-specific | change (destination parameter) |
| Releases (Base to ICP) | src/rumi_icusd_bridge/src/flows/release.rs:1-3, 36, 57-68, 166 | parse_destination (36); record_finalized_bridge_out (57) halts on sequence reuse (65-68); process_releases (166) | ICP destination generic; sequence-keyed | change (see 2b) |
| Ledger operations | src/rumi_icusd_bridge/src/flows/ledger_ops.rs:1, 35, 90, 124, 343 | Persisted call tuple, outcome apply, reconcile for pull, release, refund, sweep | Generic | reuse |
| Ledger client | src/rumi_icusd_bridge/src/ledger.rs:1-8, 115, 269-306, 345, 782 | ICRC-1/2/3 client; classify (115); icrc2_transfer_from (305); total_supply (345); ICRC-3 search for ambiguous outcomes (782) | Generic (ICP icUSD ledger) | reuse |
| Ids and domains | src/rumi_icusd_bridge/src/ids.rs:21-31, 79-105, 138-160 | Domain strings (21-25) and KEY_PATH_ROOT (31) are Base-named; mint_id (79), remint_id (90), bridge_out_transfer_id (102) include chain id and contract; derivation_path (155); LaneKind (138) | ID preimages generic; strings Base-named | reuse (keep root strings unless keys are migrated) |
| ICRC-21 consent | src/rumi_icusd_bridge/src/icrc21.rs:98-101, 118-122, 139-145 | Consent text for bridge_to_base and notify_base_bridge_out; text says Base and "about 20 minutes" | Base-specific copy | change |
| Public views | src/rumi_icusd_bridge/src/views.rs:17-20, 139, 158, 194-195, 218-238 | BaseTxStage mapping (17-20); "Base finalized (L1 batch finality)" text (158); circulating_on_base fields (218) | Base-named | change |
| Crate deps | src/rumi_icusd_bridge/Cargo.toml (dependencies) | No dependency on rumi_protocol_backend or any shared chain crate | n/a | change (only if a shared crate is wanted; design section 13 defers it) |
| Test harness | src/rumi_icusd_bridge/tests/*.rs, tests/support/anvil.rs, tests/evm_rpc_relay/ | PocketIC tests with a local anvil node and an EVM RPC relay mock | Test-only; anvil is local EVM | reuse (harness pattern) |

### 2b. Collisions a second destination chain would hit

| Collision | File:line | Effect |
| --- | --- | --- |
| Single chain id and allow-list | state.rs:195; state.rs:235-247 | Only 8453, 84532, 31337 are accepted at init. Chain id is fixed for the whole canister. |
| Single contract binding | state.rs:209, 179-188; observer.rs:261 | One BridgedIcUSD address per canister. |
| Two singleton nonce lanes | ids.rs:138-142; state.rs:384-390; state.rs:582 | lane() keys only on LaneKind. One nonce and one unincluded tx per lane, shared by all chains. |
| One observer cursor and count | state.rs:406; observer.rs:623, 838 | One scan position and one processed-event count. |
| One supply snapshot | state.rs:486, 533; observer.rs:910 | One base_total_supply snapshot and one invariant result. |
| Release indexes keyed by per-contract sequence | BridgedIcUSD.sol:262; state.rs:1262-1270 | RELEASE_BY_SEQ, SENDER_RELEASES, and OWNER_RELEASES all key on `sequence`. A second contract restarts at 1. Hard collision. |
| Halt on repeated sequence | flows/release.rs:65-68; observer.rs:395 | A second contract's event with a reused sequence halts with "maps to two transfer ids". |
| Sender index keyed by (address, sequence) | state.rs:951-956; state.rs:1289 | Sequences restart per contract, so the same address on two chains collides on equal sequence numbers and list_sender_releases would merge listings across chains. |
| Single RPC config | state.rs:198-199; state.rs:285-300 (BaseMainnet only for chain 8453 at 297) | One provider list and quorum; BaseMainnet providers are refused on any other chain. |
| Global escrow pool and caps | ids.rs:28, 41; types.rs (BridgeLimits) | One escrow subaccount and one set of limits for all destinations. The accounting must be split per destination. |
| Safe (no collision) | ids.rs:79-105; BridgedIcUSD.sol:269-270; ids.rs:155, 288-289 | mint, remint, and transfer ids include chain id and contract. Derivation paths include chain id and lane. |

## 3. Backend chains (src/rumi_protocol_backend/src/chains/)

| Item | File:line | What it does | Chain-generic or chain-specific | Verdict |
| --- | --- | --- | --- | --- |
| Module map | chains/mod.rs:146-161; header 1-12 | pub mods: adapter, admin, collateral_config, config, evm, interest, liquidation, liquidation_config, monad, multi_chain_state, recovery, settlement_queue, solana, supply, vault, xrp. No sol module. | n/a | n/a |
| ChainAdapter trait | chains/adapter.rs:16-30 | chain_id, verify_deposit, sign_withdrawal, sign_mint, sign_burn, fetch_finality, observe_event | Trait generic; calls debt-oriented | change (signatures) |
| Chain id and config | chains/config.rs:17; 114-130 (rpc_endpoints 118; finality_depth 119) | Persisted per-chain config, including RPC URLs and a finality depth | Generic | reuse (pattern; bridge keeps its own config) |
| Debt supply invariant | chains/supply.rs:96-100, 107-160, 167-178; multi_chain_state.rs:682, 690 | `chain_backing_rhs_e8s` returns debt + reserve backing + pending chain burn; `apply_supply_delta` rejects any change leaving sum(chain_supplies) != that sum, and `check_invariant` (the periodic self-check) compares the same sum. The module header (supply.rs:3-8) still says `== total_debt`; that text is stale, the function bodies are authoritative | Debt-backed | missing (do not add the bridge here; bridge design section 7.3, line 232) |
| Settlement queue | chains/settlement_queue.rs:16-21, 199 | Per-chain outbound op queue; Mint op carries vault_id | Queue generic; op kinds debt-bound | change (new op kinds) or pattern only |
| Multi-chain root | chains/multi_chain_state.rs:1119-1126 (MultiChainStateV9) | chain_supplies, chain_contracts, settlement_queues, chain_vaults, invariant_halted | Debt-specific | missing (bridge has its own store) |
| Recovery | chains/recovery.rs:1-12, 197, 359 | Re-reads on-chain state before reversing ops or releasing collateral | Pattern generic; ops debt-bound | reuse (pattern) |
| EIP-1559 builder | chains/evm/tx.rs:86, 150, 171, 185, 223, 256 | Builds, signs, recovers y-parity; mint and transfer calldata | Generic EIP-1559 | reuse (logic is duplicated in the bridge) |
| Address derivation | chains/evm/tecdsa.rs:21-31, 46 | custody path [chain u32 LE, principal, nonce]; settlement path [chain u32 LE, "settlement"]; pubkey to address | Generic; paths differ from bridge (ids.rs:155) | reuse (helper); do not share paths |
| EVM RPC transport | chains/evm/evm_rpc.rs:1-6, 794, 1464, 1493, 1718, 1808, 1909, 1964 | Calls the EVM RPC canister via call_with_payment128 (header 3-6); logs, receipts, send_raw_transaction, fees; is_block_final uses finality_depth (1493) | Generic transport; per-chain config | reuse (pattern); change (finality model) |
| Finality choice | chains/evm/mod.rs:52-60 (volatile finalized tag avoided, line 58) | Backend uses depth; the bridge uses the finalized tag | Conflicting models | change (decide before adding chains) |
| Per-chain EVM constants | chains/evm/mod.rs:61-100 (evm_chain_config) | Only 10143 (Monad, line 80), 71 (Conflux testnet, 86), 1030 (Conflux mainnet, 92) | Chain-specific rows | change (add rows) |
| Monad rail | chains/monad/config.rs:9-10; monad/chain_vault.rs; monad/mod.rs; shared chains/evm/settlement.rs and chains/evm/deposit_watch.rs | Debt-backed Monad rail on the shared chains/evm modules; MONAD_CHAIN_ID 10143 | Chain-specific, debt | missing for bridge (reference only) |
| Conflux rail | chains/evm/conflux/config.rs:13-15; chains/evm/conflux/mod.rs | Conflux eSpace config (71 testnet, 1030 mainnet) | Chain-specific, debt | missing for bridge (reference only) |
| Burn proof | chains/evm/burn_proof.rs:1 | Notify-then-verify turns a verified receipt into applied burns | Pattern generic; debt effect | reuse (pattern; bridge notify_base_bridge_out at src/rumi_icusd_bridge/src/lib.rs:143 is the same idea) |
| EIP-712 vault intents | chains/evm/eip712.rs:1-10 | User-signed vault intents for self-serve auth | Debt-specific | missing (not needed for bridge) |
| Solana adapter set | chains/solana/config.rs:13; ted25519.rs; tx.rs (header 1-6); sol_rpc.rs (header 1-6); adapter.rs; settlement.rs:115; deposit_watch.rs:114 | Solana devnet rail: threshold Ed25519, SOL RPC wrapper, durable-nonce settlement. Deposit watch polls custody SOL balances (header 3-6). SPL MintTo planned (tx.rs:6). | Solana-specific | reuse (ted25519, sol_rpc pattern); missing (SPL escrow token, burn observer) |
| Solana wiring | src/rumi_protocol_backend/src/main.rs:557-580, 2365-2372 | Observer and settlement run only when solana_workers_enabled; open_solana_vault is developer-gated (2372) | Solana-specific | n/a (live state not checked) |
| Native SOL (removed) | git history: revert c029d666 of PR #322 | Removed chains/sol/* and sol_rpc_mock; not present in this HEAD | n/a | missing (do not treat as live or pending) |
| XRPL config and adapter | chains/xrp/config.rs:1-7, 15 (XRP_CHAIN_ID 144); chains/xrp/adapter.rs:17, 273-283 | Native XRP collateral. Header says there is no icUSD token on XRPL. sign_mint and sign_burn return NotImplemented (273-283). | XRPL-specific | missing (icUSD representation on XRPL) |
| XRPL address and signing | chains/xrp/address.rs (header 1-6); sign.rs (header 1-6); ted25519.rs (header 1-6) | Classic address derivation; Ed25519 over the STX prefix (no SHA-512Half); shared threshold Ed25519 signer | XRPL-specific | reuse (address, sign, ted25519) |
| XRPL codec | chains/xrp/codec.rs:1-6 | Binary codec for native XRP Payment only | XRPL-specific | change (issued currency and trust lines absent) |
| XRPL RPC | chains/xrp/xrp_rpc.rs:1-6; main.rs:9617-9641 | Raw HTTPS to rippled with consensus transforms | XRPL-specific | reuse (pattern) |

## 4. Frontend (src/vault_frontend/, src/conflux_espace_frontend/)

| Item | File:line | What it does | Chain-generic or chain-specific | Verdict |
| --- | --- | --- | --- | --- |
| Base chain table | src/vault_frontend/src/lib/services/baseBridge/chains.ts:13-26, 28-59, 61, 63, 96, 157 | Three rows: 8453, 84532, 31337. addChainParams (96) builds wallet_addEthereumChain params. resolveBridgeConfig (157) resolves the canister's chain. | Chain-specific data | change (add rows) |
| EIP-1193 layer | baseBridge/evmWallet.ts:18-24, 98-120, 101, 218, 240, 265, 304-305 | discoverWallets: EIP-6963 then window.ethereum fallback (101-120); sendTransaction (218); switchToChain (240); watchProvider (265); localStorage keys "rumi:base-bridge:*" (304-305) | Generic | reuse |
| Wallet error handling | baseBridge/evmWallet.ts:61-77 | User rejection, unrecognised chain, and -32002 request pending (71-74) | Generic | reuse |
| Contract ABI helpers | baseBridge/abi.ts (header 1-12) | Hand-rolled ABI for five calls and one event. No EVM library. | Contract-specific | change (per contract) |
| Address and amounts | baseBridge/eip55.ts; keccak.ts (header: original Keccak, not NIST SHA3); amounts.ts | EIP-55 checksums; exact e8s arithmetic | Generic | reuse |
| Intent journal | baseBridge/intents.ts:19-23 | Intent written before the first wallet prompt; prefix "rumi:base-bridge:v1:" | Generic logic; Base-named key | reuse (rename key if needed) |
| Status and copy | baseBridge/status.ts:24, 30 | FINALITY_NOTE "about 20 minutes"; DEPOSIT_DELIVERY_NOTE | Base copy | change |
| Bridge actors | baseBridge/bridgeActor.ts:43, 76-88, 99 | getBridgeCanisterId (43); createSigners (76); loadBridgeConfig (99). One bridge canister. | Single canister | change (chain or canister parameter) |
| Base flows | baseBridge/icpToBase.ts, baseToIcp.ts, status.ts | ICP to Base and Base to ICP flows | Base naming | reuse (logic); change (copy) |
| Bridge page | src/vault_frontend/src/routes/bridge/+page.svelte; bridgePage.spec.ts | Bridge UI | Base-specific | change |
| ICP wallet list | src/vault_frontend/src/lib/components/wallet/WalletConnector.svelte:67-72, 85-86 | Internet Identity, Plug, and Oisy only | ICP-only | missing (non-ICP wallets) |
| Ethereum deposit page | src/vault_frontend/src/routes/ckusdc/+page.svelte:230-275, 411, 417, 480, 488 | Ethereum mainnet deposit: eth_requestAccounts (417), chain check 0x1 (480), eth_sendTransaction (488) | Ethereum-specific UI | reuse (Ethereum reference; deposit side only) |
| ckERC20 minter client | src/vault_frontend/src/lib/services/ckerc20Minter.ts:16, 135, 199 | Third-party ckERC20 minter: dashboard link (16), deposit encoding (135), approve and withdraw (199) | Third-party minter | reuse (reference only) |
| Conflux eSpace client | src/conflux_espace_frontend/src/evm.ts:32, 54-62, 92-120, 186-190; package.json:43 (viem ^2.56.0) | viem clients; EIP-6963 discovery; sendDeposit (186); burnIcusd (190, debt) | Conflux-specific; debt | reuse (discovery pattern only) |
| Wallet-name search | grep over src/vault_frontend/src | Rabby and MetaMask appear only in user text (ckusdc +page.svelte:411) and a comment (evmWallet.ts:71). Phantom, Sui wallet, and the WalletConnect protocol: no matches (the only "WalletConnector" hit is a component name). Xaman: text hint only (src/vault_frontend/src/lib/components/vault/XrpVaultPanel.svelte:266). | n/a | missing (Phantom, Sui, XRPL connectors) |
| Frontend dependencies | src/vault_frontend/package.json:54-59 | No viem or ethers. baseBridge avoids EVM libraries by design (evmWallet.ts header). | n/a | change (decide library policy before adding chains) |

## 5. Design docs and runbooks that touch these chains

| Doc | What it covers (status as stated in the doc) |
| --- | --- |
| docs/superpowers/specs/2026-05-27-multi-chain-rumi-design.md | Multi-chain architecture and phasing. Line 242 lists Ethereum mainnet, Base, and Arbitrum as future integrations. |
| docs/superpowers/specs/2026-06-01-solana-integration-design.md | Solana rail design. Header: approved design, pre-implementation. |
| docs/superpowers/specs/2026-06-14-xrp-native-rail-design.md | Native XRPL rail. Header: integration landed, dormant (not wired up). |
| docs/superpowers/specs/2026-06-16-native-xrp-collateral-design.md | Native XRP collateral. Header: spec, not started. |
| docs/superpowers/specs/2026-06-19-chains-liquidation-design.md | Chains-rail liquidation engine. Line 22: future EVM chains (Base, Arbitrum) plug in as data. |
| docs/superpowers/specs/2026-10-24-base-icusd-bridge-design.md | Base bridge design. Section 4 (around line 70): canister boundary and reuse table. Section 7.3 (line 232): coexistence with debt supply. Section 13 (around line 546): other destination chains deferred; canister is Base-only, chain id fixed at init. |
| docs/icp-solana-integration-playbook.md | Field guide for ICP and Solana (Motoko backend). |
| docs/integrations/cksol.md | ckSOL collateral through DFINITY's minter. Header: Rumi holds collateral on the ckSOL ledger and runs no Solana custodian. |
| foundry/DEPLOY-BASE.md | Base Sepolia runbook, draft dated 2026-10-24. |
| foundry/DEPLOY.md; foundry/README.md | Monad IcUSD (debt-backed) testnet notes. |
| docs/coordination/base-icusd-bridge-*.md | Base bridge run records (rules, Sepolia packet, staging recipe, review record, kanban, handoff). Names only; not read for this inventory. |
| Grep results | Sui, Robinhood, and HyperEVM: no matches in docs/, src/, or foundry/src. Arbitrum: two design docs (lines above) plus EVM RPC mirror enums. Ethereum mainnet: the 2026-05-27 design doc (line 242) and the ckUSDC page text. |

## 6. Per-chain table

Column 4 lists Base-hard-wired code that a chain would have to change. For Base, column 4 is n/a because Base is the reference implementation.

| Chain | Existing repo source (paths) | Reusable pieces | Base-specific pieces that must change | Missing entirely |
| --- | --- | --- | --- | --- |
| Ethereum (mainnet) | src/vault_frontend/src/routes/ckusdc/+page.svelte (chain check 480); src/rumi_icusd_bridge/src/evm/rpc.rs:119-124 (EthMainnetService mirror, never sent); src/rumi_protocol_backend/src/chains/evm/evm_rpc.rs (EthMainnet mirror) | EVM RPC transport and quorum; EIP-1559 tx; tECDSA signer; EIP-1193 wallet layer; ckUSDC deposit UX (reference) | chains.ts table; RpcServiceConfig has no Ethereum variant (src/rumi_icusd_bridge/src/types.rs:60-66); validate_init allow-list (state.rs:240-247); deploy allow-list (DeployBridgedIcUSD.s.sol:34-35) | Escrow token, deploy script, RPC config, finality policy, frontend entry. No ETH destination exists in any repo file. |
| Base | foundry/src/BridgedIcUSD.sol; src/rumi_icusd_bridge/** (about 21k lines in src/); src/vault_frontend/src/lib/services/baseBridge/**; src/vault_frontend/src/routes/bridge/; foundry/script/DeployBridgedIcUSD.s.sol; foundry/DEPLOY-BASE.md | Entire path present on this HEAD, including PocketIC and anvil tests | n/a (reference) | Mainnet deployment and live status are not established here. Design section 13: canister is Base-only. |
| Arbitrum (One) | src/rumi_icusd_bridge/src/evm/rpc.rs:134 (ArbitrumOne mirror); src/rumi_protocol_backend/src/chains/evm/evm_rpc.rs:195 (mirror); 2026-05-27 design doc line 242 | EVM path as for Ethereum; mirror enum names an ArbitrumOne provider type (whether the EVM RPC canister offers it is not verified here) | RpcServiceConfig lacks ArbitrumOne (types.rs:60-66); Base-only mapping (rpc.rs:217-223) | Contract, deploy script, RPC config, frontend entry, finality policy |
| Robinhood Chain | None (no matches in src/, foundry/, docs/) | Nothing repo-specific. EVM code applies only if the chain is EVM-compatible, which the repo does not establish. | Everything Base-hard-wired, as for Ethereum. Only the Custom variant can carry a provider URL for it (types.rs:63-65). | Everything chain-specific. EVM compatibility unclear. |
| HyperEVM | None (no matches) | Same as Robinhood Chain: EVM code only if EVM-compatible (unclear); Custom RPC route only | Same as Robinhood Chain | Everything chain-specific. EVM compatibility unclear. |
| Monad | src/rumi_protocol_backend/src/chains/monad/** (config.rs:9-10); chains/evm/** shared modules; foundry/src/IcUSD.sol; foundry/DEPLOY.md; src/monad_rpc_mock/ | EIP-1559; tECDSA; Custom RPC endpoints (config.rs:118 rpc_endpoints); per-chain evm_chain_config row (evm/mod.rs:80-85) | Token is debt-backed (IcUSD.sol:64-67), so bridge needs a new escrow token; bridge allow-list lacks 10143; finality model differs (evm/mod.rs:58 vs evm/rpc.rs:745) | Escrow-backed Monad token and deployment; bridge RPC config; bridge chain entry; Monad wallet config in baseBridge |
| Solana | src/rumi_protocol_backend/src/chains/solana/** (config.rs:13; ted25519.rs; tx.rs; sol_rpc.rs; adapter.rs; settlement.rs:115; deposit_watch.rs:114); src/rumi_protocol_backend/src/main.rs:557-580, 2365-2372; docs/superpowers/specs/2026-06-01-solana-integration-design.md; docs/integrations/cksol.md | Threshold Ed25519 signer (shared with XRPL); SOL RPC wrapper pattern; durable-nonce settlement; per-chain token address in runtime state (solana/config.rs:3-5) | Solana-specific throughout; no EVM code shared. Ed25519 threshold pattern may carry over (not verified for Sui). | SPL escrow token and mint authority; burn observer (deposit_watch polls custody balances, not burns); Solana wallet connector; Solana finality policy. Native SOL reverted (c029d666). |
| Sui | None (no matches in src/, foundry/, docs/; no Move code) | Nothing repo-specific | All of it | Everything: Move package, object model, signature scheme, RPC, wallet connector, finality |
| XRPL | src/rumi_protocol_backend/src/chains/xrp/** (config.rs:15; address.rs; codec.rs; sign.rs; ted25519.rs; xrp_rpc.rs; adapter.rs:273-283); main.rs:9617-9657; docs/superpowers/specs/2026-06-14-xrp-native-rail-design.md; docs/superpowers/specs/2026-06-16-native-xrp-collateral-design.md | Classic address derivation; Ed25519 STX signing; shared threshold Ed25519 signer; raw HTTPS RPC with consensus transforms; codec for native XRP Payment | Adapter mint and burn return NotImplemented (adapter.rs:273-283); codec has no issued-currency support (codec.rs:1-6) | icUSD issued-currency representation; trust lines; escrow account operations; XRPL burn or redemption observer; XRPL wallet connector (Xaman is text only, XrpVaultPanel.svelte:266). No XRPL icUSD escrow code in this worktree. |

## 7. Could not determine

- Live status of any rail (Solana, XRP, Monad, Conflux) or of the Base bridge. This inventory is source only.
- Whether Robinhood Chain and HyperEVM are EVM-compatible. The repo does not say.
- Whether the EVM RPC canister offers built-in providers beyond the enums mirrored here (rpc.rs:119-137; evm_rpc.rs:190-197).
- Whether the tECDSA derivation is scoped per calling canister. The bridge passes canister_id None (src/rumi_icusd_bridge/src/evm/signer.rs:171). Not checked against the IC specification.
- Finality policy for Ethereum, Arbitrum, Solana, and Sui. Not defined in the repo.
- Whether an XRPL icUSD escrow branch exists outside this worktree. Not checked.
- docs/coordination/base-icusd-bridge-* files were named but not read. The lookout-kanban repo was not read (out of scope).

## Coordinator correction and collision note (2026-10-30)

Correction (Miles' review): the supply row above was first copied from the stale header of
`chains/supply.rs`. The function bodies at supply.rs:96-178 enforce
`sum(chain_supplies) == debt + reserve backing + pending chain burn`, not `== total_debt`. Design work
uses the bodies. No backend or runtime code was changed to fix this documentation.

Sharing ONE bridge canister across destinations would collide in these singleton namespaces:

| Namespace | Where | What breaks with a second destination in the same canister |
| --- | --- | --- |
| Release index by sequence | state.rs:1262-1270 (`RELEASE_BY_SEQ`, memory id 7) | Keyed by the bare bridge-out sequence. A second contract's sequence 1 maps to a different transfer id, and flows/release.rs:65-68 halts the bridge |
| Sender and owner release indexes | state.rs:1262-1270 (`SENDER_RELEASES`, `OWNER_RELEASES`) | Keyed by (sender or owner, sequence), still without a chain or contract, so the same sender's sequence numbers collide across contracts |
| Nonce lanes | state.rs:384-390; ids.rs:138-142 | One `next_nonce` and one unincluded transaction per lane kind (minter, admin), not per chain |
| Chain and contract | state.rs:195, 209 | One `chain_id` and one `ContractBinding` in the config |
| Supply snapshot and buckets | state.rs:486, 533; accounting.rs | One destination supply snapshot and one set of liability buckets, so coverage and the supply check describe a single destination |

What is already separated: mint, re-mint and bridge-out ids hash the chain id, contract and instance
salt under their domains (ids.rs:78-105), and threshold-ECDSA derivation paths include the lane and
chain id (ids.rs:155, test at 287-290). The domain strings themselves name Base
(`rumi.icusd.base.*.v1`, ids.rs:21-25) and are live signing domains for the reviewed Base bridge, so
they stay unchanged.

A separate canister instance per destination isolates every namespace above with no change to them:
each instance has its own stable memory, configuration, contract binding, nonce lanes and escrow
account, and its threshold keys differ automatically because threshold keys are derived per
canister. What instances do NOT isolate is aggregate reporting: total escrowed icUSD and total
destination supply across instances must be summed outside any one canister.
