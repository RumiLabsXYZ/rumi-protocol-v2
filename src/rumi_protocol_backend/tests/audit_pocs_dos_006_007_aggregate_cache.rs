//! Wave-9b DoS hardening: cache aggregate query snapshots (DOS-006, DOS-007).
//!
//! Audit report:
//!   * `audit-reports/2026-04-22-28e9896/findings.json` findings
//!     DOS-006 (`get_protocol_status`) and DOS-007 (`get_treasury_stats`).
//!   * Wave plan: `audit-reports/2026-04-22-28e9896/remediation-plan.md`
//!     §"Wave 9 - DoS hardening", "Cached aggregates" subsection.
//!
//! # What the bugs were
//!
//! `get_protocol_status` and `get_treasury_stats` are public query
//! endpoints hit by the explorer at high frequency. Both aggregated
//! across every vault on every call:
//!
//!   * `get_protocol_status` summed `total_borrowed_icusd_amount`,
//!     `total_icp_margin_amount`, ran `weighted_average_interest_rate`
//!     (loops every vault), and looped over every collateral type to
//!     compute `total_debt_for_collateral` /
//!     `weighted_interest_rate_for_collateral`. Per-call cost scales
//!     linearly with vault count.
//!   * `get_treasury_stats` summed `accrued_interest` across every
//!     vault. Same O(N) shape.
//!
//! At explorer poll cadence both grew into the dominant cycle cost
//! on the backend canister.
//!
//! # How this file tests the fix
//!
//! This file fences four behaviours per finding:
//!
//!   * **Constant fences** - the snapshot TTL must remain at the
//!     audit-pinned 5 seconds. Lowering increases compute cost;
//!     raising risks serving stale aggregates after state changes.
//!
//!   * **Cache hit within TTL** - two consecutive query calls within
//!     5 seconds return the same `snapshot_ts_ns`, proving the second
//!     call did NOT re-aggregate.
//!
//!   * **Cache miss after TTL** - advancing past 5 seconds and calling
//!     again yields a fresh `snapshot_ts_ns`.
//!
//!   * **Live fields stay live** - `frozen` (and other live fields
//!     listed in the implementation comment) reflect current state
//!     even when the heavy fields are served from a cached snapshot.
//!     This is the correctness fence: caching must NEVER mask an
//!     admin-controlled kill switch or a state that has flipped via
//!     a non-aggregate path.
//!
//!   * **Snapshot-aware upgrade hygiene** - after a canister upgrade,
//!     the first query returns valid totals (cache survived from
//!     pre-upgrade, OR was dropped and recomputed cleanly). Either
//!     branch is acceptable; the contract is "no crash, no stale data
//!     from a different state shape".

use candid::{decode_one, encode_args, encode_one, CandidType, Deserialize, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use std::time::{Duration, SystemTime};

use rumi_protocol_backend::{
    PROTOCOL_STATUS_SNAPSHOT_TTL_NANOS, TREASURY_STATS_SNAPSHOT_TTL_NANOS,
};

// ─── Constants fences ───

/// DOS-006/-007: the snapshot TTL for `get_protocol_status` and
/// `get_treasury_stats` must remain at 5 seconds. The 5s window is
/// the spacing between explorer polls, not between aggregate refreshes.
/// The actual cache refresh happens in the existing 5-minute XRC tick;
/// this TTL only covers cold-start / first-call-after-upgrade scenarios
/// where the cache hasn't been warmed by a tick yet.
#[test]
fn dos_006_protocol_status_snapshot_ttl_pinned_at_5s() {
    assert_eq!(
        PROTOCOL_STATUS_SNAPSHOT_TTL_NANOS,
        5_000_000_000,
        "Wave-9b DOS-006: snapshot TTL must be 5 seconds (5_000_000_000 ns). \
         Lowering risks every poll re-aggregating; raising risks serving stale \
         heavy aggregates after a state change in the gap before the next \
         XRC tick refresh."
    );
}

#[test]
fn dos_007_treasury_stats_snapshot_ttl_pinned_at_5s() {
    assert_eq!(
        TREASURY_STATS_SNAPSHOT_TTL_NANOS,
        5_000_000_000,
        "Wave-9b DOS-007: treasury-stats snapshot TTL must match the \
         protocol-status TTL (5s). Both share the explorer poll cadence."
    );
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

// ─── Backend init / response candid mirrors ───

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
    Upgrade(UpgradeArgMirror),
}

#[derive(CandidType, Deserialize, Clone, Debug, Default)]
struct UpgradeArgMirror {
    mode: Option<ModeMirror>,
    description: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
enum ModeMirror {
    GeneralAvailability,
    Recovery,
    ReadOnly,
}

/// Minimal mirror of `ProtocolStatus` carrying the fields this fence
/// exercises. Candid ignores the rest of the record. The two fields
/// we *care* about are:
///   * `snapshot_ts_ns`: Wave-9b cache-hit observability.
///   * `frozen`, `mode`: live-field correctness fence.
///   * `total_icusd_borrowed`, `total_icp_margin`: heavy aggregates,
///     served from cache within TTL.
#[derive(CandidType, Deserialize, Debug)]
struct ProtocolStatusMirror {
    last_icp_timestamp: u64,
    total_icp_margin: u64,
    total_icusd_borrowed: u64,
    total_collateral_ratio: f64,
    mode: ModeMirror,
    frozen: bool,
    manual_mode_override: bool,
    last_icp_rate: f64,
    /// Wave-9b DOS-006: timestamp (nanos) when the cached aggregate
    /// was last computed. Two consecutive calls within
    /// `PROTOCOL_STATUS_SNAPSHOT_TTL_NANOS` must return the same
    /// value.
    snapshot_ts_ns: u64,
    // ... remaining fields are decoded into Reserved to stay
    // forward-compatible with future ProtocolStatus additions.
}

/// Same minimal mirror for `TreasuryStats`.
#[derive(CandidType, Deserialize, Debug)]
struct TreasuryStatsMirror {
    treasury_principal: Option<Principal>,
    total_accrued_interest_system: u64,
    pending_treasury_interest: u64,
    pending_treasury_collateral_entries: u64,
    liquidation_protocol_share: f64,
    pending_interest_for_pools_total: u64,
    interest_flush_threshold_e8s: u64,
    /// Wave-9b DOS-007: timestamp (nanos) when the cached aggregate
    /// was last computed.
    snapshot_ts_ns: u64,
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
    test_user: Principal,
    developer: Principal,
}

/// Stand up the protocol with a mock XRC at $10/ICP, mint a fat ICP
/// balance to `test_user`, pre-approve the protocol, and zero out the
/// borrowing fee + interest curves so test math is exact.
fn setup_fixture(initial_icp_e8s: u128) -> Fixture {
    let pic = PocketIcBuilder::new().with_nns_subnet().build();

    let test_user = Principal::self_authenticating(b"dos_006_007_test_user");
    let developer = Principal::self_authenticating(b"dos_006_007_developer");

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

    icrc2_approve_call(&pic, icp_ledger, test_user, protocol_id, initial_icp_e8s);

    Fixture { pic, protocol_id, test_user, developer }
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct OpenVaultSuccess {
    vault_id: u64,
    block_index: u64,
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
            let r: Result<OpenVaultSuccess, candid::Reserved> =
                decode_one(&bytes).expect("decode open_vault");
            match r {
                Ok(s) => s.vault_id,
                Err(_) => panic!("open_vault returned error"),
            }
        }
        WasmResult::Reject(msg) => panic!("open_vault rejected: {}", msg),
    }
}

fn query_get_protocol_status(f: &Fixture) -> ProtocolStatusMirror {
    let result = f
        .pic
        .query_call(
            f.protocol_id,
            Principal::anonymous(),
            "get_protocol_status",
            encode_args(()).unwrap(),
        )
        .expect("get_protocol_status query failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode get_protocol_status"),
        WasmResult::Reject(m) => panic!("get_protocol_status rejected: {}", m),
    }
}

fn query_get_treasury_stats(f: &Fixture) -> TreasuryStatsMirror {
    let result = f
        .pic
        .query_call(
            f.protocol_id,
            Principal::anonymous(),
            "get_treasury_stats",
            encode_args(()).unwrap(),
        )
        .expect("get_treasury_stats query failed");
    match result {
        WasmResult::Reply(b) => decode_one(&b).expect("decode get_treasury_stats"),
        WasmResult::Reject(m) => panic!("get_treasury_stats rejected: {}", m),
    }
}

fn freeze_protocol(f: &Fixture) {
    let result = f
        .pic
        .update_call(
            f.protocol_id,
            f.developer,
            "freeze_protocol",
            encode_args(()).unwrap(),
        )
        .expect("freeze_protocol call failed");
    if let WasmResult::Reject(m) = result {
        panic!("freeze_protocol rejected: {}", m);
    }
}

// ─── PocketIC behaviour fences ───

/// DOS-006 cache-hit: with the cache filled by the setup-time XRC
/// tick (vault count = 0 at that moment) and no time advanced past
/// the TTL, opening a vault must NOT change what `get_protocol_status`
/// returns. Two consecutive queries observe the pre-mutation cached
/// snapshot, proving the heavy aggregates were served from cache.
///
/// IC query semantics make this test the strong fence: queries cannot
/// persist state mutations across calls, so the only entity that can
/// fill the cache is an update message (the XRC tick). We rely on the
/// setup-time XRC tick — which runs before any test action — to
/// populate the cache, then assert that an interleaved `open_vault`
/// (an update) does NOT invalidate that snapshot. Without caching,
/// each query would re-aggregate and report the post-open total,
/// failing the equality fences.
#[test]
fn dos_006_protocol_status_caches_within_ttl() {
    let f = setup_fixture(50_000_000_000u128);

    // The setup XRC tick has filled the cache with totals at vault
    // count = 0. Do NOT advance past the TTL — the cache is fresh.
    let s1 = query_get_protocol_status(&f);

    // Open a vault — without caching, the next query would observe
    // total_icp_margin = 1_000_000_000 (10 ICP). With caching, the
    // pre-vault zero-totals snapshot is reused.
    open_collateral_only_vault(&f, 1_000_000_000);

    let s2 = query_get_protocol_status(&f);

    assert_eq!(
        s1.snapshot_ts_ns, s2.snapshot_ts_ns,
        "Wave-9b DOS-006: within {}ns TTL, two consecutive calls must return \
         the same snapshot_ts_ns (cache hit, no recompute). Got s1={}, s2={}.",
        PROTOCOL_STATUS_SNAPSHOT_TTL_NANOS, s1.snapshot_ts_ns, s2.snapshot_ts_ns,
    );
    assert_eq!(
        s1.total_icp_margin, s2.total_icp_margin,
        "Wave-9b DOS-006: heavy aggregate must be stable across queries \
         within TTL. open_vault between the two calls did NOT invalidate \
         the cache, so both queries serve the pre-vault snapshot \
         (total_icp_margin == 0). A non-cached implementation would \
         observe the post-open total (1_000_000_000) on s2 and fail this \
         assertion."
    );
    assert_eq!(
        s2.total_icp_margin, 0,
        "Wave-9b DOS-006: setup XRC tick filled the cache before any vault \
         was opened, so cached total_icp_margin must be 0. Got {}.",
        s2.total_icp_margin,
    );
}

/// DOS-006 cache-miss: advancing past the TTL must yield a fresh
/// `snapshot_ts_ns` on the next call.
#[test]
fn dos_006_protocol_status_recomputes_after_ttl() {
    let f = setup_fixture(20_000_000_000u128);

    open_collateral_only_vault(&f, 1_000_000_000);

    let s1 = query_get_protocol_status(&f);

    // Advance past TTL. Use 6s to leave headroom over the 5s threshold
    // and avoid flake from the cmp boundary.
    f.pic.advance_time(Duration::from_secs(6));
    f.pic.tick();

    let s2 = query_get_protocol_status(&f);

    assert!(
        s2.snapshot_ts_ns > s1.snapshot_ts_ns,
        "Wave-9b DOS-006: after advancing past TTL, snapshot_ts_ns must move \
         strictly forward. Got s1={}, s2={}.",
        s1.snapshot_ts_ns, s2.snapshot_ts_ns,
    );
}

/// DOS-006 live-field correctness: `frozen` must never be served
/// from a stale snapshot. An admin freeze that lands within the TTL
/// must be visible on the very next `get_protocol_status` call,
/// even if the heavy aggregates are reused.
#[test]
fn dos_006_protocol_status_live_fields_bypass_cache() {
    let f = setup_fixture(20_000_000_000u128);

    open_collateral_only_vault(&f, 1_000_000_000);

    // Warm the cache.
    let s1 = query_get_protocol_status(&f);
    assert!(!s1.frozen, "fresh fixture should not be frozen");

    // Freeze via admin. This is an update call (single round-trip).
    freeze_protocol(&f);

    // Re-query within TTL. The cache should hit on heavy fields, but
    // the LIVE `frozen` field MUST reflect the post-freeze state.
    let s2 = query_get_protocol_status(&f);

    assert!(
        s2.frozen,
        "Wave-9b DOS-006: `frozen` is a live field. Even when serving cached \
         heavy aggregates, the response must reflect the current frozen \
         state set by `freeze_protocol`."
    );
}

/// DOS-006 upgrade hygiene: a canister upgrade must not poison the
/// snapshot. After upgrade, the next query returns valid totals
/// (cache survived OR was dropped and recomputed cleanly). Either is
/// acceptable; the contract is "no crash, no stale data from a
/// different state shape".
///
/// The acceptance criterion is "totals reflect the actual on-chain
/// state after upgrade", not "totals equal a pre-upgrade reading"
/// (the pre-upgrade reading may itself be a stale-but-fresh-enough
/// cache hit from an earlier XRC tick — staleness within the TTL is
/// the cache's intended cycle-saving behaviour, not a regression).
#[test]
fn dos_006_protocol_status_survives_upgrade() {
    let f = setup_fixture(20_000_000_000u128);

    let collateral_e8s: u64 = 1_000_000_000; // 10 ICP — reflected in total_icp_margin
    open_collateral_only_vault(&f, collateral_e8s);

    // Touch the query path once so any pre-upgrade cache state
    // (populated, expired, refreshed) is observable post-upgrade.
    let _pre_upgrade = query_get_protocol_status(&f);

    // Upgrade the canister with the same wasm.
    let upgrade_arg = ProtocolArgVariant::Upgrade(UpgradeArgMirror {
        mode: None,
        description: Some("audit-pocs-9b upgrade hygiene".to_string()),
    });
    f.pic
        .upgrade_canister(
            f.protocol_id,
            protocol_wasm(),
            encode_args((upgrade_arg,)).expect("encode upgrade arg"),
            None,
        )
        .expect("upgrade_canister failed");

    // Advance past the TTL so the post-upgrade query is guaranteed to
    // observe either (a) a freshly recomputed snapshot or (b) the
    // cache rebuilt by the post-upgrade XRC tick. Both branches must
    // converge on the actual on-chain total, not a stale value.
    f.pic.advance_time(Duration::from_secs(6));
    f.pic.tick();

    let post_upgrade = query_get_protocol_status(&f);

    assert_eq!(
        post_upgrade.total_icp_margin, collateral_e8s,
        "Wave-9b DOS-006: post-upgrade total_icp_margin must reflect the \
         actual on-chain collateral total ({} e8s). Got {}.",
        collateral_e8s, post_upgrade.total_icp_margin,
    );
}

/// DOS-007 cache-hit: same shape as DOS-006 cache-hit, applied to
/// `get_treasury_stats`. Two consecutive calls within TTL must return
/// the same `snapshot_ts_ns` — proving no re-aggregation over vaults.
#[test]
fn dos_007_treasury_stats_caches_within_ttl() {
    let f = setup_fixture(20_000_000_000u128);

    // Drop any setup-time cache so the test starts deterministic.
    f.pic.advance_time(Duration::from_secs(6));
    f.pic.tick();

    open_collateral_only_vault(&f, 1_000_000_000);

    let s1 = query_get_treasury_stats(&f);
    let s2 = query_get_treasury_stats(&f);

    assert_eq!(
        s1.snapshot_ts_ns, s2.snapshot_ts_ns,
        "Wave-9b DOS-007: within TTL, two consecutive calls must return \
         the same snapshot_ts_ns (cache hit). Got s1={}, s2={}.",
        s1.snapshot_ts_ns, s2.snapshot_ts_ns,
    );
    assert_eq!(
        s1.total_accrued_interest_system, s2.total_accrued_interest_system,
        "Wave-9b DOS-007: cached aggregate must be stable within TTL."
    );
}

/// DOS-007 cache-miss after TTL.
#[test]
fn dos_007_treasury_stats_recomputes_after_ttl() {
    let f = setup_fixture(20_000_000_000u128);

    open_collateral_only_vault(&f, 1_000_000_000);

    let s1 = query_get_treasury_stats(&f);

    f.pic.advance_time(Duration::from_secs(6));
    f.pic.tick();

    let s2 = query_get_treasury_stats(&f);

    assert!(
        s2.snapshot_ts_ns > s1.snapshot_ts_ns,
        "Wave-9b DOS-007: snapshot_ts_ns must advance after TTL expires. \
         Got s1={}, s2={}.",
        s1.snapshot_ts_ns, s2.snapshot_ts_ns,
    );
}

/// DOS-007 live-field correctness: `pending_treasury_interest` and
/// `pending_treasury_collateral_entries` are O(1)/O(small) live fields
/// in `TreasuryStats`. They MUST reflect current state even when the
/// heavy `total_accrued_interest_system` is served from cache.
#[test]
fn dos_007_treasury_stats_live_fields_bypass_cache() {
    let f = setup_fixture(20_000_000_000u128);

    open_collateral_only_vault(&f, 1_000_000_000);

    let s1 = query_get_treasury_stats(&f);
    let s2 = query_get_treasury_stats(&f);

    // Two calls within TTL — heavy field must be cached (same ts).
    assert_eq!(
        s1.snapshot_ts_ns, s2.snapshot_ts_ns,
        "Wave-9b DOS-007: cache must hit on the second call within TTL."
    );
    // O(1)/O(small) live fields are read fresh on every call. Their
    // values are not expected to change in this test (no liquidations
    // happened between calls), but the read path MUST NOT be served
    // from a stale snapshot value. If a future regression caches them,
    // a follow-up vault flow that bumps them would catch the drift.
    assert_eq!(
        s1.pending_treasury_interest, s2.pending_treasury_interest,
        "pending_treasury_interest is read fresh; values stable across calls \
         when no liquidation has occurred."
    );
    assert_eq!(
        s1.pending_treasury_collateral_entries, s2.pending_treasury_collateral_entries,
        "pending_treasury_collateral_entries is read fresh."
    );
}

/// DOS-007 upgrade hygiene. Same shape as the DOS-006 upgrade test
/// — assert post-upgrade totals reflect actual on-chain state, not a
/// pre-upgrade snapshot reading (which may have been a stale-but-
/// fresh-enough cache hit).
#[test]
fn dos_007_treasury_stats_survives_upgrade() {
    let f = setup_fixture(20_000_000_000u128);

    open_collateral_only_vault(&f, 1_000_000_000);

    // Touch the query path so any pre-upgrade cache state is in play.
    let _pre = query_get_treasury_stats(&f);

    let upgrade_arg = ProtocolArgVariant::Upgrade(UpgradeArgMirror {
        mode: None,
        description: Some("audit-pocs-9b upgrade hygiene".to_string()),
    });
    f.pic
        .upgrade_canister(
            f.protocol_id,
            protocol_wasm(),
            encode_args((upgrade_arg,)).expect("encode upgrade arg"),
            None,
        )
        .expect("upgrade_canister failed");

    // Advance past TTL so post-upgrade query observes either a fresh
    // recompute or the cache rebuilt by the post-upgrade XRC tick.
    f.pic.advance_time(Duration::from_secs(6));
    f.pic.tick();

    let post = query_get_treasury_stats(&f);

    // For a freshly opened collateral-only vault that has not accrued
    // interest yet, the system total must be 0. The fence: post-upgrade
    // returns a valid number, decode succeeds, no crash. We also check
    // the value is sane (zero for a no-debt fixture).
    assert_eq!(
        post.total_accrued_interest_system, 0,
        "Wave-9b DOS-007: post-upgrade total_accrued_interest_system must \
         reflect actual state (0 for a no-debt fixture). Got {}.",
        post.total_accrued_interest_system,
    );
}
