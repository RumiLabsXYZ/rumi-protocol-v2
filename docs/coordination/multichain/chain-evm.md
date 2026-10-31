# BB-34 Six-EVM readiness and Base-bridge reuse

Card BB-34 | Worker W17 | 2026-10-30 | Read-only: no code, build, test, RPC probe or canister call was run. Every destination stays `disabled`.

Labels: **F** fact with a source (repo `file:line` at this worktree HEAD, or a facts file `sources-evm.md` = S, `reuse-inventory.md` = RI, design = `2026-10-30-multichain-icusd-groundwork-design.md` = G). **R** recommendation. **D** decision for Rob and Miles. **I** inference. **U** unverified. Sizes are estimates by lines touched.
All `file:line` below are in `src/rumi_icusd_bridge/src/` unless a path is given. Public chain ids only; no addresses, keys, hashes or local paths.

## 0. Findings that change or sharpen the design text

1. **One Wasm has one Candid (F, `lib.rs:93,143,163`; `rumi_icusd_bridge.did:354-374`).** G 1.5 says new instances name their own methods (`bridge_to_<slug>`). An in-place Wasm cannot export different names per instance. Either every instance keeps the Base-named methods and `base_*` fields (honest only if documented), or the Wasm adds generic names (additive Candid change). **D1a**, not a step. Per-instance Wasm builds would be the "separate artifact" half of D1.
2. **The slug applies to four off-chain strings, not to two contract constants (F).** `CONTRACT_VERSION` (`ids.rs:18`, checked at bind `observer.rs:290`) and `BRIDGE_OUT_DOMAIN` (`ids.rs:25`, compiled into the token `BridgedIcUSD.sol:61`) must stay Base-named while D5 says identical bytecode. Slugged: mint domain, re-mint domain, escrow label, key root (plus the init-time salt label `lib.rs:41`).
3. **D5 and D8 collide on Monad (F).** The escrow token and the debt token both hard-code `ERC20("Rumi icUSD", "icUSD")` (`foundry/src/BridgedIcUSD.sol:157`, `foundry/src/IcUSD.sol:39`). Identical bytecode cannot carry a distinct on-chain name. See 2.6.
4. **Head selection has more call sites than `finalized_head()` (F).** Callers: `observer.rs:283,598,914`, `evm/lane.rs:583,782`; plus a direct tag read in the idle probe `observer.rs:828` (`BlockTag::Finalized`). A Monad Verified rule must cover all of them.
5. **The facts file disagrees with itself on the Monad delay (F).** S section 6 row 3 says Verified is "3 blocks after Finalized"; the coordinator addendum says the block-states page gives no value. Treat the delay as unsourced (blocker) until a primary page states it.
6. **Moving tags and replicated outcalls (I/U).** The backend avoids the `finalized` tag because "a moving value breaks IC HTTPS-outcall consensus across replicas" (`rumi_protocol_backend/src/chains/evm/mod.rs:57-60`). The bridge reads the tag through the EVM RPC canister `request` (`evm/rpc.rs:303-307,745`). A 300 ms chain (Monad, S 6 row 3) advances `finalized` several times per second, so tag reads may fail consensus often. Not measured; needs a probe before Monad (and HyperEVM, Robinhood) are called feasible.
7. **`Custom` carries no chain id on the raw path (F).** The bridge sends `RpcService::Custom(RpcApi{url, headers: None})` (`evm/rpc.rs:104-107,220`), not the typed `Custom{chainId, services}` S describes. Chain identity is enforced only by an `eth_chainId` quorum at bind (`observer.rs:277-281`) and at `admin_set_rpc_services` (`admin.rs:241`), not at install.
8. **Explicit gas limits already exist as config (F).** `mint_gas_limit` and `admin_gas_limit` are fixed numbers (`state.rs:104-105`), simulated at that limit (`evm/lane.rs:1385`). The Base-tuned defaults and bounds are the problem, not estimate-derived limits (2.3 to 2.6).

## 1. D1 in-place: changes that make the Base canister multi-EVM

Recommendation (R): derive everything from the chain id through one static table, adding **no stored field and no Candid field**, so existing Base state and the Base API are untouched. A `ChainProfile` row holds: chain id, slug, display name, role (mainnet, testnet, local), allowed key names, native provider variant (if any), `HeadRule`, default gas settings, wait text.

| # | Where | Change | Reviewed Base code? | API or state impact |
|---|---|---|---|---|
| C1 | `ids.rs:21-31,41,79,90,155` | Domain, escrow label and key root become `format!` templates over the slug: `rumi.icusd.{slug}.mint.v1`, `...remint.v1`, `rumi-icusd-{slug}-bridge:escrow:v1`, root `rumi-icusd-{slug}-bridge`. `mint_id`, `remint_id`, `derivation_path` keep their signatures (they already take `chain_id`). `escrow_subaccount()` gains a chain argument (one caller, `state.rs:601`). `CONTRACT_VERSION`, `BRIDGE_OUT_DOMAIN` stay constants. | **Yes** (live signing domains) | none if the test in 1.1 passes |
| C2 | new table (in `state.rs` or a new `chains.rs`) | Rows: Base 8453/84532/31337 slug `base`; Ethereum 1/11155111 `eth`; Arbitrum 42161/421614 `arb`; Robinhood 4663/46630 `rhood`; HyperEVM 999/998 `hyper`; Monad 143/10143 `monad`. Local 31337 stays `base`. | New | none |
| C3 | `state.rs:235-248` `validate_init` | Replace the three-case `match` by a table lookup (allow-list plus key rule). Base rows must give the same results as `state.rs:243-247`. R for new rows: mainnet `key_1` only, testnet `test_key_1` only (G V6); Base Sepolia keeps accepting both. | **Yes** | none (the `base_chain_id` field name stays, D1a) |
| C4 | `state.rs:285-322` `validate_rpc_services` | Native variant valid only for its own chain (generalizes `:296`); `https` only except 31337 (`:302`); `min_quorum >= 2` for every mainnet row (generalizes `:318`). Also `validate_stored_rpc_config` (`:328`) at upgrade. | **Yes** | none |
| C5 | `types.rs:60-66` `RpcServiceConfig` | Additive variants `EthMainnet(EthMainnetService)`, `EthSepolia(EthSepoliaService)`, `ArbitrumOne(L2MainnetService)`. The service enums already exist as mirrors (`evm/rpc.rs:110-126`). The type appears only as an argument (`did:195,351`), never returned. | **Yes** (`.did`) | Candid input change, **D1b**; existing check `candid_interface_matches_checked_in_did` (`lib.rs:423`) forces a regenerated `.did`. CBOR config stays readable by the old module for old variants. |
| C6 | `evm/rpc.rs:217-223` `to_rpc_service` | Map the C5 variants onto the mirror enum (`rpc.rs:130-139` already has `EthMainnet`, `EthSepolia`, `ArbitrumOne`). Robinhood, HyperEVM, Monad and every test network (including Base Sepolia, as in `foundry/DEPLOY-BASE.md`) use `Custom`. | **Yes** | none beyond C5 |
| C7 | `evm/rpc.rs:742-766` | `finalized_head()` becomes a dispatcher over `HeadRule` (section 3). Replace the tag read at `observer.rs:828`. | **Yes** | none |
| C8 | `lib.rs:41` | Salt label `rumi-icusd-{slug}-bridge:instance-salt:v1` (init only; the salt is persisted, so a Base instance is unaffected). | **Yes** (init path) | none |
| C9 | `icrc21.rs:98-145`, `views.rs:158` | Chain name and wait text from the profile; Base text byte-identical (tests `icrc21.rs:235-251`). Method names stay Base-named unless D1a. | **Yes** | none unless D1a |
| C10 | `state.rs:101-134,111,115`; `admin.rs:178-198` | Per-chain defaults at init (fee caps, gas limits, reserve) and, if measurement requires it, per-chain bounds for `MIN_MINT_GAS_LIMIT` (250,000) and `MAX_GAS_LIMIT` (1,000,000). Config via `admin_set_gas` needs no code. | **Yes** only if bounds move | none |
| C11 | `foundry/script/` | New sibling script with its own chain allow-list. Do **not** edit `DeployBridgedIcUSD.s.sol:27,34-35,83-91`: its tests assert chain ids 1 and 10143 are rejected (`foundry/test/DeployBridgedIcUSD.t.sol:131-139`) and pin the revert strings (`:117,373,382`). | No (new file) | none |
| - | `evm/{lane,signer,tx,abi}.rs`, `accounting.rs`, `ledger.rs`, `flows/*`, `timer.rs` | No change expected: chain-generic (RI 2a). The signer already refuses a chain mismatch (`signer.rs:137-145`). | n/a | none |

**Other decisions surfaced (D, not steps):** **D1c** keyed providers (Robinhood recommends Alchemy, S 4 row 1; `Custom` has no headers by design, `types.rs:63-65`): adding headers changes Candid and puts secrets in state. R: do not; find keyless independent endpoints or leave Robinhood blocked.

### 1.1 Base domain and id regression check (future implementation test, not a groundwork gate; it checks domains and ids only, not all Base behavior)

`ids::tests::base_slug_reproduces_live_strings` (new, in `ids.rs`). For chain ids 8453, 84532 and 31337 it asserts, against literals copied from the pre-change code:
- mint domain equals `rumi.icusd.base.mint.v1`, re-mint domain equals `rumi.icusd.base.remint.v1`, and `BRIDGE_OUT_DOMAIN` equals `rumi.icusd.base.bridge-out.v1` (a constant under every slug);
- `escrow_subaccount(chain)` equals SHA-256 of `rumi-icusd-base-bridge:escrow:v1` (same pin as `ids.rs:296-300`);
- `derivation_path(Minter|Admin, 8453)` equals the literal vector `[rumi-icusd-base-bridge, v1, minter|admin, 8453 as 8 bytes BE]`;
- the salt label equals `rumi-icusd-base-bridge:instance-salt:v1`;
- `mint_id` and `remint_id` for fixed inputs equal golden values computed with the old constants;
- every non-`base` slug yields strings that differ from every Base string.
Companions that already exist and must pass unmodified: `state::tests::init_validation_binds_key_to_network` (`state.rs:1483`) and the ICRC-21 tests above.

## 2. Per chain

Config-only = admin or install arguments on the Wasm after section 1. Code = needs section 1 items or new code. Blocker = profile stays disabled.

### 2.1 Base (reference row)
- Config: mainnet 8453 install with `key_1`, five built-in providers, quorum at least 2 (`state.rs:243,296-319`; `foundry/DEPLOY-BASE.md:256-258`); deploy script needs `ALLOW_BASE_MAINNET=true` (`DeployBridgedIcUSD.s.sol:87-88`).
- Code: none. Blockers: live status not established (RI 7); controller set (D6).

### 2.2 Ethereum (1 / 11155111; Hoodi 560048 is validator testing, not a target, S 1 row 1)
- Config: raise the gas caps; the default 2 gwei cap (`state.rs:121`) is Base-tuned, the hard ceiling is 1000 gwei (`admin.rs:183`). `finalized` is about 13 minutes (S 1 row 3). Deposit fee is hard-capped at 10 icUSD (`state.rs:175`); whether that covers Ethereum mint gas is unmeasured.
- Code: C1-C4, C6, C11. Native providers are the realistic path: S 1 row 4 found no primary public endpoint list, so a keyless 2-of-3 under `Custom` has no sourced providers; that makes C5 (D1b) effectively required for Ethereum (I).
- Blockers: D1, D1b; the token must not be registered with a canonical L2 bridge (S 1 row 7; G section 10).

### 2.3 Arbitrum One (42161 / 421614; Nova out of scope)
- Config: `finalized` is the parent-finalized batch, about 13 minutes after posting (S 3 row 3). Price floor 0.02 gwei One, 0.2 gwei Sepolia; tips ignored (S 3 row 5). The documented public endpoint is IPv4 only and keyless (S 3 row 1; S Q4).
- Code: C1-C6, C11; native `ArbitrumOne` preferred (five built-in providers, S section 0, built-in providers row).
  - Gas: the poster fee is part of gas used, `eth_estimateGas` includes it and moves with L1 data price, receipts carry `gasUsedForL1` (S 3 row 5).
  - The Base-derived limits 300,000 and 100,000 were measured on a local node with no poster component (`state.rs:124-128`); re-measure on Arbitrum before activation, and check whether `MAX_GAS_LIMIT` must rise (C10).
  - The reserve comment names the Base L1 fee (`state.rs:129`); on Arbitrum that cost is inside gas. Whether the mint simulation at a fixed `gas` argument (`lane.rs:1385`) includes the poster component is U.
- Blockers: D1, D1b; type 2 acceptance is not stated in S for Arbitrum (U, settled by a testnet round trip).

### 2.4 Robinhood Chain (4663 / 46630)
- Config: `Custom` only (S 4 row 4). Only one keyless public endpoint per network was found and public endpoints are "not recommended for production" (S 4 row 1), so quorum 2 needs more independent keyless providers than the facts give (`single-keyless-endpoint`).
- Code: C1-C4, C6, C11, plus the head rule probe (section 3). Gas notes as Arbitrum: two-part fee, `eth_estimateGas` covers both (S 4 row 5). Tx types are "not stated" (S 4 row 5); the bridge sends EIP-1559 only (`evm/tx.rs:22`).
- Blockers: `robinhood-finalized-tag-unverified` (S 4 row 3, S Q1: tags on dedicated Arbitrum chains need parent-chain finality exposed by the node); `single-keyless-endpoint`; ArbOS version and PUSH0 unconfirmed (S 4 row 8); D1c.

### 2.5 HyperEVM (999 / 998)
- Config: `Custom` 999/998.
  - Block gas limits: small 3M every 1 s, big 30M every 1 min (S 5 row 2). Deploy is about 2.8M gas on Base (S section 0, deploy gas row), a thin margin under 3M; big blocks need a HyperCore action (`evmUserModify`). A Foundry broadcast may pad its gas estimate (Foundry default, not in S: U), so set the limit explicitly.
  - Each address queues only the next 8 nonces and txs older than one day are pruned (S 5 row 5). The lane keeps at most one unincluded tx per lane (`state.rs:390`, `lane.rs:410-416`), so this fits (derived).
  - `eth_maxPriorityFeePerGas` is always 0 (S 5 row 4): zero priority is handled (`lane.rs:1754`).
  - `eth_getLogs` is limited to 50 blocks: set `logs_chunk_blocks` at most 50 (`admin.rs:217`; default 2,000, `state.rs:149`).
- Code: none can lift the blocker. Also the nonce sync requires `latest` and `pending` counts to agree (`lane.rs:1284-1300`) while S 5 row 3 says the default RPC serves `eth_getTransactionCount` for the latest block only (whether `pending` works is U).
- Blockers: `hyperevm-historical-state` (S addendum: default RPC serves `eth_call` at `latest` only; the observer needs numbered finalized state, `observer.rs:283-301,641,915-917`); one official endpoint (S 5 row 1); finality semantics unverified (S 5 row 3). Exit: archive-capable independent providers in quorum number, or a reviewed observer redesign (R: do not weaken a read to `latest`).

### 2.6 Monad (143 / 10143)
- Config: `Custom` 143/10143.
  - Providers: five official endpoints from different operators (S 6 row 4). Only the Foundation endpoint is noted as serving historical state (S 6 row 1), so numbered recent state on the others is U.
  - Fees: base fee minimum is 100 gwei (S 6 row 5), so the default 2 gwei cap (`state.rs:121`) can never land a transaction; set `max_fee` above it (ceiling `admin.rs:183`). The sender pays `gas_limit x price`, so the Base-derived 250,000 minimum limit (`state.rs:111`) costs more than the measured 152,390 used (`state.rs:124-125`).
  - Reserve: 10 MON per EOA (S 6 row 5). Set `min_gas_reserve_wei` at least that plus margin; `gas_sufficient` already adds it (`lane.rs:369-374`).
  - Log chunking: provider caps are 100 to 1,000 blocks (S 6 row 4; the backend uses 100, `chains/evm/mod.rs:78-84`); set `logs_chunk_blocks` at most 100. At 300 ms blocks, 5 chunks of 100 (`observer.rs:49`) cover about 150 s of chain time per pass (derived); the hourly full pass (`observer.rs:822`) leaves roughly 12,000 blocks to work off. Measure cycles.
- Code: Verified head (section 3), C7, the idle probe tag (`observer.rs:828`), C1-C4, C6, C11.
- **D8:** debt `IcUSD` and escrow token share name and symbol (section 0 item 3). Options: (a) identical bytecode with distinct labels in the frontend and token lists; (b) a Monad-specific relabeled contract (new bytecode, review, vectors). R: (a) for groundwork, and record that on-chain lookalike risk remains. The backend's Monad config knows only 10143 (`chains/evm/mod.rs:80`).
- **Never the registered debt contract:** `bind_contract` pins `VERSION()`, 8 decimals and zero supply, bridge-outs and `totalMinted` (`observer.rs:287-301`), and `IcUSD.sol` has no `VERSION`, so a debt token cannot be bound (F). The reverse (the backend refusing to register the escrow address) is a backend-side check outside this crate: an activation checklist item plus the G section 4 negative decoder test.
- Blockers: `monad-verified-head`; delay value unsourced (section 0 item 5); fast-tag consensus (item 6); D8; D5.

## 3. Observer head selection

R: one dispatcher, `rpc::settlement_head()` (today `finalized_head`, `evm/rpc.rs:742`), reading a `HeadRule` from the C2 row. Unsourced values fail closed with `NotConfigured`. **No rule falls back to `latest`**; today any failed or unparsed tag read already errors (`evm/rpc.rs:746-747`).

| Chain | `HeadRule` | Head | State |
|---|---|---|---|
| Base 8453/84532, Ethereum, Arbitrum | `TagFinalizedQuorum` | Today's rule: `n` = `min_quorum`-th highest `finalized` height, then the block hash at `n` by quorum (`rpc.rs:712-766`) | Existing code |
| Robinhood | `TagFinalizedQuorum`, gated | Same code, enabled only after a recorded probe: `finalized` lags `latest` by minutes, is stable, and at least 2 independent providers agree | Blocked until probe (S 4 row 3; S Q1) |
| HyperEVM | `Blocked` | none | Blocked (S addendum) |
| Monad | `MonadVerified { delay_blocks: Option<u64> }` | `n` as above, head = `n - delay_blocks`, block hash at the head by quorum; all numbered reads at that head | Blocked until `delay_blocks` is sourced and the rule is built (S addendum; block-states page) |

Plug points:
- `evm/rpc.rs:742-766`: split into `tag_finalized_quorum()` (the current body, unchanged) and the dispatcher. Reuse `agreed_height` and `tally` (`rpc.rs:671,712`) for the Monad subtraction.
- `observer.rs:283` (bind), `:598` (scan), `:914` (supply audit) and `evm/lane.rs:583` (`ChainIo::finalized_head`, used at `:782` for tx finality; `finality_precheck` takes only a number, `:446`) call the dispatcher, so they need no logic change.
- `observer.rs:828` `idle_probe_unchanged` reads `BlockTag::Finalized` directly: it must read `BlockTag::Number(head)` from the dispatcher (or be skipped for Monad), otherwise Monad's idle path would trust an ordering-only tag.
- Receipts for Monad are consumed only after `finality_precheck` passes against the Verified head, so `get_receipt` (`rpc.rs:880`) needs no change (I).
- Future unit tests (mock transport, `rpc.rs:343`): `evm::rpc::tests::monad_head_is_finalized_minus_delay`, `..._refuses_without_a_sourced_delay`, `..._never_sends_latest`, `..._blocked_rule_returns_not_configured`. The existing `finalized_head_*` tests (`rpc.rs:1322-1410`) must pass unchanged.

## 4. Contract reuse (D5)

- **Bytecode (F):** `BridgedIcUSD.sol` (solc 0.8.24, optimizer 200, `foundry/foundry.toml`) uses `block.timestamp` only and no `block.number` (S section 0, Base design row). A local, git-ignored build artifact (not repo evidence; rebuild to confirm) shows EVM version cancun, PUSH0 present, and none of MCOPY, TLOAD, TSTORE or the blob opcodes. So the floor is Shanghai.
- Per chain: Ethereum meets it (PUSH0 is EIP-3855, not in S; cite when verifying); Arbitrum: PUSH0 since ArbOS 11 (S 3 row 2); HyperEVM: Cancun without blobs (S 5 row 2); Monad: all Fusaka opcodes (S 6 row 2); **Robinhood: ArbOS version not stated, PUSH0 U (S 4 row 8)**, to be settled by a testnet deploy, not by assumption. Size is well under every limit (24,576 bytes on Ethereum, S 1 row 2).
- **What "Base" inside means (F):** `VERSION = "rumi-base-icusd-bridge-v1"` (`foundry/src/BridgedIcUSD.sol:55`) and `BRIDGE_OUT_DOMAIN` (`:61`) name Base. Consequences: (1) transfer ids stay unique across chains because they also hash `block.chainid` and `address(this)` (`:269-270`); (2) the bind check requires that exact `VERSION` on every chain (`observer.rs:290`), so it must not be slugged; (3) `VERSION()` is cosmetic on other chains and the frontend must not use it as a chain label; (4) a relabel is new bytecode, a new Foundry suite and new pinned vectors.
- R: identical bytecode on all six. No sourced fact contradicts it except the Monad name collision (2.6), which is a labeling decision.

## 5. Implementation sequence (future steps; nothing is run now)

Order follows G section 11: Ethereum, Arbitrum, Robinhood, then Monad and HyperEVM. Owners are assigned by BB-40 (the integrated plan). D1, D1a, D1b and D5 are settled for planning (design section 12); D6 stays open and gates only live activation, not these source steps.

| Step | Files | Focused check (later) | Size |
|---|---|---|---|
| S1 Profile table and slug builders, Base rows only | `ids.rs`, `state.rs`, new table | `cargo test -p rumi_icusd_bridge --lib ids::tests` (includes the 1.1 test) | S |
| S2 Table-driven init and RPC validation, `HeadRule` dispatcher with `TagFinalizedQuorum` and `Blocked`; Ethereum and Arbitrum rows (disabled) | `state.rs`, `evm/rpc.rs`, `observer.rs:828` | `cargo test -p rumi_icusd_bridge --lib state::tests::init_validation` and `evm::rpc::tests::finalized_head` | M |
| S3 Native provider variants (only if D1b = yes) | `types.rs`, `evm/rpc.rs:217`, `rumi_icusd_bridge.did` | `cargo test -p rumi_icusd_bridge --lib candid_interface_matches_checked_in_did` plus new `evm::rpc::tests::to_rpc_service` | S |
| S4 Ethereum: sibling deploy script and tests, gas and fee-cap measurement on Sepolia | `foundry/script`, `foundry/test`, `state.rs` defaults | `forge test --match-contract <new Ethereum deploy test>` (run in `foundry/`) | M |
| S5 Arbitrum: re-measure gas with the poster component, decide C10 bounds, type 2 check on 421614 | `state.rs:111,115`, `admin.rs:178` | `cargo test -p rumi_icusd_bridge --lib admin::tests` (gas bounds) and the `forge` test as S4 | M |
| S6 Robinhood: independent keyless provider search, `finalized` tag probe record, PUSH0 and type 2 check on 46630 | profile notes, evidence only | read-only probe script (to be written; no probe is run here) | S code, M evidence |
| S7 Monad: source `delay_blocks`, implement `MonadVerified`, fast-tag consensus probe, D8 labels, gas and reserve and log-chunk config | `evm/rpc.rs`, `observer.rs:828`, profile | `cargo test -p rumi_icusd_bridge --lib evm::rpc::tests::monad` | L |
| S8 HyperEVM: held; exit is independent archive-capable providers or a reviewed observer redesign (design first, then the same pass as S7) | none until the exit is met | none | XL |

Frontend actor lookup by `network_id` belongs to BB-39, not here.

## 6. Not grounded in source

- Monad execution delay value (facts conflict, item 5); Robinhood and HyperEVM `finalized` tag support; Robinhood ArbOS and tx types; Arbitrum and Robinhood type 2 acceptance; whether Arbitrum `eth_call` at a fixed gas includes the poster component.
- Consensus behavior of fast-moving tags through the EVM RPC canister (item 6); numbered-state support of non-Foundation Monad endpoints; `pending` nonce support on HyperEVM.
- Mint gas on Arbitrum, Robinhood and Monad (only the local-node figure 152,390 exists); Ethereum mint gas cost versus the 10 icUSD fee cap.
- Foundry's default gas padding on a 3M-gas chain; CI wiring of the `.did` check was not read (only `lib.rs:423`).
- Controller set and live status of any instance (D6; RI 7).

## Coordinator notes (2026-10-30, on acceptance)

- Monad delay: sourced as 3 blocks (Monad developer summary; see the reconciliation at the end of `sources-evm.md`). The
  Verified-head change in the observer is still required, so the Monad blocker stays.
- Decisions raised here are recorded in the design's section 12: D1a (one Wasm has one Candid, so per-instance method
  names need either the Base names on every EVM instance or additive generic methods), D1c (keyed RPC providers versus
  the no-secrets-in-state rule), and the D5 and D8 coupling (identical bytecode hard-codes `ERC20("Rumi icUSD", "icUSD")`
  at `foundry/src/BridgedIcUSD.sol:157`, so a distinct Monad name needs a new reviewed contract or a label-only answer).
- Decisions applied (Miles, 2026-10-30): D1 in place on a working branch with no deployed upgrade; D1a legacy Base-named methods kept on new EVM instances; D1b additive native RPC variants where useful; D5 identical bytecode; D8 explicit escrow-backed labels and exact binding on Monad. All execution and deploy checks listed here are future implementation work, not groundwork gates.
