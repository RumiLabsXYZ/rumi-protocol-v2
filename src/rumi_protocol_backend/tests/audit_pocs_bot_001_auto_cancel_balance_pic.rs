//! Wave-11 BOT-001: auto-cancel collateral-return verification — Layer 3 PocketIC fence.
//!
//! Layer 1 (`audit_pocs_bot_001_auto_cancel_balance.rs`) covers the pure data
//! shape of the new `BotClaimReconciliationNeeded` event variant. Layer 3
//! exercises the canister-boundary path that Layer 1 cannot:
//!
//!   * `check_vaults` actually queries `icrc1_balance_of` on the collateral
//!     ledger when an expired `bot_claims` entry is detected;
//!   * a balance shortfall (the bot retained the collateral) leaves the
//!     claim in place, leaves `bot_budget_remaining_e8s` unchanged, and
//!     emits `BotClaimReconciliationNeeded`;
//!   * a sufficient balance (the bot returned the collateral) clears the
//!     claim and restores the budget — preserving the pre-Wave-11 happy
//!     path so we don't regress the original "bot crashed" auto-cancel.
//!
//! The "icrc1_balance_of returns an error" branch is exercised in code
//! review only; reliably injecting a ledger-side error from PocketIC would
//! require a custom mock ledger that can't transact, which would also
//! prevent the bot's collateral transfer in setup. The Layer-1 fence
//! confirms the variant + dispatch are correct.
//!
//! Fixture is lifted from `audit_pocs_liq_008_circuit_breaker_pic.rs`. The
//! breaker is left at its default-disabled state since the BOT-001 path
//! sits *above* the breaker gate in `check_vaults`.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use std::time::{Duration, SystemTime};

use rumi_protocol_backend::event::Event;
use rumi_protocol_backend::ProtocolError;
use rumi_protocol_backend::EventTypeFilter;

// ─── Local mirrors of ICRC-1 Candid types ───

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
struct TransferArg {
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    amount: Nat,
    fee: Option<Nat>,
    memo: Option<Vec<u8>>,
    created_at_time: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum TransferError {
    BadFee { expected_fee: Nat },
    BadBurn { min_burn_amount: Nat },
    InsufficientFunds { balance: Nat },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

// ─── Backend init / vault types ───

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
struct VaultArg {
    vault_id: u64,
    amount: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct OpenVaultSuccess {
    vault_id: u64,
    block_index: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct SuccessWithFee {
    block_index: u64,
    fee_amount_paid: u64,
    collateral_amount_received: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct BotLiquidationResult {
    vault_id: u64,
    collateral_amount: u64,
    debt_covered: u64,
    collateral_price_e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct BotStatsResponse {
    liquidation_bot_principal: Option<Principal>,
    budget_total_e8s: u64,
    budget_remaining_e8s: u64,
    budget_start_timestamp: u64,
    total_debt_covered_e8s: u64,
}

// Subset of the real GetEventsArg / GetEventsFilteredResponse — only the
// fields this fence needs. Field-keyed Candid records let us subset safely.
#[derive(CandidType, Deserialize, Clone, Debug, Default)]
struct GetEventsArg {
    start: u64,
    length: u64,
    types: Option<Vec<EventTypeFilter>>,
    principal: Option<Principal>,
    collateral_token: Option<Principal>,
    time_range: Option<EventTimeRange>,
    min_size_e8s: Option<u64>,
    admin_labels: Option<Vec<String>>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct EventTimeRange {
    start_ns: u64,
    end_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct GetEventsFilteredResponse {
    total: u64,
    events: Vec<(u64, Event)>,
}

// ─── WASM fixtures ───

fn icrc1_ledger_wasm() -> Vec<u8> {
    include_bytes!("../../ledger/ic-icrc1-ledger.wasm").to_vec()
}

fn protocol_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_protocol_backend.wasm")
        .to_vec()
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
        rates: vec![("ICP/USD".to_string(), 1_000_000_000)], // $10.00 (e8s)
    };
    encode_one(mock).expect("encode mock XRC init")
}

// ─── Helpers ───

fn account(owner: Principal) -> Account {
    Account {
        owner,
        subaccount: None,
    }
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
        WasmResult::Reply(b) => decode_one(&b).expect("decode icrc2_approve"),
        WasmResult::Reject(m) => panic!("icrc2_approve rejected: {}", m),
    };
    parsed.expect("approve returned error");
}

fn icrc1_transfer_call(
    pic: &PocketIc,
    ledger: Principal,
    sender: Principal,
    to: Principal,
    amount: u128,
) {
    let args = TransferArg {
        from_subaccount: None,
        to: account(to),
        amount: Nat::from(amount),
        fee: None,
        memo: None,
        created_at_time: None,
    };
    let result = pic
        .update_call(ledger, sender, "icrc1_transfer", encode_one(args).unwrap())
        .expect("icrc1_transfer call failed");
    let parsed: Result<Nat, TransferError> = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode icrc1_transfer"),
        WasmResult::Reject(m) => panic!("icrc1_transfer rejected: {}", m),
    };
    parsed.expect("transfer returned error");
}

fn icrc1_balance_of_call(pic: &PocketIc, ledger: Principal, owner: Principal) -> u64 {
    let result = pic
        .query_call(
            ledger,
            Principal::anonymous(),
            "icrc1_balance_of",
            encode_one(account(owner)).unwrap(),
        )
        .expect("icrc1_balance_of call failed");
    let parsed: Nat = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode balance"),
        WasmResult::Reject(m) => panic!("balance rejected: {}", m),
    };
    use num_traits::ToPrimitive;
    parsed.0.to_u64().unwrap_or(0)
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
    match result {
        WasmResult::Reply(_) => {}
        WasmResult::Reject(m) => panic!("set_exchange_rate rejected: {}", m),
    }
}

fn get_bot_stats(pic: &PocketIc, protocol_id: Principal) -> BotStatsResponse {
    let result = pic
        .query_call(
            protocol_id,
            Principal::anonymous(),
            "get_bot_stats",
            encode_args(()).unwrap(),
        )
        .expect("get_bot_stats call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode get_bot_stats"),
        WasmResult::Reject(m) => panic!("get_bot_stats rejected: {}", m),
    }
}

fn get_bot_001_events(pic: &PocketIc, protocol_id: Principal) -> Vec<Event> {
    let args = GetEventsArg {
        start: 0,
        length: 200,
        types: Some(vec![EventTypeFilter::BotClaimReconciliationNeeded]),
        ..Default::default()
    };
    // `get_events_filtered` is `#[query]` and traps on update calls. Hit it
    // as a query so the data-certificate guard in main.rs doesn't reject.
    let result = pic
        .query_call(
            protocol_id,
            Principal::anonymous(),
            "get_events_filtered",
            encode_one(args).unwrap(),
        )
        .expect("get_events_filtered call failed");
    let parsed: GetEventsFilteredResponse = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode get_events_filtered"),
        WasmResult::Reject(m) => panic!("get_events_filtered rejected: {}", m),
    };
    parsed.events.into_iter().map(|(_, e)| e).collect()
}

fn set_liquidation_bot_config_admin(
    fixture: &Fixture,
    bot_principal: Principal,
    monthly_budget_e8s: u64,
) {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            fixture.developer,
            "set_liquidation_bot_config",
            encode_args((bot_principal, monthly_budget_e8s)).unwrap(),
        )
        .expect("set_liquidation_bot_config call failed");
    let parsed: Result<(), ProtocolError> = match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode set_liquidation_bot_config"),
        WasmResult::Reject(m) => panic!("set_liquidation_bot_config rejected: {}", m),
    };
    parsed.expect("set_liquidation_bot_config returned error");
}

fn bot_claim_call(
    fixture: &Fixture,
    bot: Principal,
    vault_id: u64,
) -> Result<BotLiquidationResult, ProtocolError> {
    let result = fixture
        .pic
        .update_call(
            fixture.protocol_id,
            bot,
            "bot_claim_liquidation",
            encode_args((vault_id,)).unwrap(),
        )
        .expect("bot_claim_liquidation call failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode bot_claim_liquidation"),
        WasmResult::Reject(m) => panic!("bot_claim_liquidation rejected: {}", m),
    }
}

// ─── Fixture ───

struct Fixture {
    pic: PocketIc,
    protocol_id: Principal,
    icp_ledger: Principal,
    #[allow(dead_code)]
    icusd_ledger: Principal,
    xrc_id: Principal,
    developer: Principal,
    #[allow(dead_code)]
    test_user: Principal,
    /// Pre-opened vault id with 50 ICP collateral and 100 icUSD borrowed.
    /// At $10/ICP starting price → CR = 500%. Drop ICP to $0.10 to push
    /// well below the 110% liquidation threshold.
    vault_id: u64,
}

fn setup_fixture() -> Fixture {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();

    let test_user = Principal::self_authenticating(b"bot_001_pic_user");
    let developer = Principal::self_authenticating(b"bot_001_pic_developer");
    let treasury = Principal::self_authenticating(b"bot_001_pic_treasury");
    // Wave-11 BOT-001: the ICP ledger MUST use a minting account that is
    // NOT the protocol. The LIQ-008 fixture uses `protocol_id` as the
    // minting account for convenience, but that turns every protocol →
    // bot transfer into a mint (and bot → protocol return into a burn) —
    // which makes `icrc1_balance_of(protocol_id)` always 0 and breaks
    // the BOT-001 gate's premise. A separate minter lets the protocol
    // hold a real balance that the gate can observe.
    let icp_minter = Principal::self_authenticating(b"bot_001_pic_icp_minter");

    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 2_000_000_000_000);
    pic.set_controllers(protocol_id, None, vec![Principal::anonymous(), developer])
        .expect("set_controllers failed");

    let icp_ledger = deploy_icrc1_ledger(
        &pic,
        account(icp_minter),
        10_000,
        vec![(account(test_user), Nat::from(1_000_000_000_000u64))],
        "Internet Computer Protocol",
        "ICP",
        developer,
    );

    // icUSD ledger keeps protocol as minter — that's how icUSD actually
    // works (the protocol mints/burns icUSD on borrow/repay). Only the
    // collateral ledger needs a separate minter.
    let icusd_ledger = deploy_icrc1_ledger(
        &pic,
        account(protocol_id),
        0,
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

    // Quiet down rate / fee curves so the vault math stays predictable across
    // ticks. Same boilerplate as the LIQ-008 fixture.
    let _ = pic
        .update_call(
            protocol_id,
            developer,
            "set_borrowing_fee_curve",
            encode_args((None::<String>,)).unwrap(),
        )
        .expect("set_borrowing_fee_curve");
    let _ = pic
        .update_call(
            protocol_id,
            developer,
            "set_rate_curve_markers",
            encode_args((None::<Principal>, vec![(1.5f64, 1.0f64), (3.0f64, 1.0f64)])).unwrap(),
        )
        .expect("set_rate_curve_markers");
    let _ = pic
        .update_call(
            protocol_id,
            developer,
            "set_borrowing_fee",
            encode_args((0.0f64,)).unwrap(),
        )
        .expect("set_borrowing_fee");
    let _ = pic
        .update_call(
            protocol_id,
            developer,
            "set_interest_rate",
            encode_args((icp_ledger, 0.0f64)).unwrap(),
        )
        .expect("set_interest_rate");

    let _ = pic
        .update_call(
            protocol_id,
            developer,
            "set_treasury_principal",
            encode_args((treasury,)).unwrap(),
        )
        .expect("set_treasury_principal");

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
        amount: 10_000_000_000u64, // 100 icUSD borrowed
    };
    let borrow_result = pic
        .update_call(
            protocol_id,
            test_user,
            "borrow_from_vault",
            encode_args((borrow_arg,)).unwrap(),
        )
        .expect("borrow_from_vault failed");
    match borrow_result {
        WasmResult::Reply(bytes) => {
            let r: Result<SuccessWithFee, ProtocolError> =
                decode_one(&bytes).expect("decode borrow");
            r.expect("borrow_from_vault returned error");
        }
        WasmResult::Reject(msg) => panic!("borrow rejected: {}", msg),
    }

    Fixture {
        pic,
        protocol_id,
        icp_ledger,
        icusd_ledger,
        xrc_id,
        developer,
        test_user,
        vault_id,
    }
}

/// Drop ICP price and tick until the protocol's cached price reflects it.
/// Drops outside the 70%-143% sanity band (e.g. $10 → $0.10) need three
/// consecutive matching XRC samples before `check_price_sanity_band`
/// confirms — each XRC interval is 300s, so we advance 310s and tick four
/// times to land safely past the third confirmation.
fn drop_icp_price(fixture: &Fixture, new_price_e8s: u64) {
    xrc_set_rate(
        &fixture.pic,
        fixture.xrc_id,
        fixture.developer,
        "ICP",
        "USD",
        new_price_e8s,
    );
    for _ in 0..4 {
        fixture.pic.advance_time(Duration::from_secs(310));
        for _ in 0..15 {
            fixture.pic.tick();
        }
    }
}

/// Make `vault_id` underwater AND configure the bot, then have the bot
/// claim the vault. After this call, the bot principal holds the
/// collateral and the protocol has a live `bot_claims` entry. Returns
/// `(pre_claim_budget, claim)` so callers can compare against the
/// post-claim budget without races against the bot-config write.
fn seed_bot_claim(fixture: &Fixture) -> (u64, BotLiquidationResult) {
    // Bot = developer for fixture simplicity. Budget large enough for
    // the full 100 icUSD claim ($10k).
    set_liquidation_bot_config_admin(fixture, fixture.developer, 1_000_000_000_000u64);

    // Drop to $2.50/ICP. Vault: 50 ICP × $2.50 / $100 debt = 125% CR
    // (< 133% liq threshold → liquidatable) yet TCR also = 125% (> 100%
    // → no ReadOnly auto-latch, so `check_vaults` keeps running and the
    // BOT-001 gate has a chance to fire on the next tick).
    drop_icp_price(fixture, 250_000_000);

    let pre_claim_budget = get_bot_stats(&fixture.pic, fixture.protocol_id).budget_remaining_e8s;

    let claim = bot_claim_call(fixture, fixture.developer, fixture.vault_id)
        .expect("bot_claim_liquidation must succeed against underwater vault");

    (pre_claim_budget, claim)
}

/// Advance time + tick enough for the XRC interval to fire and
/// `check_vaults` (and its BOT-001 gate) to run. The bot-claim timeout is
/// 600s; we advance 700s so the claim qualifies as expired by the time
/// the next XRC tick runs.
fn fast_forward_past_bot_timeout(fixture: &Fixture) {
    fixture.pic.advance_time(Duration::from_secs(700));
    for _ in 0..15 {
        fixture.pic.tick();
    }
}

// ─── Tests ───

/// BOT-001 PIC #1: when the bot did NOT return the collateral, the
/// auto-cancel must skip — keeping `bot_claims`, `vault.bot_processing`,
/// and `bot_budget_remaining_e8s` intact — and emit a
/// `BotClaimReconciliationNeeded` event so admin can reconcile.
#[test]
fn bot_001_pic_auto_cancel_skipped_when_balance_below_required() {
    let f = setup_fixture();

    let (pre_claim_budget, claim) = seed_bot_claim(&f);

    // Sanity: the budget was deducted by the claim.
    let post_claim_budget = get_bot_stats(&f.pic, f.protocol_id).budget_remaining_e8s;
    assert!(
        post_claim_budget < pre_claim_budget,
        "bot_claim_liquidation must deduct from budget (before {} after {})",
        pre_claim_budget,
        post_claim_budget
    );

    // Sanity: the bot now actually holds the collateral (so the BOT-001
    // gate has something to detect).
    let bot_balance = icrc1_balance_of_call(&f.pic, f.icp_ledger, f.developer);
    assert!(
        bot_balance >= claim.collateral_amount.saturating_sub(10_000),
        "bot must hold the seized collateral; got {} expected ~{}",
        bot_balance,
        claim.collateral_amount
    );

    // Sanity: no BOT-001 events yet.
    assert!(
        get_bot_001_events(&f.pic, f.protocol_id).is_empty(),
        "no BotClaimReconciliationNeeded events expected before timeout"
    );

    // Advance past the 10-min auto-cancel timeout and trigger
    // `check_vaults` via XRC tick. Critically, the bot did NOT return the
    // collateral, so the BOT-001 gate must fire.
    fast_forward_past_bot_timeout(&f);

    // The auto-cancel must NOT have fired: budget unchanged from
    // post-claim baseline (no restore), claim still present.
    let final_budget = get_bot_stats(&f.pic, f.protocol_id).budget_remaining_e8s;
    assert_eq!(
        final_budget, post_claim_budget,
        "BOT-001 gate must prevent budget restore when collateral wasn't returned (saw budget {} vs expected {})",
        final_budget, post_claim_budget
    );

    // A `BotClaimReconciliationNeeded` event must exist for this vault.
    let events = get_bot_001_events(&f.pic, f.protocol_id);
    assert!(
        !events.is_empty(),
        "BOT-001 gate must emit BotClaimReconciliationNeeded when shortfall detected"
    );
    let matched = events.iter().any(|e| match e {
        Event::BotClaimReconciliationNeeded {
            vault_id,
            observed_balance,
            required_balance,
            ..
        } => *vault_id == f.vault_id && observed_balance < required_balance,
        _ => false,
    });
    assert!(
        matched,
        "expected a BotClaimReconciliationNeeded event for vault #{} with observed < required, saw {:?}",
        f.vault_id, events
    );
}

/// BOT-001 PIC #2: when the bot DID return the collateral, the
/// auto-cancel proceeds — clearing the claim, restoring the budget, and
/// NOT emitting `BotClaimReconciliationNeeded`. This preserves the
/// pre-Wave-11 happy path so we don't regress the "bot crashed but
/// returned funds" recovery.
#[test]
fn bot_001_pic_auto_cancel_proceeds_when_balance_sufficient() {
    let f = setup_fixture();

    let (pre_claim_budget, claim) = seed_bot_claim(&f);

    let post_claim_budget = get_bot_stats(&f.pic, f.protocol_id).budget_remaining_e8s;
    assert!(
        post_claim_budget < pre_claim_budget,
        "bot_claim_liquidation must deduct from budget"
    );

    // Bot returns the collateral to the protocol's main account, paying
    // the ICP transfer fee. The BOT-001 gate compares against
    // `claim.collateral_amount - ledger_fee`, so transferring exactly
    // that amount is the threshold case where the gate must NOT fire.
    let icp_fee: u64 = 10_000;
    let return_amount = claim.collateral_amount.saturating_sub(icp_fee);
    icrc1_transfer_call(
        &f.pic,
        f.icp_ledger,
        f.developer,
        f.protocol_id,
        return_amount as u128,
    );

    // Sanity: the protocol's main account must now hold AT LEAST the
    // required collateral so the BOT-001 gate has something to detect on
    // the next `check_vaults` tick. If this assertion fires, the rest of
    // the test is moot — the bot return didn't actually credit the
    // protocol.
    let protocol_balance = icrc1_balance_of_call(&f.pic, f.icp_ledger, f.protocol_id);
    let required = claim.collateral_amount.saturating_sub(icp_fee);
    assert!(
        protocol_balance >= required,
        "protocol balance {} must cover required {} after bot return",
        protocol_balance,
        required
    );

    fast_forward_past_bot_timeout(&f);

    // Auto-cancel must have fired: budget restored, no BOT-001 event.
    let final_budget = get_bot_stats(&f.pic, f.protocol_id).budget_remaining_e8s;
    assert_eq!(
        final_budget, pre_claim_budget,
        "auto-cancel must restore budget when collateral was returned (saw {} expected pre-claim baseline {})",
        final_budget, pre_claim_budget
    );

    let events = get_bot_001_events(&f.pic, f.protocol_id);
    assert!(
        events.is_empty(),
        "no BotClaimReconciliationNeeded events expected on the happy path; got {:?}",
        events
    );
}
