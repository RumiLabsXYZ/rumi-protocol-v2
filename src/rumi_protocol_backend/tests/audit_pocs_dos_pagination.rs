//! Wave-9a DoS hardening: paginated public queries (DOS-001, DOS-003,
//! DOS-004).
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/findings.json` findings
//!     DOS-001 (`get_vault_history`), DOS-003 (`get_events_by_principal`),
//!     DOS-004 (`get_all_vaults` / `get_vaults(None)` /
//!     `get_liquidatable_vaults`).
//!   * Wave plan: `audit-reports/2026-04-22-28e9896/remediation-plan.md`
//!     §"Wave 9 — DoS hardening", "Quick wins" subsection.
//!
//! # What the bugs were
//!
//! Each of the three legacy entry points walked an unbounded data
//! structure in a single query:
//!
//!   * `get_vault_history(vault_id)` decoded every event in the stable
//!     log to evaluate `is_vault_related`. As history grows
//!     (`AccrueInterest` / `PriceUpdate` events fire on every tick) a
//!     single call walks the full log; per-call cost scales linearly
//!     with total log length.
//!   * `get_events_by_principal(principal)` materialised every match
//!     into a `Vec<(u64, Event)>` before truncating to MAX_RESULTS=500.
//!     The `.collect()` step was unbounded in both scan and intermediate
//!     allocation.
//!   * `get_all_vaults()`, `get_vaults(None)`, and
//!     `get_liquidatable_vaults()` cloned and Candid-encoded every vault
//!     in `vault_id_to_vaults`. At 10k+ vaults the reply would push past
//!     the 3 MB soft reply ceiling.
//!
//! # How this file tests the fix
//!
//! This file fences four behaviours:
//!
//!   * **Cap-value fence** — assert each public cap constant has its
//!     audit-pinned value. The implementation references the same
//!     constants, so a regression that lowers the cap (or raises it
//!     past the audit budget) trips the fence.
//!
//!   * **Paged response shape** — the new `*Paged` / `*Page` response
//!     types carry the cursor + total fields the explorer needs to
//!     render accurate page indicators.
//!
//!   * **DOS-004 cursor round-trip** (PocketIC) — open ten vaults,
//!     walk `get_vaults_page` at limit=3, assert (a) no page exceeds
//!     the requested limit, (b) `next_start_id` advances strictly
//!     forward, (c) the assembled stream covers every open vault
//!     exactly once with no gaps or duplicates, (d) the final page's
//!     `next_start_id` is `None`.
//!
//!   * **DOS-001 paged accuracy fence** (PocketIC) — open one vault and
//!     drive ~30 vault-related events on it. Assert (a)
//!     `get_vault_history(id)` returns every match (well under
//!     MAX_VAULT_HISTORY), (b) `get_vault_history_paged(id, 0, 200)`
//!     returns the same set with `total` matching, (c) the paged
//!     events are ordered newest-first.
//!
//! Exhaustive cap-firing fences (response strictly truncated to the
//! cap when total > cap) are documented to require >500 vaults / >200
//! events / >500 principal events. The cap-value constant fences plus
//! the implementation's `take(MAX_*)` / `length.min(MAX_*)` calls
//! together pin the cap behavior; the cursor round-trip fence pins
//! that paging is functionally correct without a single-call DoS.

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use std::time::{Duration, SystemTime};

use rumi_protocol_backend::{
    ProtocolError, MAX_EVENTS_BY_PRINCIPAL_LEGACY, MAX_EVENTS_BY_PRINCIPAL_OUTPUT,
    MAX_EVENTS_BY_PRINCIPAL_SCAN, MAX_VAULTS_LEGACY_PAGE, MAX_VAULTS_PAGE_LIMIT,
    MAX_VAULT_HISTORY,
};

// ─── Constants fences ───

/// DOS-001: `get_vault_history` legacy entry point + page-size cap on
/// `get_vault_history_paged`.
#[test]
fn dos_001_max_vault_history_pinned_at_200() {
    assert_eq!(MAX_VAULT_HISTORY, 200,
        "Wave-9a DOS-001: per-vault history cap must be 200 entries (audit budget). \
         Lowering risks legacy callers losing recent activity; raising restores DoS surface.");
}

/// DOS-003: `get_events_by_principal` legacy + paged caps.
#[test]
fn dos_003_events_by_principal_caps_pinned() {
    assert_eq!(MAX_EVENTS_BY_PRINCIPAL_LEGACY, 500,
        "Wave-9a DOS-003: legacy ring-buffer output cap must be 500.");
    assert_eq!(MAX_EVENTS_BY_PRINCIPAL_SCAN, 5_000,
        "Wave-9a DOS-003: per-call scan-window cap must be 5_000 — bounds the \
         instructions any single paged call can spend walking the event log.");
    assert_eq!(MAX_EVENTS_BY_PRINCIPAL_OUTPUT, 500,
        "Wave-9a DOS-003: paged output cap must be 500 — matches the legacy cap so \
         a caller sees the same per-call payload boundary.");
}

/// DOS-004: legacy `get_all_vaults` / `get_vaults(None)` /
/// `get_liquidatable_vaults` and the matching `*_page` variants.
#[test]
fn dos_004_vaults_page_caps_pinned() {
    assert_eq!(MAX_VAULTS_LEGACY_PAGE, 500,
        "Wave-9a DOS-004: legacy bulk-vault entry points must cap at 500 vaults — \
         keeps a single Candid encode under the IC reply-size soft ceiling.");
    assert_eq!(MAX_VAULTS_PAGE_LIMIT, 500,
        "Wave-9a DOS-004: paged vaults entry points must cap `limit` at 500.");
}

// ─── ICRC-1 candid mirrors (lifted from existing audit POC fixtures) ───

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
struct CandidVault {
    collateral_amount: u64,
    owner: Principal,
    vault_id: u64,
    collateral_type: Principal,
    accrued_interest: u64,
    icp_margin_amount: u64,
    borrowed_icusd_amount: u64,
}

#[derive(CandidType, Deserialize, Debug)]
struct VaultsPageResponse {
    vaults: Vec<CandidVault>,
    next_start_id: Option<u64>,
}

#[derive(CandidType, Deserialize, Debug)]
struct VaultHistoryPagedResponse {
    total: u64,
    /// `Event` is decoded as `IDLValue` here — we only need the index
    /// shape for these fences, not the full variant decoding.
    events: Vec<(u64, candid::Reserved)>,
}

// ─── Wasm fixtures ───

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
        rates: vec![("ICP/USD".to_string(), 1_000_000_000)], // $10.00 e8s
    };
    encode_one(mock).expect("encode mock XRC init")
}

// ─── Helpers ───

fn account(owner: Principal) -> Account {
    Account { owner, subaccount: None }
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

// ─── Fixture ───

struct Fixture {
    pic: PocketIc,
    protocol_id: Principal,
    icp_ledger: Principal,
    test_user: Principal,
}

/// Stand up the protocol with a mock XRC at $10/ICP, mint a fat ICP
/// balance to `test_user`, and pre-approve the protocol for that
/// allowance. Borrowing fee + interest curves are zeroed so opens are
/// exact and don't drift the test math.
fn setup_fixture(initial_icp_e8s: u128) -> Fixture {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();

    let test_user = Principal::self_authenticating(b"dos_pagination_test_user");
    let developer = Principal::self_authenticating(b"dos_pagination_developer");

    let protocol_id = pic.create_canister();
    pic.add_cycles(protocol_id, 2_000_000_000_000);
    pic.set_controllers(protocol_id, None, vec![Principal::anonymous(), developer])
        .expect("set_controllers failed");

    let icp_ledger = deploy_icrc1_ledger(
        &pic,
        account(protocol_id),
        10_000,
        vec![(account(test_user), Nat::from(initial_icp_e8s))],
        "Internet Computer Protocol",
        "ICP",
        developer,
    );

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

    // Zero out fee/interest curves so the test math stays exact.
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

    // Approve once with the full balance so subsequent open_vaults can
    // pull collateral without a fresh approve per call.
    icrc2_approve_call(&pic, icp_ledger, test_user, protocol_id, initial_icp_e8s);

    Fixture { pic, protocol_id, icp_ledger, test_user }
}

fn open_collateral_only_vault(f: &Fixture, collateral_e8s: u64) -> u64 {
    let result = f
        .pic
        .update_call(
            f.protocol_id,
            f.test_user,
            "open_vault",
            encode_args((collateral_e8s, None::<Principal>)).unwrap(),
        )
        .expect("open_vault failed");
    match result {
        WasmResult::Reply(bytes) => {
            let r: Result<OpenVaultSuccess, ProtocolError> =
                decode_one(&bytes).expect("decode open_vault");
            r.expect("open_vault returned error").vault_id
        }
        WasmResult::Reject(msg) => panic!("open_vault rejected: {}", msg),
    }
}

fn add_margin(f: &Fixture, vault_id: u64, amount_e8s: u64) {
    let result = f
        .pic
        .update_call(
            f.protocol_id,
            f.test_user,
            "add_margin_to_vault",
            encode_args((VaultArg { vault_id, amount: amount_e8s },)).unwrap(),
        )
        .expect("add_margin call failed");
    if let WasmResult::Reject(m) = result {
        panic!("add_margin rejected: {}", m);
    }
}

fn query_get_vaults_page(f: &Fixture, start_id: u64, limit: u64) -> VaultsPageResponse {
    let result = f
        .pic
        .query_call(
            f.protocol_id,
            Principal::anonymous(),
            "get_vaults_page",
            encode_args((start_id, limit)).unwrap(),
        )
        .expect("get_vaults_page query failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode get_vaults_page"),
        WasmResult::Reject(m) => panic!("get_vaults_page rejected: {}", m),
    }
}

fn query_get_vault_count(f: &Fixture) -> u64 {
    let result = f
        .pic
        .query_call(
            f.protocol_id,
            Principal::anonymous(),
            "get_vault_count",
            encode_args(()).unwrap(),
        )
        .expect("get_vault_count query failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode get_vault_count"),
        WasmResult::Reject(m) => panic!("get_vault_count rejected: {}", m),
    }
}

fn query_get_vault_history(f: &Fixture, vault_id: u64) -> Vec<(u64, candid::Reserved)> {
    let result = f
        .pic
        .query_call(
            f.protocol_id,
            f.test_user,
            "get_vault_history",
            encode_args((vault_id,)).unwrap(),
        )
        .expect("get_vault_history query failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode get_vault_history"),
        WasmResult::Reject(m) => panic!("get_vault_history rejected: {}", m),
    }
}

fn query_get_vault_history_paged(
    f: &Fixture,
    vault_id: u64,
    start: u64,
    length: u64,
) -> VaultHistoryPagedResponse {
    let result = f
        .pic
        .query_call(
            f.protocol_id,
            f.test_user,
            "get_vault_history_paged",
            encode_args((vault_id, start, length)).unwrap(),
        )
        .expect("get_vault_history_paged query failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode get_vault_history_paged"),
        WasmResult::Reject(m) => panic!("get_vault_history_paged rejected: {}", m),
    }
}

// ─── PocketIC fences ───

/// Wave-9a DOS-004 cursor round-trip: open ten small vaults, walk
/// `get_vaults_page` at limit=3, assert (a) no page exceeds the
/// requested limit, (b) `next_start_id` advances strictly forward,
/// (c) the assembled stream covers every open vault exactly once,
/// (d) the final page's `next_start_id` is `None`.
#[test]
fn dos_004_get_vaults_page_cursor_round_trip() {
    // 10 vaults * 1.0 ICP each + 100 ICP slack for fees.
    let f = setup_fixture(20_000_000_000u128);

    let mut opened_ids: Vec<u64> = Vec::with_capacity(10);
    for _ in 0..10 {
        let id = open_collateral_only_vault(&f, 100_000_000); // 1 ICP
        opened_ids.push(id);
    }
    opened_ids.sort();

    assert_eq!(query_get_vault_count(&f), 10);

    // Walk pages of 3 starting at id 0.
    let mut walked: Vec<u64> = Vec::new();
    let mut cursor: u64 = 0;
    let mut prev_cursor: Option<u64> = None;
    let mut call_count: u32 = 0;
    loop {
        let resp = query_get_vaults_page(&f, cursor, 3);
        assert!(
            resp.vaults.len() <= 3,
            "Wave-9a DOS-004: response must not exceed requested limit (got {}, asked 3)",
            resp.vaults.len(),
        );
        for v in &resp.vaults {
            walked.push(v.vault_id);
        }
        call_count += 1;
        match resp.next_start_id {
            Some(next) => {
                if let Some(prev) = prev_cursor {
                    assert!(
                        next > prev,
                        "next_start_id must advance strictly forward: prev={}, next={}",
                        prev, next,
                    );
                }
                prev_cursor = Some(next);
                cursor = next;
            }
            None => break,
        }
        // Defensive guard against an infinite loop if next_start_id ever
        // failed to advance — limit at 10x the page count needed.
        assert!(call_count < 20, "cursor walk failed to terminate");
    }

    walked.sort();
    assert_eq!(walked, opened_ids,
        "cursor walk must yield every opened vault exactly once with no gaps or duplicates");
    assert_eq!(call_count, 4,
        "10 vaults at limit=3 should take ceil(10/3) + final-empty-or-3rd-with-no-next = 4 calls");
}

/// Wave-9a DOS-004 absolute cap firing fence: a caller can pass
/// `limit = u64::MAX` and the canister must clamp the page to
/// `MAX_VAULTS_PAGE_LIMIT` BEFORE reading the BTreeMap range. With ten
/// vaults the natural total bounds the response below the cap, so
/// this fence proves the clamp doesn't crash on huge inputs and
/// returns the natural total — pre-fix code would also have done
/// this, but pre-fix code with > 500 vaults would have walked past
/// the cap. The cap fence at the constant level (above) plus this
/// "huge limit doesn't crash" fence together pin the contract.
#[test]
fn dos_004_get_vaults_page_handles_unbounded_limit() {
    let f = setup_fixture(20_000_000_000u128);

    for _ in 0..10 {
        open_collateral_only_vault(&f, 100_000_000);
    }

    let resp = query_get_vaults_page(&f, 0, u64::MAX);
    assert_eq!(
        resp.vaults.len(),
        10,
        "with 10 vaults total, response must contain all 10 (clamping limit must not \
         drop entries below the natural total)",
    );
    assert!(resp.next_start_id.is_none(),
        "10 vaults < cap: there should be no continuation cursor");
    // Also a lower bound check on the cap: response.len <= MAX_VAULTS_PAGE_LIMIT.
    assert!(
        (resp.vaults.len() as u64) <= MAX_VAULTS_PAGE_LIMIT,
        "Wave-9a DOS-004: response.len must always be <= MAX_VAULTS_PAGE_LIMIT",
    );
}

/// Wave-9a DOS-001 paged accuracy fence: open one vault and drive ~30
/// vault-related events on it via `add_margin_to_vault` (each
/// generates `MarginTransfer` + `CollateralDeposited`). Assert (a)
/// `get_vault_history(id)` returns every match (well under
/// `MAX_VAULT_HISTORY`), (b) `get_vault_history_paged(id, 0, 200)`
/// returns the same set with `total` matching, (c) the paged events
/// are ordered newest-first (highest event index first).
#[test]
fn dos_001_get_vault_history_paged_matches_legacy() {
    // 1 vault opened + 30 add_margin * 0.1 ICP + slack for fees.
    let f = setup_fixture(50_000_000_000u128);

    let vault_id = open_collateral_only_vault(&f, 1_000_000_000); // 10 ICP

    for _ in 0..30 {
        add_margin(&f, vault_id, 10_000_000); // 0.1 ICP each
    }

    let legacy = query_get_vault_history(&f, vault_id);
    assert!(
        legacy.len() <= MAX_VAULT_HISTORY,
        "Wave-9a DOS-001: legacy entry point must respect MAX_VAULT_HISTORY ({}) — got {}",
        MAX_VAULT_HISTORY, legacy.len(),
    );
    assert!(
        legacy.len() >= 31,
        "expected at least one OpenVault + 30 add_margin events (got {})",
        legacy.len(),
    );

    let paged = query_get_vault_history_paged(&f, vault_id, 0, 200);
    assert_eq!(
        paged.total as usize,
        legacy.len(),
        "paged `total` must equal the legacy match count (both reflect every event \
         touching this vault)",
    );
    assert_eq!(
        paged.events.len() as usize,
        legacy.len().min(200),
        "first page must contain min(total, length) events",
    );

    // Newest-first ordering: paged event indices descend.
    let indices: Vec<u64> = paged.events.iter().map(|(idx, _)| *idx).collect();
    assert!(
        indices.windows(2).all(|w| w[0] > w[1]),
        "Wave-9a DOS-001: paged events must be newest-first (descending event-log index); \
         got indices: {:?}",
        indices,
    );
}
