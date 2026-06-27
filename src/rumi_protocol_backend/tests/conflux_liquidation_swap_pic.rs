//! Conflux eSpace testnet (chain 71) Increment-3 Tier-1 bot LIQUIDATION SWAP
//! end-to-end integration test against the scripted mock EVM RPC canister.
//!
//! This is the integration PROOF that the Increment-3 bot SWAP executes the full
//! two-phase liquidation against the EVM-RPC mock:
//!
//!   1. DETECTION (Increment 2, reused verbatim): the observer tick marks an
//!      underwater Open vault with a `pending_liquidation` (Bot tier) marker,
//!      reserving its collateral and enqueueing a `LiquidationSwap` op. Debt +
//!      `chain_supplies` are UNCHANGED at trigger (Design B).
//!   2. SWAP SUBMIT (Increment 3): the settlement worker reads the DEX
//!      token0/getReserves at the finalized block, computes a JIT min-out gated by
//!      the oracle cross-check, nets gas via `fundable_swap_value`, signs with the
//!      vault's custody key, and broadcasts a `swapExactETHForTokens` to the
//!      router (CFX in `value`, USDC `to` = the tECDSA reserve address). The op
//!      goes Inflight.
//!   3. SWAP CONFIRM (Increment 3): once the receipt is mined + final, the worker
//!      reads the REALIZED USDC output from the `Transfer(_, reserve, amount)` log
//!      (never min-out) and moves `debt_e8s -> reserve_backing_e8s` (the ONLY
//!      invariant move — NO icUSD burned, this is a PSM). The vault drains fully
//!      to debt 0 and Closes; `chain_supplies` is UNCHANGED.
//!
//! The reserve is BACKING, not unbacked supply (findings #17/#20/#29):
//! `reconcile_chain_supply` reports `reserve_backing_e8s` + `reserve_usdc_native`
//! as informational RHS terms and `unbacked_excess == false` (on-chain totalSupply
//! still == recorded chain supply).
//!
//! Harness (boot, register_chain, deposit->mint->Open sequence, ECDSA-gating,
//! supply invariant, candid mirrors) is COPIED from
//! `conflux_liquidation_detection_pic.rs`. As in that test, the flow PROBES ECDSA
//! via `get_chain_settlement_address`; if ECDSA is unavailable in this PocketIC
//! build it runs a GATED subset (which still proves the new config fields decode +
//! the reserve/settlement address endpoints signal ECDSA-unavailable) and returns
//! early. The swap is NEVER faked.

use candid::{decode_one, encode_args, encode_one, CandidType, Decode, Deserialize, Encode, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use std::time::Duration;

// ─── Locally-mirrored backend types (shapes mirror src/.../*.rs exactly) ─────

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ProtocolInitArg {
    xrc_principal: Principal,
    icusd_ledger_principal: Principal,
    icp_ledger_principal: Principal,
    fee_e8s: u64,
    developer_principal: Principal,
    treasury_principal: Option<Principal>,
    stability_pool_principal: Option<Principal>,
    ckusdt_ledger_principal: Option<Principal>,
    ckusdc_ledger_principal: Option<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolArg {
    Init(ProtocolInitArg),
}

// ─── ICRC-1 / Stability Pool mirrors used by the Inc-4 e2e ───────────────────

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Account {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct FeatureFlags {
    icrc2: bool,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ArchiveOptions {
    num_blocks_to_archive: u64,
    trigger_threshold: u64,
    controller_id: Principal,
    max_transactions_per_response: Option<u64>,
    max_message_size_bytes: Option<u64>,
    cycles_for_archive_creation: Option<u64>,
    node_max_memory_size_bytes: Option<u64>,
    more_controller_ids: Option<Vec<Principal>>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum MetadataValue {
    Nat(Nat),
    Int(candid::Int),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct LedgerInitArgs {
    minting_account: Account,
    fee_collector_account: Option<Account>,
    transfer_fee: Nat,
    decimals: Option<u8>,
    max_memo_length: Option<u16>,
    token_name: String,
    token_symbol: String,
    metadata: Vec<(String, MetadataValue)>,
    initial_balances: Vec<(Account, Nat)>,
    feature_flags: Option<FeatureFlags>,
    maximum_number_of_accounts: Option<u64>,
    accounts_overflow_trim_quantity: Option<u64>,
    archive_options: ArchiveOptions,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum LedgerArg {
    Init(LedgerInitArgs),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ApproveArgs {
    from_subaccount: Option<[u8; 32]>,
    spender: Account,
    amount: Nat,
    expected_allowance: Option<Nat>,
    expires_at: Option<u64>,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ApproveError {
    BadFee { expected_fee: Nat },
    InsufficientFunds { balance: Nat },
    AllowanceChanged { current_allowance: Nat },
    Expired { ledger_time: u64 },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct StabilityPoolInitArgs {
    protocol_canister_id: Principal,
    authorized_admins: Vec<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct StablecoinConfig {
    ledger_id: Principal,
    symbol: String,
    decimals: u8,
    priority: u8,
    is_active: bool,
    transfer_fee: Option<u64>,
    is_lp_token: Option<bool>,
    underlying_pool: Option<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
struct ChainId(u32);

#[derive(CandidType, Deserialize, Clone, Debug)]
enum GasStrategy {
    EvmEip1559 {
        max_priority_fee_gwei: u64,
        max_fee_gwei_ceiling: u64,
    },
    EvmLegacy {
        gas_price_gwei_ceiling: u64,
    },
    SolanaPriorityFee {
        lamports_per_cu_ceiling: u64,
    },
    NotApplicable,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct RegisterChainArg {
    chain_id: ChainId,
    display_name: String,
    rpc_endpoints: Vec<String>,
    finality_depth: u32,
    gas_strategy: GasStrategy,
    chain_native_decimals: u8,
    min_quorum_providers: Option<u32>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
enum ChainVaultStatus {
    AwaitingDeposit,
    MintPending,
    Open,
    Closing,
    Closed,
}

// ─── Liquidation mirrors ──────────────────────────────────────────────────────

/// Mirror of `chains::vault::LiquidationTier`.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
enum LiquidationTier {
    Bot,
    StabilityPool,
}

/// Mirror of `chains::vault::PendingLiquidationV1` (the in-flight marker).
#[derive(CandidType, Deserialize, Clone, Debug)]
struct PendingLiquidationV1 {
    op_id: u64,
    debt_to_clear_e8s: candid::Nat,
    collateral_reserved_native: candid::Nat,
    tier: LiquidationTier,
    started_at_ns: u64,
}

/// Mirror of `chains::vault::ChainVaultV1`, EXTENDED with `pending_liquidation`.
/// Candid records decode by field name and ignore extra wire fields, so the
/// happy-path subset of fields + the marker field is sufficient. NOTE: the wire
/// field for the native collateral amount is `collateral_amount_e18`.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct ChainVaultV1 {
    vault_id: u64,
    owner: Principal,
    collateral_chain: ChainId,
    custody_address: String,
    collateral_amount_e18: candid::Nat,
    debt_e8s: candid::Nat,
    mint_recipient: String,
    pending_mint_e8s: candid::Nat,
    status: ChainVaultStatus,
    opened_at_ns: u64,
    owner_evm: Option<String>,
    pending_liquidation: Option<PendingLiquidationV1>,
}

/// Mirror of `chains::liquidation_config::DexKind`.
#[derive(CandidType, Deserialize, Clone, Debug)]
enum DexKind {
    UniswapV2,
}

/// Mirror of `chains::liquidation_config::ChainLiquidationConfigV1`, INCLUDING the
/// Increment-2 fields (`max_swap_value_e8s`, `max_price_age_ns`) AND the four
/// Increment-3 fields (`max_dex_oracle_divergence_bps`, `fee_bps`,
/// `settle_stable_decimals`, `deadline_secs`). Field set + types verified against
/// `rumi_protocol_backend.did`'s `type ChainLiquidationConfigV1`.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct ChainLiquidationConfigV1 {
    dex: DexKind,
    router: String,
    factory: String,
    pair: String,
    collateral_token: String,
    settle_stable_token: String,
    slippage_cap_bps: u16,
    restore_target_cr_e4: u64,
    enabled: bool,
    max_swap_value_e8s: candid::Nat,
    max_price_age_ns: u64,
    // ── Increment 3 ──
    max_dex_oracle_divergence_bps: u32,
    fee_bps: u16,
    settle_stable_decimals: u8,
    deadline_secs: u64,
}

/// Mirror of `main::ChainSupplyReconciliation`. Field set + types verified against
/// `rumi_protocol_backend.did`'s `type ChainSupplyReconciliation` (10 fields). The
/// swap assertions only read `reserve_backing_e8s` / `reserve_usdc_native` /
/// `unbacked_excess`, but the full field set is mirrored so candid decodes.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct ChainSupplyReconciliation {
    chain_id: ChainId,
    finalized_block: u64,
    onchain_total_supply_e8s: candid::Nat,
    recorded_supply_e8s: candid::Nat,
    in_flight_mint_e8s: candid::Nat,
    unbacked_excess: bool,
    gap_e8s: candid::Int,
    reserve_backing_e8s: candid::Nat,
    pending_chain_burn_e8s: candid::Nat,
    reserve_usdc_native: candid::Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct SupplyAuditEntryWire {
    chain_id: u32,
    display_name: String,
    supply_e8s: candid::Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct SupplyAuditWire {
    total_e8s: candid::Nat,
    per_chain: Vec<SupplyAuditEntryWire>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum TransferError {
    GenericError { message: String, error_code: Nat },
    TemporarilyUnavailable,
    BadBurn { min_burn_amount: Nat },
    Duplicate { duplicate_of: Nat },
    BadFee { expected_fee: Nat },
    CreatedInFuture { ledger_time: u64 },
    TooOld,
    InsufficientFunds { balance: Nat },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum TransferFromError {
    GenericError { message: String, error_code: Nat },
    TemporarilyUnavailable,
    InsufficientAllowance { allowance: Nat },
    BadBurn { min_burn_amount: Nat },
    Duplicate { duplicate_of: Nat },
    BadFee { expected_fee: Nat },
    CreatedInFuture { ledger_time: u64 },
    TooOld,
    InsufficientFunds { balance: Nat },
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolError {
    ChainAdmin(String),
    EvmAuth(String),
    TemporarilyUnavailable(String),
    GenericError(String),
    TransferError(TransferError),
    TransferFromError(TransferFromError, u64),
    AlreadyProcessing,
    NotLowestCR,
    SupplyInvariantHalted,
    AnonymousCallerNotAllowed,
    AmountTooLow { minimum_amount: u64 },
    CallerNotOwner,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum StabilityPoolError {
    InsufficientBalance { token: Principal, required: u64, available: u64 },
    AmountTooLow { minimum_e8s: u64 },
    NoPositionFound,
    InsufficientPoolBalance,
    Unauthorized,
    TokenNotAccepted { ledger: Principal },
    TokenNotActive { ledger: Principal },
    CollateralNotFound { ledger: Principal },
    LedgerTransferFailed { reason: String },
    InterCanisterCallFailed { target: String, method: String },
    LiquidationFailed { vault_id: u64, reason: String },
    EmergencyPaused,
    SystemBusy,
    AlreadyOptedOut { collateral: Principal },
    AlreadyOptedIn { collateral: Principal },
    RefundClaimNotFound,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ChainLiquidatableVaultInfo {
    vault_id: u64,
    chain_id: ChainId,
    chain_collateral_sentinel: Principal,
    sp_attempted: bool,
    debt_e8s: u128,
    effective_debt_e8s: u128,
    collateral_native: u128,
    cr_e4: u64,
    liquidation_threshold_e4: u64,
    sized_repay_e8s: u128,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct ChainSpAbsorbResult {
    success: bool,
    vault_id: u64,
    chain_id: ChainId,
    icusd_burned_e8s: u64,
    liquidated_debt_e8s: u128,
    collateral_received_native: u128,
    claim_id: u64,
    custody_address: String,
    block_index: u64,
    collateral_price_e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct HttpRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct HttpResponse {
    status_code: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(Deserialize, Debug)]
struct LogEntryWire {
    #[allow(dead_code)]
    timestamp: u64,
    message: String,
}

#[derive(Deserialize, Debug)]
struct LogWire {
    entries: Vec<LogEntryWire>,
}

// ─── Wasm loaders ────────────────────────────────────────────────────────────

fn backend_wasm() -> Vec<u8> {
    read_workspace_wasm("rumi_protocol_backend.wasm")
}

fn mock_wasm() -> Vec<u8> {
    read_workspace_wasm("monad_rpc_mock.wasm")
}

fn stability_pool_wasm() -> Vec<u8> {
    read_workspace_wasm("stability_pool.wasm")
}

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn read_workspace_wasm(name: &str) -> Vec<u8> {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest_dir.join("../../target/wasm32-unknown-unknown/release").join(name),
        manifest_dir.join("../target/wasm32-unknown-unknown/release").join(name),
        manifest_dir.join("../../../../target/wasm32-unknown-unknown/release").join(name),
    ];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(&path) {
            return bytes;
        }
    }
    panic!("missing wasm artifact {name} in worktree target/, worktree src/target/, or main checkout target/");
}

// ─── Constants ───────────────────────────────────────────────────────────────

const CONFLUX_CHAIN_ID: u32 = 71;
const E18: u128 = 1_000_000_000_000_000_000;
const E8: u128 = 100_000_000;
/// keccak256("Mint(uint256,address,uint256)"), must match evm_rpc.rs.
const MINT_EVENT_TOPIC0: &str =
    "0x4e3883c75cc9c752bb1db2e406a822e4a75067ae77ad9a0a4d179f2709b9e1f6";
/// keccak256("Transfer(address,address,uint256)"), must match evm_rpc.rs.
const TRANSFER_TOPIC0: &str =
    "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
/// UniswapV2 pair `token0()` selector (must match evm_rpc.rs `TOKEN0_SELECTOR`).
const TOKEN0_SELECTOR: &str = "0x0dfe1681";
/// UniswapV2 pair `getReserves()` selector (must match `GET_RESERVES_SELECTOR`).
const GET_RESERVES_SELECTOR: &str = "0x0902f1ac";
/// UniswapV2 factory `getPair(address,address)` selector.
const GET_PAIR_SELECTOR: &str = "0xe6a43905";

/// Conflux register arg's `finality_depth` (mirrors conflux_testnet_register_arg()).
const CONFLUX_FINALITY_DEPTH: u64 = 100;
/// Observer block-scan window (`MAX_BLOCK_SCAN_WINDOW`).
const SCAN_WINDOW: u64 = 1024;

/// The specific DEX wiring addresses (valid 42-char 0x). The collateral_token and
/// settle_stable_token are REUSED below for the canned DEX reads + the Transfer
/// log contract, so they must be byte-identical everywhere.
const DEX_ROUTER: &str = "0x1111111111111111111111111111111111111111";
const DEX_FACTORY: &str = "0x2222222222222222222222222222222222222222";
const DEX_PAIR: &str = "0x3333333333333333333333333333333333333333";
/// WCFX (the collateral token + path[0]).
const WCFX: &str = "0x14b2d3bc65e74dae1030eafd8ac30c533c976a9b";
/// USDC (the settle stable token + path[1]); 18-dec on eSpace.
const USDC: &str = "0x6963efed0ab40f6c3d7bda44a05dcf1437c44372";

// ─── PocketIC call helpers ───────────────────────────────────────────────────

fn dev() -> Principal {
    Principal::from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9])
}

fn depositor() -> Principal {
    Principal::self_authenticating(b"inc4-cfx-sp-depositor")
}

fn minting_account_owner() -> Principal {
    Principal::self_authenticating(b"inc4-icusd-minter")
}

fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

fn query_unit<T>(pic: &PocketIc, cid: Principal, method: &str) -> T
where
    T: CandidType + for<'a> Deserialize<'a>,
{
    let reply = pic
        .query_call(
            cid,
            Principal::anonymous(),
            method,
            encode_one(()).expect("encode unit"),
        )
        .expect("query call");
    match reply {
        WasmResult::Reply(b) => Decode!(&b, T).expect("decode reply"),
        WasmResult::Reject(msg) => panic!("query {} rejected: {}", method, msg),
    }
}

fn update_dev(pic: &PocketIc, cid: Principal, method: &str, args: Vec<u8>) -> WasmResult {
    pic.update_call(cid, dev(), method, args)
        .unwrap_or_else(|e| panic!("update {} failed: {}", method, e))
}

fn update_any(pic: &PocketIc, cid: Principal, method: &str, args: Vec<u8>) -> WasmResult {
    pic.update_call(cid, Principal::anonymous(), method, args)
        .unwrap_or_else(|e| panic!("update {} failed: {}", method, e))
}

fn update_as(
    pic: &PocketIc,
    cid: Principal,
    caller: Principal,
    method: &str,
    args: Vec<u8>,
) -> WasmResult {
    pic.update_call(cid, caller, method, args)
        .unwrap_or_else(|e| panic!("update {} failed: {}", method, e))
}

fn decode_result(reply: WasmResult, method: &str) -> Result<(), ProtocolError> {
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Result<(), ProtocolError>)
            .unwrap_or_else(|e| panic!("decode {} Result: {}", method, e)),
        WasmResult::Reject(msg) => panic!("{} rejected: {}", method, msg),
    }
}

// ─── Boot: II subnet (for tECDSA) + application subnet (backend + mock) ───────

fn boot() -> (PocketIc, Principal, Principal) {
    let pic = PocketIcBuilder::new()
        .with_ii_subnet()
        .with_application_subnet()
        .build();

    let backend_id = pic.create_canister();
    pic.add_cycles(backend_id, 100_000_000_000_000);
    let mock_id = pic.create_canister();
    pic.add_cycles(mock_id, 100_000_000_000_000);

    let mgmt = Principal::from_text("aaaaa-aa").expect("mgmt principal");
    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: mgmt,
        icusd_ledger_principal: mgmt,
        icp_ledger_principal: mgmt,
        fee_e8s: 10_000,
        developer_principal: dev(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(
        backend_id,
        backend_wasm(),
        encode_args((init,)).expect("encode init"),
        None,
    );
    pic.install_canister(mock_id, mock_wasm(), Encode!().expect("encode mock init"), None);

    for _ in 0..5 {
        pic.tick();
    }

    // Pin observer + settlement cadence to 30s (code default is 300s).
    let _ = update_dev(&pic, backend_id, "set_observer_tick_interval_secs", Encode!(&30u64).unwrap());
    let _ = update_dev(&pic, backend_id, "set_settlement_tick_interval_secs", Encode!(&30u64).unwrap());

    (pic, backend_id, mock_id)
}

fn install_icrc1_ledger(
    pic: &PocketIc,
    ledger_id: Principal,
    user: Principal,
    initial_balance: u128,
) {
    let init = LedgerArg::Init(LedgerInitArgs {
        minting_account: account(minting_account_owner()),
        fee_collector_account: None,
        transfer_fee: Nat::from(0u64),
        decimals: Some(8),
        max_memo_length: Some(64),
        token_name: "Rumi icUSD".to_string(),
        token_symbol: "icUSD".to_string(),
        metadata: vec![],
        initial_balances: vec![(account(user), Nat::from(initial_balance))],
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 2_000,
            trigger_threshold: 1_000,
            controller_id: dev(),
            max_transactions_per_response: None,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            more_controller_ids: None,
        },
    });
    pic.install_canister(
        ledger_id,
        icrc1_ledger_wasm(),
        encode_args((init,)).expect("encode ledger init"),
        None,
    );
}

fn boot_with_sp() -> (PocketIc, Principal, Principal, Principal, Principal, Principal) {
    let pic = PocketIcBuilder::new()
        .with_ii_subnet()
        .with_application_subnet()
        .build();

    let backend_id = pic.create_canister();
    pic.add_cycles(backend_id, 100_000_000_000_000);
    let mock_id = pic.create_canister();
    pic.add_cycles(mock_id, 100_000_000_000_000);
    let icusd_ledger = pic.create_canister();
    pic.add_cycles(icusd_ledger, 10_000_000_000_000);
    let sp_id = pic.create_canister();
    pic.add_cycles(sp_id, 10_000_000_000_000);

    let user = depositor();
    install_icrc1_ledger(&pic, icusd_ledger, user, 1_000u128 * E8);

    let mgmt = Principal::from_text("aaaaa-aa").expect("mgmt principal");
    let init = ProtocolArg::Init(ProtocolInitArg {
        xrc_principal: mgmt,
        icusd_ledger_principal: icusd_ledger,
        icp_ledger_principal: mgmt,
        fee_e8s: 10_000,
        developer_principal: dev(),
        treasury_principal: None,
        stability_pool_principal: Some(sp_id),
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    pic.install_canister(
        backend_id,
        backend_wasm(),
        encode_args((init,)).expect("encode init"),
        None,
    );
    pic.install_canister(mock_id, mock_wasm(), Encode!().expect("encode mock init"), None);

    let sp_init = StabilityPoolInitArgs {
        protocol_canister_id: backend_id,
        authorized_admins: vec![dev()],
    };
    pic.install_canister(
        sp_id,
        stability_pool_wasm(),
        encode_one(sp_init).expect("encode sp init"),
        None,
    );
    pic.set_controllers(backend_id, None, vec![Principal::anonymous(), dev()])
        .expect("set backend controllers after install");

    for _ in 0..5 {
        pic.tick();
    }

    let _ = update_dev(&pic, backend_id, "set_observer_tick_interval_secs", Encode!(&30u64).unwrap());
    let _ = update_dev(&pic, backend_id, "set_settlement_tick_interval_secs", Encode!(&30u64).unwrap());

    (pic, backend_id, mock_id, sp_id, icusd_ledger, user)
}

/// Tick the canister timers across `windows` 35s windows so the observer (Timer
/// A) and settlement (Timer D) timers fire and their async futures run.
fn advance_and_tick(pic: &PocketIc, windows: u32) {
    for _ in 0..windows {
        pic.advance_time(Duration::from_secs(35));
        for _ in 0..10 {
            pic.tick();
        }
    }
}

// ─── Supply-invariant assertion (PSM: a liquidation swap does NOT move supply) ─

fn assert_supply(pic: &PocketIc, cid: Principal, expected_e8s: u128, step: &str) {
    let global: candid::Nat = query_unit(pic, cid, "get_global_icusd_supply");
    assert_eq!(
        global,
        candid::Nat::from(expected_e8s),
        "[{step}] get_global_icusd_supply mismatch"
    );

    let audit: SupplyAuditWire = query_unit(pic, cid, "get_supply_audit");
    assert_eq!(
        audit.total_e8s,
        candid::Nat::from(expected_e8s),
        "[{step}] supply_audit.total mismatch"
    );
    let sum: candid::Nat = audit
        .per_chain
        .iter()
        .fold(candid::Nat::from(0u32), |acc, e| acc + e.supply_e8s.clone());
    assert_eq!(audit.total_e8s, sum, "[{step}] audit total != sum(per_chain)");

    if let Some(conflux) = audit.per_chain.iter().find(|e| e.chain_id == CONFLUX_CHAIN_ID) {
        assert_eq!(
            conflux.supply_e8s,
            candid::Nat::from(expected_e8s),
            "[{step}] Conflux per-chain supply mismatch"
        );
    }
}

// ─── 32-byte ABI word encoding for log topics / data / eth_call returns ───────

fn word_u128(v: u128) -> String {
    format!("0x{:064x}", v)
}

fn word_addr(addr: &str) -> String {
    let raw = addr.strip_prefix("0x").or_else(|| addr.strip_prefix("0X")).unwrap_or(addr);
    format!("0x{:0>64}", raw.to_lowercase())
}

fn script_factory_pair_sanity(pic: &PocketIc, mock: Principal, pair: &str) {
    let _ = update_any(
        pic,
        mock,
        "set_eth_call_response",
        Encode!(&GET_PAIR_SELECTOR.to_string(), &word_addr(pair)).unwrap(),
    );
}

// ─── liquidation config builder (ENABLED, with the Increment-3 fields) ────────

/// The ENABLED Conflux liquidation config used throughout this test. Uses the
/// SPECIFIC WCFX/USDC token addresses (reused for the canned DEX reads + the
/// realized Transfer log). 18-dec USDC, 0.25% pool fee, 2.5% slippage cap, 5%
/// oracle-divergence ceiling, 155% restore target, $2k depth cap, 30-min
/// staleness ceiling, 180s on-chain deadline.
fn enabled_liq_config() -> ChainLiquidationConfigV1 {
    ChainLiquidationConfigV1 {
        dex: DexKind::UniswapV2,
        router: DEX_ROUTER.to_string(),
        factory: DEX_FACTORY.to_string(),
        pair: DEX_PAIR.to_string(),
        collateral_token: WCFX.to_string(),
        settle_stable_token: USDC.to_string(),
        slippage_cap_bps: 250,
        restore_target_cr_e4: 15_500,
        enabled: true,
        max_swap_value_e8s: candid::Nat::from(2_000u128 * E8),
        max_price_age_ns: 1_800_000_000_000, // 30 min
        max_dex_oracle_divergence_bps: 500,
        fee_bps: 25,
        settle_stable_decimals: 18,
        deadline_secs: 180,
    }
}

// ─── The test ────────────────────────────────────────────────────────────────

#[test]
fn conflux_liquidation_swap_executes_and_credits_reserve() {
    let (pic, backend, mock) = boot();

    // ── Step 1: point the backend's EVM wrapper at the mock + Conflux quirks ──
    decode_result(
        update_dev(&pic, backend, "set_evm_rpc_principal", Encode!(&mock).unwrap()),
        "set_evm_rpc_principal",
    )
    .expect("set_evm_rpc_principal");
    update_any(&pic, mock, "set_getlogs_max_range", Encode!(&1000u64).unwrap());
    update_any(&pic, mock, "set_espace_receipt_fields", Encode!(&true).unwrap());

    // ── Step 2: register Conflux + contract + manual CFX price ($0.15) ───────
    let reg = RegisterChainArg {
        chain_id: ChainId(CONFLUX_CHAIN_ID),
        display_name: "ConfluxESpaceTestnet".to_string(),
        rpc_endpoints: vec!["https://evmtestnet.confluxrpc.com".to_string()],
        finality_depth: CONFLUX_FINALITY_DEPTH as u32,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 1,
            max_fee_gwei_ceiling: 100,
        },
        chain_native_decimals: 18,
        min_quorum_providers: Some(1),
    };
    decode_result(
        update_dev(&pic, backend, "register_chain", Encode!(&reg).unwrap()),
        "register_chain",
    )
    .expect("register_chain");

    decode_result(
        update_dev(
            &pic,
            backend,
            "set_chain_contract",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &"0x00000000000000000000000000000000cf1c0de5".to_string())
                .unwrap(),
        ),
        "set_chain_contract",
    )
    .expect("set_chain_contract");

    // $0.15 / CFX in e8 => 15_000_000. Healthy price for the open.
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_manual_collateral_price",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &"CFX".to_string(), &15_000_000u64).unwrap(),
        ),
        "set_manual_collateral_price",
    )
    .expect("set_manual_collateral_price");

    // ── Seed the burn-watch cursor + enable poll-scan (harness parity) ───────
    let seed: u64 = 1_000_000;
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_last_observed_block",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &seed).unwrap(),
        ),
        "set_last_observed_block",
    )
    .expect("set_last_observed_block");
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_burn_watch_poll_enabled",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &true).unwrap(),
        ),
        "set_burn_watch_poll_enabled",
    )
    .expect("set_burn_watch_poll_enabled");

    assert_supply(&pic, backend, 0, "after register/config");

    // ── Block plan (finality_depth = 100) ────────────────────────────────────
    // The mint receipt + log AND the swap receipt + Transfer log all land at the
    // SAME finalized cursor (cursor1), the deepest block the observer + settlement
    // finality gate treat as final.
    let cursor1 = seed + SCAN_WINDOW;
    let head1 = cursor1 + CONFLUX_FINALITY_DEPTH + 24;
    update_any(&pic, mock, "set_blocks", Encode!(&head1, &head1).unwrap());
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xcfxmint1".to_string()).unwrap());

    // ── ECDSA probe: decide full vs gated ────────────────────────────────────
    let settlement_addr = match update_dev(
        &pic,
        backend,
        "get_chain_settlement_address",
        Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap(),
    ) {
        WasmResult::Reply(b) => match Decode!(&b, Result<String, ProtocolError>) {
            Ok(Ok(addr)) => Some(addr),
            Ok(Err(e)) => {
                eprintln!("[swap] ECDSA UNAVAILABLE (get_chain_settlement_address returned Err: {e:?}); running GATED subset");
                None
            }
            Err(decode_err) => {
                eprintln!("[swap] get_chain_settlement_address decode error ({decode_err}); running GATED subset");
                None
            }
        },
        WasmResult::Reject(msg) => {
            eprintln!("[swap] ECDSA UNAVAILABLE (get_chain_settlement_address rejected: {msg}); running GATED subset");
            None
        }
    };

    let settlement_addr = match settlement_addr {
        Some(addr) => {
            eprintln!("[swap] ECDSA AVAILABLE; settlement address = {addr}; running FULL swap path");
            addr
        }
        None => {
            // ── GATED subset: the new config (incl. the 4 Increment-3 fields)
            // decodes + round-trips, and the reserve/settlement address endpoints
            // correctly signal ECDSA-unavailable. We do NOT fake the swap.
            script_factory_pair_sanity(&pic, mock, DEX_PAIR);
            decode_result(
                update_dev(
                    &pic,
                    backend,
                    "set_chain_liquidation_config",
                    Encode!(&ChainId(CONFLUX_CHAIN_ID), &enabled_liq_config()).unwrap(),
                ),
                "set_chain_liquidation_config",
            )
            .expect("gated: set_chain_liquidation_config (proves the 4 Inc-3 fields decode)");

            let cfg = get_liq_config(&pic, backend).expect("gated: config round-trips");
            assert_eq!(cfg.max_dex_oracle_divergence_bps, 500, "gated: divergence bps round-trips");
            assert_eq!(cfg.fee_bps, 25, "gated: fee_bps round-trips");
            assert_eq!(cfg.settle_stable_decimals, 18, "gated: settle decimals round-trips");
            assert_eq!(cfg.deadline_secs, 180, "gated: deadline_secs round-trips");
            assert_eq!(cfg.collateral_token, WCFX, "gated: collateral_token round-trips");
            assert_eq!(cfg.settle_stable_token, USDC, "gated: settle_stable_token round-trips");
            assert!(cfg.enabled, "gated: enabled round-trips");

            // The reserve-address endpoint must ALSO signal ECDSA-unavailable.
            match get_reserve_address(&pic, backend) {
                Err(_) => { /* expected: ECDSA off */ }
                Ok(a) => panic!("gated: reserve address returned Ok({a}) but settlement ECDSA was unavailable"),
            }

            assert_supply(&pic, backend, 0, "gated: ECDSA unavailable, invariant at 0");
            eprintln!("[swap] ECDSA unavailable; ran gated subset (Inc-3 config decodes; reserve/settlement signal ECDSA-off)");
            return;
        }
    };

    // ════════════════════════════════════════════════════════════════════════
    // FULL SWAP PATH (ECDSA available)
    // ════════════════════════════════════════════════════════════════════════

    // Fund the settlement (hot-wallet) address so the mint submit-path gas gate passes.
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&settlement_addr, &candid::Nat::from(1_000_000u128 * E18)).unwrap(),
    );

    // ── Step 1: Open a vault (1400 CFX @ $0.15 = $210 backing 100 icUSD = 210% CR) ──
    let collateral_e18 = 1_400u128 * E18;
    let debt_e8s = 100u128 * E8;
    let recipient = "0x000000000000000000000000000000000000c0de".to_string();
    let vault_id: u64 = match update_dev(
        &pic,
        backend,
        "open_chain_vault",
        Encode!(
            &ChainId(CONFLUX_CHAIN_ID),
            &candid::Nat::from(collateral_e18),
            &candid::Nat::from(debt_e8s),
            &recipient
        )
        .unwrap(),
    ) {
        WasmResult::Reply(b) => Decode!(&b, Result<u64, ProtocolError>)
            .expect("decode open_chain_vault")
            .expect("open_chain_vault Ok"),
        WasmResult::Reject(msg) => panic!("open_chain_vault rejected: {msg}"),
    };

    let v = get_vault(&pic, backend, vault_id).expect("vault exists after open");
    assert_eq!(v.status, ChainVaultStatus::AwaitingDeposit, "open => AwaitingDeposit");
    let custody = v.custody_address.clone();
    assert_supply(&pic, backend, 0, "after open (AwaitingDeposit)");

    // ── deposit -> MintPending -> mint confirm (Open, supply 100e8) ──────────
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&custody, &candid::Nat::from(collateral_e18)).unwrap(),
    );
    advance_and_tick(&pic, 2);
    assert_eq!(
        get_vault(&pic, backend, vault_id).unwrap().status,
        ChainVaultStatus::MintPending,
        "deposit verified => MintPending"
    );

    advance_and_tick(&pic, 1); // submit mint (Queued -> Inflight)
    update_any(
        &pic,
        mock,
        "set_receipt",
        Encode!(&"0xcfxmint1".to_string(), &true, &cursor1).unwrap(),
    );
    push_mint_log(&pic, mock, vault_id, &recipient, debt_e8s, "0xcfxmint1", cursor1);
    advance_and_tick(&pic, 4); // confirm mint

    let v = get_vault(&pic, backend, vault_id).expect("vault after mint confirm");
    assert_eq!(v.status, ChainVaultStatus::Open, "mint confirmed => Open");
    assert_eq!(v.debt_e8s, candid::Nat::from(debt_e8s), "mint confirmed => debt 100e8");
    assert!(v.pending_liquidation.is_none(), "healthy vault has no liquidation marker");
    assert_supply(&pic, backend, 100 * E8, "after mint");

    // ── Step 2: learn the reserve address (the swap's USDC `to` + Transfer match) ──
    // Derived from the tECDSA reserve key — independent of any vault state, so it is
    // resolved up front (before detection) and reused to seed the canned reads.
    let reserve = match get_reserve_address(&pic, backend) {
        Ok(a) => a.to_lowercase(),
        Err(e) => {
            // ECDSA derived the SETTLEMENT address above but not the RESERVE
            // address: we are in the gated regime after all. Bail honestly.
            eprintln!("[swap] reserve address derive Err ({e:?}) despite settlement ECDSA; running gated subset");
            return;
        }
    };
    eprintln!("[swap] reserve address = {reserve}");

    // ── Step 3: seed everything the swap SUBMIT reads, BEFORE detection ───────
    // The settlement worker can pick up the freshly-enqueued LiquidationSwap op on
    // the very next tick after detection marks it (both timers fire in the same
    // advance_and_tick window). If the DEX reads / custody balance / send-hash are
    // not in place by then, the swap fail-closes (escalates) and clears the marker.
    // So seed them all FIRST, then drop the price to trigger detection.

    // 3a. Fund custody so the swap is fundable (>= 1400 CFX collateral + gas).
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&custody, &candid::Nat::from(2_000u128 * E18)).unwrap(),
    );
    // 3b. token0() == collateral_token (WCFX), ABI-encoded as a single 32-byte word.
    update_any(
        &pic,
        mock,
        "set_eth_call_response",
        Encode!(&TOKEN0_SELECTOR.to_string(), &word_addr(WCFX)).unwrap(),
    );
    // 3c. getReserves() -> (reserve0, reserve1, ts). token0 == WCFX so reserve0 =
    // WCFX reserve, reserve1 = USDC reserve. A DEEP pool priced at $0.08/CFX so the
    // oracle cross-check passes: 1e24 WCFX vs 8e22 USDC (8e22/1e24 = 0.08).
    let reserve0_wcfx = 1_000_000u128 * E18; // 1e24
    let reserve1_usdc = 80_000u128 * E18; // 8e22
    let getreserves_blob = format!(
        "0x{:064x}{:064x}{:064x}",
        reserve0_wcfx, reserve1_usdc, 0u128
    );
    update_any(
        &pic,
        mock,
        "set_eth_call_response",
        Encode!(&GET_RESERVES_SELECTOR.to_string(), &getreserves_blob).unwrap(),
    );
    // 3d. The hash the swap broadcast returns (so the confirm can match it).
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xcfxswap1".to_string()).unwrap());

    // ── Step 4: enable the liquidation config + drop the price to $0.08 ──────
    script_factory_pair_sanity(&pic, mock, DEX_PAIR);
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_chain_liquidation_config",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &enabled_liq_config()).unwrap(),
        ),
        "set_chain_liquidation_config",
    )
    .expect("set_chain_liquidation_config Ok");

    // $0.08 / CFX => 1400 * 0.08 = $112 vs 100 debt => CR ~112% < 133%.
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_manual_collateral_price",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &"CFX".to_string(), &8_000_000u64).unwrap(),
        ),
        "set_manual_collateral_price (drop)",
    )
    .expect("set_manual_collateral_price (drop)");

    // ── Step 5: observer tick marks the vault (Bot tier; collateral reserved) +
    // the settlement worker SUBMITS the swap (DEX reads + JIT min-out + oracle
    // gate, all seeded above) -> the op goes Inflight, the marker persists. ─────
    advance_and_tick(&pic, 3);

    let v = get_vault(&pic, backend, vault_id).expect("vault after detection + swap submit");
    let marker = v
        .pending_liquidation
        .clone()
        .expect("detection set a pending_liquidation marker (NOT escalated: the swap broadcast)");
    assert_eq!(marker.tier, LiquidationTier::Bot, "Tier-1 bot liquidation");
    assert!(
        marker.debt_to_clear_e8s > candid::Nat::from(0u32),
        "marker sized to clear a non-zero debt"
    );
    // The full $112 of collateral covers the full $100 debt at the 1.12x bonus, so
    // the entire 1400 CFX is reserved and the vault's remaining collateral is 0.
    assert_eq!(
        v.collateral_amount_e18,
        candid::Nat::from(0u32),
        "all collateral reserved (full liquidation): remaining collateral == 0"
    );
    assert_eq!(v.debt_e8s, candid::Nat::from(debt_e8s), "debt UNCHANGED at trigger (Design B)");
    assert_eq!(v.status, ChainVaultStatus::Open, "vault stays Open under the marker");
    assert_supply(&pic, backend, 100 * E8, "after detection + swap submit (supply unchanged)");

    // ── Step 6: provide the realized-USDC Transfer log + receipt at the cursor ─
    // The confirm reads the receipt (mined+final) then queries eth_getLogs on the
    // USDC token for TRANSFER_TOPIC0 at [cursor1, cursor1]. The mock's get_logs
    // filters by topic0 + block range (NOT contract address), so a push_log with
    // the Transfer topic0 + the reserve `to` topic at cursor1 is returned.
    update_any(
        &pic,
        mock,
        "set_receipt",
        Encode!(&"0xcfxswap1".to_string(), &true, &cursor1).unwrap(),
    );
    // realized = 110 USDC (18-dec) > the 100 icUSD debt; from(any), to(reserve).
    let realized_usdc_native = 110u128 * E18;
    let transfer_topics = vec![
        TRANSFER_TOPIC0.to_string(),
        word_addr("0x000000000000000000000000000000000000babe"), // from (any)
        word_addr(&reserve),                                     // to == reserve
    ];
    update_any(
        &pic,
        mock,
        "push_log",
        Encode!(
            &transfer_topics,
            &word_u128(realized_usdc_native),
            &"0xcfxswap1".to_string(),
            &cursor1
        )
        .unwrap(),
    );
    advance_and_tick(&pic, 4); // confirm the swap (Phase 2: debt -> reserve)

    // ════════════════════════════════════════════════════════════════════════
    // HAPPY-PATH ASSERTIONS
    // ════════════════════════════════════════════════════════════════════════
    let v = get_vault(&pic, backend, vault_id).expect("vault after swap confirm");
    assert_eq!(v.debt_e8s, candid::Nat::from(0u32), "swap cleared the full debt => debt_e8s 0");
    assert_eq!(
        v.status,
        ChainVaultStatus::Closed,
        "fully drained (debt 0 + collateral 0) => Closed"
    );
    assert!(
        v.pending_liquidation.is_none(),
        "swap settled => pending_liquidation marker cleared"
    );

    // PSM: NO icUSD burned. chain_supplies is UNCHANGED across the whole swap.
    assert_supply(&pic, backend, 100 * E8, "after swap confirm (PSM: supply unchanged)");

    // Reserve credited: reconcile reads on-chain totalSupply (== recorded => no
    // unbacked excess) and reports the reserve backing + physical USDC.
    update_any(&pic, mock, "set_total_supply", Encode!(&(100u128 * E8)).unwrap());
    let recon = reconcile_chain_supply(&pic, backend).expect("reconcile_chain_supply Ok");
    assert_eq!(
        recon.reserve_backing_e8s,
        candid::Nat::from(100u128 * E8),
        "reserve_backing_e8s == the cleared debt (100e8)"
    );
    assert_eq!(
        recon.reserve_usdc_native,
        candid::Nat::from(110u128 * E18),
        "reserve_usdc_native == the realized USDC (110e18)"
    );
    assert!(
        !recon.unbacked_excess,
        "reserve is BACKING, not unbacked supply (findings #17/#20/#29): unbacked_excess == false"
    );

    eprintln!("[swap] FULL swap path PASSED: detection marked an underwater vault (Bot tier, all collateral reserved), the settlement worker SUBMITTED swapExactETHForTokens (DEX reads + JIT min-out + oracle gate) and CONFIRMED it (decoded the realized 110 USDC Transfer to the reserve), moving 100e8 debt -> reserve_backing (vault Closed, debt 0). chain_supplies UNCHANGED (PSM, no icUSD burned); reconcile reports reserve_backing=100e8, reserve_usdc=110e18, unbacked_excess=false on chain 71.");
}

#[test]
fn conflux_liquidation_bot_failure_sp_absorb_claims_cfx() {
    let (pic, backend, mock, sp, icusd_ledger, user) = boot_with_sp();

    decode_result(
        update_dev(&pic, backend, "set_evm_rpc_principal", Encode!(&mock).unwrap()),
        "set_evm_rpc_principal",
    )
    .expect("set_evm_rpc_principal");
    update_any(&pic, mock, "set_getlogs_max_range", Encode!(&1000u64).unwrap());
    update_any(&pic, mock, "set_espace_receipt_fields", Encode!(&true).unwrap());

    let reg = RegisterChainArg {
        chain_id: ChainId(CONFLUX_CHAIN_ID),
        display_name: "ConfluxESpaceTestnet".to_string(),
        rpc_endpoints: vec!["https://evmtestnet.confluxrpc.com".to_string()],
        finality_depth: CONFLUX_FINALITY_DEPTH as u32,
        gas_strategy: GasStrategy::EvmEip1559 {
            max_priority_fee_gwei: 1,
            max_fee_gwei_ceiling: 100,
        },
        chain_native_decimals: 18,
        min_quorum_providers: Some(1),
    };
    decode_result(
        update_dev(&pic, backend, "register_chain", Encode!(&reg).unwrap()),
        "register_chain",
    )
    .expect("register_chain");
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_chain_contract",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &"0x00000000000000000000000000000000cf1c0de5".to_string())
                .unwrap(),
        ),
        "set_chain_contract",
    )
    .expect("set_chain_contract");
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_manual_collateral_price",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &"CFX".to_string(), &15_000_000u64).unwrap(),
        ),
        "set_manual_collateral_price",
    )
    .expect("set_manual_collateral_price");

    let seed: u64 = 1_000_000;
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_last_observed_block",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &seed).unwrap(),
        ),
        "set_last_observed_block",
    )
    .expect("set_last_observed_block");
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_burn_watch_poll_enabled",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &true).unwrap(),
        ),
        "set_burn_watch_poll_enabled",
    )
    .expect("set_burn_watch_poll_enabled");

    let cursor1 = seed + SCAN_WINDOW;
    let head1 = cursor1 + CONFLUX_FINALITY_DEPTH + 24;
    update_any(&pic, mock, "set_blocks", Encode!(&head1, &head1).unwrap());
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xcfxmint-inc4".to_string()).unwrap());

    let settlement_addr = match update_dev(
        &pic,
        backend,
        "get_chain_settlement_address",
        Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap(),
    ) {
        WasmResult::Reply(b) => match Decode!(&b, Result<String, ProtocolError>) {
            Ok(Ok(addr)) => Some(addr),
            Ok(Err(e)) => {
                eprintln!("[sp-fallback] ECDSA UNAVAILABLE (get_chain_settlement_address returned Err: {e:?}); running gated subset");
                None
            }
            Err(decode_err) => {
                eprintln!("[sp-fallback] get_chain_settlement_address decode error ({decode_err}); running gated subset");
                None
            }
        },
        WasmResult::Reject(msg) => {
            eprintln!("[sp-fallback] ECDSA UNAVAILABLE (get_chain_settlement_address rejected: {msg}); running gated subset");
            None
        }
    };
    let settlement_addr = match settlement_addr {
        Some(addr) => addr,
        None => {
            let sentinel = register_sp_cfx(&pic, sp);
            assert_ne!(sentinel, Principal::anonymous(), "gated: CFX sentinel registers");
            register_sp_icusd(&pic, sp, icusd_ledger);
            eprintln!("[sp-fallback] ECDSA unavailable; ran gated SP subset (icUSD + CFX sentinel registration)");
            return;
        }
    };

    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&settlement_addr, &candid::Nat::from(1_000_000u128 * E18)).unwrap(),
    );

    let collateral_e18 = 1_400u128 * E18;
    let debt_e8s = 100u128 * E8;
    let recipient = "0x000000000000000000000000000000000000c0de".to_string();
    let vault_id: u64 = match update_dev(
        &pic,
        backend,
        "open_chain_vault",
        Encode!(
            &ChainId(CONFLUX_CHAIN_ID),
            &candid::Nat::from(collateral_e18),
            &candid::Nat::from(debt_e8s),
            &recipient
        )
        .unwrap(),
    ) {
        WasmResult::Reply(b) => Decode!(&b, Result<u64, ProtocolError>)
            .expect("decode open_chain_vault")
            .expect("open_chain_vault Ok"),
        WasmResult::Reject(msg) => panic!("open_chain_vault rejected: {msg}"),
    };
    let custody = get_vault(&pic, backend, vault_id)
        .expect("vault exists")
        .custody_address;
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&custody, &candid::Nat::from(collateral_e18)).unwrap(),
    );
    advance_and_tick(&pic, 2);
    assert_eq!(
        get_vault(&pic, backend, vault_id).unwrap().status,
        ChainVaultStatus::MintPending,
        "deposit verified => MintPending"
    );

    advance_and_tick(&pic, 1);
    update_any(
        &pic,
        mock,
        "set_receipt",
        Encode!(&"0xcfxmint-inc4".to_string(), &true, &cursor1).unwrap(),
    );
    push_mint_log(&pic, mock, vault_id, &recipient, debt_e8s, "0xcfxmint-inc4", cursor1);
    advance_and_tick(&pic, 4);
    assert_eq!(
        get_vault(&pic, backend, vault_id).unwrap().status,
        ChainVaultStatus::Open,
        "mint confirmed => Open"
    );
    assert_supply(&pic, backend, 100 * E8, "after mint");

    script_factory_pair_sanity(&pic, mock, DEX_PAIR);
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_chain_liquidation_config",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &enabled_liq_config()).unwrap(),
        ),
        "set_chain_liquidation_config",
    )
    .expect("set_chain_liquidation_config");
    update_any(
        &pic,
        mock,
        "set_balance",
        Encode!(&custody, &candid::Nat::from(10_000u128 * E18)).unwrap(),
    );
    update_any(
        &pic,
        mock,
        "set_eth_call_response",
        Encode!(&TOKEN0_SELECTOR.to_string(), &word_addr(WCFX)).unwrap(),
    );
    let zero_reserves_blob = format!("0x{:064x}{:064x}{:064x}", 0u128, 0u128, 0u128);
    update_any(
        &pic,
        mock,
        "set_eth_call_response",
        Encode!(&GET_RESERVES_SELECTOR.to_string(), &zero_reserves_blob).unwrap(),
    );
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_manual_collateral_price",
            Encode!(&ChainId(CONFLUX_CHAIN_ID), &"CFX".to_string(), &8_000_000u64).unwrap(),
        ),
        "set_manual_collateral_price (drop)",
    )
    .expect("set_manual_collateral_price (drop)");

    advance_and_tick(&pic, 3);
    let v = get_vault(&pic, backend, vault_id).expect("vault after failed bot swap");
    assert_eq!(v.status, ChainVaultStatus::Open, "bot failure restores the Open vault");
    assert!(v.pending_liquidation.is_none(), "bot failure clears the pending marker");
    assert_eq!(
        v.collateral_amount_e18,
        candid::Nat::from(collateral_e18),
        "bot failure restores reserved collateral"
    );
    assert_eq!(v.debt_e8s, candid::Nat::from(debt_e8s), "bot failure leaves debt live");
    let candidates = get_chain_liquidatable_vaults(&pic, backend);
    let candidate = candidates
        .iter()
        .find(|v| v.vault_id == vault_id)
        .expect("SP discovery sees the escalated vault");
    assert!(candidate.sp_attempted, "bot failure sets sp_attempted");
    assert_eq!(
        candidate.chain_collateral_sentinel,
        chain_collateral_sentinel_for(CONFLUX_CHAIN_ID),
        "backend sentinel is deterministic from chain id"
    );
    assert_supply(&pic, backend, 100 * E8, "after bot failure");
    advance_and_tick(&pic, 2);
    let v = get_vault(&pic, backend, vault_id).expect("vault after no-retry windows");
    assert!(
        v.pending_liquidation.is_none(),
        "sp_attempted vault must not be re-routed to the bot"
    );

    register_sp_icusd(&pic, sp, icusd_ledger);
    let sentinel = register_sp_cfx(&pic, sp);
    assert_eq!(
        sentinel,
        candidate.chain_collateral_sentinel,
        "SP and backend derive the same CFX sentinel"
    );
    let no_coverage = sp_absorb_chain_vault(&pic, sp, vault_id)
        .expect_err("zero icUSD coverage must not absorb");
    assert!(
        matches!(
            no_coverage,
            StabilityPoolError::InsufficientPoolBalance
                | StabilityPoolError::LiquidationFailed { .. }
        ),
        "zero coverage error should be a clean pre-burn failure, got {:?}",
        no_coverage
    );
    approve_icusd(&pic, icusd_ledger, user, sp, debt_e8s * 2);
    deposit_sp_icusd(&pic, sp, user, icusd_ledger, debt_e8s as u64);
    let zero_opt_in = sp_absorb_chain_vault(&pic, sp, vault_id)
        .expect_err("deposited but not opted-in icUSD must not cover CFX");
    assert!(
        matches!(
            zero_opt_in,
            StabilityPoolError::InsufficientPoolBalance
                | StabilityPoolError::LiquidationFailed { .. }
        ),
        "zero opt-in error should be a clean pre-burn failure, got {:?}",
        zero_opt_in
    );
    assert_eq!(
        icrc1_balance_of(&pic, icusd_ledger, sp),
        debt_e8s,
        "failed pre-opt absorb must not burn SP icUSD"
    );
    opt_in_sp_cfx(&pic, sp, user, sentinel);

    let absorb = sp_absorb_chain_vault(&pic, sp, vault_id)
        .expect("SP absorbs the bot-failed chain vault");
    assert!(absorb.success, "SP absorb returns success");
    assert_eq!(absorb.vault_id, vault_id);
    assert_eq!(absorb.chain_id, ChainId(CONFLUX_CHAIN_ID));
    assert_eq!(absorb.icusd_burned_e8s, debt_e8s as u64);
    assert_eq!(absorb.liquidated_debt_e8s, debt_e8s);
    assert_eq!(
        absorb.collateral_received_native, collateral_e18,
        "100 icUSD at $0.08 with 12% penalty seizes 1400 CFX"
    );
    assert_eq!(absorb.claim_id, vault_id, "claim id is the absorbed vault id");
    assert_eq!(
        icrc1_balance_of(&pic, icusd_ledger, sp),
        0,
        "SP burned the deposited icUSD during chain absorb"
    );

    let v = get_vault(&pic, backend, vault_id).expect("vault after SP absorb");
    assert_eq!(v.debt_e8s, candid::Nat::from(0u32), "SP absorb clears live debt");
    assert_eq!(v.collateral_amount_e18, candid::Nat::from(0u32), "SP absorb seizes collateral");
    assert_eq!(v.status, ChainVaultStatus::Closed, "fully absorbed vault closes");
    assert!(v.pending_liquidation.is_none(), "SP absorb does not create a marker");
    assert_supply(&pic, backend, 100 * E8, "after SP absorb (foreign supply unchanged)");
    let duplicate_absorb = sp_absorb_chain_vault(&pic, sp, vault_id)
        .expect("duplicate SP absorb returns the stored idempotent result");
    assert!(duplicate_absorb.success, "duplicate SP absorb returns success");
    assert_eq!(duplicate_absorb.vault_id, absorb.vault_id);
    assert_eq!(duplicate_absorb.chain_id, absorb.chain_id);
    assert_eq!(duplicate_absorb.icusd_burned_e8s, absorb.icusd_burned_e8s);
    assert_eq!(
        duplicate_absorb.liquidated_debt_e8s, absorb.liquidated_debt_e8s,
        "duplicate SP absorb must not burn or absorb a different debt amount"
    );
    assert_eq!(
        duplicate_absorb.collateral_received_native, absorb.collateral_received_native,
        "duplicate SP absorb must replay the stored collateral seizure"
    );
    assert_eq!(duplicate_absorb.claim_id, absorb.claim_id);
    assert_eq!(duplicate_absorb.block_index, absorb.block_index);

    update_any(&pic, mock, "set_total_supply", Encode!(&(100u128 * E8)).unwrap());
    let recon = reconcile_chain_supply(&pic, backend).expect("reconcile_chain_supply Ok");
    assert_eq!(recon.recorded_supply_e8s, candid::Nat::from(100u128 * E8));
    assert_eq!(
        recon.pending_chain_burn_e8s,
        candid::Nat::from(100u128 * E8),
        "SP burn is booked as pending foreign-chain burn"
    );
    assert_eq!(recon.reserve_backing_e8s, candid::Nat::from(0u32), "SP fallback is not PSM reserve");
    assert!(!recon.unbacked_excess, "pending burn must not false-trip unbacked_excess");

    let claim_dest = "0x000000000000000000000000000000000000c0de".to_string();
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_sp_writedown_disabled",
            Encode!(&true).unwrap(),
        ),
        "set_sp_writedown_disabled(true)",
    )
    .expect("set_sp_writedown_disabled true");
    let rejected_claim = claim_sp_cfx(&pic, sp, user, sentinel, claim_dest.clone())
        .expect_err("backend rejection must surface to SP claimant");
    assert!(
        matches!(rejected_claim, StabilityPoolError::LiquidationFailed { .. }),
        "backend rejection should return LiquidationFailed, got {:?}",
        rejected_claim
    );
    decode_result(
        update_dev(
            &pic,
            backend,
            "set_sp_writedown_disabled",
            Encode!(&false).unwrap(),
        ),
        "set_sp_writedown_disabled(false)",
    )
    .expect("set_sp_writedown_disabled false");
    update_any(&pic, mock, "set_next_send_hash", Encode!(&"0xcfxclaim-inc4".to_string()).unwrap());
    let claimed = claim_sp_cfx(&pic, sp, user, sentinel, claim_dest.clone())
        .expect("claim_cfx enqueues backend CFX payout");
    assert_eq!(claimed, collateral_e18, "single opted-in depositor claims all seized CFX");
    let second_claim = claim_sp_cfx(&pic, sp, user, sentinel, claim_dest.clone())
        .expect("second claim is idempotently empty at SP layer");
    assert_eq!(second_claim, 0, "claim_cfx zeroes the SP-side entitlement");

    advance_and_tick(&pic, 2);
    update_any(
        &pic,
        mock,
        "set_receipt",
        Encode!(&"0xcfxclaim-inc4".to_string(), &true, &cursor1).unwrap(),
    );
    advance_and_tick(&pic, 4);
    let logs = fetch_info_logs(&pic, backend);
    assert!(
        logs.iter().any(|m| {
            m.contains("chain-collateral payout op")
                && m.contains("confirmed")
                && m.contains("0xcfxclaim-inc4")
                && m.contains(&claim_dest)
        }),
        "payout confirm log not found; logs: {:?}",
        logs
    );

    eprintln!("[sp-fallback] FULL path PASSED: bot swap failed closed into sp_attempted, SP burned 100e8 icUSD, backend moved debt -> pending_chain_burn (foreign supply unchanged), credited 1400 CFX to the opted-in depositor, claim_cfx enqueued and confirmed the ChainCollateralPayout.");
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn get_vault(pic: &PocketIc, backend: Principal, vault_id: u64) -> Option<ChainVaultV1> {
    let reply = pic
        .query_call(
            backend,
            Principal::anonymous(),
            "get_chain_vault",
            Encode!(&vault_id).unwrap(),
        )
        .expect("get_chain_vault query");
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Option<ChainVaultV1>).expect("decode get_chain_vault"),
        WasmResult::Reject(msg) => panic!("get_chain_vault rejected: {msg}"),
    }
}

fn get_liq_config(pic: &PocketIc, backend: Principal) -> Option<ChainLiquidationConfigV1> {
    let reply = pic
        .query_call(
            backend,
            Principal::anonymous(),
            "get_chain_liquidation_config",
            Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap(),
        )
        .expect("get_chain_liquidation_config query");
    match reply {
        WasmResult::Reply(b) => {
            Decode!(&b, Option<ChainLiquidationConfigV1>).expect("decode get_chain_liquidation_config")
        }
        WasmResult::Reject(msg) => panic!("get_chain_liquidation_config rejected: {msg}"),
    }
}

fn get_reserve_address(pic: &PocketIc, backend: Principal) -> Result<String, ProtocolError> {
    match update_dev(
        pic,
        backend,
        "get_chain_reserve_address",
        Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap(),
    ) {
        WasmResult::Reply(b) => {
            Decode!(&b, Result<String, ProtocolError>).expect("decode get_chain_reserve_address")
        }
        WasmResult::Reject(msg) => {
            // A transport-level reject (ECDSA off in this build) => treat as Err.
            Err(ProtocolError::ChainAdmin(format!("reject: {msg}")))
        }
    }
}

fn reconcile_chain_supply(
    pic: &PocketIc,
    backend: Principal,
) -> Result<ChainSupplyReconciliation, ProtocolError> {
    match update_dev(
        pic,
        backend,
        "reconcile_chain_supply",
        Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap(),
    ) {
        WasmResult::Reply(b) => Decode!(&b, Result<ChainSupplyReconciliation, ProtocolError>)
            .expect("decode reconcile_chain_supply"),
        WasmResult::Reject(msg) => panic!("reconcile_chain_supply rejected: {msg}"),
    }
}

fn get_chain_liquidatable_vaults(
    pic: &PocketIc,
    backend: Principal,
) -> Vec<ChainLiquidatableVaultInfo> {
    let reply = pic
        .query_call(
            backend,
            Principal::anonymous(),
            "get_chain_liquidatable_vaults",
            Encode!(&ChainId(CONFLUX_CHAIN_ID)).unwrap(),
        )
        .expect("get_chain_liquidatable_vaults query");
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Vec<ChainLiquidatableVaultInfo>)
            .expect("decode get_chain_liquidatable_vaults"),
        WasmResult::Reject(msg) => panic!("get_chain_liquidatable_vaults rejected: {msg}"),
    }
}

fn chain_collateral_sentinel_for(chain_id: u32) -> Principal {
    let mut bytes = [0u8; 29];
    let prefix = b"rumi-chain-collateral";
    bytes[..prefix.len()].copy_from_slice(prefix);
    bytes[24..28].copy_from_slice(&chain_id.to_le_bytes());
    bytes[28] = 0x7f;
    Principal::from_slice(&bytes)
}

fn sp_result_unit(reply: WasmResult, method: &str) -> Result<(), StabilityPoolError> {
    match reply {
        WasmResult::Reply(b) => Decode!(&b, Result<(), StabilityPoolError>)
            .unwrap_or_else(|e| panic!("decode {method} Result<(), StabilityPoolError>: {e}")),
        WasmResult::Reject(msg) => panic!("{method} rejected: {msg}"),
    }
}

fn register_sp_icusd(pic: &PocketIc, sp: Principal, ledger: Principal) {
    let cfg = StablecoinConfig {
        ledger_id: ledger,
        symbol: "icUSD".to_string(),
        decimals: 8,
        priority: 0,
        is_active: true,
        transfer_fee: Some(0),
        is_lp_token: Some(false),
        underlying_pool: None,
    };
    sp_result_unit(
        update_as(pic, sp, dev(), "register_stablecoin", Encode!(&cfg).unwrap()),
        "register_stablecoin",
    )
    .expect("register icUSD in SP");
}

fn register_sp_cfx(pic: &PocketIc, sp: Principal) -> Principal {
    match update_as(
        pic,
        sp,
        dev(),
        "register_cfx_collateral",
        Encode!(&CONFLUX_CHAIN_ID).unwrap(),
    ) {
        WasmResult::Reply(b) => Decode!(&b, Result<Principal, StabilityPoolError>)
            .expect("decode register_cfx_collateral")
            .expect("register CFX collateral in SP"),
        WasmResult::Reject(msg) => panic!("register_cfx_collateral rejected: {msg}"),
    }
}

fn approve_icusd(
    pic: &PocketIc,
    ledger: Principal,
    user: Principal,
    spender: Principal,
    amount: u128,
) {
    let args = ApproveArgs {
        from_subaccount: None,
        spender: account(spender),
        amount: Nat::from(amount),
        expected_allowance: None,
        expires_at: None,
        fee: None,
        memo: None,
        created_at_time: None,
    };
    match update_as(pic, ledger, user, "icrc2_approve", encode_args((args,)).unwrap()) {
        WasmResult::Reply(b) => Decode!(&b, Result<Nat, ApproveError>)
            .expect("decode icrc2_approve")
            .expect("icrc2_approve"),
        WasmResult::Reject(msg) => panic!("icrc2_approve rejected: {msg}"),
    };
}

fn deposit_sp_icusd(
    pic: &PocketIc,
    sp: Principal,
    user: Principal,
    ledger: Principal,
    amount_e8s: u64,
) {
    sp_result_unit(
        update_as(
            pic,
            sp,
            user,
            "deposit",
            Encode!(&ledger, &amount_e8s).unwrap(),
        ),
        "deposit",
    )
    .expect("deposit icUSD into SP");
}

fn opt_in_sp_cfx(pic: &PocketIc, sp: Principal, user: Principal, sentinel: Principal) {
    sp_result_unit(
        update_as(pic, sp, user, "opt_in_cfx", Encode!(&sentinel).unwrap()),
        "opt_in_cfx",
    )
    .expect("opt in CFX");
}

fn sp_absorb_chain_vault(
    pic: &PocketIc,
    sp: Principal,
    vault_id: u64,
) -> Result<ChainSpAbsorbResult, StabilityPoolError> {
    match update_as(
        pic,
        sp,
        dev(),
        "sp_absorb_chain_vault",
        Encode!(&vault_id).unwrap(),
    ) {
        WasmResult::Reply(b) => Decode!(&b, Result<ChainSpAbsorbResult, StabilityPoolError>)
            .expect("decode sp_absorb_chain_vault"),
        WasmResult::Reject(msg) => panic!("sp_absorb_chain_vault rejected: {msg}"),
    }
}

fn claim_sp_cfx(
    pic: &PocketIc,
    sp: Principal,
    user: Principal,
    sentinel: Principal,
    dest_evm: String,
) -> Result<u128, StabilityPoolError> {
    match update_as(
        pic,
        sp,
        user,
        "claim_cfx",
        Encode!(&sentinel, &dest_evm).unwrap(),
    ) {
        WasmResult::Reply(b) => Decode!(&b, Result<u128, StabilityPoolError>)
            .expect("decode claim_cfx"),
        WasmResult::Reject(msg) => panic!("claim_cfx rejected: {msg}"),
    }
}

fn icrc1_balance_of(pic: &PocketIc, ledger: Principal, owner: Principal) -> u128 {
    let reply = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_args((account(owner),)).unwrap(),
        )
        .expect("icrc1_balance_of query");
    match reply {
        WasmResult::Reply(b) => {
            let balance: Nat = decode_one(&b).expect("decode icrc1_balance_of");
            balance.0.try_into().expect("balance fits u128")
        }
        WasmResult::Reject(msg) => panic!("icrc1_balance_of rejected: {msg}"),
    }
}

fn fetch_info_logs(pic: &PocketIc, backend: Principal) -> Vec<String> {
    let req = HttpRequest {
        method: "GET".to_string(),
        url: "/logs?priority=info".to_string(),
        headers: vec![],
        body: vec![],
    };
    let reply = pic
        .query_call(
            backend,
            Principal::anonymous(),
            "http_request",
            encode_one(req).unwrap(),
        )
        .expect("http_request query");
    let response: HttpResponse = match reply {
        WasmResult::Reply(b) => decode_one(&b).expect("decode http response"),
        WasmResult::Reject(msg) => panic!("http_request rejected: {msg}"),
    };
    let body = String::from_utf8(response.body).expect("logs response utf8");
    let log: LogWire = serde_json::from_str(&body).expect("parse logs JSON");
    log.entries.into_iter().map(|e| e.message).collect()
}

fn push_mint_log(
    pic: &PocketIc,
    mock: Principal,
    vault_id: u64,
    recipient: &str,
    amount_e8s: u128,
    tx_hash: &str,
    block: u64,
) {
    let topics = vec![
        MINT_EVENT_TOPIC0.to_string(),
        word_u128(vault_id as u128),
        word_addr(recipient),
    ];
    let data = word_u128(amount_e8s);
    update_any(
        pic,
        mock,
        "push_log",
        Encode!(&topics, &data, &tx_hash.to_string(), &block).unwrap(),
    );
}
