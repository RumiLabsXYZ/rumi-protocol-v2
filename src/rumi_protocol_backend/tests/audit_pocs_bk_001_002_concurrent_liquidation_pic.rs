//! BK-001 / BK-002 PocketIC fence (Layer 3 — canister boundary, audit 2026-06-05).
//!
//! The per-vault `VaultLiquidationGuard` closes the cross-caller liquidation
//! over-seize. `GuardPrincipal` keys on the CALLER, so before the fix two
//! different liquidators racing the SAME vault both passed the guard, both
//! snapshotted the vault's full collateral pre-`await`, and both were paid that
//! full collateral out of the SHARED collateral pool — draining OTHER vaults'
//! backing. The ASYNC-001 re-cap fixed the vault accounting but NOT the payout.
//!
//! This fixture seeds the pool with TWO vaults of equal collateral and fires
//! two concurrent `liquidate_vault_partial` calls (from two different
//! principals) at ONE of them via `submit_call` (both ingress messages are
//! queued before either executes, so they interleave). The economic invariant
//! is then checked: the total collateral paid out across both attempts must not
//! exceed the targeted vault's collateral. On the pre-fix wasm both liquidators
//! are paid ~the full vault collateral (sum ~= 2x), draining the bystander
//! vault's backing; with the per-vault guard at most one full payout occurs.
//!
//! Fixture mirrors `audit_pocs_liq_005_deficit_account_pic.rs`.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use std::time::{Duration, SystemTime};

use rumi_protocol_backend::ProtocolError;

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

// ─── Backend types (mirrored locally; extra Candid fields are ignored) ───

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

// Minimal mirror — Candid ignores the other CandidVault fields on decode.
#[derive(CandidType, Deserialize, Clone, Debug)]
struct LiquidatableVault {
    vault_id: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct SuccessWithFee {
    block_index: u64,
    fee_amount_paid: u64,
    collateral_amount_received: Option<u64>,
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

fn xrc_set_rate(pic: &PocketIc, xrc: Principal, sender: Principal, base: &str, quote: &str, rate_e8s: u64) {
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

fn open_and_borrow(
    pic: &PocketIc,
    protocol_id: Principal,
    icp_ledger: Principal,
    user: Principal,
    collateral_e8s: u64,
    borrow_e8s: u64,
) -> u64 {
    icrc2_approve_call(pic, icp_ledger, user, protocol_id, collateral_e8s as u128);
    let open_result = pic
        .update_call(
            protocol_id,
            user,
            "open_vault",
            encode_args((collateral_e8s, None::<Principal>)).unwrap(),
        )
        .expect("open_vault failed");
    let vault_id = match open_result {
        WasmResult::Reply(bytes) => {
            let r: Result<OpenVaultSuccess, ProtocolError> = decode_one(&bytes).expect("decode open_vault");
            r.expect("open_vault returned error").vault_id
        }
        WasmResult::Reject(msg) => panic!("open_vault rejected: {}", msg),
    };
    let borrow_result = pic
        .update_call(
            protocol_id,
            user,
            "borrow_from_vault",
            encode_args((VaultArg { vault_id, amount: borrow_e8s },)).unwrap(),
        )
        .expect("borrow_from_vault failed");
    match borrow_result {
        WasmResult::Reply(bytes) => {
            let r: Result<SuccessWithFee, ProtocolError> = decode_one(&bytes).expect("decode borrow");
            r.expect("borrow_from_vault returned error");
        }
        WasmResult::Reject(msg) => panic!("borrow rejected: {}", msg),
    }
    vault_id
}

// ─── Test ───

/// BK-001/002: two concurrent `liquidate_vault_partial` calls (distinct callers)
/// against ONE vault must not pay out more collateral than that vault holds.
#[test]
fn bk_001_002_concurrent_liquidation_does_not_over_seize() {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();

    let user_a = Principal::self_authenticating(b"bk_001_user_a");
    let user_b = Principal::self_authenticating(b"bk_001_user_b");
    let developer = Principal::self_authenticating(b"bk_001_developer");
    let treasury = Principal::self_authenticating(b"bk_001_treasury");

    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 2_000_000_000_000);
    pic.set_controllers(protocol_id, None, vec![Principal::anonymous(), developer])
        .expect("set_controllers failed");

    let icp_ledger = deploy_icrc1_ledger(
        &pic,
        account(protocol_id),
        10_000,
        vec![
            (account(user_a), Nat::from(1_000_000_000_000u64)),
            (account(user_b), Nat::from(1_000_000_000_000u64)),
        ],
        "Internet Computer Protocol",
        "ICP",
        developer,
    );
    let icusd_ledger = deploy_icrc1_ledger(&pic, account(protocol_id), 0, vec![], "icUSD", "icUSD", developer);

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
    pic.install_canister(protocol_id, protocol_wasm(), encode_args((init,)).expect("encode init"), None);
    pic.advance_time(Duration::from_secs(1));
    for _ in 0..10 {
        pic.tick();
    }

    // Disable dynamic fee/interest curves so both users get their full 100 icUSD.
    for (method, payload) in [
        ("set_borrowing_fee_curve", encode_args((None::<String>,)).unwrap()),
        ("set_borrowing_fee", encode_args((0.0f64,)).unwrap()),
        ("set_treasury_principal", encode_args((treasury,)).unwrap()),
    ] {
        pic.update_call(protocol_id, developer, method, payload).expect(method);
    }
    pic.update_call(
        protocol_id,
        developer,
        "set_rate_curve_markers",
        encode_args((None::<Principal>, vec![(1.5f64, 1.0f64), (3.0f64, 1.0f64)])).unwrap(),
    )
    .expect("set_rate_curve_markers");
    pic.update_call(protocol_id, developer, "set_interest_rate", encode_args((icp_ledger, 0.0f64)).unwrap())
        .expect("set_interest_rate");

    // Vault A (target) and Vault B (bystander), each 50 ICP collateral / 100 icUSD debt.
    // The pool now holds 100 ICP; vault B's 50 ICP is the "other vault" backing
    // that a cross-caller over-seize on A would drain.
    let collateral_e8s = 5_000_000_000u64; // 50 ICP
    let borrow_e8s = 10_000_000_000u64; // 100 icUSD
    let vault_a = open_and_borrow(&pic, protocol_id, icp_ledger, user_a, collateral_e8s, borrow_e8s);
    let _vault_b = open_and_borrow(&pic, protocol_id, icp_ledger, user_b, collateral_e8s, borrow_e8s);

    // Both users approve the protocol to pull their icUSD for liquidation payment.
    icrc2_approve_call(&pic, icusd_ledger, user_a, protocol_id, 100_000_000_000u128);
    icrc2_approve_call(&pic, icusd_ledger, user_b, protocol_id, 100_000_000_000u128);

    // Drop ICP to $0.10 so vault A is deeply underwater (≈$5 backing $100 debt):
    // a single partial liquidation seizes ≈ all 50 ICP of A. Then tick until the
    // protocol's CACHED price has propagated and vault A actually shows up as
    // liquidatable — otherwise one racer could be rejected by a stale ($10)
    // price rather than by the per-vault guard, which would make this test pass
    // even on the buggy wasm.
    xrc_set_rate(&pic, xrc_id, developer, "ICP", "USD", 10_000_000);
    let mut liquidatable = false;
    for _ in 0..12 {
        pic.advance_time(Duration::from_secs(310));
        for _ in 0..6 {
            pic.tick();
        }
        let result = pic
            .query_call(protocol_id, Principal::anonymous(), "get_liquidatable_vaults", encode_args(()).unwrap())
            .expect("get_liquidatable_vaults");
        if let WasmResult::Reply(b) = result {
            if let Ok(vaults) = decode_one::<Vec<LiquidatableVault>>(&b) {
                if vaults.iter().any(|v| v.vault_id == vault_a) {
                    liquidatable = true;
                    break;
                }
            }
        }
    }
    assert!(
        liquidatable,
        "fixture precondition: vault A must be liquidatable (cached price propagated to $0.10) \
         before the race so both racers see it liquidatable"
    );

    // Race: submit BOTH liquidations before either executes, so the two ingress
    // messages interleave. user_a and user_b are distinct callers, so the
    // per-CALLER GuardPrincipal does not serialize them — only the per-VAULT
    // VaultLiquidationGuard does.
    let arg = encode_args((VaultArg { vault_id: vault_a, amount: borrow_e8s },)).unwrap();
    let msg_a = pic
        .submit_call(protocol_id, user_a, "liquidate_vault_partial", arg.clone())
        .expect("submit liquidation A");
    let msg_b = pic
        .submit_call(protocol_id, user_b, "liquidate_vault_partial", arg)
        .expect("submit liquidation B");

    let decode = |res: WasmResult| -> Result<SuccessWithFee, ProtocolError> {
        match res {
            WasmResult::Reply(b) => decode_one(&b).expect("decode liquidation result"),
            WasmResult::Reject(m) => panic!("liquidation rejected at canister level: {}", m),
        }
    };
    let res_a = decode(pic.await_call(msg_a).expect("await A"));
    let res_b = decode(pic.await_call(msg_b).expect("await B"));

    let seized = |r: &Result<SuccessWithFee, ProtocolError>| -> u64 {
        match r {
            Ok(s) => s.collateral_amount_received.unwrap_or(0),
            Err(_) => 0,
        }
    };
    let total_seized = seized(&res_a) + seized(&res_b);

    // At least one liquidation must have succeeded (the fix must not break liquidation).
    assert!(
        res_a.is_ok() || res_b.is_ok(),
        "expected at least one liquidation to succeed; A={:?} B={:?}",
        res_a,
        res_b
    );

    // EXACTLY ONE may succeed: two liquidators racing one (fully-underwater) vault
    // cannot both be paid. Pre-fix, the per-CALLER guard let both through and both
    // were paid the stale full collateral; with the per-VAULT guard the loser is
    // rejected (or finds the vault already gone).
    assert!(
        res_a.is_ok() ^ res_b.is_ok(),
        "BK-001/002: exactly one of the two concurrent liquidations must succeed; A={:?} B={:?}",
        res_a,
        res_b
    );

    // THE INVARIANT: total collateral paid to liquidators must not exceed the
    // targeted vault's collateral. Pre-fix, both callers were paid ~50 ICP from
    // the shared pool (sum ≈ 2x), draining the bystander vault's backing.
    assert!(
        total_seized <= collateral_e8s,
        "BK-001/002 over-seize: total collateral paid {} exceeds vault A's collateral {} \
         (A={:?}, B={:?})",
        total_seized,
        collateral_e8s,
        res_a,
        res_b
    );
}
