//! PocketIC integration coverage for the Cycle Sentinel wiring.
//!
//! Build the two Wasm fixtures before running this suite.  The stateful tests
//! call the feature-gated operation fixture, so keep it in a separate target
//! directory and never accidentally exercise a production Wasm here:
//!
//! ```text
//! CARGO_TARGET_DIR=target/test-endpoints cargo build --release \
//!   --target wasm32-unknown-unknown -p rumi_cycle_sentinel \
//!   -p cycle_sentinel_test_mock --features test_endpoints
//! POCKET_IC_BIN=/private/tmp/pocket-ic cargo test -p rumi_cycle_sentinel \
//!   --test pocket_ic_integration
//! ```
//!
//! The mock fixture is installed at the protocol's pinned principals.  No
//! mainnet call or funding action is possible from this test: every endpoint
//! is local Wasm. All 16 bootstrap targets begin disabled/auto-top-up-off;
//! funding tests enable only their local fixtures through governance.

use candid::{CandidType, Decode, Encode, IDLArgs, IDLValue, Nat, Principal};
use pocket_ic::{PocketIc, PocketIcBuilder, WasmResult};
use serde::Deserialize;
use std::time::Duration;

const SENTINEL_WASM: &[u8] = include_bytes!(
    "../../../target/test-endpoints/wasm32-unknown-unknown/release/rumi_cycle_sentinel.wasm"
);
const MOCK_WASM: &[u8] =
    include_bytes!("../../../target/wasm32-unknown-unknown/release/cycle_sentinel_test_mock.wasm");

const CYCLES_LEDGER: &str = "um5iw-rqaaa-aaaaq-qaaba-cai";
const ICP_LEDGER: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";
const CMC: &str = "rkp4c-7iaaa-aaaaa-aaaca-cai";
const BLACKHOLE: &str = "e3mmv-5qaaa-aaaah-aadma-cai";
const CONFLUX_FRONTEND: &str = "a52ri-naaaa-aaaas-qgy4a-cai";

const SELF_REPORT_TARGETS: &[&str] = &[
    "bfnu3-6aaaa-aaaab-qhanq-cai",
    "tlg74-oiaaa-aaaap-qrd6a-cai",
    "tmhzi-dqaaa-aaaap-qrd6q-cai",
    "tfesu-vyaaa-aaaap-qrd7a-cai",
    "fohh4-yyaaa-aaaap-qtkpa-cai",
    "nygob-3qaaa-aaaap-qttcq-cai",
    "ijlzs-2yaaa-aaaap-quaaq-cai",
    "dtlu2-uqaaa-aaaap-qugcq-cai",
];

const ALL_TARGETS: &[&str] = &[
    "bfnu3-6aaaa-aaaab-qhanq-cai",
    "ucjxv-nqaaa-aaaaj-qrsaq-cai",
    "t6bor-paaaa-aaaap-qrd5q-cai",
    "tlg74-oiaaa-aaaap-qrd6a-cai",
    "tmhzi-dqaaa-aaaap-qrd6q-cai",
    "tfesu-vyaaa-aaaap-qrd7a-cai",
    "tcfua-yaaaa-aaaap-qrd7q-cai",
    "t2xrh-2aaaa-aaaap-qreaa-cai",
    "6niqu-siaaa-aaaap-qrjeq-cai",
    "pspic-iaaaa-aaaap-qrkna-cai",
    "fohh4-yyaaa-aaaap-qtkpa-cai",
    "jagpu-pyaaa-aaaap-qtm6q-cai",
    "nygob-3qaaa-aaaap-qttcq-cai",
    "ijlzs-2yaaa-aaaap-quaaq-cai",
    "dtlu2-uqaaa-aaaap-qugcq-cai",
    CONFLUX_FRONTEND,
];

#[derive(CandidType)]
struct InitArgs {
    signers: Vec<Principal>,
    approval_threshold: u32,
    global_policy: GlobalPolicyArgs,
}

#[derive(CandidType)]
struct GlobalPolicyArgs {
    global_daily_cap_cycles: Nat,
    sample_interval_secs: u64,
    stale_after_secs: u64,
    min_icp_reserve_e8s: Nat,
    timelocks: Timelocks,
    self_recovery_policy: SelfRecoveryPolicyArgs,
}

#[derive(CandidType)]
struct Timelocks {
    target_registry_secs: u64,
    spend_policy_secs: u64,
    signer_change_secs: u64,
    unpause_secs: u64,
}

#[derive(CandidType)]
struct SelfRecoveryPolicyArgs {
    protected_reserve_cycles: Nat,
    daily_cap_cycles: Nat,
    low_balance_threshold_cycles: Nat,
    refill_cycles: Nat,
}

#[derive(CandidType)]
struct MockInit {
    role: MockRole,
}

#[derive(CandidType)]
enum MockRole {
    SelfReport,
    Blackhole,
    CyclesLedger,
    IcpLedger,
    Cmc,
}

/// Mirrors the mock Cycles Ledger's own `WithdrawMode`.  `Unknown` makes the
/// fixture ledger reply `TemporarilyUnavailable`, which the adapter
/// classifies as an indeterminate withdrawal — the only way to hold an
/// injected operation in an unresolved, in-flight state while the maintenance
/// timer is free to run.
// These enums mirror complete wire-level fixture surfaces; individual tests
// intentionally exercise only the variants needed by each lifecycle case.
#[allow(dead_code)]
#[derive(CandidType)]
enum WithdrawMode {
    Confirmed,
    CommittedLostReply,
    Unknown,
    Duplicate,
    TooOld,
    TerminalNoSpend,
    InvalidReceiver,
    CreatedInFuture,
    FeeDebited,
    FullAmountDebited,
}

#[allow(dead_code)]
#[derive(CandidType)]
enum TransferMode {
    Confirmed,
    CommittedLostReply,
    Unknown,
    Duplicate,
    TooOld,
    BadFee,
}

#[allow(dead_code)]
#[derive(CandidType)]
enum NotifyMode {
    Completed,
    CommittedLostReply,
    Processing,
    Refunded,
    Invalid,
}

#[allow(dead_code)]
#[derive(CandidType)]
enum IcpLedgerValue {
    Blob(Vec<u8>),
    Text(String),
    Nat(Nat),
    Nat64(u64),
    Int(candid::Int),
    Array(Vec<IcpLedgerValue>),
    Map(Vec<(String, IcpLedgerValue)>),
}

#[derive(CandidType)]
enum TestCyclesState {
    PlannedReserved,
    Submitted,
    Confirmed,
    Unknown,
    Complete,
    Terminal,
    Quarantined,
}

#[derive(CandidType)]
enum TestIcpState {
    PlannedReserved,
    LedgerSubmitted,
    TransferUnknown,
    TransferConfirmed,
    NotifyPending,
    Complete,
    Refunded,
    Terminal,
    Quarantined,
}

#[derive(CandidType)]
enum TestOperationState {
    Cycles(TestCyclesState),
    Icp(TestIcpState),
}

#[derive(CandidType, Clone, Copy)]
enum TestBoundsMode {
    Targets,
    Samples,
    Alarms,
    Proposals,
    TerminalSummaries,
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
enum FundingCyclesState {
    PlannedReserved,
    Submitted,
    Confirmed,
    Unknown,
    Complete,
    Terminal,
    Quarantined,
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
enum FundingIcpState {
    PlannedReserved,
    LedgerSubmitted,
    TransferUnknown,
    TransferConfirmed,
    NotifyPending,
    Complete,
    Refunded,
    Terminal,
    Quarantined,
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
enum FundingState {
    Cycles(FundingCyclesState),
    Icp(FundingIcpState),
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
struct TestOperationView {
    id: u64,
    target: Principal,
    target_registry_revision: u64,
    reserved_amount_cycles: Nat,
    state: FundingState,
    attempt_count: u32,
    confirmed_block_index: Option<u64>,
    actual_cycles: Option<Nat>,
    refund_block_hint: Option<u64>,
    refund_block_index: Option<u64>,
}

#[derive(CandidType)]
struct TargetStatus {
    kind: TargetStatusKind,
    cycles: Nat,
    module_hash: Option<Vec<u8>>,
}

#[allow(dead_code)]
#[derive(CandidType)]
enum TargetStatusKind {
    Running,
    Stopping,
    Stopped,
    Uninstalled,
}

#[derive(CandidType)]
struct TargetPatch {
    display_name: Option<String>,
    project: Option<String>,
    environment: Option<Environment>,
    criticality: Option<Criticality>,
    observation_mode: Option<ObservationMode>,
    tags: Option<Vec<String>>,
    funding_policy: Option<FundingPolicyArgs>,
    enabled: Option<bool>,
    auto_topup: Option<bool>,
}

#[allow(dead_code)]
#[derive(CandidType)]
enum Environment {
    Production,
    Staging,
    Test,
    Local,
    Archived,
}

#[allow(dead_code)]
#[derive(CandidType)]
enum Criticality {
    Critical,
    Important,
    Standard,
    Experimental,
}

#[derive(CandidType)]
struct FundingPolicyArgs {
    low_balance_threshold_cycles: Nat,
    refill_cycles: Nat,
    daily_cap_cycles: Nat,
    cooldown_secs: u64,
    burn_anomaly_limit_cycles_per_day: Option<Nat>,
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
struct PublicOverview {
    target_count: u64,
    healthy_count: u64,
    low_count: u64,
    stopped_count: u64,
    uninstalled_count: u64,
    unreachable_count: u64,
    unobserved_count: u64,
    total_observed_cycles: Nat,
    runtime_cycles: Nat,
    cycles_ledger_available_cycles: Option<Nat>,
    icp_available_e8s: Option<Nat>,
    protected_self_reserve_cycles: Option<Nat>,
    alarm_count: u64,
    last_sample_at_secs: Option<u64>,
    next_sample_at_secs: Option<u64>,
    sample_interval_secs: Option<u64>,
    stale_after_secs: Option<u64>,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
enum ObservationMode {
    SelfReport,
    BlackholeRelay,
    Unobserved,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
enum PublicTargetState {
    Healthy,
    Low,
    Stopped,
    Uninstalled,
    Unreachable,
    Unobserved,
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
struct PublicTargetRow {
    principal: Principal,
    observation_mode: ObservationMode,
    state: PublicTargetState,
    // The remaining row fields are intentionally omitted; Candid record
    // subtyping lets this test inspect only identity and state.
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
struct TargetFlags {
    enabled: bool,
    auto_topup: bool,
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
struct TestBoundsReport {
    target_count: u64,
    sample_count: u64,
    alarm_count: u64,
    proposal_count: u64,
    terminal_summary_count: u64,
    target_overflow_rejected: bool,
    sample_overflow_pruned: bool,
    alarm_overflow_rejected: bool,
    alarm_oldest_pruned: bool,
    proposal_overflow_rejected: bool,
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
struct PublicPage<T> {
    items: Vec<T>,
    next_cursor: Option<String>,
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
enum PublicQueryError {
    InvalidCursor,
    FutureCursor,
    StaleCursor,
    TargetNotFound,
    TooManyItems,
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
enum AuthenticatedQueryError {
    NotSigner,
    InvalidCursor,
    FutureCursor,
    TooManyItems,
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
struct PermissionsView {
    is_signer: bool,
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
struct OperationStub {
    id: u64,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
enum FundingRailView {
    CyclesLedger,
    IcpCmc,
}

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
enum FundingOutcomeView {
    Completed,
    Refunded,
    Terminal,
}

#[derive(CandidType, Deserialize, Debug, PartialEq, Eq)]
struct PublicTopupSummary {
    rail: FundingRailView,
    outcome: FundingOutcomeView,
    amount_cycles: Nat,
    // `resolved_at_secs` is intentionally omitted: Candid record subtyping
    // lets this test assert the rail/outcome/amount of a compacted operation
    // without pinning the replica clock.
}

fn principal(text: &str) -> Principal {
    Principal::from_text(text).expect("valid fixture principal")
}

fn telemetry_viewer() -> Principal {
    principal("zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae")
}

fn create_and_install_mock(pic: &PocketIc, id: Principal, role: MockRole) {
    pic.create_canister_with_id(None, None, id)
        .expect("create fixed mock principal");
    pic.add_cycles(id, 10_000_000_000_000_000);
    pic.install_canister(
        id,
        MOCK_WASM.to_vec(),
        Encode!(&MockInit { role }).expect("encode mock init"),
        None,
    );
}

fn call_query<T: for<'de> Deserialize<'de> + CandidType>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: Vec<u8>,
) -> T {
    match pic
        .query_call(canister, caller, method, args)
        .expect("query call")
    {
        WasmResult::Reply(bytes) => Decode!(&bytes, T).expect("decode query reply"),
        WasmResult::Reject(message) => panic!("query {method} rejected: {message}"),
    }
}

fn call_update<T: for<'de> Deserialize<'de> + CandidType>(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: Vec<u8>,
) -> T {
    match pic
        .update_call(canister, caller, method, args)
        .expect("update call")
    {
        WasmResult::Reply(bytes) => Decode!(&bytes, T).expect("decode update reply"),
        WasmResult::Reject(message) => panic!("update {method} rejected: {message}"),
    }
}

fn call_update_unit(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: Vec<u8>,
) {
    match pic
        .update_call(canister, caller, method, args)
        .expect("update call")
    {
        WasmResult::Reply(bytes) => Decode!(&bytes, ()).expect("decode unit reply"),
        WasmResult::Reject(message) => panic!("update {method} rejected: {message}"),
    }
}

fn call_update_raw(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: Vec<u8>,
) -> Vec<u8> {
    match pic
        .update_call(canister, caller, method, args)
        .expect("update call")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("update {method} rejected: {message}"),
    }
}

fn call_query_raw(
    pic: &PocketIc,
    canister: Principal,
    caller: Principal,
    method: &str,
    args: Vec<u8>,
) -> Vec<u8> {
    match pic
        .query_call(canister, caller, method, args)
        .expect("query call")
    {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("query {method} rejected: {message}"),
    }
}

fn decode_ok_u64(bytes: &[u8]) -> u64 {
    let values = IDLArgs::from_bytes(bytes).expect("decode result");
    let IDLValue::Variant(value) = &values.args[0] else {
        panic!("expected Result variant")
    };
    if value.0.id.get_id() != candid::idl_hash("Ok") {
        if let IDLValue::Text(message) = &value.0.val {
            panic!("operation injection returned Err: {message}");
        }
        panic!("operation injection returned Err");
    }
    match &value.0.val {
        IDLValue::Number(number) => number.parse().expect("u64 result"),
        IDLValue::Nat(number) => number.0.clone().try_into().expect("u64 result"),
        IDLValue::Nat64(number) => *number,
        _ => panic!("expected Nat/u64 result"),
    }
}

fn assert_ok(bytes: &[u8]) {
    let values = IDLArgs::from_bytes(bytes).expect("decode result");
    let IDLValue::Variant(value) = &values.args[0] else {
        panic!("expected Result variant")
    };
    assert_eq!(value.0.id.get_id(), candid::idl_hash("Ok"));
}

fn test_inject(
    pic: &PocketIc,
    sentinel: Principal,
    signer: Principal,
    target: Principal,
    operation_state: TestOperationState,
) -> u64 {
    decode_ok_u64(&call_update_raw(
        pic,
        sentinel,
        signer,
        "test_inject_operation",
        Encode!(&target, &operation_state).expect("encode operation injection"),
    ))
}

fn test_operation(pic: &PocketIc, sentinel: Principal, id: u64) -> TestOperationView {
    call_query::<Option<TestOperationView>>(
        pic,
        sentinel,
        Principal::anonymous(),
        "test_get_operation",
        Encode!(&id).expect("encode operation id"),
    )
    .expect("injected operation remains present")
}

/// `test_get_operation` without the "must still exist" expectation.  A
/// resolved operation is compacted out of the live outbox by the same call
/// that resolves it, so its absence is the assertion, not a harness failure.
fn maybe_test_operation(pic: &PocketIc, sentinel: Principal, id: u64) -> Option<TestOperationView> {
    call_query::<Option<TestOperationView>>(
        pic,
        sentinel,
        Principal::anonymous(),
        "test_get_operation",
        Encode!(&id).expect("encode operation id"),
    )
}

/// The signer-only live outbox inventory, projected down to operation ids.
fn unresolved_operation_ids(pic: &PocketIc, sentinel: Principal, signer: Principal) -> Vec<u64> {
    let page: Result<PublicPage<OperationStub>, AuthenticatedQueryError> = call_query(
        pic,
        sentinel,
        signer,
        "list_unresolved_funding_operations",
        Encode!(&None::<String>, &100u16).unwrap(),
    );
    page.expect("signer reads the unresolved funding outbox")
        .items
        .iter()
        .map(|operation| operation.id)
        .collect()
}

/// The bounded terminal summaries a compacted operation leaves behind.
fn public_topups(
    pic: &PocketIc,
    sentinel: Principal,
    target: Principal,
) -> Vec<PublicTopupSummary> {
    let page: Result<PublicPage<PublicTopupSummary>, PublicQueryError> = call_query(
        pic,
        sentinel,
        telemetry_viewer(),
        "list_public_topups",
        Encode!(&target, &None::<String>, &100u16).unwrap(),
    );
    page.expect("public top-up history is anonymous").items
}

fn withdraw_call_count(pic: &PocketIc) -> u64 {
    call_query(
        pic,
        principal(CYCLES_LEDGER),
        Principal::anonymous(),
        "withdraw_call_count",
        Encode!().unwrap(),
    )
}

fn withdraw_delivery_count(pic: &PocketIc) -> u64 {
    call_query(
        pic,
        principal(CYCLES_LEDGER),
        Principal::anonymous(),
        "withdraw_delivery_count",
        Encode!().unwrap(),
    )
}

fn transfer_call_count(pic: &PocketIc) -> u64 {
    call_query(
        pic,
        principal(ICP_LEDGER),
        Principal::anonymous(),
        "transfer_call_count",
        Encode!().unwrap(),
    )
}

fn notify_call_count(pic: &PocketIc) -> u64 {
    call_query(
        pic,
        principal(CMC),
        Principal::anonymous(),
        "notify_call_count",
        Encode!().unwrap(),
    )
}

fn upgrade_sentinel(pic: &PocketIc, sentinel: Principal, wasm: &[u8], signer: Principal) {
    pic.upgrade_canister(
        sentinel,
        wasm.to_vec(),
        Encode!().expect("encode upgrade args"),
        None,
    )
    .expect("upgrade Sentinel canister");
    let _ = signer;
}

fn restart_sentinel(pic: &PocketIc, sentinel: Principal) {
    pic.stop_canister(sentinel, None)
        .expect("stop Sentinel for restart");
    pic.start_canister(sentinel, None)
        .expect("start Sentinel after restart");
}

/// Every persisted Cycles-rail outbox state, paired with the projection the
/// canister must report for it.
fn cycles_outbox_fixtures() -> [(TestCyclesState, FundingState); 7] {
    [
        (
            TestCyclesState::PlannedReserved,
            FundingState::Cycles(FundingCyclesState::PlannedReserved),
        ),
        (
            TestCyclesState::Submitted,
            FundingState::Cycles(FundingCyclesState::Submitted),
        ),
        (
            TestCyclesState::Confirmed,
            FundingState::Cycles(FundingCyclesState::Confirmed),
        ),
        (
            TestCyclesState::Unknown,
            FundingState::Cycles(FundingCyclesState::Unknown),
        ),
        (
            TestCyclesState::Complete,
            FundingState::Cycles(FundingCyclesState::Complete),
        ),
        (
            TestCyclesState::Terminal,
            FundingState::Cycles(FundingCyclesState::Terminal),
        ),
        (
            TestCyclesState::Quarantined,
            FundingState::Cycles(FundingCyclesState::Quarantined),
        ),
    ]
}

/// Every persisted ICP/CMC-rail outbox state, same contract as above.
fn icp_outbox_fixtures() -> [(TestIcpState, FundingState); 9] {
    [
        (
            TestIcpState::PlannedReserved,
            FundingState::Icp(FundingIcpState::PlannedReserved),
        ),
        (
            TestIcpState::LedgerSubmitted,
            FundingState::Icp(FundingIcpState::LedgerSubmitted),
        ),
        (
            TestIcpState::TransferUnknown,
            FundingState::Icp(FundingIcpState::TransferUnknown),
        ),
        (
            TestIcpState::TransferConfirmed,
            FundingState::Icp(FundingIcpState::TransferConfirmed),
        ),
        (
            TestIcpState::NotifyPending,
            FundingState::Icp(FundingIcpState::NotifyPending),
        ),
        (
            TestIcpState::Complete,
            FundingState::Icp(FundingIcpState::Complete),
        ),
        (
            TestIcpState::Refunded,
            FundingState::Icp(FundingIcpState::Refunded),
        ),
        (
            TestIcpState::Terminal,
            FundingState::Icp(FundingIcpState::Terminal),
        ),
        (
            TestIcpState::Quarantined,
            FundingState::Icp(FundingIcpState::Quarantined),
        ),
    ]
}

/// One operation per persisted Cycles state, each on its own registered
/// target, all resident in the SAME canister.  A single upgrade or a single
/// stop/start then covers the whole rail, instead of booting one replica per
/// state.
fn inject_every_cycles_state(
    pic: &PocketIc,
    sentinel: Principal,
    signer: Principal,
) -> Vec<(u64, Principal, FundingState)> {
    cycles_outbox_fixtures()
        .into_iter()
        .enumerate()
        .map(|(index, (fixture, expected))| {
            let target = principal(SELF_REPORT_TARGETS[index]);
            let id = test_inject(
                pic,
                sentinel,
                signer,
                target,
                TestOperationState::Cycles(fixture),
            );
            (id, target, expected)
        })
        .collect()
}

/// ICP counterpart of `inject_every_cycles_state`.  The caller must have run
/// the maintenance timer first: the ICP fixture reserves against the real
/// cached ICP source balance, which only the timer refreshes.
fn inject_every_icp_state(
    pic: &PocketIc,
    sentinel: Principal,
    signer: Principal,
) -> Vec<(u64, Principal, FundingState)> {
    icp_outbox_fixtures()
        .into_iter()
        .enumerate()
        .map(|(index, (fixture, expected))| {
            let target = principal(ALL_TARGETS[index + 7]);
            let id = test_inject(
                pic,
                sentinel,
                signer,
                target,
                TestOperationState::Icp(fixture),
            );
            (id, target, expected)
        })
        .collect()
}

/// Reads back every injected operation, proving each fixture actually reached
/// the persisted state it claims before the durable-state event under test.
fn snapshot_operations(
    pic: &PocketIc,
    sentinel: Principal,
    injected: &[(u64, Principal, FundingState)],
) -> Vec<TestOperationView> {
    injected
        .iter()
        .map(|(id, target, expected)| {
            let view = test_operation(pic, sentinel, *id);
            assert_eq!(
                &view.state, expected,
                "fixture {id} reached its requested persisted state"
            );
            assert_eq!(view.target, *target);
            view
        })
        .collect()
}

/// Whole-projection equality, not just state equality: id, target, registry
/// revision, immutable reserved amount, attempt history length, and every
/// block/cycle evidence field must come back byte-identical.
fn assert_operations_unchanged(
    pic: &PocketIc,
    sentinel: Principal,
    before: &[TestOperationView],
    event: &str,
) {
    for expected in before {
        let after = test_operation(pic, sentinel, expected.id);
        assert_eq!(
            &after, expected,
            "operation {} survived {event} unchanged",
            expected.id
        );
    }
}

fn set_mock_cycles_balance(pic: &PocketIc, target: Principal, balance: u128) {
    call_update_unit(
        pic,
        target,
        Principal::anonymous(),
        "set_cycles_balance",
        Encode!(&Nat::from(balance)).expect("encode mock balance"),
    );
}

/// Holds the fixture Cycles Ledger in an indeterminate reply mode.  Every
/// test that injects an in-flight operation and then lets the maintenance
/// timer run needs this: with the default `Confirmed` mode the timer's resume
/// step legitimately drives the operation to `Complete` and compacts it away,
/// so the in-flight invariant under test would never actually be exercised.
fn set_mock_withdraw_mode(pic: &PocketIc, ledger: Principal, mode: WithdrawMode) {
    call_update_unit(
        pic,
        ledger,
        Principal::anonymous(),
        "set_withdraw_mode",
        Encode!(&mode).expect("encode mock withdraw mode"),
    );
}

fn set_mock_transfer_mode(pic: &PocketIc, ledger: Principal, mode: TransferMode) {
    call_update_unit(
        pic,
        ledger,
        Principal::anonymous(),
        "set_transfer_mode",
        Encode!(&mode).expect("encode mock transfer mode"),
    );
}

fn set_mock_rate_current(pic: &PocketIc, cmc: Principal) {
    let timestamp = pic
        .get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("PocketIC time is after UNIX epoch")
        .as_secs();
    call_update_unit(
        pic,
        cmc,
        Principal::anonymous(),
        "set_rate",
        Encode!(&timestamp, &10_000u64).expect("encode current fixture rate"),
    );
}

fn set_mock_notify_mode(pic: &PocketIc, cmc: Principal, mode: NotifyMode) {
    call_update_unit(
        pic,
        cmc,
        Principal::anonymous(),
        "set_notify_mode",
        Encode!(&mode).expect("encode mock notify mode"),
    );
}

fn set_mock_refund_block(
    pic: &PocketIc,
    ledger: Principal,
    block_index: u64,
    block: IcpLedgerValue,
) {
    call_update_unit(
        pic,
        ledger,
        Principal::anonymous(),
        "set_refund_block",
        Encode!(&Some(block_index), &Some(block)).expect("encode mock refund block"),
    );
}

fn set_mock_self_report_reject(pic: &PocketIc, target: Principal, reject: bool) {
    call_update_unit(
        pic,
        target,
        Principal::anonymous(),
        "set_self_report_reject",
        Encode!(&reject).expect("encode mock rejection"),
    );
}

fn set_mock_target_status(
    pic: &PocketIc,
    blackhole: Principal,
    kind: TargetStatusKind,
    cycles: u128,
    module_hash: Option<Vec<u8>>,
) {
    call_update_unit(
        pic,
        blackhole,
        Principal::anonymous(),
        "set_target_status",
        Encode!(&TargetStatus {
            kind,
            cycles: Nat::from(cycles),
            module_hash,
        })
        .expect("encode mock target status"),
    );
}

fn target_patch_display_name(name: &str) -> TargetPatch {
    TargetPatch {
        display_name: Some(name.to_string()),
        project: None,
        environment: None,
        criticality: None,
        observation_mode: None,
        tags: None,
        funding_policy: None,
        enabled: None,
        auto_topup: None,
    }
}

fn target_patch_enable(name: &str) -> TargetPatch {
    TargetPatch {
        enabled: Some(true),
        auto_topup: Some(true),
        ..target_patch_display_name(name)
    }
}

fn boot() -> (PocketIc, Principal, Principal) {
    boot_with_funding(
        10_000_000_000_000,
        100_000_000_000_000,
        20_000_000_000_000_000,
    )
}

fn boot_with_funding(
    global_cap: u128,
    protected_reserve: u128,
    runtime_cycles: u128,
) -> (PocketIc, Principal, Principal) {
    boot_with_funding_and_interval(global_cap, protected_reserve, runtime_cycles, 1)
}

fn boot_with_funding_and_interval(
    global_cap: u128,
    protected_reserve: u128,
    runtime_cycles: u128,
    sample_interval_secs: u64,
) -> (PocketIc, Principal, Principal) {
    // The fixtures intentionally use the real mainnet principals. PocketIC's
    // routing table assigns the ledger/system principals to their matching
    // subnet kinds, so those subnets must exist even though every installed
    // Wasm in this test is local.
    let mut builder = PocketIcBuilder::new()
        .with_nns_subnet()
        .with_ii_subnet()
        .with_fiduciary_subnet()
        .with_application_subnet();
    // An explicitly managed matching server keeps startup and stderr evidence
    // available on busy hosts; default local developer autostart is unchanged.
    if let Ok(server_url) = std::env::var("SENTINEL_POCKET_IC_SERVER_URL") {
        builder = builder.with_server_url(server_url.parse().expect("PocketIC server URL"));
    }
    let pic = builder.build();
    let sentinel = pic.create_canister();
    let signer = Principal::from_slice(&[9; 10]);
    pic.add_cycles(sentinel, runtime_cycles);

    for text in SELF_REPORT_TARGETS {
        create_and_install_mock(&pic, principal(text), MockRole::SelfReport);
    }
    create_and_install_mock(&pic, principal(BLACKHOLE), MockRole::Blackhole);
    create_and_install_mock(&pic, principal(CYCLES_LEDGER), MockRole::CyclesLedger);
    create_and_install_mock(&pic, principal(ICP_LEDGER), MockRole::IcpLedger);
    create_and_install_mock(&pic, principal(CMC), MockRole::Cmc);

    let init = InitArgs {
        signers: vec![signer],
        approval_threshold: 1,
        global_policy: GlobalPolicyArgs {
            global_daily_cap_cycles: Nat::from(global_cap),
            sample_interval_secs,
            stale_after_secs: (2 * sample_interval_secs).max(10),
            min_icp_reserve_e8s: Nat::from(0u8),
            timelocks: Timelocks {
                target_registry_secs: 1,
                spend_policy_secs: 1,
                signer_change_secs: 1,
                unpause_secs: 1,
            },
            self_recovery_policy: SelfRecoveryPolicyArgs {
                protected_reserve_cycles: Nat::from(protected_reserve),
                daily_cap_cycles: Nat::from(10_000_000_000_000u128),
                low_balance_threshold_cycles: Nat::from(1_000_000_000_000u128),
                refill_cycles: Nat::from(1_000_000_000_000u128),
            },
        },
    };
    pic.install_canister(
        sentinel,
        SENTINEL_WASM.to_vec(),
        Encode!(&init).expect("encode Sentinel init"),
        None,
    );
    (pic, sentinel, signer)
}

fn run_timer(pic: &PocketIc, sentinel: Principal) {
    run_timer_after(pic, sentinel, Duration::from_secs(2));
}

fn run_timer_after(pic: &PocketIc, sentinel: Principal, elapsed: Duration) {
    const MAX_MAINTENANCE_PROGRESS_TICKS: usize = 256;
    let generation_before: u64 = call_query(
        pic,
        sentinel,
        Principal::anonymous(),
        "test_get_completed_tick_generation",
        Encode!().unwrap(),
    );
    pic.advance_time(elapsed);
    for _ in 0..MAX_MAINTENANCE_PROGRESS_TICKS {
        pic.tick();
        let generation_after: u64 = call_query(
            pic,
            sentinel,
            Principal::anonymous(),
            "test_get_completed_tick_generation",
            Encode!().unwrap(),
        );
        if generation_after > generation_before {
            return;
        }
    }
    panic!(
        "scheduled maintenance did not complete within {MAX_MAINTENANCE_PROGRESS_TICKS} ticks: generation stayed at {generation_before}"
    );
}

#[test]
fn bootstrap_exposes_all_sixteen_and_leaves_them_disabled() {
    let (pic, sentinel, _) = boot();
    let page: Result<PublicPage<PublicTargetRow>, PublicQueryError> = call_query(
        &pic,
        sentinel,
        telemetry_viewer(),
        "list_public_targets",
        Encode!(&None::<String>, &100u16).unwrap(),
    );
    let page = page.expect("allowlisted viewer target query succeeds");
    let ids: std::collections::BTreeSet<_> = page.items.iter().map(|row| row.principal).collect();
    assert_eq!(ids.len(), 16);
    assert_eq!(
        ids,
        ALL_TARGETS.iter().map(|text| principal(text)).collect()
    );
    assert!(page.next_cursor.is_none(), "16 entries fit in one page");
    assert!(page.items.iter().all(|row| {
        row.observation_mode == ObservationMode::Unobserved
            || row.observation_mode == ObservationMode::SelfReport
            || row.observation_mode == ObservationMode::BlackholeRelay
    }));
    for target in &page.items {
        let flags: Option<TargetFlags> = call_query(
            &pic,
            sentinel,
            Principal::anonymous(),
            "test_get_target_flags",
            Encode!(&target.principal).unwrap(),
        );
        let flags = flags.expect("bootstrap target flags are present");
        assert!(!flags.enabled, "{} starts disabled", target.principal);
        assert!(
            !flags.auto_topup,
            "{} starts auto-top-up off",
            target.principal
        );
    }
}

#[test]
fn timer_samples_cached_states_and_allowlisted_viewers_read_telemetry() {
    let (pic, sentinel, _) = boot();
    run_timer(&pic, sentinel);

    let overview: PublicOverview = call_query(
        &pic,
        sentinel,
        telemetry_viewer(),
        "get_public_overview",
        Encode!().unwrap(),
    );
    assert_eq!(overview.target_count, 16);
    assert_eq!(overview.unobserved_count, 16);
    assert_eq!(overview.healthy_count, 0);
    assert_eq!(overview.low_count, 0);
    assert_eq!(overview.unreachable_count, 0);
    // The mock ledgers expose a 1e15 balance.  Public availability is the
    // refreshed cache less durable source holds and the protected
    // self-recovery floor (1e14 cycles); it is not a raw ledger balance.
    assert_eq!(
        overview.cycles_ledger_available_cycles,
        Some(Nat::from(900_000_000_000_000u128))
    );
    assert_eq!(
        overview.icp_available_e8s,
        Some(Nat::from(1_000_000_000_000_000u128))
    );
    assert!(overview.last_sample_at_secs.is_none());
    assert!(overview.next_sample_at_secs.is_some());
    assert_eq!(overview.sample_interval_secs, Some(1));
    assert_eq!(overview.stale_after_secs, Some(10));

    let conflux: Option<PublicTargetRow> = call_query(
        &pic,
        sentinel,
        telemetry_viewer(),
        "get_public_target",
        Encode!(&principal(CONFLUX_FRONTEND)).unwrap(),
    );
    let conflux = conflux.expect("Conflux inventory target is public");
    assert_eq!(conflux.observation_mode, ObservationMode::Unobserved);
    assert_eq!(conflux.state, PublicTargetState::Unobserved);
}

#[test]
fn anonymous_authenticated_reads_and_mutations_are_rejected_in_canister_code() {
    let (pic, sentinel, _) = boot();
    let permissions: Result<PermissionsView, AuthenticatedQueryError> = call_query(
        &pic,
        sentinel,
        Principal::anonymous(),
        "get_my_permissions",
        Encode!().unwrap(),
    );
    assert_eq!(permissions, Err(AuthenticatedQueryError::NotSigner));

    let proposals: Result<PublicPage<OperationStub>, AuthenticatedQueryError> = call_query(
        &pic,
        sentinel,
        Principal::anonymous(),
        "list_governance_proposals",
        Encode!(&None::<String>, &10u16).unwrap(),
    );
    assert_eq!(proposals, Err(AuthenticatedQueryError::NotSigner));

    let manual: Result<OperationStub, String> = call_update(
        &pic,
        sentinel,
        Principal::anonymous(),
        "manual_top_up",
        Encode!(&principal(ALL_TARGETS[0])).unwrap(),
    );
    assert!(
        manual.is_err(),
        "anonymous manual mutation must fail closed"
    );
    assert!(
        manual.unwrap_err().contains("NotSigner"),
        "failure identifies authorization boundary"
    );
}

#[test]
fn signer_only_reconciliation_endpoints_enforce_authentication() {
    let (pic, sentinel, signer) = boot();
    let target = principal(SELF_REPORT_TARGETS[0]);
    let operation_id = test_inject(
        &pic,
        sentinel,
        signer,
        target,
        TestOperationState::Icp(TestIcpState::Quarantined),
    );

    let anonymous = IDLArgs::from_bytes(&call_query_raw(
        &pic,
        sentinel,
        Principal::anonymous(),
        "list_unresolved_funding_operations",
        Encode!(&None::<String>, &10u16).unwrap(),
    ))
    .expect("decode anonymous reconciliation response");
    let IDLValue::Variant(value) = &anonymous.args[0] else {
        panic!("expected reconciliation Result")
    };
    assert_eq!(value.0.id.get_id(), candid::idl_hash("Err"));
    let IDLValue::Variant(error) = &value.0.val else {
        panic!("expected authenticated query error variant")
    };
    assert_eq!(error.0.id.get_id(), candid::idl_hash("NotSigner"));

    let signer_response = IDLArgs::from_bytes(&call_query_raw(
        &pic,
        sentinel,
        signer,
        "list_unresolved_funding_operations",
        Encode!(&None::<String>, &10u16).unwrap(),
    ))
    .expect("decode signer reconciliation response");
    let IDLValue::Variant(value) = &signer_response.args[0] else {
        panic!("expected reconciliation Result")
    };
    assert_eq!(value.0.id.get_id(), candid::idl_hash("Ok"));

    let anonymous_refund = call_update_raw(
        &pic,
        sentinel,
        Principal::anonymous(),
        "attach_refund_block_proof",
        Encode!(&operation_id, &0u64).unwrap(),
    );
    let values = IDLArgs::from_bytes(&anonymous_refund).expect("decode anonymous refund response");
    let IDLValue::Variant(value) = &values.args[0] else {
        panic!("expected refund Result")
    };
    assert_eq!(value.0.id.get_id(), candid::idl_hash("Err"));
    assert!(matches!(&value.0.val, IDLValue::Text(message) if message.contains("NotSigner")));

    let signer_refund = call_update_raw(
        &pic,
        sentinel,
        signer,
        "attach_refund_block_proof",
        Encode!(&operation_id, &0u64).unwrap(),
    );
    let values = IDLArgs::from_bytes(&signer_refund).expect("decode signer refund response");
    let IDLValue::Variant(value) = &values.args[0] else {
        panic!("expected refund Result")
    };
    assert_eq!(value.0.id.get_id(), candid::idl_hash("Err"));
    assert!(matches!(&value.0.val, IDLValue::Text(message) if !message.contains("NotSigner")));
}

#[test]
fn every_cycles_outbox_state_survives_upgrade_with_immutable_snapshot() {
    // One replica for the whole rail: all seven persisted Cycles states are
    // resident at once, so a single upgrade re-decodes every one of them
    // together (and `post_upgrade`'s whole-state validation has to accept the
    // combined reservation picture, not one state in isolation).
    let (pic, sentinel, signer) = boot();
    let injected = inject_every_cycles_state(&pic, sentinel, signer);
    let before = snapshot_operations(&pic, sentinel, &injected);
    upgrade_sentinel(&pic, sentinel, SENTINEL_WASM, signer);
    assert_operations_unchanged(&pic, sentinel, &before, "upgrade");
}

#[test]
fn every_icp_outbox_state_survives_upgrade_with_immutable_snapshot() {
    let (pic, sentinel, signer) = boot();
    // Warms the ICP source cache the fixture reserves against.
    run_timer(&pic, sentinel);
    let injected = inject_every_icp_state(&pic, sentinel, signer);
    let before = snapshot_operations(&pic, sentinel, &injected);
    upgrade_sentinel(&pic, sentinel, SENTINEL_WASM, signer);
    assert_operations_unchanged(&pic, sentinel, &before, "upgrade");
}

#[test]
fn outbox_snapshots_survive_stop_start_restart() {
    // The accepted gate is EVERY persisted state across both rails, not two
    // representative ones.  Runtime stays bounded by reusing one replica per
    // rail: sixteen fixtures, two boots, two restarts.
    let (pic, sentinel, signer) = boot();
    let injected = inject_every_cycles_state(&pic, sentinel, signer);
    let before = snapshot_operations(&pic, sentinel, &injected);
    restart_sentinel(&pic, sentinel);
    assert_operations_unchanged(&pic, sentinel, &before, "Cycles-rail stop/start restart");

    let (pic, sentinel, signer) = boot();
    run_timer(&pic, sentinel);
    let injected = inject_every_icp_state(&pic, sentinel, signer);
    let before = snapshot_operations(&pic, sentinel, &injected);
    restart_sentinel(&pic, sentinel);
    assert_operations_unchanged(&pic, sentinel, &before, "ICP-rail stop/start restart");
}

#[test]
fn timer_resume_is_idempotent_and_never_reopens_a_completed_operation() {
    let (pic, sentinel, signer) = boot();
    let target = principal(SELF_REPORT_TARGETS[0]);
    let operation_id = test_inject(
        &pic,
        sentinel,
        signer,
        target,
        TestOperationState::Cycles(TestCyclesState::Unknown),
    );
    assert!(
        unresolved_operation_ids(&pic, sentinel, signer).contains(&operation_id),
        "the injected operation starts unresolved and in flight"
    );
    assert!(
        public_topups(&pic, sentinel, target).is_empty(),
        "an unresolved operation has produced no terminal summary yet"
    );
    let before_calls = withdraw_call_count(&pic);

    run_timer(&pic, sentinel);

    let after_first = withdraw_call_count(&pic);
    assert_eq!(
        after_first,
        before_calls + 1,
        "resume performs exactly one exact retry of the persisted withdrawal"
    );
    // Production compacts a resolved operation out of the live outbox inside
    // the very call that resolves it, so "it completed" is proven by its
    // DISAPPEARANCE plus the terminal summary it left behind — not by reading
    // a `Complete` operation back, which can never exist.
    assert!(
        !unresolved_operation_ids(&pic, sentinel, signer).contains(&operation_id),
        "the resolved operation no longer holds a slot in the unresolved outbox"
    );
    assert_eq!(
        maybe_test_operation(&pic, sentinel, operation_id),
        None,
        "the resolved operation was compacted out of durable storage"
    );
    let summaries = public_topups(&pic, sentinel, target);
    assert_eq!(
        summaries.len(),
        1,
        "exactly one terminal summary records the completed top-up"
    );
    assert_eq!(summaries[0].rail, FundingRailView::CyclesLedger);
    assert_eq!(summaries[0].outcome, FundingOutcomeView::Completed);

    run_timer(&pic, sentinel);

    assert_eq!(
        withdraw_call_count(&pic),
        after_first,
        "completed operation is not retried"
    );
    assert_eq!(
        public_topups(&pic, sentinel, target).len(),
        1,
        "no second terminal summary is recorded for the same operation"
    );
}

#[test]
fn observation_states_render_healthy_low_stopped_uninstalled_and_unreachable() {
    let (pic, sentinel, signer) = boot();
    set_mock_cycles_balance(&pic, principal(SELF_REPORT_TARGETS[0]), 0);
    set_mock_self_report_reject(&pic, principal(SELF_REPORT_TARGETS[1]), true);
    set_mock_target_status(
        &pic,
        principal(BLACKHOLE),
        TargetStatusKind::Stopped,
        1_000_000_000_000_000,
        Some(vec![7; 32]),
    );
    propose_update_and_execute(
        &pic,
        sentinel,
        signer,
        principal(SELF_REPORT_TARGETS[0]),
        TargetPatch {
            enabled: Some(true),
            ..target_patch_display_name("observation fixture")
        },
    );
    propose_update_and_execute(
        &pic,
        sentinel,
        signer,
        principal(SELF_REPORT_TARGETS[1]),
        TargetPatch {
            enabled: Some(true),
            ..target_patch_display_name("unreachable observation fixture")
        },
    );
    propose_update_and_execute(
        &pic,
        sentinel,
        signer,
        principal(ALL_TARGETS[1]),
        TargetPatch {
            enabled: Some(true),
            ..target_patch_display_name("stopped observation fixture")
        },
    );
    run_timer(&pic, sentinel);
    let low: PublicTargetRow = call_query::<Option<PublicTargetRow>>(
        &pic,
        sentinel,
        telemetry_viewer(),
        "get_public_target",
        Encode!(&principal(SELF_REPORT_TARGETS[0])).unwrap(),
    )
    .expect("low target row");
    assert_eq!(low.state, PublicTargetState::Low);
    let unreachable: PublicTargetRow = call_query::<Option<PublicTargetRow>>(
        &pic,
        sentinel,
        telemetry_viewer(),
        "get_public_target",
        Encode!(&principal(SELF_REPORT_TARGETS[1])).unwrap(),
    )
    .expect("unreachable target row");
    assert_eq!(unreachable.state, PublicTargetState::Unreachable);
    let stopped: PublicTargetRow = call_query::<Option<PublicTargetRow>>(
        &pic,
        sentinel,
        telemetry_viewer(),
        "get_public_target",
        Encode!(&principal(ALL_TARGETS[1])).unwrap(),
    )
    .expect("stopped target row");
    assert_eq!(stopped.state, PublicTargetState::Stopped);

    let (pic, sentinel, signer) = boot();
    set_mock_target_status(
        &pic,
        principal(BLACKHOLE),
        TargetStatusKind::Uninstalled,
        1_000,
        None,
    );
    propose_update_and_execute(
        &pic,
        sentinel,
        signer,
        principal(ALL_TARGETS[1]),
        TargetPatch {
            enabled: Some(true),
            ..target_patch_display_name("uninstalled observation fixture")
        },
    );
    run_timer(&pic, sentinel);
    let uninstalled: PublicTargetRow = call_query::<Option<PublicTargetRow>>(
        &pic,
        sentinel,
        telemetry_viewer(),
        "get_public_target",
        Encode!(&principal(ALL_TARGETS[1])).unwrap(),
    )
    .expect("uninstalled target row");
    assert_eq!(uninstalled.state, PublicTargetState::Uninstalled);
}

#[test]
fn private_history_queries_remain_bounded_for_allowlisted_viewers() {
    let (pic, sentinel, _) = boot();
    let too_many: Result<PublicPage<PublicTargetRow>, PublicQueryError> = call_query(
        &pic,
        sentinel,
        telemetry_viewer(),
        "list_public_targets",
        Encode!(&None::<String>, &101u16).unwrap(),
    );
    let page = too_many.expect("limit above the public bound is clamped safely");
    assert!(page.items.len() <= 100);
    let invalid: Result<PublicPage<PublicTargetRow>, PublicQueryError> = call_query(
        &pic,
        sentinel,
        telemetry_viewer(),
        "list_public_targets",
        Encode!(&Some("not-a-cursor".to_string()), &10u16).unwrap(),
    );
    assert_eq!(invalid, Err(PublicQueryError::InvalidCursor));
}

fn propose_update_and_execute(
    pic: &PocketIc,
    sentinel: Principal,
    signer: Principal,
    target: Principal,
    patch: TargetPatch,
) {
    let proposal = decode_ok_u64(&call_update_raw(
        pic,
        sentinel,
        signer,
        "propose_update_target",
        Encode!(&target, &patch).expect("encode update proposal"),
    ));
    assert_ok(&call_update_raw(
        pic,
        sentinel,
        signer,
        "approve_proposal",
        Encode!(&proposal).expect("encode approval"),
    ));
    pic.advance_time(Duration::from_secs(2));
    assert_ok(&call_update_raw(
        pic,
        sentinel,
        signer,
        "execute_proposal",
        Encode!(&proposal).expect("encode execution"),
    ));
}

#[test]
fn target_edit_preserves_injected_operation_snapshot_and_removal_is_blocked() {
    let (pic, sentinel, signer) = boot();
    // Governance execution advances the clock, which lets the maintenance
    // timer resume the injected operation.  Hold the fixture ledger in its
    // indeterminate reply mode so the operation genuinely stays unresolved
    // and in flight across the edit and the removal attempt; otherwise the
    // resume completes and compacts it, and this test would assert nothing
    // about an unresolved operation at all.
    set_mock_withdraw_mode(&pic, principal(CYCLES_LEDGER), WithdrawMode::Unknown);
    let target = principal(SELF_REPORT_TARGETS[0]);
    let operation_id = test_inject(
        &pic,
        sentinel,
        signer,
        target,
        TestOperationState::Cycles(TestCyclesState::Unknown),
    );
    let before = test_operation(&pic, sentinel, operation_id);
    propose_update_and_execute(
        &pic,
        sentinel,
        signer,
        target,
        target_patch_display_name("edited while funding is unresolved"),
    );
    let after_edit = test_operation(&pic, sentinel, operation_id);
    assert_eq!(after_edit.target, target);
    assert_eq!(
        after_edit.target_registry_revision,
        before.target_registry_revision
    );
    assert_eq!(
        after_edit.reserved_amount_cycles,
        before.reserved_amount_cycles
    );

    let proposal = decode_ok_u64(&call_update_raw(
        &pic,
        sentinel,
        signer,
        "propose_remove_target",
        Encode!(&target).unwrap(),
    ));
    assert_ok(&call_update_raw(
        &pic,
        sentinel,
        signer,
        "approve_proposal",
        Encode!(&proposal).unwrap(),
    ));
    pic.advance_time(Duration::from_secs(2));
    let result = call_update_raw(
        &pic,
        sentinel,
        signer,
        "execute_proposal",
        Encode!(&proposal).unwrap(),
    );
    let values = IDLArgs::from_bytes(&result).expect("decode removal result");
    let IDLValue::Variant(value) = &values.args[0] else {
        panic!("expected removal Result")
    };
    assert_eq!(value.0.id.get_id(), candid::idl_hash("Err"));
    assert!(
        unresolved_operation_ids(&pic, sentinel, signer).contains(&operation_id),
        "removal was refused because the operation is still unresolved"
    );
    assert!(test_operation(&pic, sentinel, operation_id).target == target);
    assert!(call_query::<Option<PublicTargetRow>>(
        &pic,
        sentinel,
        telemetry_viewer(),
        "get_public_target",
        Encode!(&target).unwrap(),
    )
    .is_some());
}

#[test]
fn timer_and_manual_paths_share_the_same_inflight_reservation() {
    let (pic, sentinel, signer) = boot();
    // Same reason as the removal test: the in-flight invariant is only under
    // test while the injected operation is actually still in flight, so the
    // fixture ledger must keep replying indeterminately when the timer
    // resumes it.
    set_mock_withdraw_mode(&pic, principal(CYCLES_LEDGER), WithdrawMode::Unknown);
    let target = principal(SELF_REPORT_TARGETS[0]);
    set_mock_cycles_balance(&pic, target, 0);
    run_timer(&pic, sentinel);
    propose_update_and_execute(
        &pic,
        sentinel,
        signer,
        target,
        TargetPatch {
            enabled: Some(true),
            ..target_patch_display_name("enabled for race fixture")
        },
    );
    let operation_id = test_inject(
        &pic,
        sentinel,
        signer,
        target,
        TestOperationState::Cycles(TestCyclesState::Unknown),
    );
    let result = call_update_raw(
        &pic,
        sentinel,
        signer,
        "manual_top_up",
        Encode!(&target).unwrap(),
    );
    let values = IDLArgs::from_bytes(&result).expect("decode manual result");
    let IDLValue::Variant(value) = &values.args[0] else {
        panic!("expected manual Result")
    };
    assert_eq!(value.0.id.get_id(), candid::idl_hash("Err"));
    assert_eq!(
        test_operation(&pic, sentinel, operation_id).state,
        FundingState::Cycles(FundingCyclesState::Unknown)
    );
}

#[test]
fn normal_cycles_adapter_settles_successfully_without_duplicate_spend() {
    let (pic, sentinel, signer) = boot();
    let target = principal(SELF_REPORT_TARGETS[0]);
    set_mock_cycles_balance(&pic, target, 0);
    propose_update_and_execute(
        &pic,
        sentinel,
        signer,
        target,
        target_patch_enable("cycles live"),
    );
    let before = withdraw_call_count(&pic);
    run_timer(&pic, sentinel);
    assert_eq!(withdraw_call_count(&pic), before + 1);
    assert_eq!(withdraw_delivery_count(&pic), 1);
    let summaries = public_topups(&pic, sentinel, target);
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].rail, FundingRailView::CyclesLedger);
    assert_eq!(summaries[0].outcome, FundingOutcomeView::Completed);
}

#[test]
fn committed_lost_reply_retries_exactly_once_then_quarantines_duplicate() {
    let (pic, sentinel, signer) = boot();
    let target = principal(SELF_REPORT_TARGETS[0]);
    set_mock_withdraw_mode(
        &pic,
        principal(CYCLES_LEDGER),
        WithdrawMode::CommittedLostReply,
    );
    let operation_id = test_inject(
        &pic,
        sentinel,
        signer,
        target,
        TestOperationState::Cycles(TestCyclesState::Unknown),
    );
    let before_calls = withdraw_call_count(&pic);
    let before_deliveries = withdraw_delivery_count(&pic);
    run_timer(&pic, sentinel);
    run_timer(&pic, sentinel);
    assert_eq!(withdraw_call_count(&pic), before_calls + 2);
    assert_eq!(withdraw_delivery_count(&pic), before_deliveries + 1);
    assert_eq!(
        test_operation(&pic, sentinel, operation_id).state,
        FundingState::Cycles(FundingCyclesState::Quarantined)
    );
    assert!(public_topups(&pic, sentinel, target).is_empty());
}

#[test]
fn too_old_cycles_reply_is_quarantined_without_releasing_holds() {
    let (pic, sentinel, signer) = boot();
    let target = principal(SELF_REPORT_TARGETS[0]);
    set_mock_withdraw_mode(&pic, principal(CYCLES_LEDGER), WithdrawMode::TooOld);
    let operation_id = test_inject(
        &pic,
        sentinel,
        signer,
        target,
        TestOperationState::Cycles(TestCyclesState::Unknown),
    );
    run_timer(&pic, sentinel);
    let operation = test_operation(&pic, sentinel, operation_id);
    assert_eq!(
        operation.state,
        FundingState::Cycles(FundingCyclesState::Quarantined)
    );
    assert!(unresolved_operation_ids(&pic, sentinel, signer).contains(&operation_id));
    assert!(public_topups(&pic, sentinel, target).is_empty());
}

#[test]
fn normal_icp_transfer_and_cmc_notify_settle_successfully() {
    let (pic, sentinel, signer) = boot();
    let target = principal(SELF_REPORT_TARGETS[0]);
    set_mock_transfer_mode(&pic, principal(ICP_LEDGER), TransferMode::Confirmed);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Completed);
    set_mock_rate_current(&pic, principal(CMC));
    run_timer(&pic, sentinel);
    let _operation_id = test_inject(
        &pic,
        sentinel,
        signer,
        target,
        TestOperationState::Icp(TestIcpState::LedgerSubmitted),
    );
    run_timer(&pic, sentinel);
    assert!(transfer_call_count(&pic) >= 1);
    assert!(notify_call_count(&pic) >= 1);
    let summaries = public_topups(&pic, sentinel, target);
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].rail, FundingRailView::IcpCmc);
    assert_eq!(summaries[0].outcome, FundingOutcomeView::Completed);
}

fn cmc_subaccount(target: Principal) -> Vec<u8> {
    let mut bytes = vec![0u8; 32];
    bytes[0] = target.as_slice().len() as u8;
    bytes[1..1 + target.as_slice().len()].copy_from_slice(target.as_slice());
    bytes
}

#[test]
fn refunded_icp_topup_requires_signer_proof_before_reconciliation() {
    let (pic, sentinel, signer) = boot();
    let target = principal(SELF_REPORT_TARGETS[0]);
    set_mock_transfer_mode(&pic, principal(ICP_LEDGER), TransferMode::Confirmed);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Refunded);
    set_mock_rate_current(&pic, principal(CMC));
    run_timer(&pic, sentinel);
    let _operation_id = test_inject(
        &pic,
        sentinel,
        signer,
        target,
        TestOperationState::Icp(TestIcpState::TransferConfirmed),
    );
    run_timer(&pic, sentinel);
    let operation_id = unresolved_operation_ids(&pic, sentinel, signer)
        .into_iter()
        .next()
        .expect("refunded CMC call leaves a quarantined operation");
    let operation = test_operation(&pic, sentinel, operation_id);
    let refund_block = operation
        .refund_block_hint
        .expect("CMC refund hint persisted");
    // For the boot fixture: refill 10e12 cycles, 10k cycles/XDR unit rate,
    // 1 e8s ledger fee. CMC refunds amount - three fees.
    let refund = IcpLedgerValue::Map(vec![
        (
            "from".to_string(),
            IcpLedgerValue::Array(vec![
                IcpLedgerValue::Blob(principal(CMC).as_slice().to_vec()),
                IcpLedgerValue::Blob(cmc_subaccount(target)),
            ]),
        ),
        (
            "to".to_string(),
            IcpLedgerValue::Array(vec![IcpLedgerValue::Blob(sentinel.as_slice().to_vec())]),
        ),
        (
            "amt".to_string(),
            IcpLedgerValue::Nat(Nat::from(1_099_999_997u64)),
        ),
        ("fee".to_string(), IcpLedgerValue::Nat(Nat::from(1u64))),
        ("memo".to_string(), IcpLedgerValue::Blob(Vec::new())),
    ]);
    set_mock_refund_block(&pic, principal(ICP_LEDGER), refund_block, refund);
    let attached = call_update_raw(
        &pic,
        sentinel,
        signer,
        "attach_refund_block_proof",
        Encode!(&operation_id, &refund_block).unwrap(),
    );
    assert_ok(&attached);
    let summaries = public_topups(&pic, sentinel, target);
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].rail, FundingRailView::IcpCmc);
    assert_eq!(summaries[0].outcome, FundingOutcomeView::Refunded);
}

#[test]
fn timer_manual_interleave_observes_stable_reservation_across_external_await() {
    let (pic, sentinel, signer) = boot();
    let target = principal(SELF_REPORT_TARGETS[0]);
    set_mock_cycles_balance(&pic, target, 0);
    set_mock_withdraw_mode(&pic, principal(CYCLES_LEDGER), WithdrawMode::Unknown);
    // Enable observation first, but keep automatic funding off while the
    // timer warms the sample and source cache.
    propose_update_and_execute(
        &pic,
        sentinel,
        signer,
        target,
        TargetPatch {
            enabled: Some(true),
            ..target_patch_display_name("race")
        },
    );
    run_timer(&pic, sentinel);
    propose_update_and_execute(
        &pic,
        sentinel,
        signer,
        target,
        TargetPatch {
            auto_topup: Some(true),
            ..target_patch_display_name("race-ready")
        },
    );
    let timer_call = pic
        .submit_call(
            sentinel,
            signer,
            "test_start_timer_funding",
            Encode!(&target).unwrap(),
        )
        .expect("submit timer-triggered funding ingress");
    // One replica tick executes the reservation and sends the ledger request,
    // but leaves the update awaiting the mock's response.
    pic.tick();
    assert!(
        !unresolved_operation_ids(&pic, sentinel, signer).is_empty(),
        "reservation is durable before the external reply arrives"
    );
    let manual_call = pic
        .submit_call(sentinel, signer, "manual_top_up", Encode!(&target).unwrap())
        .expect("submit concurrent manual ingress");
    let manual = match pic.await_call(manual_call).expect("manual ingress status") {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("manual call rejected at transport: {message}"),
    };
    let values = IDLArgs::from_bytes(&manual).expect("decode manual result");
    let IDLValue::Variant(value) = &values.args[0] else {
        panic!("expected manual Result")
    };
    assert_eq!(value.0.id.get_id(), candid::idl_hash("Err"));
    assert!(
        matches!(&value.0.val, IDLValue::Text(message) if message.contains("OperationInFlight"))
    );
    let _ = pic.await_call(timer_call).expect("timer ingress status");
}

#[test]
fn live_stable_bounds_fill_prune_reject_and_survive_upgrade() {
    // Keep each maximum-sized store in its own canister instance.  The old
    // all-in-one fixture filled all five stores together, and the first
    // post-fill query crossed PocketIC's 5B instruction cap before it could
    // even reach the upgrade persistence assertion.
    let fixtures = [
        TestBoundsMode::Targets,
        TestBoundsMode::Samples,
        TestBoundsMode::Alarms,
        TestBoundsMode::Proposals,
        TestBoundsMode::TerminalSummaries,
    ];
    for mode in fixtures {
        let (pic, sentinel, signer) = boot();
        let report: Result<TestBoundsReport, String> = call_update(
            &pic,
            sentinel,
            signer,
            "test_fill_bounds",
            Encode!(&mode).unwrap(),
        );
        let report = report.expect("bounded fixture succeeds");
        match mode {
            TestBoundsMode::Targets => {
                assert_eq!(report.target_count, 128);
                assert_eq!(report.sample_count, 0);
                assert_eq!(report.alarm_count, 0);
                assert_eq!(report.proposal_count, 0);
                assert_eq!(report.terminal_summary_count, 0);
                assert!(report.target_overflow_rejected);
                assert!(!report.sample_overflow_pruned);
                assert!(!report.alarm_overflow_rejected);
                assert!(!report.alarm_oldest_pruned);
                assert!(!report.proposal_overflow_rejected);
            }
            TestBoundsMode::Samples => {
                assert_eq!(report.target_count, 16);
                assert_eq!(report.sample_count, 2_160);
                assert_eq!(report.alarm_count, 0);
                assert_eq!(report.proposal_count, 0);
                assert_eq!(report.terminal_summary_count, 0);
                assert!(!report.target_overflow_rejected);
                assert!(report.sample_overflow_pruned);
                assert!(!report.alarm_overflow_rejected);
                assert!(!report.alarm_oldest_pruned);
                assert!(!report.proposal_overflow_rejected);
            }
            TestBoundsMode::Alarms => {
                assert_eq!(report.target_count, 16);
                assert_eq!(report.sample_count, 0);
                assert_eq!(report.alarm_count, 1_024);
                assert_eq!(report.proposal_count, 0);
                assert_eq!(report.terminal_summary_count, 0);
                assert!(!report.target_overflow_rejected);
                assert!(!report.sample_overflow_pruned);
                assert!(report.alarm_overflow_rejected);
                assert!(report.alarm_oldest_pruned);
                assert!(!report.proposal_overflow_rejected);
            }
            TestBoundsMode::Proposals => {
                assert_eq!(report.target_count, 16);
                assert_eq!(report.sample_count, 0);
                assert_eq!(report.alarm_count, 0);
                assert_eq!(report.proposal_count, 256);
                assert_eq!(report.terminal_summary_count, 0);
                assert!(!report.target_overflow_rejected);
                assert!(!report.sample_overflow_pruned);
                assert!(!report.alarm_overflow_rejected);
                assert!(!report.alarm_oldest_pruned);
                assert!(report.proposal_overflow_rejected);
            }
            TestBoundsMode::TerminalSummaries => {
                assert_eq!(report.target_count, 16);
                assert_eq!(report.sample_count, 0);
                assert_eq!(report.alarm_count, 0);
                assert_eq!(report.proposal_count, 0);
                assert_eq!(report.terminal_summary_count, 512);
                assert!(!report.target_overflow_rejected);
                assert!(!report.sample_overflow_pruned);
                assert!(!report.alarm_overflow_rejected);
                assert!(!report.alarm_oldest_pruned);
                assert!(!report.proposal_overflow_rejected);
            }
        }

        let before: TestBoundsReport = call_query(
            &pic,
            sentinel,
            Principal::anonymous(),
            "test_get_bound_counts",
            Encode!(&principal(SELF_REPORT_TARGETS[0])).unwrap(),
        );
        assert_eq!(before.target_count, report.target_count);
        assert_eq!(before.sample_count, report.sample_count);
        assert_eq!(before.alarm_count, report.alarm_count);
        assert_eq!(before.proposal_count, report.proposal_count);
        assert_eq!(before.terminal_summary_count, report.terminal_summary_count);
        upgrade_sentinel(&pic, sentinel, SENTINEL_WASM, signer);
        let after: TestBoundsReport = call_query(
            &pic,
            sentinel,
            Principal::anonymous(),
            "test_get_bound_counts",
            Encode!(&principal(SELF_REPORT_TARGETS[0])).unwrap(),
        );
        assert_eq!(after.target_count, before.target_count);
        assert_eq!(after.sample_count, before.sample_count);
        assert_eq!(after.alarm_count, before.alarm_count);
        assert_eq!(after.proposal_count, before.proposal_count);
        assert_eq!(after.terminal_summary_count, before.terminal_summary_count);

        if matches!(mode, TestBoundsMode::Targets) {
            let page: Result<PublicPage<PublicTargetRow>, PublicQueryError> = call_query(
                &pic,
                sentinel,
                telemetry_viewer(),
                "list_public_targets",
                Encode!(&None::<String>, &101u16).unwrap(),
            );
            assert_eq!(page.unwrap().items.len(), 100);
        }
    }
}

#[test]
fn checked_in_sentinel_init_pins_reviewed_signers_and_threshold() {
    let manifest = include_str!("../../../icp.yaml");
    let sentinel = manifest
        .split_once("  - name: rumi_cycle_sentinel\n")
        .expect("Sentinel manifest entry")
        .1
        .split("\n  - name:")
        .next()
        .unwrap();
    let signer_block = sentinel
        .split_once("signers = vec {")
        .expect("Sentinel signer vector")
        .1
        .split_once("};")
        .expect("Sentinel signer vector terminator")
        .0;
    let signers: Vec<Principal> = signer_block
        .lines()
        .filter_map(|line| line.trim().strip_prefix("principal \""))
        .map(|line| principal(line.split_once('"').expect("signer principal").0))
        .collect();
    let expected: Vec<Principal> = [
        "ft3ml-xex6k-ppiwj-ie6tc-zwkgb-ybm2x-eat4a-5p2jg-auzl3-latf4-aae",
        "zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae",
        "stzp3-bnvwm-zqzjh-o6mv6-ci53m-wj5k6-xyhe7-fnyp2-c64o3-7vokj-bqe",
        "4alqm-afk6k-bybok-qvdyo-cnv7y-klel6-xm2pz-7h7jk-utmys-kttf3-vqe",
    ]
    .into_iter()
    .map(principal)
    .collect();
    assert_eq!(signers, expected);
    assert!(sentinel.contains("approval_threshold = 2 : nat32;"));
}

// Shared-reserve funding tests use the actual maintenance timer and opt-in
// ledger accounting. Mint receipts never count as target delivery; only the
// Cycles Ledger's exact withdrawal produces a public target top-up summary.
const T: u128 = 1_000_000_000_000;
const CYCLES_FEE: u128 = 100_000_000;
const ICP_FEE: u128 = 10_000;
const DEPOSIT_ICP_E8S: u128 = 10_000_000_000;

fn mock_nat(pic: &PocketIc, id: &str, method: &str) -> u128 {
    let value: Nat = call_query(
        pic,
        principal(id),
        Principal::anonymous(),
        method,
        Encode!().unwrap(),
    );
    value.0.try_into().unwrap()
}

fn mock_count(pic: &PocketIc, id: &str, method: &str) -> u64 {
    call_query(
        pic,
        principal(id),
        Principal::anonymous(),
        method,
        Encode!().unwrap(),
    )
}

fn set_mock_ledger_balance(pic: &PocketIc, ledger: Principal, balance: u128) {
    call_update_unit(
        pic,
        ledger,
        Principal::anonymous(),
        "set_ledger_balance",
        Encode!(&Nat::from(balance)).unwrap(),
    );
}

fn set_mock_fee(pic: &PocketIc, ledger: Principal, fee: u128) {
    call_update_unit(
        pic,
        ledger,
        Principal::anonymous(),
        "set_fee",
        Encode!(&Nat::from(fee)).unwrap(),
    );
}

fn shared_setup(pic: &PocketIc, cycles_balance: u128) {
    for id in [CYCLES_LEDGER, ICP_LEDGER] {
        call_update_unit(
            pic,
            principal(id),
            Principal::anonymous(),
            "enable_accounting",
            Encode!().unwrap(),
        );
    }
    set_mock_ledger_balance(pic, principal(CYCLES_LEDGER), cycles_balance);
    set_mock_fee(pic, principal(CYCLES_LEDGER), CYCLES_FEE);
    set_mock_ledger_balance(pic, principal(ICP_LEDGER), DEPOSIT_ICP_E8S);
    set_mock_fee(pic, principal(ICP_LEDGER), ICP_FEE);
    set_mock_rate_current(pic, principal(CMC));
}

fn enable_shared_target(pic: &PocketIc, sentinel: Principal, signer: Principal) -> Principal {
    let target = principal(SELF_REPORT_TARGETS[0]);
    set_mock_cycles_balance(pic, target, T);
    let mut patch = target_patch_enable("shared reserve test");
    patch.funding_policy = Some(FundingPolicyArgs {
        low_balance_threshold_cycles: Nat::from(3 * T),
        refill_cycles: Nat::from(2 * T),
        daily_cap_cycles: Nat::from(6 * T),
        cooldown_secs: 3_600,
        burn_anomaly_limit_cycles_per_day: None,
    });
    // Governance advances replica time. Deposit ICP after enablement so an
    // interleaved timer cannot consume the first modeled reply before the
    // explicit maintenance tick under test.
    set_mock_ledger_balance(pic, principal(ICP_LEDGER), 0);
    propose_update_and_execute(pic, sentinel, signer, target, patch);
    set_mock_ledger_balance(pic, principal(ICP_LEDGER), DEPOSIT_ICP_E8S);
    target
}

fn shared_overview(pic: &PocketIc, sentinel: Principal) -> PublicOverview {
    call_query(
        pic,
        sentinel,
        telemetry_viewer(),
        "get_public_overview",
        Encode!().unwrap(),
    )
}

fn assert_exact_withdrawals(pic: &PocketIc, amounts: &[u128]) {
    let actual: Vec<Nat> = call_query(
        pic,
        principal(CYCLES_LEDGER),
        Principal::anonymous(),
        "withdrawal_amounts",
        Encode!().unwrap(),
    );
    assert_eq!(
        actual,
        amounts.iter().copied().map(Nat::from).collect::<Vec<_>>()
    );
}

#[test]
fn shared_reserve_empty_cycles_mints_deficit_and_delivers_exact_refill() {
    let (pic, sentinel, signer) = boot_with_funding(40 * T, 10 * T, 20_000 * T);
    shared_setup(&pic, 0);
    let target = enable_shared_target(&pic, sentinel, signer);
    run_timer(&pic, sentinel);
    assert_eq!(
        mock_nat(&pic, CMC, "minted_cycles"),
        12 * T + 2 * CYCLES_FEE
    );
    assert_eq!(mock_nat(&pic, CYCLES_LEDGER, "ledger_balance"), 10 * T);
    assert_eq!(
        mock_nat(&pic, ICP_LEDGER, "ledger_balance"),
        DEPOSIT_ICP_E8S - (12 * T + 2 * CYCLES_FEE) / 10_000 - ICP_FEE
    );
    assert_exact_withdrawals(&pic, &[2 * T]);
    assert_eq!(
        public_topups(&pic, sentinel, target),
        vec![PublicTopupSummary {
            rail: FundingRailView::CyclesLedger,
            outcome: FundingOutcomeView::Completed,
            amount_cycles: Nat::from(2 * T)
        }]
    );
    assert_eq!(
        shared_overview(&pic, sentinel).cycles_ledger_available_cycles,
        Some(Nat::from(0u8))
    );
    assert!(unresolved_operation_ids(&pic, sentinel, signer).is_empty());
}

#[test]
fn shared_reserve_uses_cycles_first_without_icp_payment_or_mint() {
    let (pic, sentinel, signer) = boot_with_funding(40 * T, 10 * T, 20_000 * T);
    shared_setup(&pic, 12 * T + CYCLES_FEE);
    let target = enable_shared_target(&pic, sentinel, signer);
    run_timer(&pic, sentinel);
    assert_eq!(transfer_call_count(&pic), 0);
    assert_eq!(notify_call_count(&pic), 0);
    assert_eq!(
        mock_nat(&pic, ICP_LEDGER, "ledger_balance"),
        DEPOSIT_ICP_E8S
    );
    assert_eq!(mock_nat(&pic, CYCLES_LEDGER, "ledger_balance"), 10 * T);
    assert_exact_withdrawals(&pic, &[2 * T]);
    assert_eq!(public_topups(&pic, sentinel, target).len(), 1);
}

#[test]
fn shared_reserve_processing_upgrade_keeps_hold_and_conversion_cap() {
    let (pic, sentinel, signer) = boot_with_funding(13 * T, 10 * T, 20_000 * T);
    shared_setup(&pic, 0);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Processing);
    let target = enable_shared_target(&pic, sentinel, signer);
    run_timer(&pic, sentinel);
    let ids = unresolved_operation_ids(&pic, sentinel, signer);
    assert_eq!(ids.len(), 1);
    let before = test_operation(&pic, sentinel, ids[0]);
    assert_eq!(
        before.reserved_amount_cycles,
        Nat::from(12 * T + 2 * CYCLES_FEE)
    );
    assert_eq!(
        before.state,
        FundingState::Icp(FundingIcpState::NotifyPending)
    );
    let available_before = shared_overview(&pic, sentinel).icp_available_e8s;
    assert!(public_topups(&pic, sentinel, target).is_empty());
    assert_exact_withdrawals(&pic, &[]);
    upgrade_sentinel(&pic, sentinel, SENTINEL_WASM, signer);
    assert_eq!(
        test_operation(&pic, sentinel, ids[0]).reserved_amount_cycles,
        before.reserved_amount_cycles
    );
    assert_eq!(
        shared_overview(&pic, sentinel).icp_available_e8s,
        available_before
    );
    run_timer(&pic, sentinel);
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(mock_count(&pic, CMC, "mint_delivery_count"), 0);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Completed);
    run_timer(&pic, sentinel);
    pic.advance_time(Duration::from_secs(3_601));
    set_mock_rate_current(&pic, principal(CMC));
    run_timer(&pic, sentinel);
    // The target remains low in the reporting fixture, but the first mint
    // exhausted all but 0.9998T of the 13T conversion cap across upgrade.
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(mock_count(&pic, CMC, "mint_delivery_count"), 1);
    assert_exact_withdrawals(&pic, &[2 * T]);
    assert_eq!(public_topups(&pic, sentinel, target).len(), 1);
    assert!(unresolved_operation_ids(&pic, sentinel, signer).is_empty());
}

#[test]
fn shared_reserve_committed_icp_reply_loss_upgrade_retries_identical_payment() {
    let (pic, sentinel, signer) = boot_with_funding(40 * T, 10 * T, 20_000 * T);
    shared_setup(&pic, 0);
    set_mock_transfer_mode(
        &pic,
        principal(ICP_LEDGER),
        TransferMode::CommittedLostReply,
    );
    let target = enable_shared_target(&pic, sentinel, signer);
    run_timer(&pic, sentinel);
    let ids = unresolved_operation_ids(&pic, sentinel, signer);
    assert_eq!(ids.len(), 1);
    assert_eq!(
        test_operation(&pic, sentinel, ids[0]).state,
        FundingState::Icp(FundingIcpState::TransferUnknown)
    );
    let paid_balance = mock_nat(&pic, ICP_LEDGER, "ledger_balance");
    assert!(public_topups(&pic, sentinel, target).is_empty());
    upgrade_sentinel(&pic, sentinel, SENTINEL_WASM, signer);
    run_timer(&pic, sentinel);
    assert_eq!(mock_nat(&pic, ICP_LEDGER, "ledger_balance"), paid_balance);
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(mock_count(&pic, CMC, "mint_delivery_count"), 1);
    assert_exact_withdrawals(&pic, &[2 * T]);
    assert_eq!(public_topups(&pic, sentinel, target).len(), 1);
    assert!(unresolved_operation_ids(&pic, sentinel, signer).is_empty());
}

#[test]
fn shared_reserve_conversion_cap_blocks_initial_deficit_without_debit() {
    let (pic, sentinel, signer) = boot_with_funding(11 * T, 10 * T, 20_000 * T);
    shared_setup(&pic, 0);
    let target = enable_shared_target(&pic, sentinel, signer);
    run_timer(&pic, sentinel);
    assert_eq!(transfer_call_count(&pic), 0);
    assert_eq!(
        mock_nat(&pic, ICP_LEDGER, "ledger_balance"),
        DEPOSIT_ICP_E8S
    );
    assert_exact_withdrawals(&pic, &[]);
    assert!(public_topups(&pic, sentinel, target).is_empty());
    assert!(unresolved_operation_ids(&pic, sentinel, signer).is_empty());
}

#[test]
fn shared_reserve_icp_only_runtime_recovery_waits_for_exact_runtime_delivery() {
    let (pic, sentinel, signer) = boot_with_funding(40 * T, 10 * T, 900_000_000_000);
    shared_setup(&pic, 0);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Processing);
    let target = enable_shared_target(&pic, sentinel, signer);
    // First tick refreshes empty caches; the next opens recovery.
    run_timer(&pic, sentinel);
    run_timer(&pic, sentinel);
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(mock_count(&pic, CMC, "mint_delivery_count"), 0);
    assert_exact_withdrawals(&pic, &[]);
    assert!(public_topups(&pic, sentinel, target).is_empty());
    upgrade_sentinel(&pic, sentinel, SENTINEL_WASM, signer);
    // A mint receipt followed by ambiguous runtime withdrawal still cannot
    // authorize target funding. Only a confirmed exact runtime withdrawal can.
    set_mock_withdraw_mode(&pic, principal(CYCLES_LEDGER), WithdrawMode::Unknown);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Completed);
    run_timer(&pic, sentinel);
    assert_eq!(
        mock_nat(&pic, CMC, "minted_cycles"),
        11 * T + 2 * CYCLES_FEE
    );
    assert_exact_withdrawals(&pic, &[]);
    assert!(public_topups(&pic, sentinel, target).is_empty());
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    set_mock_withdraw_mode(&pic, principal(CYCLES_LEDGER), WithdrawMode::Confirmed);
    run_timer(&pic, sentinel);
    let withdrawals: Vec<Nat> = call_query(
        &pic,
        principal(CYCLES_LEDGER),
        Principal::anonymous(),
        "withdrawal_amounts",
        Encode!().unwrap(),
    );
    assert_eq!(withdrawals.first(), Some(&Nat::from(T)));
    assert!(
        pic.cycle_balance(sentinel) > T,
        "runtime received the configured 1T before ordinary distribution"
    );
}

#[test]
fn shared_reserve_pending_runtime_mint_skips_withdrawal_after_external_recovery() {
    let (pic, sentinel, signer) = boot_with_funding(40 * T, 10 * T, 900_000_000_000);
    shared_setup(&pic, 0);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Processing);
    run_timer(&pic, sentinel);
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(mock_count(&pic, CMC, "mint_delivery_count"), 0);
    assert!(!unresolved_operation_ids(&pic, sentinel, signer).is_empty());
    assert_exact_withdrawals(&pic, &[]);

    // A separate operator has already recovered runtime while the CMC receipt
    // was pending. Settlement must recheck runtime before withdrawing again.
    pic.add_cycles(sentinel, 20 * T);
    let restored_runtime = pic.cycle_balance(sentinel);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Completed);
    run_timer(&pic, sentinel);
    assert_eq!(
        mock_nat(&pic, CMC, "minted_cycles"),
        11 * T + 2 * CYCLES_FEE
    );
    assert_exact_withdrawals(&pic, &[]);
    assert!(unresolved_operation_ids(&pic, sentinel, signer).is_empty());
    assert!(pic.cycle_balance(sentinel) <= restored_runtime);
    assert!(pic.cycle_balance(sentinel) > restored_runtime - T);
    let target = principal(SELF_REPORT_TARGETS[0]);
    assert!(public_topups(&pic, sentinel, target).is_empty());

    // The settled mint leaves no recovery latch that blocks later ordinary
    // distribution from an independently sufficient deposited cycles balance.
    set_mock_ledger_balance(&pic, principal(CYCLES_LEDGER), 12 * T + CYCLES_FEE);
    enable_shared_target(&pic, sentinel, signer);
    run_timer(&pic, sentinel);
    assert_exact_withdrawals(&pic, &[2 * T]);
    let history = public_topups(&pic, sentinel, target);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].amount_cycles, Nat::from(2 * T));
    assert!(unresolved_operation_ids(&pic, sentinel, signer).is_empty());
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
}

#[test]
fn shared_reserve_lost_mint_receipt_upgrade_reuses_cmc_block_without_second_mint() {
    let (pic, sentinel, signer) = boot_with_funding(40 * T, 10 * T, 20_000 * T);
    shared_setup(&pic, 0);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::CommittedLostReply);
    let target = enable_shared_target(&pic, sentinel, signer);
    run_timer(&pic, sentinel);
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(mock_count(&pic, CMC, "mint_delivery_count"), 1);
    assert_eq!(
        mock_nat(&pic, CMC, "minted_cycles"),
        12 * T + 2 * CYCLES_FEE
    );
    assert!(public_topups(&pic, sentinel, target).is_empty());
    assert_exact_withdrawals(&pic, &[]);
    let ids = unresolved_operation_ids(&pic, sentinel, signer);
    assert_eq!(ids.len(), 1);
    let paid_balance = mock_nat(&pic, ICP_LEDGER, "ledger_balance");
    upgrade_sentinel(&pic, sentinel, SENTINEL_WASM, signer);
    run_timer(&pic, sentinel);
    assert_eq!(mock_count(&pic, CMC, "mint_delivery_count"), 1);
    assert_eq!(
        mock_nat(&pic, CMC, "minted_cycles"),
        12 * T + 2 * CYCLES_FEE
    );
    assert_eq!(mock_nat(&pic, ICP_LEDGER, "ledger_balance"), paid_balance);
    assert_exact_withdrawals(&pic, &[2 * T]);
    assert_eq!(public_topups(&pic, sentinel, target).len(), 1);
    assert!(unresolved_operation_ids(&pic, sentinel, signer).is_empty());
}

#[test]
fn shared_reserve_two_low_targets_processing_has_only_one_icp_debit() {
    let (pic, sentinel, signer) = boot_with_funding(40 * T, 10 * T, 20_000 * T);
    shared_setup(&pic, 0);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Processing);
    let first = enable_shared_target(&pic, sentinel, signer);
    let second = principal(SELF_REPORT_TARGETS[1]);
    set_mock_cycles_balance(&pic, second, T);
    let mut patch = target_patch_enable("second low target");
    patch.funding_policy = Some(FundingPolicyArgs {
        low_balance_threshold_cycles: Nat::from(3 * T),
        refill_cycles: Nat::from(2 * T),
        daily_cap_cycles: Nat::from(6 * T),
        cooldown_secs: 3_600,
        burn_anomaly_limit_cycles_per_day: None,
    });
    propose_update_and_execute(&pic, sentinel, signer, second, patch);
    run_timer(&pic, sentinel);
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(unresolved_operation_ids(&pic, sentinel, signer).len(), 1);
    assert_exact_withdrawals(&pic, &[]);
    assert!(public_topups(&pic, sentinel, first).is_empty());
    assert!(public_topups(&pic, sentinel, second).is_empty());
    upgrade_sentinel(&pic, sentinel, SENTINEL_WASM, signer);
    run_timer(&pic, sentinel);
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(unresolved_operation_ids(&pic, sentinel, signer).len(), 1);
}

#[test]
fn shared_reserve_bad_fee_receiver_and_clock_errors_never_trigger_icp_conversion() {
    for mode in [
        WithdrawMode::TerminalNoSpend,
        WithdrawMode::InvalidReceiver,
        WithdrawMode::CreatedInFuture,
    ] {
        let (pic, sentinel, signer) = boot_with_funding(40 * T, 10 * T, 20_000 * T);
        shared_setup(&pic, 12 * T + CYCLES_FEE);
        set_mock_withdraw_mode(&pic, principal(CYCLES_LEDGER), mode);
        let target = enable_shared_target(&pic, sentinel, signer);
        run_timer(&pic, sentinel);
        assert!(withdraw_call_count(&pic) >= 1);
        assert_eq!(transfer_call_count(&pic), 0);
        assert_eq!(
            mock_nat(&pic, ICP_LEDGER, "ledger_balance"),
            DEPOSIT_ICP_E8S
        );
        assert_exact_withdrawals(&pic, &[]);
        assert!(public_topups(&pic, sentinel, target)
            .iter()
            .all(|topup| topup.outcome != FundingOutcomeView::Completed));
    }
}

#[test]
fn shared_reserve_refund_hint_keeps_payment_and_budget_holds_across_upgrade() {
    let (pic, sentinel, signer) = boot_with_funding(40 * T, 10 * T, 20_000 * T);
    shared_setup(&pic, 0);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Refunded);
    let target = enable_shared_target(&pic, sentinel, signer);
    run_timer(&pic, sentinel);
    let ids = unresolved_operation_ids(&pic, sentinel, signer);
    assert_eq!(ids.len(), 1);
    let before = test_operation(&pic, sentinel, ids[0]);
    assert_eq!(
        before.state,
        FundingState::Icp(FundingIcpState::Quarantined)
    );
    assert_eq!(before.refund_block_hint, Some(1));
    assert!(
        before.refund_block_index.is_none(),
        "CMC hint is not verified refund proof"
    );
    assert_exact_withdrawals(&pic, &[]);
    assert!(public_topups(&pic, sentinel, target).is_empty());
    let balance = mock_nat(&pic, ICP_LEDGER, "ledger_balance");
    upgrade_sentinel(&pic, sentinel, SENTINEL_WASM, signer);
    run_timer(&pic, sentinel);
    assert_operations_unchanged(&pic, sentinel, &[before], "quarantined refund upgrade");
    assert_eq!(mock_nat(&pic, ICP_LEDGER, "ledger_balance"), balance);
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(mock_count(&pic, CMC, "mint_delivery_count"), 0);
    assert!(public_topups(&pic, sentinel, target).is_empty());
}

#[test]
fn shared_reserve_unknown_spend_resolution_charges_conversion_cap_without_target_history() {
    let (pic, sentinel, signer) = boot_with_funding(13 * T, 10 * T, 20_000 * T);
    shared_setup(&pic, 0);
    set_mock_transfer_mode(
        &pic,
        principal(ICP_LEDGER),
        TransferMode::CommittedLostReply,
    );
    let target = enable_shared_target(&pic, sentinel, signer);
    run_timer(&pic, sentinel);
    let ids = unresolved_operation_ids(&pic, sentinel, signer);
    assert_eq!(ids.len(), 1);
    assert_eq!(
        test_operation(&pic, sentinel, ids[0]).state,
        FundingState::Icp(FundingIcpState::TransferUnknown)
    );
    let paid_balance = mock_nat(&pic, ICP_LEDGER, "ledger_balance");
    assert_ok(&call_update_raw(
        &pic,
        sentinel,
        signer,
        "resolve_unknown_as_spent",
        Encode!(&ids[0]).unwrap(),
    ));
    assert!(unresolved_operation_ids(&pic, sentinel, signer).is_empty());
    assert!(
        public_topups(&pic, sentinel, target).is_empty(),
        "reserve conversion resolution must not masquerade as a target top-up"
    );
    assert_eq!(
        shared_overview(&pic, sentinel).icp_available_e8s,
        Some(Nat::from(paid_balance)),
        "ICP hold was settled against the conservative debit"
    );
    upgrade_sentinel(&pic, sentinel, SENTINEL_WASM, signer);
    run_timer(&pic, sentinel);
    // The uncertain 12.0002T conversion remains charged against 13T. With
    // no known mint credit, another deficit requires 12.0002T and is denied.
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(mock_nat(&pic, ICP_LEDGER, "ledger_balance"), paid_balance);
    assert_eq!(mock_count(&pic, CMC, "mint_delivery_count"), 0);
    assert_exact_withdrawals(&pic, &[]);
    assert!(public_topups(&pic, sentinel, target).is_empty());
    assert!(unresolved_operation_ids(&pic, sentinel, signer).is_empty());
}

#[test]
fn shared_reserve_at_protected_floor_mints_only_exact_refill_and_fees() {
    let (pic, sentinel, signer) = boot_with_funding(40 * T, 10 * T, 20_000 * T);
    shared_setup(&pic, 10 * T);
    let target = enable_shared_target(&pic, sentinel, signer);
    run_timer(&pic, sentinel);
    assert_eq!(mock_nat(&pic, CMC, "minted_cycles"), 2 * T + 2 * CYCLES_FEE);
    assert_eq!(mock_nat(&pic, CYCLES_LEDGER, "ledger_balance"), 10 * T);
    assert_exact_withdrawals(&pic, &[2 * T]);
    assert_eq!(
        public_topups(&pic, sentinel, target),
        vec![PublicTopupSummary {
            rail: FundingRailView::CyclesLedger,
            outcome: FundingOutcomeView::Completed,
            amount_cycles: Nat::from(2 * T)
        }]
    );
    assert!(unresolved_operation_ids(&pic, sentinel, signer).is_empty());
}

#[test]
fn shared_reserve_partial_deficit_processing_upgrade_preserves_small_mint() {
    let (pic, sentinel, signer) = boot_with_funding(40 * T, 10 * T, 20_000 * T);
    shared_setup(&pic, 11 * T);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Processing);
    let target = enable_shared_target(&pic, sentinel, signer);
    run_timer(&pic, sentinel);
    let ids = unresolved_operation_ids(&pic, sentinel, signer);
    assert_eq!(ids.len(), 1);
    let before = test_operation(&pic, sentinel, ids[0]);
    assert_eq!(before.reserved_amount_cycles, Nat::from(T + 2 * CYCLES_FEE));
    assert_eq!(
        before.state,
        FundingState::Icp(FundingIcpState::NotifyPending)
    );
    assert_exact_withdrawals(&pic, &[]);
    upgrade_sentinel(&pic, sentinel, SENTINEL_WASM, signer);
    assert_operations_unchanged(&pic, sentinel, &[before], "partial deficit upgrade");
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Completed);
    run_timer(&pic, sentinel);
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(mock_nat(&pic, CMC, "minted_cycles"), T + 2 * CYCLES_FEE);
    assert_eq!(mock_nat(&pic, CYCLES_LEDGER, "ledger_balance"), 10 * T);
    assert_exact_withdrawals(&pic, &[2 * T]);
    assert_eq!(public_topups(&pic, sentinel, target).len(), 1);
    assert!(unresolved_operation_ids(&pic, sentinel, signer).is_empty());
}

#[test]
fn shared_reserve_production_cadence_recovers_runtime_without_prewarmed_sources() {
    let (pic, sentinel, _) = boot_with_funding_and_interval(40 * T, 10 * T, 900_000_000_000, 3_600);
    shared_setup(&pic, 0);
    assert_eq!(transfer_call_count(&pic), 0);
    // The CMC fixture publishes a rate current at the future scheduled tick;
    // no Sentinel source-cache refresh or maintenance call prewarms state.
    let scheduled_rate_at = pic
        .get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3_601;
    call_update_unit(
        &pic,
        principal(CMC),
        Principal::anonymous(),
        "set_rate",
        Encode!(&scheduled_rate_at, &10_000u64).unwrap(),
    );
    run_timer_after(&pic, sentinel, Duration::from_secs(3_601));
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(
        mock_nat(&pic, CMC, "minted_cycles"),
        11 * T + 2 * CYCLES_FEE
    );
    assert_exact_withdrawals(&pic, &[T]);
    assert_eq!(mock_nat(&pic, CYCLES_LEDGER, "ledger_balance"), 10 * T);
    assert!(
        pic.cycle_balance(sentinel) > T,
        "first production-cadence timer delivered runtime fuel without a prewarm tick"
    );
    assert!(public_topups(&pic, sentinel, principal(SELF_REPORT_TARGETS[0])).is_empty());
}

#[test]
fn shared_reserve_minimum_icp_processing_refresh_settles_without_double_debit() {
    let (pic, sentinel, signer) = boot_with_funding(40 * T, 10 * T, 20_000 * T);
    shared_setup(&pic, 0);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Processing);
    let target = enable_shared_target(&pic, sentinel, signer);
    let minimum_icp = (12 * T + 2 * CYCLES_FEE) / 10_000 + ICP_FEE;
    set_mock_ledger_balance(&pic, principal(ICP_LEDGER), minimum_icp);
    run_timer(&pic, sentinel);
    assert_eq!(
        mock_nat(&pic, ICP_LEDGER, "ledger_balance"),
        0,
        "one exact transfer and its fee consumed the minimally sufficient deposit"
    );
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(unresolved_operation_ids(&pic, sentinel, signer).len(), 1);
    run_timer(&pic, sentinel); // refresh attempts observe the post-debit zero
    assert_eq!(mock_nat(&pic, ICP_LEDGER, "ledger_balance"), 0);
    assert_exact_withdrawals(&pic, &[]);
    set_mock_notify_mode(&pic, principal(CMC), NotifyMode::Completed);
    run_timer(&pic, sentinel);
    assert_eq!(mock_count(&pic, ICP_LEDGER, "transfer_delivery_count"), 1);
    assert_eq!(mock_count(&pic, CMC, "mint_delivery_count"), 1);
    assert_exact_withdrawals(&pic, &[2 * T]);
    assert_eq!(mock_nat(&pic, CYCLES_LEDGER, "ledger_balance"), 10 * T);
    assert_eq!(
        shared_overview(&pic, sentinel).icp_available_e8s,
        Some(Nat::from(0u8))
    );
    assert_eq!(public_topups(&pic, sentinel, target).len(), 1);
    assert!(
        unresolved_operation_ids(&pic, sentinel, signer).is_empty(),
        "confirmed mint and withdrawal settled without subtracting the ICP payment twice"
    );
}

// Manual maintenance shares the actual timer path. These cases intentionally
// use long deadlines so signer requests do not accidentally rely on a timer.
const FOUR_HOUR_INTERVAL: u64 = 14_400;

fn maintenance_generation(pic: &PocketIc, sentinel: Principal) -> u64 {
    call_query(
        pic,
        sentinel,
        Principal::anonymous(),
        "test_get_completed_tick_generation",
        Encode!().unwrap(),
    )
}

fn maintenance_now(pic: &PocketIc, sentinel: Principal, caller: Principal) -> Result<(), String> {
    call_update(
        pic,
        sentinel,
        caller,
        "run_maintenance_now",
        Encode!().unwrap(),
    )
}

fn maintenance_time_secs(pic: &PocketIc) -> u64 {
    pic.get_time()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[derive(CandidType, Deserialize)]
struct MaintenanceTargetDeadline {
    next_sample_at_secs: Option<u64>,
}

fn maintenance_target_deadline(
    pic: &PocketIc,
    sentinel: Principal,
    target: Principal,
) -> Option<u64> {
    call_query::<Option<MaintenanceTargetDeadline>>(
        pic,
        sentinel,
        telemetry_viewer(),
        "get_public_target",
        Encode!(&target).unwrap(),
    )
    .expect("registered target")
    .next_sample_at_secs
}

fn maintenance_set_four_hour_policy(pic: &PocketIc, sentinel: Principal, signer: Principal) {
    let policy = GlobalPolicyArgs {
        global_daily_cap_cycles: Nat::from(40 * T),
        sample_interval_secs: FOUR_HOUR_INTERVAL,
        stale_after_secs: 2 * FOUR_HOUR_INTERVAL,
        min_icp_reserve_e8s: Nat::from(0u8),
        timelocks: Timelocks {
            target_registry_secs: 1,
            spend_policy_secs: 1,
            signer_change_secs: 1,
            unpause_secs: 1,
        },
        self_recovery_policy: SelfRecoveryPolicyArgs {
            protected_reserve_cycles: Nat::from(10 * T),
            daily_cap_cycles: Nat::from(10 * T),
            low_balance_threshold_cycles: Nat::from(T),
            refill_cycles: Nat::from(T),
        },
    };
    let proposal = decode_ok_u64(&call_update_raw(
        pic,
        sentinel,
        signer,
        "propose_set_global_policy",
        Encode!(&policy).unwrap(),
    ));
    assert_ok(&call_update_raw(
        pic,
        sentinel,
        signer,
        "approve_proposal",
        Encode!(&proposal).unwrap(),
    ));
    pic.advance_time(Duration::from_secs(2));
    assert_ok(&call_update_raw(
        pic,
        sentinel,
        signer,
        "execute_proposal",
        Encode!(&proposal).unwrap(),
    ));
}

#[test]
fn maintenance_rejects_anonymous_and_nonsigner_without_observation_or_funding() {
    let (pic, sentinel, signer) =
        boot_with_funding_and_interval(40 * T, 10 * T, 20_000 * T, FOUR_HOUR_INTERVAL);
    shared_setup(&pic, 12 * T + CYCLES_FEE);
    let target = enable_shared_target(&pic, sentinel, signer);
    let before = shared_overview(&pic, sentinel);
    assert!(before.last_sample_at_secs.is_none());
    assert!(before.cycles_ledger_available_cycles.is_none());
    assert!(before.icp_available_e8s.is_none());
    for caller in [Principal::anonymous(), Principal::from_slice(&[8; 10])] {
        let error =
            maintenance_now(&pic, sentinel, caller).expect_err("only a signer may check now");
        assert!(error.contains("anonymous") || error.contains("NotSigner"));
    }
    let after = shared_overview(&pic, sentinel);
    assert_eq!(maintenance_generation(&pic, sentinel), 0);
    assert_eq!(after.last_sample_at_secs, before.last_sample_at_secs);
    assert_eq!(after.total_observed_cycles, before.total_observed_cycles);
    // Empty caches remain empty; no successful source refresh happened.
    assert_eq!(
        after.cycles_ledger_available_cycles,
        before.cycles_ledger_available_cycles
    );
    assert_eq!(after.icp_available_e8s, before.icp_available_e8s);
    assert_eq!(after.next_sample_at_secs, before.next_sample_at_secs);
    assert_eq!(
        mock_nat(&pic, CYCLES_LEDGER, "ledger_balance"),
        12 * T + CYCLES_FEE
    );
    assert_eq!(
        mock_nat(&pic, ICP_LEDGER, "ledger_balance"),
        DEPOSIT_ICP_E8S
    );
    assert_eq!(
        (
            withdraw_call_count(&pic),
            transfer_call_count(&pic),
            notify_call_count(&pic)
        ),
        (0, 0, 0)
    );
    assert!(public_topups(&pic, sentinel, target).is_empty());
    assert!(unresolved_operation_ids(&pic, sentinel, signer).is_empty());
}

#[test]
fn maintenance_signer_runs_exact_refill_now_and_respects_target_cap() {
    let (pic, sentinel, signer) =
        boot_with_funding_and_interval(40 * T, 10 * T, 20_000 * T, FOUR_HOUR_INTERVAL);
    shared_setup(&pic, 14 * T + 2 * CYCLES_FEE);
    let target = principal(SELF_REPORT_TARGETS[0]);
    set_mock_cycles_balance(&pic, target, T);
    let mut patch = target_patch_enable("manual maintenance cap");
    patch.funding_policy = Some(FundingPolicyArgs {
        low_balance_threshold_cycles: Nat::from(3 * T),
        refill_cycles: Nat::from(2 * T),
        daily_cap_cycles: Nat::from(2 * T),
        cooldown_secs: 0,
        burn_anomaly_limit_cycles_per_day: None,
    });
    propose_update_and_execute(&pic, sentinel, signer, target, patch);
    let before = shared_overview(&pic, sentinel);
    let start_secs = maintenance_time_secs(&pic);
    assert_eq!(before.sample_interval_secs, Some(FOUR_HOUR_INTERVAL));
    assert_eq!(before.stale_after_secs, Some(2 * FOUR_HOUR_INTERVAL));
    assert!(before.next_sample_at_secs.unwrap() > start_secs);
    maintenance_now(&pic, sentinel, signer).expect("signer runs complete maintenance");
    assert_eq!(maintenance_generation(&pic, sentinel), 1);
    assert_exact_withdrawals(&pic, &[2 * T]);
    assert_eq!(
        public_topups(&pic, sentinel, target),
        vec![PublicTopupSummary {
            rail: FundingRailView::CyclesLedger,
            outcome: FundingOutcomeView::Completed,
            amount_cycles: Nat::from(2 * T),
        }]
    );
    let after = shared_overview(&pic, sentinel);
    assert!(after.last_sample_at_secs.is_some());
    assert_eq!(after.next_sample_at_secs, before.next_sample_at_secs);
    assert_eq!(maintenance_time_secs(&pic), start_secs);
    // The target still reports a low balance and has no cooldown. A second
    // manual pass is refused funding solely by its spent 2T daily allowance.
    maintenance_now(&pic, sentinel, signer).expect("maintenance does not bypass funding caps");
    assert_eq!(maintenance_generation(&pic, sentinel), 2);
    assert_exact_withdrawals(&pic, &[2 * T]);
    assert_eq!(withdraw_call_count(&pic), 1);
    assert_eq!(
        mock_nat(&pic, CYCLES_LEDGER, "ledger_balance"),
        12 * T + CYCLES_FEE
    );
    assert_eq!((transfer_call_count(&pic), notify_call_count(&pic)), (0, 0));
    assert_eq!(maintenance_time_secs(&pic), start_secs);
    assert_eq!(
        shared_overview(&pic, sentinel).next_sample_at_secs,
        before.next_sample_at_secs
    );
    assert!(unresolved_operation_ids(&pic, sentinel, signer).is_empty());
}

#[test]
fn maintenance_four_hour_policy_cancels_old_timer_and_manual_keeps_deadline() {
    let (pic, sentinel, signer) = boot_with_funding_and_interval(40 * T, 10 * T, 20_000 * T, 3_600);
    let old_deadline = shared_overview(&pic, sentinel).next_sample_at_secs.unwrap();
    let target = principal(SELF_REPORT_TARGETS[0]);
    propose_update_and_execute(
        &pic,
        sentinel,
        signer,
        target,
        TargetPatch {
            enabled: Some(true),
            ..target_patch_display_name("cadence observation")
        },
    );
    maintenance_set_four_hour_policy(&pic, sentinel, signer);
    let governed = shared_overview(&pic, sentinel);
    let deadline = governed.next_sample_at_secs.unwrap();
    assert_eq!(governed.sample_interval_secs, Some(FOUR_HOUR_INTERVAL));
    assert_eq!(governed.stale_after_secs, Some(2 * FOUR_HOUR_INTERVAL));
    assert_eq!(deadline, maintenance_time_secs(&pic) + FOUR_HOUR_INTERVAL);
    assert!(deadline > old_deadline);
    assert_eq!(
        maintenance_target_deadline(&pic, sentinel, target),
        Some(deadline)
    );
    // The previous one-hour callback must have been cleared by governance.
    pic.advance_time(Duration::from_secs(
        old_deadline + 1 - maintenance_time_secs(&pic),
    ));
    pic.tick();
    assert_eq!(maintenance_generation(&pic, sentinel), 0);
    maintenance_now(&pic, sentinel, signer).unwrap();
    assert_eq!(maintenance_generation(&pic, sentinel), 1);
    assert!(shared_overview(&pic, sentinel)
        .last_sample_at_secs
        .is_some());
    assert_eq!(
        shared_overview(&pic, sentinel).next_sample_at_secs,
        Some(deadline)
    );
    // A manual pass immediately before the real deadline updates observations
    // but must neither postpone the automatic callback nor invent early work.
    pic.advance_time(Duration::from_secs(
        deadline - 1 - maintenance_time_secs(&pic),
    ));
    pic.tick();
    assert_eq!(maintenance_generation(&pic, sentinel), 1);
    maintenance_now(&pic, sentinel, signer).unwrap();
    assert_eq!(maintenance_generation(&pic, sentinel), 2);
    assert_eq!(
        shared_overview(&pic, sentinel).next_sample_at_secs,
        Some(deadline)
    );
    assert_eq!(
        maintenance_target_deadline(&pic, sentinel, target),
        Some(deadline)
    );
    run_timer_after(&pic, sentinel, Duration::from_secs(2));
    assert_eq!(maintenance_generation(&pic, sentinel), 3);
    let after = shared_overview(&pic, sentinel);
    assert_eq!(
        after.next_sample_at_secs,
        Some(maintenance_time_secs(&pic) + FOUR_HOUR_INTERVAL)
    );
    assert_eq!(
        (
            withdraw_call_count(&pic),
            transfer_call_count(&pic),
            notify_call_count(&pic)
        ),
        (0, 0, 0)
    );
}

#[test]
fn maintenance_overlap_rejects_second_manual_and_skips_busy_timer_once() {
    let (pic, sentinel, signer) =
        boot_with_funding_and_interval(40 * T, 10 * T, 20_000 * T, FOUR_HOUR_INTERVAL);
    let deadline = shared_overview(&pic, sentinel).next_sample_at_secs.unwrap();
    // Existing self-report calls provide real external await boundaries;
    // observation is enabled while every target's automatic funding stays off.
    for text in SELF_REPORT_TARGETS {
        propose_update_and_execute(
            &pic,
            sentinel,
            signer,
            principal(text),
            TargetPatch {
                enabled: Some(true),
                ..target_patch_display_name("maintenance overlap")
            },
        );
    }
    let first = pic
        .submit_call(sentinel, signer, "run_maintenance_now", Encode!().unwrap())
        .unwrap();
    pic.tick();
    assert_eq!(maintenance_generation(&pic, sentinel), 0);
    assert_eq!(
        maintenance_now(&pic, sentinel, signer),
        Err("maintenance already running".into())
    );
    assert_eq!(maintenance_generation(&pic, sentinel), 0);
    pic.advance_time(Duration::from_secs(
        deadline + 1 - maintenance_time_secs(&pic),
    ));
    // Observe the callback actually re-arm while the first pass is unfinished,
    // proving the timer encountered the same held work guard as manual ingress.
    for _ in 0..256 {
        if shared_overview(&pic, sentinel).next_sample_at_secs != Some(deadline) {
            break;
        }
        pic.tick();
        assert_eq!(
            maintenance_generation(&pic, sentinel),
            0,
            "manual pass remains pending until timer overlap is observed"
        );
    }
    let next = shared_overview(&pic, sentinel).next_sample_at_secs.unwrap();
    assert!(
        next > deadline,
        "scheduled callback ran and re-armed its timer"
    );
    let reply = match pic.await_call(first).unwrap() {
        WasmResult::Reply(bytes) => bytes,
        WasmResult::Reject(message) => panic!("maintenance ingress rejected: {message}"),
    };
    assert_eq!(Decode!(&reply, Result<(), String>).unwrap(), Ok(()));
    assert_eq!(
        maintenance_generation(&pic, sentinel),
        1,
        "busy callback did not create another pass"
    );
    run_timer_after(
        &pic,
        sentinel,
        Duration::from_secs(next + 1 - maintenance_time_secs(&pic)),
    );
    assert_eq!(
        maintenance_generation(&pic, sentinel),
        2,
        "work guard was released for the next automatic pass"
    );
    assert_eq!(
        (
            withdraw_call_count(&pic),
            transfer_call_count(&pic),
            notify_call_count(&pic)
        ),
        (0, 0, 0)
    );
}
