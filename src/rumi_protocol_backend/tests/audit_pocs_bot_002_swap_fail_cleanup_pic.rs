//! Wave-13 BOT-002: liquidation_bot swap-failure cleanup outcomes —
//! Layer 3 PocketIC fence.
//!
//! Wave 13 (commit 169eb62) replaced the bot's `let _ = ...` calls in the
//! swap-failure cleanup with explicit error capture: the integration now
//! pipes `return_result` and `cancel_err` into `decide_swap_failure_outcome`
//! and writes the helper's chosen `LiquidationStatus` (and STUCK log) to the
//! bot's stable history. The Layer-1 unit tests in
//! `liquidation_bot::process::tests` cover the helper directly with all four
//! input combinations; this file is the canister-boundary fence that proves
//! the integration WIRING (return-error capture, conditional cancel-attempt,
//! status mapping) actually works end-to-end.
//!
//! # Scenarios covered
//!
//!   1. `bot_002_pic_return_failed_records_transfer_failed` — flaky ledger
//!      rejects icrc1_transfer when the caller is the bot. Protocol →
//!      bot claim transfer (caller=protocol) succeeds. Swap call rejects
//!      (icpswap_pool stub doesn't implement `quote`). Bot's return call
//!      (caller=bot) rejects. Cancel is skipped. Bot writes
//!      `TransferFailed`; Wave-12 BOT-001b would otherwise reject the
//!      cancel anyway.
//!
//!   2. `bot_002_pic_cancel_stuck_records_confirm_failed` — flaky ledger
//!      reports a fake zero balance for the protocol's account. Protocol →
//!      bot claim transfer succeeds. Swap rejects. Bot's return succeeds.
//!      All three cancel attempts hit the BOT-001b balance gate (which sees
//!      the faked 0 balance vs `claim.collateral_amount - fee` required) and
//!      reject. Bot writes `ConfirmFailed`. Wave-11 auto-cancel will
//!      reconcile the protocol side after the 10-minute window.
//!
//!   3. `bot_002_pic_clean_cleanup_records_swap_failed` — no failure
//!      injection on the icp ledger. Protocol → bot claim succeeds. Swap
//!      rejects. Bot's return succeeds. Bot's cancel attempt succeeds (the
//!      protocol's balance after a clean return is exactly the BOT-001b
//!      required threshold). Bot writes `SwapFailed`.
//!
//! Scenario 4 of `decide_swap_failure_outcome` (both `return_err` and
//! `cancel_err` set — the defensive tiebreak) is unreachable from the
//! integration code (cancel is short-circuited when the return fails) and
//! is fenced by the Layer-1 unit test
//! `process::tests::return_error_takes_priority_over_cancel_error`.
//!
//! # Test driver
//!
//! Each scenario uses a fresh fixture: a real `liquidation_bot` canister
//! deployed alongside the protocol. The test plays the role of the protocol
//! when calling `notify_liquidatable_vaults` (sender = protocol_id) and of
//! the developer/admin elsewhere. Bot processing is timer-driven — the test
//! advances PIC time past the bot's 30-second tick and ticks enough rounds
//! to let `process_pending` complete its claim → swap → return → cancel
//! cleanup → write_record sequence.
//!
//! The fake icpswap pool is just a second `flaky_ledger`: it doesn't
//! implement `quote` or `depositFromAndSwap`, so the bot's swap call rejects
//! with `CanisterMethodNotFound` and that surfaces as the `swap_err` string
//! that decide_swap_failure_outcome receives.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use std::time::{Duration, SystemTime};

use rumi_protocol_backend::ProtocolError;

// ─── Local Candid mirrors ────────────────────────────────────────────────

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
struct MetadataValue {
    #[serde(rename = "Text")]
    text: Option<String>,
    #[serde(rename = "Nat")]
    nat: Option<Nat>,
    #[serde(rename = "Int")]
    int: Option<i64>,
    #[serde(rename = "Blob")]
    blob: Option<Vec<u8>>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct InitArgs {
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
    #[serde(rename = "Init")]
    Init(InitArgs),
    #[serde(rename = "Upgrade")]
    Upgrade(Option<()>),
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
struct ProtocolInitArg {
    xrc_principal: Principal,
    icusd_ledger_principal: Principal,
    icp_ledger_principal: Principal,
    fee_e8s: u64,
    developer_principal: Principal,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum ProtocolArgVariant {
    Init(ProtocolInitArg),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct OpenVaultSuccess {
    vault_id: u64,
    block_index: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct VaultArg {
    vault_id: u64,
    amount: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct SuccessWithFee {
    block_index: u64,
    fee_amount_paid: u64,
    collateral_amount_received: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct LiquidatableVaultInfo {
    vault_id: u64,
    collateral_type: Principal,
    debt_amount: u64,
    collateral_amount: u64,
    recommended_liquidation_amount: u64,
    collateral_price_e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct BotConfig {
    backend_principal: Principal,
    treasury_principal: Principal,
    admin: Principal,
    max_slippage_bps: u16,
    icp_ledger: Principal,
    ckusdc_ledger: Principal,
    icpswap_pool: Principal,
    icpswap_zero_for_one: Option<bool>,
    icp_fee_e8s: Option<u64>,
    ckusdc_fee_e6: Option<u64>,
    three_pool_principal: Option<Principal>,
    kong_swap_principal: Option<Principal>,
    ckusdt_ledger: Option<Principal>,
    icusd_ledger: Option<Principal>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct BotInitArgs {
    config: BotConfig,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
enum LiquidationStatus {
    Completed,
    SwapFailed,
    TransferFailed,
    ConfirmFailed,
    ClaimFailed,
    AdminResolved,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct LiquidationRecordV1 {
    id: u64,
    vault_id: u64,
    timestamp: u64,
    status: LiquidationStatus,
    collateral_claimed_e8s: u64,
    debt_to_cover_e8s: u64,
    icp_swapped_e8s: u64,
    ckusdc_received_e6: u64,
    ckusdc_transferred_e6: u64,
    icp_to_treasury_e8s: u64,
    oracle_price_e8s: u64,
    effective_price_e8s: u64,
    slippage_bps: i32,
    error_message: Option<String>,
    confirm_retry_count: u8,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum LiquidationRecordVersioned {
    V1(LiquidationRecordV1),
}

// ─── WASM ────────────────────────────────────────────────────────────────

fn protocol_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
}

fn flaky_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/flaky_ledger.wasm").to_vec()
}

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn liquidation_bot_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/liquidation_bot.wasm").to_vec()
}

fn xrc_wasm() -> Vec<u8> {
    include_bytes!("../../xrc_demo/xrc/xrc.wasm").to_vec()
}

#[derive(CandidType, Deserialize, Clone, Debug, Default)]
struct MockXRC {
    rates: Vec<(String, u64)>,
}

fn prepare_mock_xrc() -> Vec<u8> {
    let mock = MockXRC {
        rates: vec![("ICP/USD".to_string(), 1_000_000_000)],
    };
    encode_one(mock).expect("encode mock XRC init")
}

// ─── Helpers ─────────────────────────────────────────────────────────────

fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
}

fn deploy_flaky(pic: &PocketIc) -> Principal {
    let id = pic.create_canister();
    pic.add_cycles(id, 2_000_000_000_000);
    pic.install_canister(id, flaky_ledger_wasm(), encode_one(()).unwrap(), None);
    id
}

fn deploy_icrc1_ledger(
    pic: &PocketIc,
    minting_account: Account,
    transfer_fee: u64,
    initial_balances: Vec<(Account, Nat)>,
    name: &str,
    symbol: &str,
    controller: Principal,
) -> Principal {
    let ledger_id = pic.create_canister();
    pic.add_cycles(ledger_id, 2_000_000_000_000);
    let init = InitArgs {
        minting_account,
        fee_collector_account: None,
        transfer_fee: Nat::from(transfer_fee),
        decimals: Some(8),
        max_memo_length: Some(64),
        token_name: name.into(),
        token_symbol: symbol.into(),
        metadata: vec![],
        initial_balances,
        feature_flags: Some(FeatureFlags { icrc2: true }),
        maximum_number_of_accounts: None,
        accounts_overflow_trim_quantity: None,
        archive_options: ArchiveOptions {
            num_blocks_to_archive: 2000,
            trigger_threshold: 1000,
            controller_id: controller,
            max_transactions_per_response: None,
            max_message_size_bytes: None,
            cycles_for_archive_creation: None,
            node_max_memory_size_bytes: None,
            more_controller_ids: None,
        },
    };
    pic.install_canister(
        ledger_id,
        icrc1_ledger_wasm(),
        encode_args((LedgerArg::Init(init),)).expect("encode ledger init"),
        None,
    );
    ledger_id
}

fn flaky_mint(pic: &PocketIc, ledger: Principal, owner: Principal, amount: u128) {
    let acct = account(owner);
    pic.update_call(
        ledger,
        Principal::anonymous(),
        "mint",
        encode_args((acct, Nat::from(amount))).unwrap(),
    )
    .expect("flaky mint failed");
}

fn flaky_set_fail_for_caller(pic: &PocketIc, ledger: Principal, target: Option<Principal>) {
    pic.update_call(
        ledger,
        Principal::anonymous(),
        "set_fail_transfers_for_caller",
        encode_one(target).unwrap(),
    )
    .expect("set_fail_transfers_for_caller failed");
}

fn flaky_set_fake_zero_balance(pic: &PocketIc, ledger: Principal, target: Option<Principal>) {
    pic.update_call(
        ledger,
        Principal::anonymous(),
        "set_fake_zero_balance_for",
        encode_one(target).unwrap(),
    )
    .expect("set_fake_zero_balance_for failed");
}

fn icrc2_approve_call(
    pic: &PocketIc,
    ledger: Principal,
    sender: Principal,
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
    let result = pic
        .update_call(ledger, sender, "icrc2_approve", encode_one(args).unwrap())
        .expect("icrc2_approve call failed");
    let parsed: Result<Nat, ApproveError> = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode approve"),
        WasmResult::Reject(m) => panic!("approve rejected: {}", m),
    };
    parsed.expect("approve returned error");
}

fn xrc_set_rate(
    pic: &PocketIc,
    xrc: Principal,
    sender: Principal,
    base: &str,
    quote: &str,
    rate_e8s: u64,
) {
    let result = pic
        .update_call(
            xrc,
            sender,
            "set_exchange_rate",
            encode_args((base.to_string(), quote.to_string(), rate_e8s)).unwrap(),
        )
        .expect("set_exchange_rate call failed");
    if let WasmResult::Reject(m) = result {
        panic!("set_exchange_rate rejected: {}", m);
    }
}

fn admin_call_unit(
    pic: &PocketIc,
    target: Principal,
    sender: Principal,
    method: &str,
    payload: Vec<u8>,
) {
    let result = pic
        .update_call(target, sender, method, payload)
        .unwrap_or_else(|e| panic!("{} call failed: {:?}", method, e));
    if let WasmResult::Reject(m) = result {
        panic!("{} rejected: {}", method, m);
    }
}

fn admin_call_returning_protocol_result(
    pic: &PocketIc,
    target: Principal,
    sender: Principal,
    method: &str,
    payload: Vec<u8>,
) {
    let result = pic
        .update_call(target, sender, method, payload)
        .unwrap_or_else(|e| panic!("{} call failed: {:?}", method, e));
    let parsed: Result<(), ProtocolError> = match result {
        WasmResult::Reply(b) => decode_one(&b).unwrap_or_else(|e| {
            panic!("{} decode failed: {:?}", method, e)
        }),
        WasmResult::Reject(m) => panic!("{} rejected: {}", method, m),
    };
    parsed.unwrap_or_else(|e| panic!("{} returned error: {:?}", method, e));
}

fn get_bot_liquidation_count(pic: &PocketIc, bot_id: Principal) -> u64 {
    let result = pic
        .query_call(
            bot_id,
            Principal::anonymous(),
            "get_liquidation_count",
            encode_args(()).unwrap(),
        )
        .expect("get_liquidation_count call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode count"),
        WasmResult::Reject(m) => panic!("get_liquidation_count rejected: {}", m),
    }
}

fn get_bot_liquidations(pic: &PocketIc, bot_id: Principal) -> Vec<LiquidationRecordVersioned> {
    let result = pic
        .query_call(
            bot_id,
            Principal::anonymous(),
            "get_liquidations",
            encode_args((0u64, 100u64)).unwrap(),
        )
        .expect("get_liquidations call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode liquidations"),
        WasmResult::Reject(m) => panic!("get_liquidations rejected: {}", m),
    }
}

fn get_bot_stuck_liquidations(
    pic: &PocketIc,
    bot_id: Principal,
) -> Vec<LiquidationRecordVersioned> {
    let result = pic
        .query_call(
            bot_id,
            Principal::anonymous(),
            "get_stuck_liquidations",
            encode_args(()).unwrap(),
        )
        .expect("get_stuck_liquidations call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode stuck"),
        WasmResult::Reject(m) => panic!("get_stuck_liquidations rejected: {}", m),
    }
}

/// Advance enough time + ticks for the bot's 30-second timer to fire and
/// for `process_pending` to complete its full claim → swap → return →
/// cancel-loop (3 attempts) → write_record sequence. Each inter-canister
/// call is one PIC round-trip, so we tick generously.
fn drive_bot_timer(pic: &PocketIc) {
    pic.advance_time(Duration::from_secs(40));
    for _ in 0..40 {
        pic.tick();
    }
}

// ─── Fixture ─────────────────────────────────────────────────────────────

struct Fixture {
    pic: PocketIc,
    protocol_id: Principal,
    bot_id: Principal,
    icp_ledger: Principal,
    #[allow(dead_code)]
    developer: Principal,
    /// 50 ICP collateral, 100 icUSD borrowed. After the price drop in
    /// `setup_fixture` it is liquidatable but the global TCR stays above
    /// 100% so the protocol does NOT auto-latch ReadOnly.
    vault_id: u64,
    /// Snapshot of the LiquidatableVaultInfo the test pushes into the
    /// bot's pending queue (so each scenario starts the bot's
    /// process_pending against the same vault).
    liquidatable: LiquidatableVaultInfo,
}

fn setup_fixture() -> Fixture {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();

    let test_user = Principal::self_authenticating(b"bot_002_pic_user");
    let developer = Principal::self_authenticating(b"bot_002_pic_developer");
    let treasury = Principal::self_authenticating(b"bot_002_pic_treasury");

    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 2_000_000_000_000);
    pic.set_controllers(protocol_id, None, vec![Principal::anonymous(), developer])
        .expect("set_controllers failed");

    // ICP ledger (flaky) — both the protocol and the bot transact via this
    // one. Failure injection knobs apply to the bot's outbound transfers
    // and the protocol's balance reads; the protocol's bot_claim transfer
    // (caller=protocol) stays unaffected.
    let icp_ledger = deploy_flaky(&pic);

    // Pre-mint balances:
    //   - test_user: enough to fund the vault collateral via icrc2_approve.
    flaky_mint(&pic, icp_ledger, test_user, 1_000_000_000_000u128);

    // ckUSDC ledger and "icpswap pool" — both flaky. Bot doesn't actually
    // need them to function for this test (swap fails before depositing,
    // ckUSDC transfer never runs), but the bot config requires real
    // principals.
    let ckusdc_ledger = deploy_flaky(&pic);
    let icpswap_pool = deploy_flaky(&pic);

    // icUSD ledger — uses the standard ic-icrc1-ledger with the protocol
    // as the minting account. The protocol mints icUSD on borrow via
    // icrc1_transfer(from=minting_account, to=user), which only works on
    // a ledger that recognises the protocol as the minter. flaky_ledger
    // does not — it just sees the protocol as a regular account with
    // zero balance and would reject the borrow with InsufficientFunds.
    let icusd_ledger = deploy_icrc1_ledger(
        &pic,
        account(protocol_id),
        10_000,
        vec![],
        "icUSD",
        "icUSD",
        developer,
    );

    let xrc_id = pic.create_canister();
    pic.add_cycles(xrc_id, 1_000_000_000_000);
    pic.install_canister(xrc_id, xrc_wasm(), prepare_mock_xrc(), None);

    pic.set_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1_711_324_800));

    let init = ProtocolArgVariant::Init(ProtocolInitArg {
        fee_e8s: 10_000,
        icp_ledger_principal: icp_ledger,
        xrc_principal: xrc_id,
        icusd_ledger_principal: icusd_ledger,
        developer_principal: developer,
    });
    pic.install_canister(
        protocol_id,
        protocol_wasm(),
        encode_args((init,)).expect("encode protocol init"),
        None,
    );

    pic.advance_time(Duration::from_secs(1));
    for _ in 0..10 {
        pic.tick();
    }

    // Quiet the rate / fee curves so vault math stays predictable.
    let _ = pic.update_call(
        protocol_id,
        developer,
        "set_rate_curve_markers",
        encode_args((None::<Principal>, vec![(1.5f64, 1.0f64), (3.0f64, 1.0f64)])).unwrap(),
    );
    let _ = pic.update_call(
        protocol_id,
        developer,
        "set_borrowing_fee_curve",
        encode_args((None::<String>,)).unwrap(),
    );
    let _ = pic.update_call(
        protocol_id,
        developer,
        "set_borrowing_fee",
        encode_args((0.0f64,)).unwrap(),
    );
    let _ = pic.update_call(
        protocol_id,
        developer,
        "set_interest_rate",
        encode_args((icp_ledger, 0.0f64)).unwrap(),
    );

    let _ = pic.update_call(
        protocol_id,
        developer,
        "set_treasury_principal",
        encode_args((treasury,)).unwrap(),
    );

    // Open + borrow on the vault.
    icrc2_approve_call(&pic, icp_ledger, test_user, protocol_id, 50_000_000_000u128);
    let open_result = pic
        .update_call(
            protocol_id,
            test_user,
            "open_vault",
            encode_args((5_000_000_000u64, None::<Principal>)).unwrap(),
        )
        .expect("open_vault failed");
    let vault_id = match open_result {
        WasmResult::Reply(bytes) => {
            let r: Result<OpenVaultSuccess, ProtocolError> =
                decode_one(&bytes).expect("decode open_vault");
            r.expect("open_vault returned error").vault_id
        }
        WasmResult::Reject(msg) => panic!("open_vault rejected: {}", msg),
    };
    let borrow_arg = VaultArg {
        vault_id,
        amount: 10_000_000_000u64,
    };
    let borrow_result = pic
        .update_call(
            protocol_id,
            test_user,
            "borrow_from_vault",
            encode_args((borrow_arg,)).unwrap(),
        )
        .expect("borrow_from_vault failed");
    if let WasmResult::Reply(bytes) = borrow_result {
        let r: Result<SuccessWithFee, ProtocolError> = decode_one(&bytes).expect("decode borrow");
        r.expect("borrow_from_vault returned error");
    } else if let WasmResult::Reject(msg) = borrow_result {
        panic!("borrow rejected: {}", msg);
    }

    // Now deploy the liquidation_bot canister, configured to point at our
    // protocol + flaky ledgers + flaky icpswap pool. Use `developer` as the
    // bot's admin so set_config works if we need to tweak post-init.
    let bot_id = pic.create_canister();
    pic.add_cycles(bot_id, 2_000_000_000_000);
    let bot_init = BotInitArgs {
        config: BotConfig {
            backend_principal: protocol_id,
            treasury_principal: treasury,
            admin: developer,
            max_slippage_bps: 200,
            icp_ledger,
            ckusdc_ledger,
            icpswap_pool,
            // Required for swap.rs's quote() to even attempt the call.
            // Value is irrelevant — the swap rejects regardless because
            // the icpswap_pool canister doesn't implement `quote`.
            icpswap_zero_for_one: Some(true),
            icp_fee_e8s: Some(10_000),
            ckusdc_fee_e6: Some(10),
            three_pool_principal: None,
            kong_swap_principal: None,
            ckusdt_ledger: None,
            icusd_ledger: Some(icusd_ledger),
        },
    };
    pic.install_canister(
        bot_id,
        liquidation_bot_wasm(),
        encode_one(bot_init).expect("encode bot init"),
        None,
    );

    // Drop ICP price to make the vault liquidatable but keep TCR > 100%
    // so the protocol does not auto-latch ReadOnly. Three confirmation
    // ticks for the price-sanity band (each XRC interval is 300s).
    //
    // NOTE: the bot is intentionally NOT yet registered with the protocol
    // via `set_liquidation_bot_config`. Doing so before this drop would
    // make the protocol's `check_vaults` (which fires every 5 min) auto-
    // notify the bot once the third XRC confirmation lands — and since the
    // bot's own 30-second timer is already active, the bot would race the
    // test fixture by processing the vault before the per-scenario flaky
    // knobs are armed. Registering the bot AFTER the price is confirmed
    // keeps `s.liquidation_bot_principal` `None` during the auto-notify
    // window so check_vaults silently skips the bot route.
    xrc_set_rate(&pic, xrc_id, developer, "ICP", "USD", 250_000_000);
    for _ in 0..4 {
        pic.advance_time(Duration::from_secs(310));
        for _ in 0..15 {
            pic.tick();
        }
    }

    // Register the bot canister with the protocol now that the price
    // confirmation window has closed. The bot can now receive
    // `notify_liquidatable_vaults` calls from this test.
    admin_call_returning_protocol_result(
        &pic,
        protocol_id,
        developer,
        "set_liquidation_bot_config",
        encode_args((bot_id, 1_000_000_000_000u64)).unwrap(),
    );

    // Build the LiquidatableVaultInfo the test will push into the bot's
    // queue. Values mirror what `check_vaults` would compute — the bot
    // doesn't validate them server-side; it just hands them back to the
    // protocol via bot_claim_liquidation.
    let liquidatable = LiquidatableVaultInfo {
        vault_id,
        collateral_type: icp_ledger,
        debt_amount: 10_000_000_000u64,
        collateral_amount: 5_000_000_000u64,
        recommended_liquidation_amount: 10_000_000_000u64,
        collateral_price_e8s: 250_000_000u64,
    };

    Fixture {
        pic,
        protocol_id,
        bot_id,
        icp_ledger,
        developer,
        vault_id,
        liquidatable,
    }
}

/// Push the fixture's liquidatable vault into the bot's pending queue.
/// `notify_liquidatable_vaults` requires `caller == config.backend_principal`
/// so the test sends the call from the protocol's principal.
fn notify_bot_of_liquidatable(f: &Fixture) {
    admin_call_unit(
        &f.pic,
        f.bot_id,
        f.protocol_id,
        "notify_liquidatable_vaults",
        encode_one(vec![f.liquidatable.clone()]).unwrap(),
    );
}

/// Helper to inspect the most recently written record (id = count - 1).
fn latest_record(pic: &PocketIc, bot_id: Principal) -> LiquidationRecordV1 {
    let count = get_bot_liquidation_count(pic, bot_id);
    assert!(count > 0, "expected at least one record");
    let records = get_bot_liquidations(pic, bot_id);
    // get_liquidations returns newest-first when offset=0, but the API
    // sorts by stable map order (ascending id) and slices from the end.
    // Defensive: pick by max id rather than by index.
    let mut latest = records[0].clone();
    let mut max_id = match &latest {
        LiquidationRecordVersioned::V1(r) => r.id,
    };
    for r in records {
        match &r {
            LiquidationRecordVersioned::V1(v) => {
                if v.id >= max_id {
                    max_id = v.id;
                    latest = r.clone();
                }
            }
        }
    }
    match latest {
        LiquidationRecordVersioned::V1(r) => r,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────

/// **Scenario 1 — TransferFailed.** Arming
/// `set_fail_transfers_for_caller(Some(bot))` on the icp ledger fails the
/// bot's outbound `icrc1_transfer` (return-collateral) without breaking
/// the protocol's bot_claim transfer (which has caller=protocol). Cancel
/// is short-circuited by the integration when `return_result.is_err()`.
/// Bot writes `TransferFailed`. Without Wave-13 the bot would have written
/// `SwapFailed` because the cancel-loop result was discarded.
#[test]
fn bot_002_pic_return_failed_records_transfer_failed() {
    let f = setup_fixture();

    // Arm the failure BEFORE the bot's process_pending fires so the bot's
    // first attempt at return_collateral_to_backend rejects.
    flaky_set_fail_for_caller(&f.pic, f.icp_ledger, Some(f.bot_id));

    notify_bot_of_liquidatable(&f);
    drive_bot_timer(&f.pic);

    let count = get_bot_liquidation_count(&f.pic, f.bot_id);
    assert_eq!(
        count, 1,
        "bot must write exactly one record per processed vault; got {}",
        count
    );

    let record = latest_record(&f.pic, f.bot_id);
    assert_eq!(
        record.status,
        LiquidationStatus::TransferFailed,
        "Wave-13 must surface the return-collateral failure as TransferFailed; \
         pre-Wave-13 the cancel-loop result was discarded and this would \
         have written SwapFailed. error_message={:?}",
        record.error_message
    );
    assert_eq!(record.vault_id, f.vault_id);

    let err_msg = record
        .error_message
        .as_ref()
        .expect("TransferFailed must carry an error message");
    assert!(
        err_msg.contains("swap: ") && err_msg.contains("return: "),
        "error_message must compose the swap and return errors per \
         decide_swap_failure_outcome's TransferFailed branch; got: {}",
        err_msg
    );

    // get_stuck_liquidations must include this record (TransferFailed is
    // a stuck status) so the operator dashboard surfaces it.
    let stuck = get_bot_stuck_liquidations(&f.pic, f.bot_id);
    assert_eq!(
        stuck.len(),
        1,
        "exactly one stuck record expected; got {}",
        stuck.len()
    );
    let stuck_v1 = match &stuck[0] {
        LiquidationRecordVersioned::V1(v) => v,
    };
    assert_eq!(stuck_v1.status, LiquidationStatus::TransferFailed);
    assert_eq!(stuck_v1.vault_id, f.vault_id);
}

/// **Scenario 2 — ConfirmFailed.** The bot's return succeeds. The
/// protocol's BOT-001b cancel gate fails because we arm
/// `set_fake_zero_balance_for(Some(protocol))` on the icp ledger — the
/// gate calls `icrc1_balance_of(protocol)` and the faked-zero reading
/// trips the `< required` check. The bot retries cancel `CANCEL_ATTEMPTS`
/// (3) times, all reject for the same reason, and the integration records
/// `ConfirmFailed`. Wave-11 auto-cancel will reconcile the protocol side
/// after the 10-minute window.
#[test]
fn bot_002_pic_cancel_stuck_records_confirm_failed() {
    let f = setup_fixture();

    // Bot's return must succeed (don't fail caller=bot transfers), but
    // the cancel gate must reject. Faking the protocol's balance to 0 is
    // the simplest deterministic injection for the BOT-001b gate.
    flaky_set_fake_zero_balance(&f.pic, f.icp_ledger, Some(f.protocol_id));

    notify_bot_of_liquidatable(&f);
    drive_bot_timer(&f.pic);

    let count = get_bot_liquidation_count(&f.pic, f.bot_id);
    assert_eq!(count, 1, "bot must write exactly one record");

    let record = latest_record(&f.pic, f.bot_id);
    assert_eq!(
        record.status,
        LiquidationStatus::ConfirmFailed,
        "Wave-13 must surface the cancel-loop failure as ConfirmFailed; \
         pre-Wave-13 the cancel result was discarded and this would have \
         written SwapFailed. error_message={:?}",
        record.error_message
    );
    assert_eq!(record.vault_id, f.vault_id);

    let err_msg = record
        .error_message
        .as_ref()
        .expect("ConfirmFailed must carry an error message");
    assert!(
        err_msg.contains("cancel after"),
        "error_message must reference the cancel-retry-attempt count per \
         decide_swap_failure_outcome's ConfirmFailed branch; got: {}",
        err_msg
    );
    assert!(
        err_msg.contains("retries:"),
        "error_message must show the cancel error after the retries; got: {}",
        err_msg
    );

    let stuck = get_bot_stuck_liquidations(&f.pic, f.bot_id);
    assert_eq!(stuck.len(), 1, "stuck record expected");
    let stuck_v1 = match &stuck[0] {
        LiquidationRecordVersioned::V1(v) => v,
    };
    assert_eq!(stuck_v1.status, LiquidationStatus::ConfirmFailed);
    assert_eq!(stuck_v1.vault_id, f.vault_id);
}

/// **Scenario 3 — SwapFailed (clean cleanup).** No flaky knobs armed. The
/// bot's claim transfer succeeds, the swap rejects, the return succeeds,
/// and the cancel succeeds (the protocol's post-return balance covers
/// the BOT-001b required threshold). The integration records `SwapFailed`
/// and emits NO STUCK log line (`outcome.stuck_log` is `None` on the
/// happy cleanup branch).
#[test]
fn bot_002_pic_clean_cleanup_records_swap_failed() {
    let f = setup_fixture();

    notify_bot_of_liquidatable(&f);
    drive_bot_timer(&f.pic);

    let count = get_bot_liquidation_count(&f.pic, f.bot_id);
    assert_eq!(count, 1, "bot must write exactly one record");

    let record = latest_record(&f.pic, f.bot_id);
    assert_eq!(
        record.status,
        LiquidationStatus::SwapFailed,
        "clean cleanup must record SwapFailed; got status={:?} \
         error_message={:?}",
        record.status, record.error_message
    );
    assert_eq!(record.vault_id, f.vault_id);

    let err_msg = record
        .error_message
        .as_ref()
        .expect("SwapFailed must carry an error message");
    assert!(
        !err_msg.contains("return: "),
        "clean cleanup error_message must NOT mention a return failure; \
         got: {}",
        err_msg
    );
    assert!(
        !err_msg.contains("cancel after"),
        "clean cleanup error_message must NOT mention a cancel failure; \
         got: {}",
        err_msg
    );

    // Clean cleanup is NOT a stuck status — get_stuck_liquidations must
    // exclude it, even though the swap failed.
    let stuck = get_bot_stuck_liquidations(&f.pic, f.bot_id);
    assert!(
        stuck.is_empty(),
        "SwapFailed must NOT appear in the stuck-records dashboard; got {:?}",
        stuck
    );
}
