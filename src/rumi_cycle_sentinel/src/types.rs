//! Pure domain types for Cycle Sentinel.
//!
//! Everything here is deliberately free of `ic-cdk` calls, stable-memory
//! access, and timers: constructors validate their own invariants and can be
//! exercised with plain unit tests. `state.rs` (a later task) is responsible
//! for wiring these types into `ic-stable-structures` storage.
//!
//! Convention: any type that is meant to be a Candid method argument (an
//! `*Args` struct, `InitArgs`, `TargetPatch`, proposal payloads, or a public
//! read-model) carries cycle amounts as `Nat` so that oversized wire values
//! never trap decoding and are instead rejected by an explicit, testable
//! `Result`. Once a `Nat` has been checked-converted to `u128` it lives in a
//! validated internal type (`GlobalPolicy`, `TargetFundingPolicy`,
//! `SelfRecoveryPolicy`, funding/reservation state) whose constructors are
//! the only way to produce a value — including `Decode!`/`decode_one`, since
//! every such type has a hand-written `Deserialize` impl (never a derive)
//! that re-runs the same checked constructor against the freshly decoded
//! bytes.
//!
//! **Scope of that guarantee.** Every custom `Deserialize` impl in this file
//! re-checks only *self-contained* invariants: ones checkable from the
//! decoded value's own fields alone, with no other loaded state. Holding
//! one of these types is proof the value could have been produced by its
//! own checked constructors in isolation — no more, no less. Some types
//! additionally have *whole-state* invariants that need data no single
//! decoded value can see (a sibling registry, the currently loaded
//! `GlobalPolicy`, another type's own stable-memory instance) — those are
//! called out on the specific type's own doc comment (for example
//! `TargetFundingPolicy`'s cap-vs-`GlobalPolicy` bound, or
//! `TargetRecord`'s reserved/duplicate-principal check) and are
//! `state.rs`'s responsibility to re-check on every load path (init,
//! `post_upgrade`, and any future migration/repair tooling), not something
//! this module can guarantee on its own. Do not read "an invalid value
//! cannot exist" as "a value inconsistent with the rest of the loaded
//! state cannot exist" — only the former is this file's job.

use candid::{CandidType, Nat, Principal};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;

/// Maps any `Debug`-formattable validation failure into a `serde`
/// deserialization error. Every custom `Deserialize` impl below re-runs a
/// checked constructor (`validate`, `new`, or a self-contained invariant
/// check) against the freshly decoded wire bytes and routes a failure
/// through this helper, so a stable/domain value that fails its own
/// invariants can never be produced by `Decode!`/`decode_one`, only by the
/// checked constructors that already existed for the non-decode path.
fn invariant_decode_error<T, E>(err: T) -> E
where
    T: std::fmt::Debug,
    E: serde::de::Error,
{
    E::custom(format!("{err:?}"))
}

// ─────────────────────────── Bounds ───────────────────────────

pub const MAX_TARGETS: usize = 128;
pub const MAX_SAMPLES_PER_TARGET: usize = 2_160;
pub const MAX_ALARMS: usize = 1_024;
pub const MAX_PROPOSALS: usize = 256;
pub const MAX_TERMINAL_SUMMARIES: usize = 512;
pub const MAX_PUBLIC_PAGE: usize = 100;
/// Shared byte bound for `TargetArgs::display_name` and `TargetArgs::project`.
pub const MAX_NAME_BYTES: usize = 64;
pub const MAX_TAGS: usize = 16;
pub const MAX_TAG_BYTES: usize = 32;
/// A single refill can never exceed 100T cycles.
pub const MAX_REFILL_CYCLES: u128 = 100_000_000_000_000;
/// A low-balance threshold can never exceed 100T cycles (same order as a
/// single refill).
pub const MAX_LOW_BALANCE_THRESHOLD_CYCLES: u128 = 100_000_000_000_000;
/// A target's rolling daily cap can never exceed 1,000T cycles (10x the max
/// single refill).
pub const MAX_TARGET_DAILY_CAP_CYCLES: u128 = 1_000_000_000_000_000;
/// A burn-anomaly limit can never exceed 1,000T cycles/day (same ceiling as
/// the daily cap it is compared against).
pub const MAX_BURN_ANOMALY_CYCLES_PER_DAY: u128 = 1_000_000_000_000_000;
/// Bound on how many recent top-ups a single `PublicTargetRow` carries.
pub const MAX_RECENT_TOPUPS_PER_TARGET: usize = 10;
/// A rolling-spend ledger can never hold more pending (in-flight)
/// reservations than there are targets, since each target may have at most
/// one in-flight operation at a time.
pub const MAX_PENDING_RESERVATIONS: usize = MAX_TARGETS;
/// Bound on how many settled `SpendEntry` timestamps a single rolling-spend
/// ledger retains; entries older than the window are pruned on `reserve`,
/// this is only a safety net against unbounded growth between prunes.
pub const MAX_ROLLING_SPEND_SETTLED_ENTRIES: usize = 512;
/// A single funding operation can never accumulate more than this many
/// attempt records; a runaway retry loop is a bug, not a reason to grow
/// stable memory without bound.
pub const MAX_FUNDING_ATTEMPTS: usize = 32;

// ─────────────────────── Reserved principals ───────────────────────

/// Mainnet ICP ledger canister. Public network infrastructure, not a
/// deployment-specific secret.
fn icp_ledger_principal() -> Principal {
    Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").unwrap()
}

/// Mainnet NNS Cycles Minting Canister.
fn cycles_minting_canister_principal() -> Principal {
    Principal::from_text("rkp4c-7iaaa-aaaaa-aaaca-cai").unwrap()
}

/// Mainnet ICRC Cycles Ledger canister.
fn cycles_ledger_principal() -> Principal {
    Principal::from_text("um5iw-rqaaa-aaaaq-qaaba-cai").unwrap()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReservedPrincipalKind {
    Anonymous,
    SentinelSelf,
    ManagementCanister,
    IcpLedger,
    Cmc,
    CyclesLedger,
}

/// Sentinel is never a target and never controls a target, so every one of
/// these identities is rejected as a registration argument.
pub fn reserved_principal_kind(
    principal: Principal,
    sentinel_id: Principal,
) -> Option<ReservedPrincipalKind> {
    if principal == Principal::anonymous() {
        Some(ReservedPrincipalKind::Anonymous)
    } else if principal == sentinel_id {
        Some(ReservedPrincipalKind::SentinelSelf)
    } else if principal == Principal::management_canister() {
        Some(ReservedPrincipalKind::ManagementCanister)
    } else if principal == icp_ledger_principal() {
        Some(ReservedPrincipalKind::IcpLedger)
    } else if principal == cycles_minting_canister_principal() {
        Some(ReservedPrincipalKind::Cmc)
    } else if principal == cycles_ledger_principal() {
        Some(ReservedPrincipalKind::CyclesLedger)
    } else {
        None
    }
}

// ─────────────────────── Nat <-> u128 conversion ───────────────────────

/// Checked conversion for policy and funding inputs. Oversized values are
/// rejected (`None`), never silently truncated or saturated.
pub fn checked_cycles_from_nat(value: &Nat) -> Option<u128> {
    u128::try_from(value.0.clone()).ok()
}

/// A target-reported balance used only for human display. `Overflow` is
/// tracked as its own case instead of silently saturating, because a
/// malicious or buggy target must never be able to make Sentinel trap by
/// reporting an oversized number, but a clamped value must also never be
/// mistaken for a genuinely low (or genuinely huge) one.
///
/// There is no `PartialOrd`/`Ord`, and `low_balance_value()` returns `None`
/// for `Overflow`: any future low-balance comparison has to explicitly
/// decide what an unrepresentable balance means rather than accidentally
/// comparing it via a derived ordering.
#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvisoryCyclesBalance {
    Exact(u128),
    Overflow,
}

impl AdvisoryCyclesBalance {
    pub fn from_nat(value: &Nat) -> Self {
        match u128::try_from(value.0.clone()) {
            Ok(exact) => Self::Exact(exact),
            Err(_) => Self::Overflow,
        }
    }

    /// The magnitude usable for low-balance comparisons. `None` for
    /// `Overflow` — an overflowed report must never be coerced into looking
    /// low (or into any other comparison outcome).
    pub fn low_balance_value(&self) -> Option<u128> {
        match self {
            Self::Exact(value) => Some(*value),
            Self::Overflow => None,
        }
    }

    /// Safe for public display: `Overflow` renders as `u128::MAX` rather
    /// than trapping or losing precision.
    pub fn to_nat(&self) -> Nat {
        match self {
            Self::Exact(value) => Nat::from(*value),
            Self::Overflow => Nat::from(u128::MAX),
        }
    }

    /// True when `to_nat()` is a clamped display value rather than a
    /// genuine reported balance. Public projections must carry this
    /// alongside `to_nat()` so an overflow is truthfully labeled instead of
    /// being indistinguishable from a real `u128::MAX` balance.
    pub fn is_overflow(&self) -> bool {
        matches!(self, Self::Overflow)
    }
}

// ─────────────────────── Bounded string fields ───────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundedFieldError {
    Empty,
    TooLong,
    TooMany,
    Duplicate,
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BoundedName(String);

impl BoundedName {
    pub fn new(value: &str) -> Result<Self, BoundedFieldError> {
        if value.is_empty() {
            return Err(BoundedFieldError::Empty);
        }
        if value.as_bytes().len() > MAX_NAME_BYTES {
            return Err(BoundedFieldError::TooLong);
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Re-runs `new`'s bounds against the raw decoded string, so a
/// `BoundedName` can never be constructed by decode with an invariant `new`
/// would have rejected.
impl<'de> Deserialize<'de> for BoundedName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(&value).map_err(invariant_decode_error)
    }
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct BoundedTags(Vec<String>);

impl BoundedTags {
    pub fn new(tags: Vec<String>) -> Result<Self, BoundedFieldError> {
        if tags.len() > MAX_TAGS {
            return Err(BoundedFieldError::TooMany);
        }
        let mut seen = BTreeSet::new();
        for tag in &tags {
            if tag.is_empty() {
                return Err(BoundedFieldError::Empty);
            }
            if tag.as_bytes().len() > MAX_TAG_BYTES {
                return Err(BoundedFieldError::TooLong);
            }
            if !seen.insert(tag.clone()) {
                return Err(BoundedFieldError::Duplicate);
            }
        }
        Ok(Self(tags))
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

/// Re-runs `new`'s bounds (count, per-tag length, emptiness, uniqueness)
/// against the raw decoded vector.
impl<'de> Deserialize<'de> for BoundedTags {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let tags = Vec::<String>::deserialize(deserializer)?;
        Self::new(tags).map_err(invariant_decode_error)
    }
}

// ─────────────────────── Simple descriptive enums ───────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Environment {
    Production,
    Staging,
    Test,
    Local,
    Archived,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Criticality {
    Critical,
    Important,
    Standard,
    Experimental,
}

/// How Sentinel gets a target's cycle balance.
#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservationMode {
    /// The target exposes `cycles_status` directly (Rumi-owned canisters).
    SelfReport,
    /// The target is read indirectly through the immutable `ic-blackhole`
    /// status relay.
    BlackholeRelay,
    /// Registered but not yet wired for observation.
    Unobserved,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicTargetState {
    Healthy,
    Low,
    Stopped,
    Uninstalled,
    Unreachable,
    Unobserved,
}

// ─────────────────────── Governance timelocks ───────────────────────

/// Per-kind elapsed-time requirements `execute_proposal` (implemented in a
/// later task) must enforce before a threshold-approved proposal of that
/// kind can execute. Deployment-manifest input, not a hardcoded constant.
#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct GovernanceTimelocksArgs {
    pub target_registry_secs: u64,
    pub spend_policy_secs: u64,
    pub signer_change_secs: u64,
    pub unpause_secs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GovernanceTimelocksError {
    ZeroTargetRegistrySecs,
    ZeroSpendPolicySecs,
    ZeroSignerChangeSecs,
    ZeroUnpauseSecs,
}

#[derive(CandidType, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct GovernanceTimelocks {
    target_registry_secs: u64,
    spend_policy_secs: u64,
    signer_change_secs: u64,
    unpause_secs: u64,
}

/// `GovernanceTimelocksArgs` has the identical field set, so decoding
/// through it and re-running `validate` re-enforces every zero check
/// without changing the wire shape at all.
impl<'de> Deserialize<'de> for GovernanceTimelocks {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let args = GovernanceTimelocksArgs::deserialize(deserializer)?;
        Self::validate(&args).map_err(invariant_decode_error)
    }
}

impl GovernanceTimelocks {
    /// Rejects `0` for every timelock: a zero timelock collapses "approve"
    /// and "execute" into the same instant for that proposal kind, which
    /// defeats the reaction-time protection timelocks exist to provide.
    pub fn validate(args: &GovernanceTimelocksArgs) -> Result<Self, GovernanceTimelocksError> {
        if args.target_registry_secs == 0 {
            return Err(GovernanceTimelocksError::ZeroTargetRegistrySecs);
        }
        if args.spend_policy_secs == 0 {
            return Err(GovernanceTimelocksError::ZeroSpendPolicySecs);
        }
        if args.signer_change_secs == 0 {
            return Err(GovernanceTimelocksError::ZeroSignerChangeSecs);
        }
        if args.unpause_secs == 0 {
            return Err(GovernanceTimelocksError::ZeroUnpauseSecs);
        }
        Ok(Self {
            target_registry_secs: args.target_registry_secs,
            spend_policy_secs: args.spend_policy_secs,
            signer_change_secs: args.signer_change_secs,
            unpause_secs: args.unpause_secs,
        })
    }

    pub fn target_registry_secs(&self) -> u64 {
        self.target_registry_secs
    }

    pub fn spend_policy_secs(&self) -> u64 {
        self.spend_policy_secs
    }

    pub fn signer_change_secs(&self) -> u64 {
        self.signer_change_secs
    }

    pub fn unpause_secs(&self) -> u64 {
        self.unpause_secs
    }

    pub fn to_args(&self) -> GovernanceTimelocksArgs {
        GovernanceTimelocksArgs {
            target_registry_secs: self.target_registry_secs,
            spend_policy_secs: self.spend_policy_secs,
            signer_change_secs: self.signer_change_secs,
            unpause_secs: self.unpause_secs,
        }
    }
}

// ─────────────────────── Global policy ───────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct GlobalPolicyArgs {
    pub global_daily_cap_cycles: Nat,
    pub sample_interval_secs: u64,
    pub stale_after_secs: u64,
    pub min_icp_reserve_e8s: Nat,
    pub timelocks: GovernanceTimelocksArgs,
    pub self_recovery_policy: SelfRecoveryPolicyArgs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalPolicyError {
    CyclesValueOverflow,
    ZeroGlobalDailyCap,
    ZeroSampleInterval,
    StaleAfterBelowSampleInterval,
    InvalidSelfRecoveryPolicy(SelfRecoveryPolicyError),
    InvalidTimelocks(GovernanceTimelocksError),
}

/// An immutable, already-checked snapshot of the sentinel-wide policy. Every
/// field, including the nested `SelfRecoveryPolicy`, has passed its own
/// zero/ordering checks, so downstream code never has to re-validate it.
#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct GlobalPolicy {
    global_daily_cap_cycles: u128,
    sample_interval_secs: u64,
    stale_after_secs: u64,
    min_icp_reserve_e8s: u128,
    timelocks: GovernanceTimelocks,
    self_recovery_policy: SelfRecoveryPolicy,
}

/// `GlobalPolicyArgs` has the identical field set (`Nat`/`*Args` in place of
/// this type's `u128`/checked-internal fields), so decoding through it and
/// re-running `validate` re-enforces every bound — including the nested
/// `SelfRecoveryPolicy`/`GovernanceTimelocks` checks — without changing the
/// wire shape.
impl<'de> Deserialize<'de> for GlobalPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let args = GlobalPolicyArgs::deserialize(deserializer)?;
        Self::validate(&args).map_err(invariant_decode_error)
    }
}

impl GlobalPolicy {
    pub fn validate(args: &GlobalPolicyArgs) -> Result<Self, GlobalPolicyError> {
        let global_daily_cap_cycles = checked_cycles_from_nat(&args.global_daily_cap_cycles)
            .ok_or(GlobalPolicyError::CyclesValueOverflow)?;
        if global_daily_cap_cycles == 0 {
            return Err(GlobalPolicyError::ZeroGlobalDailyCap);
        }
        if args.sample_interval_secs == 0 {
            return Err(GlobalPolicyError::ZeroSampleInterval);
        }
        if args.stale_after_secs < args.sample_interval_secs {
            return Err(GlobalPolicyError::StaleAfterBelowSampleInterval);
        }
        let min_icp_reserve_e8s = checked_cycles_from_nat(&args.min_icp_reserve_e8s)
            .ok_or(GlobalPolicyError::CyclesValueOverflow)?;
        let self_recovery_policy = SelfRecoveryPolicy::validate(&args.self_recovery_policy)
            .map_err(GlobalPolicyError::InvalidSelfRecoveryPolicy)?;
        let timelocks = GovernanceTimelocks::validate(&args.timelocks)
            .map_err(GlobalPolicyError::InvalidTimelocks)?;

        Ok(Self {
            global_daily_cap_cycles,
            sample_interval_secs: args.sample_interval_secs,
            stale_after_secs: args.stale_after_secs,
            min_icp_reserve_e8s,
            timelocks,
            self_recovery_policy,
        })
    }

    pub fn global_daily_cap_cycles(&self) -> u128 {
        self.global_daily_cap_cycles
    }

    pub fn sample_interval_secs(&self) -> u64 {
        self.sample_interval_secs
    }

    pub fn stale_after_secs(&self) -> u64 {
        self.stale_after_secs
    }

    pub fn min_icp_reserve_e8s(&self) -> u128 {
        self.min_icp_reserve_e8s
    }

    pub fn timelocks(&self) -> &GovernanceTimelocks {
        &self.timelocks
    }

    pub fn self_recovery_policy(&self) -> &SelfRecoveryPolicy {
        &self.self_recovery_policy
    }

    pub fn to_args(&self) -> GlobalPolicyArgs {
        GlobalPolicyArgs {
            global_daily_cap_cycles: Nat::from(self.global_daily_cap_cycles),
            sample_interval_secs: self.sample_interval_secs,
            stale_after_secs: self.stale_after_secs,
            min_icp_reserve_e8s: Nat::from(self.min_icp_reserve_e8s),
            timelocks: self.timelocks.to_args(),
            self_recovery_policy: self.self_recovery_policy.to_args(),
        }
    }
}

// ─────────────────────── Init args / signers ───────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct InitArgs {
    pub signers: Vec<Principal>,
    pub approval_threshold: u32,
    pub global_policy: GlobalPolicyArgs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InitArgsError {
    EmptySigners,
    AnonymousSigner,
    DuplicateSigner(Principal),
    ThresholdZero,
    ThresholdExceedsSigners { threshold: u32, signer_count: u32 },
    InvalidGlobalPolicy(GlobalPolicyError),
}

/// A `ValidatedInitArgs` can only be constructed by `validate`, so once one
/// exists its signer list is nonempty, unique, non-anonymous, and its
/// threshold satisfies `1 <= threshold <= signers.len()`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedInitArgs {
    signers: Vec<Principal>,
    approval_threshold: u32,
    global_policy: GlobalPolicy,
}

impl ValidatedInitArgs {
    pub fn validate(args: InitArgs) -> Result<Self, InitArgsError> {
        if args.signers.is_empty() {
            return Err(InitArgsError::EmptySigners);
        }
        let mut seen = BTreeSet::new();
        for signer in &args.signers {
            if *signer == Principal::anonymous() {
                return Err(InitArgsError::AnonymousSigner);
            }
            if !seen.insert(*signer) {
                return Err(InitArgsError::DuplicateSigner(*signer));
            }
        }
        if args.approval_threshold == 0 {
            return Err(InitArgsError::ThresholdZero);
        }
        if args.approval_threshold as usize > args.signers.len() {
            return Err(InitArgsError::ThresholdExceedsSigners {
                threshold: args.approval_threshold,
                signer_count: args.signers.len() as u32,
            });
        }
        let global_policy = GlobalPolicy::validate(&args.global_policy)
            .map_err(InitArgsError::InvalidGlobalPolicy)?;
        Ok(Self {
            signers: args.signers,
            approval_threshold: args.approval_threshold,
            global_policy,
        })
    }

    pub fn signers(&self) -> &[Principal] {
        &self.signers
    }

    pub fn approval_threshold(&self) -> u32 {
        self.approval_threshold
    }

    pub fn global_policy(&self) -> &GlobalPolicy {
        &self.global_policy
    }
}

// ─────────────────────── Target funding policy ───────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct TargetFundingPolicyArgs {
    pub low_balance_threshold_cycles: Nat,
    pub refill_cycles: Nat,
    pub daily_cap_cycles: Nat,
    pub cooldown_secs: u64,
    pub burn_anomaly_limit_cycles_per_day: Option<Nat>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetValidationError {
    ReservedPrincipal(ReservedPrincipalKind),
    DuplicateTarget,
    TooManyTargets,
    NameEmpty,
    NameTooLong,
    ProjectEmpty,
    ProjectTooLong,
    TagEmpty,
    TooManyTags,
    TagTooLong,
    DuplicateTag,
    CyclesValueOverflow,
    ZeroLowBalanceThreshold,
    LowBalanceThresholdExceedsMaximum,
    ZeroRefillCycles,
    RefillExceedsMaximum,
    DailyCapBelowRefill,
    DailyCapExceedsMaximum,
    GlobalCapBelowDailyCap,
    ZeroBurnAnomalyLimit,
    BurnAnomalyLimitExceedsMaximum,
    AutoTopupRequiresEnabledAndObserved,
}

/// An immutable, already-checked snapshot of a target's funding policy.
/// Every field has passed the zero/maximum/ordering checks, so downstream
/// funding code never has to re-validate it.
#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct TargetFundingPolicy {
    low_balance_threshold_cycles: u128,
    refill_cycles: u128,
    daily_cap_cycles: u128,
    cooldown_secs: u64,
    burn_anomaly_limit_cycles_per_day: Option<u128>,
}

impl TargetFundingPolicy {
    pub fn validate(
        args: &TargetFundingPolicyArgs,
        global_policy: &GlobalPolicy,
    ) -> Result<Self, TargetValidationError> {
        let value = Self::validate_self_contained(args)?;
        if global_policy.global_daily_cap_cycles() < value.daily_cap_cycles {
            return Err(TargetValidationError::GlobalCapBelowDailyCap);
        }
        Ok(value)
    }

    /// Every bound checkable from `args` alone, i.e. everything `validate`
    /// enforces except `GlobalCapBelowDailyCap`, which needs the target's
    /// enclosing `GlobalPolicy`. Used by `validate` and by this type's
    /// custom `Deserialize` impl, which has no `GlobalPolicy` available at
    /// decode time and so can only re-run this self-contained subset — the
    /// cross-policy check is a residual gap decode cannot close, documented
    /// in the Task 1b Finding G correction report.
    fn validate_self_contained(
        args: &TargetFundingPolicyArgs,
    ) -> Result<Self, TargetValidationError> {
        let low_balance_threshold_cycles =
            checked_cycles_from_nat(&args.low_balance_threshold_cycles)
                .ok_or(TargetValidationError::CyclesValueOverflow)?;
        let refill_cycles = checked_cycles_from_nat(&args.refill_cycles)
            .ok_or(TargetValidationError::CyclesValueOverflow)?;
        let daily_cap_cycles = checked_cycles_from_nat(&args.daily_cap_cycles)
            .ok_or(TargetValidationError::CyclesValueOverflow)?;
        let burn_anomaly_limit_cycles_per_day = match &args.burn_anomaly_limit_cycles_per_day {
            Some(value) => {
                let value = checked_cycles_from_nat(value)
                    .ok_or(TargetValidationError::CyclesValueOverflow)?;
                if value == 0 {
                    return Err(TargetValidationError::ZeroBurnAnomalyLimit);
                }
                if value > MAX_BURN_ANOMALY_CYCLES_PER_DAY {
                    return Err(TargetValidationError::BurnAnomalyLimitExceedsMaximum);
                }
                Some(value)
            }
            None => None,
        };

        if low_balance_threshold_cycles == 0 {
            return Err(TargetValidationError::ZeroLowBalanceThreshold);
        }
        if low_balance_threshold_cycles > MAX_LOW_BALANCE_THRESHOLD_CYCLES {
            return Err(TargetValidationError::LowBalanceThresholdExceedsMaximum);
        }
        if refill_cycles == 0 {
            return Err(TargetValidationError::ZeroRefillCycles);
        }
        if refill_cycles > MAX_REFILL_CYCLES {
            return Err(TargetValidationError::RefillExceedsMaximum);
        }
        if daily_cap_cycles < refill_cycles {
            return Err(TargetValidationError::DailyCapBelowRefill);
        }
        if daily_cap_cycles > MAX_TARGET_DAILY_CAP_CYCLES {
            return Err(TargetValidationError::DailyCapExceedsMaximum);
        }

        Ok(Self {
            low_balance_threshold_cycles,
            refill_cycles,
            daily_cap_cycles,
            cooldown_secs: args.cooldown_secs,
            burn_anomaly_limit_cycles_per_day,
        })
    }

    pub fn to_args(&self) -> TargetFundingPolicyArgs {
        TargetFundingPolicyArgs {
            low_balance_threshold_cycles: Nat::from(self.low_balance_threshold_cycles),
            refill_cycles: Nat::from(self.refill_cycles),
            daily_cap_cycles: Nat::from(self.daily_cap_cycles),
            cooldown_secs: self.cooldown_secs,
            burn_anomaly_limit_cycles_per_day: self
                .burn_anomaly_limit_cycles_per_day
                .map(Nat::from),
        }
    }

    pub fn low_balance_threshold_cycles(&self) -> u128 {
        self.low_balance_threshold_cycles
    }

    pub fn refill_cycles(&self) -> u128 {
        self.refill_cycles
    }

    pub fn daily_cap_cycles(&self) -> u128 {
        self.daily_cap_cycles
    }

    pub fn cooldown_secs(&self) -> u64 {
        self.cooldown_secs
    }

    pub fn burn_anomaly_limit_cycles_per_day(&self) -> Option<u128> {
        self.burn_anomaly_limit_cycles_per_day
    }
}

/// `TargetFundingPolicyArgs` has the identical field set, so decoding
/// through it and re-running `validate_self_contained` re-enforces every
/// bound checkable without a `GlobalPolicy` in hand.
impl<'de> Deserialize<'de> for TargetFundingPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let args = TargetFundingPolicyArgs::deserialize(deserializer)?;
        Self::validate_self_contained(&args).map_err(invariant_decode_error)
    }
}

fn validate_descriptive_fields(
    display_name: &str,
    project: &str,
    tags: &[String],
    funding_policy_args: &TargetFundingPolicyArgs,
    global_policy: &GlobalPolicy,
) -> Result<(BoundedName, BoundedName, BoundedTags, TargetFundingPolicy), TargetValidationError> {
    let display_name = BoundedName::new(display_name).map_err(|err| match err {
        BoundedFieldError::Empty => TargetValidationError::NameEmpty,
        BoundedFieldError::TooLong => TargetValidationError::NameTooLong,
        BoundedFieldError::TooMany | BoundedFieldError::Duplicate => {
            unreachable!("BoundedName only produces Empty/TooLong")
        }
    })?;
    let project = BoundedName::new(project).map_err(|err| match err {
        BoundedFieldError::Empty => TargetValidationError::ProjectEmpty,
        BoundedFieldError::TooLong => TargetValidationError::ProjectTooLong,
        BoundedFieldError::TooMany | BoundedFieldError::Duplicate => {
            unreachable!("BoundedName only produces Empty/TooLong")
        }
    })?;
    let tags = BoundedTags::new(tags.to_vec()).map_err(|err| match err {
        BoundedFieldError::TooMany => TargetValidationError::TooManyTags,
        BoundedFieldError::TooLong => TargetValidationError::TagTooLong,
        BoundedFieldError::Empty => TargetValidationError::TagEmpty,
        BoundedFieldError::Duplicate => TargetValidationError::DuplicateTag,
    })?;
    let funding_policy = TargetFundingPolicy::validate(funding_policy_args, global_policy)?;
    Ok((display_name, project, tags, funding_policy))
}

// ─────────────────────── Target registry ───────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct TargetArgs {
    pub principal: Principal,
    pub display_name: String,
    pub project: String,
    pub environment: Environment,
    pub criticality: Criticality,
    pub observation_mode: ObservationMode,
    pub tags: Vec<String>,
    pub funding_policy: TargetFundingPolicyArgs,
}

/// Everything governable on a target except `principal` and `revision`
/// (immutable) and `paused` (only `TargetRecord::pause`/`unpause` may change
/// it, since pause is callable by any single signer while unpause is not).
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct TargetPatch {
    pub display_name: Option<String>,
    pub project: Option<String>,
    pub environment: Option<Environment>,
    pub criticality: Option<Criticality>,
    pub observation_mode: Option<ObservationMode>,
    pub tags: Option<Vec<String>>,
    pub funding_policy: Option<TargetFundingPolicyArgs>,
    pub enabled: Option<bool>,
    pub auto_topup: Option<bool>,
}

pub struct TargetRegistrationContext<'a> {
    pub sentinel_id: Principal,
    pub existing_target_count: usize,
    pub existing_target_principals: &'a BTreeSet<Principal>,
    pub global_policy: &'a GlobalPolicy,
}

/// `principal` and `revision` are immutable from the outside: `revision` is
/// only ever advanced by `apply_patch`, `pause`, or `unpause` (each of which
/// increments it by exactly one), and there is no way to change `principal`
/// at all. `enabled`, `auto_topup`, and `paused` are not present in
/// `TargetArgs`, so `register` always starts a new record disabled,
/// auto-topup-off, and unpaused regardless of caller input; `enabled` and
/// `auto_topup` become governable via `apply_patch`, while `paused` is only
/// ever changed by `pause`/`unpause`.
#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct TargetRecord {
    principal: Principal,
    revision: u64,
    display_name: BoundedName,
    project: BoundedName,
    environment: Environment,
    criticality: Criticality,
    observation_mode: ObservationMode,
    tags: BoundedTags,
    funding_policy: TargetFundingPolicy,
    enabled: bool,
    auto_topup: bool,
    paused: bool,
}

impl TargetRecord {
    pub fn register(
        args: TargetArgs,
        ctx: &TargetRegistrationContext,
    ) -> Result<Self, TargetValidationError> {
        if let Some(kind) = reserved_principal_kind(args.principal, ctx.sentinel_id) {
            return Err(TargetValidationError::ReservedPrincipal(kind));
        }
        if ctx.existing_target_principals.contains(&args.principal) {
            return Err(TargetValidationError::DuplicateTarget);
        }
        if ctx.existing_target_count >= MAX_TARGETS {
            return Err(TargetValidationError::TooManyTargets);
        }
        let (display_name, project, tags, funding_policy) = validate_descriptive_fields(
            &args.display_name,
            &args.project,
            &args.tags,
            &args.funding_policy,
            ctx.global_policy,
        )?;

        Ok(Self {
            principal: args.principal,
            revision: 1,
            display_name,
            project,
            environment: args.environment,
            criticality: args.criticality,
            observation_mode: args.observation_mode,
            tags,
            funding_policy,
            enabled: false,
            auto_topup: false,
            paused: false,
        })
    }

    pub fn apply_patch(
        &self,
        patch: TargetPatch,
        global_policy: &GlobalPolicy,
    ) -> Result<Self, TargetValidationError> {
        let display_name = patch
            .display_name
            .unwrap_or_else(|| self.display_name.as_str().to_string());
        let project = patch
            .project
            .unwrap_or_else(|| self.project.as_str().to_string());
        let environment = patch.environment.unwrap_or(self.environment);
        let criticality = patch.criticality.unwrap_or(self.criticality);
        let observation_mode = patch.observation_mode.unwrap_or(self.observation_mode);
        let tags = patch.tags.unwrap_or_else(|| self.tags.as_slice().to_vec());
        let funding_policy_args = patch
            .funding_policy
            .unwrap_or_else(|| self.funding_policy.to_args());
        let enabled = patch.enabled.unwrap_or(self.enabled);
        let auto_topup = patch.auto_topup.unwrap_or(self.auto_topup);

        if auto_topup && (!enabled || observation_mode == ObservationMode::Unobserved) {
            return Err(TargetValidationError::AutoTopupRequiresEnabledAndObserved);
        }

        let (display_name, project, tags, funding_policy) = validate_descriptive_fields(
            &display_name,
            &project,
            &tags,
            &funding_policy_args,
            global_policy,
        )?;

        Ok(Self {
            principal: self.principal,
            revision: self.revision + 1,
            display_name,
            project,
            environment,
            criticality,
            observation_mode,
            tags,
            funding_policy,
            enabled,
            auto_topup,
            paused: self.paused,
        })
    }

    /// Callable by any single signer per the design's pause authority.
    /// Increments `revision`; every other field is preserved unchanged.
    pub fn pause(&self) -> Self {
        self.with_paused(true)
    }

    /// Never reachable through the same call path as `pause` — unpausing
    /// goes through the governed proposal/threshold/timelock system.
    /// Increments `revision`; every other field is preserved unchanged.
    pub fn unpause(&self) -> Self {
        self.with_paused(false)
    }

    fn with_paused(&self, paused: bool) -> Self {
        Self {
            principal: self.principal,
            revision: self.revision + 1,
            display_name: self.display_name.clone(),
            project: self.project.clone(),
            environment: self.environment,
            criticality: self.criticality,
            observation_mode: self.observation_mode,
            tags: self.tags.clone(),
            funding_policy: self.funding_policy.clone(),
            enabled: self.enabled,
            auto_topup: self.auto_topup,
            paused,
        }
    }

    pub fn principal(&self) -> Principal {
        self.principal
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn display_name(&self) -> &str {
        self.display_name.as_str()
    }

    pub fn project(&self) -> &str {
        self.project.as_str()
    }

    pub fn environment(&self) -> Environment {
        self.environment
    }

    pub fn criticality(&self) -> Criticality {
        self.criticality
    }

    pub fn observation_mode(&self) -> ObservationMode {
        self.observation_mode
    }

    pub fn tags(&self) -> &[String] {
        self.tags.as_slice()
    }

    pub fn funding_policy(&self) -> &TargetFundingPolicy {
        &self.funding_policy
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn auto_topup(&self) -> bool {
        self.auto_topup
    }

    pub fn paused(&self) -> bool {
        self.paused
    }
}

#[derive(Debug)]
enum TargetRecordDecodeError {
    /// `register` always starts at `1` and every mutator only ever
    /// increments; `0` is unreachable through any public constructor.
    ZeroRevision,
    AutoTopupRequiresEnabledAndObserved,
}

/// Re-enforces the two invariants `TargetRecord`'s own constructors
/// guarantee but a struct-literal-style decode would otherwise bypass:
/// `revision >= 1`, and `auto_topup` implying `enabled` and an observed
/// mode. The registry-context checks `register`/`apply_patch` also perform
/// (duplicate/reserved principal, funding policy vs. the live
/// `GlobalPolicy`) need state this pure-domain type has no access to at
/// decode time, so they remain the caller's responsibility — the same
/// residual gap documented for `TargetFundingPolicy`. Every other field
/// (`display_name`, `project`, `tags`, `funding_policy`) is already
/// re-validated compositionally through its own custom `Deserialize` impl.
impl<'de> Deserialize<'de> for TargetRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            principal: Principal,
            revision: u64,
            display_name: BoundedName,
            project: BoundedName,
            environment: Environment,
            criticality: Criticality,
            observation_mode: ObservationMode,
            tags: BoundedTags,
            funding_policy: TargetFundingPolicy,
            enabled: bool,
            auto_topup: bool,
            paused: bool,
        }

        let raw = Raw::deserialize(deserializer)?;
        if raw.revision == 0 {
            return Err(invariant_decode_error(
                TargetRecordDecodeError::ZeroRevision,
            ));
        }
        if raw.auto_topup && (!raw.enabled || raw.observation_mode == ObservationMode::Unobserved) {
            return Err(invariant_decode_error(
                TargetRecordDecodeError::AutoTopupRequiresEnabledAndObserved,
            ));
        }
        Ok(Self {
            principal: raw.principal,
            revision: raw.revision,
            display_name: raw.display_name,
            project: raw.project,
            environment: raw.environment,
            criticality: raw.criticality,
            observation_mode: raw.observation_mode,
            tags: raw.tags,
            funding_policy: raw.funding_policy,
            enabled: raw.enabled,
            auto_topup: raw.auto_topup,
            paused: raw.paused,
        })
    }
}

// ─────────────────────── Samples ───────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    pub timestamp_secs: u64,
    pub balance: AdvisoryCyclesBalance,
    pub state: PublicTargetState,
    /// `None` when the interval contains an unknown funding operation and
    /// burn cannot be attributed.
    pub burn_cycles_per_hour: Option<u128>,
}

// ─────────────────────── Alarms ───────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlarmKind {
    BurnAnomaly,
    LowBalance,
    Unreachable,
    FundingQuarantined,
    SelfRecoveryUnresolved,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlarmStatus {
    Open,
    Acknowledged,
    Resolved,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Alarm {
    pub id: u64,
    pub target: Option<Principal>,
    pub kind: AlarmKind,
    pub status: AlarmStatus,
    pub opened_at_secs: u64,
    pub acknowledged_at_secs: Option<u64>,
    pub resolved_at_secs: Option<u64>,
}

// ─────────────────────── Governance proposals ───────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalKind {
    RegisterTarget,
    UpdateTarget,
    RemoveTarget,
    SetGlobalPolicy,
    AddSigner,
    RemoveSigner,
    SetSignerThreshold,
    UnpauseTarget,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum ProposalPayload {
    RegisterTarget(TargetArgs),
    UpdateTarget {
        principal: Principal,
        patch: TargetPatch,
    },
    RemoveTarget {
        principal: Principal,
    },
    SetGlobalPolicy(GlobalPolicyArgs),
    AddSigner {
        signer: Principal,
    },
    RemoveSigner {
        signer: Principal,
    },
    SetSignerThreshold {
        threshold: u32,
    },
    UnpauseTarget {
        principal: Principal,
    },
}

impl ProposalPayload {
    pub fn kind(&self) -> ProposalKind {
        match self {
            Self::RegisterTarget(_) => ProposalKind::RegisterTarget,
            Self::UpdateTarget { .. } => ProposalKind::UpdateTarget,
            Self::RemoveTarget { .. } => ProposalKind::RemoveTarget,
            Self::SetGlobalPolicy(_) => ProposalKind::SetGlobalPolicy,
            Self::AddSigner { .. } => ProposalKind::AddSigner,
            Self::RemoveSigner { .. } => ProposalKind::RemoveSigner,
            Self::SetSignerThreshold { .. } => ProposalKind::SetSignerThreshold,
            Self::UnpauseTarget { .. } => ProposalKind::UnpauseTarget,
        }
    }
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProposalStatus {
    Open,
    Executed,
    Cancelled,
}

#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ProposalRecord {
    pub id: u64,
    pub payload: ProposalPayload,
    pub proposer: Principal,
    approvals: Vec<Principal>,
    pub status: ProposalStatus,
    pub created_at_secs: u64,
}

#[derive(Debug)]
struct DuplicateApprovalError;

/// Re-enforces `record_approval`'s idempotency guarantee (no signer appears
/// twice in `approvals`) against the raw decoded vector, so a decoded
/// `ProposalRecord` can never carry a duplicate that would inflate
/// `approval_count()` for a future threshold check.
impl<'de> Deserialize<'de> for ProposalRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            id: u64,
            payload: ProposalPayload,
            proposer: Principal,
            approvals: Vec<Principal>,
            status: ProposalStatus,
            created_at_secs: u64,
        }

        let raw = Raw::deserialize(deserializer)?;
        let mut seen = BTreeSet::new();
        for approver in &raw.approvals {
            if !seen.insert(*approver) {
                return Err(invariant_decode_error(DuplicateApprovalError));
            }
        }
        Ok(Self {
            id: raw.id,
            payload: raw.payload,
            proposer: raw.proposer,
            approvals: raw.approvals,
            status: raw.status,
            created_at_secs: raw.created_at_secs,
        })
    }
}

impl ProposalRecord {
    pub fn new(
        id: u64,
        payload: ProposalPayload,
        proposer: Principal,
        created_at_secs: u64,
    ) -> Self {
        Self {
            id,
            payload,
            proposer,
            approvals: Vec::new(),
            status: ProposalStatus::Open,
            created_at_secs,
        }
    }

    pub fn kind(&self) -> ProposalKind {
        self.payload.kind()
    }

    pub fn approvals(&self) -> &[Principal] {
        &self.approvals
    }

    pub fn has_approved(&self, signer: Principal) -> bool {
        self.approvals.contains(&signer)
    }

    /// Idempotent: recording the same signer twice never applies the
    /// payload and never duplicates the approval. Returns whether the
    /// approval was newly recorded.
    pub fn record_approval(&mut self, signer: Principal) -> bool {
        if self.has_approved(signer) {
            false
        } else {
            self.approvals.push(signer);
            true
        }
    }

    pub fn approval_count(&self) -> usize {
        self.approvals.len()
    }
}

// ─────────────────────── Funding: rails and state machines ───────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FundingRail {
    CyclesLedger,
    IcpCmc,
}

/// What caused a `FundingOperation` to be opened.
#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FundingTrigger {
    /// Detected automatically against `low_balance_threshold_cycles`.
    LowBalanceAutoTopup,
    /// Requested directly by a signer.
    ManualTopup,
    /// Sentinel's own protected-reserve refill lane.
    SelfRecovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixedBytes32Error {
    WrongLength { actual: usize },
}

/// A bounded 32-byte value. Both ICRC-1 subaccounts and ICP
/// `AccountIdentifier`s are fixed-size 32-byte protocol values, so this one
/// newtype covers both. The only way to produce one is `new`, which rejects
/// anything but exactly 32 bytes, so a rail-argument snapshot persisted in
/// stable memory can never carry an oversized or malformed ledger byte
/// field.
#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedBytes32([u8; 32]);

impl FixedBytes32 {
    pub fn new(bytes: Vec<u8>) -> Result<Self, FixedBytes32Error> {
        let actual = bytes.len();
        let array: [u8; 32] = bytes
            .try_into()
            .map_err(|_| FixedBytes32Error::WrongLength { actual })?;
        Ok(Self(array))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.0.to_vec()
    }
}

/// The exact `icrc1_transfer`-style withdraw arguments sent to the Cycles
/// Ledger, snapshotted at the moment the operation is opened.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct CyclesWithdrawSnapshot {
    pub destination: Principal,
    pub from_subaccount: Option<FixedBytes32>,
    pub amount_cycles: u128,
    pub fee_cycles: u128,
    pub created_at_time_ns: u64,
}

/// The exact ICP transfer sent to the CMC top-up subaccount, snapshotted at
/// the moment the operation is opened. This is the *immutable* argument
/// snapshot only — it never carries the later-discovered confirmed block
/// index. That evidence lives on `FundingOperation::confirmed_block_index`,
/// a genuinely mutable field set exactly once via
/// `FundingOperation::attach_confirmed_block`, so reconciliation evidence
/// can never be mixed into the arguments a retry must reuse unchanged.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct IcpCmcSnapshot {
    pub source_subaccount: Option<FixedBytes32>,
    pub cmc_account_identifier: FixedBytes32,
    pub target_canister: Principal,
    pub amount_e8s: u64,
    pub fee_e8s: u64,
    pub memo: u64,
    pub created_at_time_ns: u64,
    pub rate_xdr_permyriad_per_icp: u64,
    pub rate_timestamp_secs: u64,
    pub expected_cycles: u128,
}

/// The rail-specific argument snapshot for a `FundingOperation`. Which
/// variant is present is the single source of truth for which rail an
/// operation uses.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum FundingRailArguments {
    Cycles(CyclesWithdrawSnapshot),
    Icp(IcpCmcSnapshot),
}

impl FundingRailArguments {
    pub fn rail(&self) -> FundingRail {
        match self {
            Self::Cycles(_) => FundingRail::CyclesLedger,
            Self::Icp(_) => FundingRail::IcpCmc,
        }
    }

    /// The cycle amount embedded in this rail's own snapshot
    /// (`CyclesWithdrawSnapshot::amount_cycles` or
    /// `IcpCmcSnapshot::expected_cycles`) — the single source of truth
    /// `FundingOperation::open` and its decode-time re-validation both
    /// check `reserved_amount_cycles` against.
    pub fn embedded_amount_cycles(&self) -> u128 {
        match self {
            Self::Cycles(snapshot) => snapshot.amount_cycles,
            Self::Icp(snapshot) => snapshot.expected_cycles,
        }
    }
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CyclesFundingState {
    PlannedReserved,
    Submitted,
    Confirmed,
    Unknown,
    Complete,
    Terminal,
    Quarantined,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum IcpFundingState {
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

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FundingOperationState {
    Cycles(CyclesFundingState),
    Icp(IcpFundingState),
}

impl FundingOperationState {
    pub fn rail(&self) -> FundingRail {
        match self {
            Self::Cycles(_) => FundingRail::CyclesLedger,
            Self::Icp(_) => FundingRail::IcpCmc,
        }
    }

    /// True once no further automatic attempt will ever be made. This is
    /// true for `Quarantined` too — but `Quarantined` still needs a signer
    /// to call `resolve_unknown_as_spent`/`attach_block_proof` before the
    /// operation is actually done, so it is deliberately **not** included
    /// in `is_resolved`.
    pub fn stops_automatic_retry(&self) -> bool {
        matches!(
            self,
            Self::Cycles(CyclesFundingState::Complete)
                | Self::Cycles(CyclesFundingState::Terminal)
                | Self::Cycles(CyclesFundingState::Quarantined)
                | Self::Icp(IcpFundingState::Complete)
                | Self::Icp(IcpFundingState::Refunded)
                | Self::Icp(IcpFundingState::Terminal)
                | Self::Icp(IcpFundingState::Quarantined)
        )
    }

    /// True only for states that are genuinely done: safe to release
    /// reserved capacity and prune. `Quarantined` and `Unknown`/
    /// `TransferUnknown` are never resolved — a quarantined operation stays
    /// reserved until a signer resolves it, and an unknown one stays
    /// reserved until reconciliation proves what happened.
    pub fn is_resolved(&self) -> bool {
        self.resolved_outcome().is_some()
    }

    /// The `FundingOutcome` a resolved state implies, or `None` for a state
    /// that is not yet resolved. This is the single source of truth for
    /// `TerminalFundingSummary::from_resolved`'s outcome — nothing else may
    /// supply an independent, potentially inconsistent `FundingOutcome`.
    pub fn resolved_outcome(&self) -> Option<FundingOutcome> {
        match self {
            Self::Cycles(CyclesFundingState::Complete) => Some(FundingOutcome::Completed),
            Self::Cycles(CyclesFundingState::Terminal) => Some(FundingOutcome::Terminal),
            Self::Icp(IcpFundingState::Complete) => Some(FundingOutcome::Completed),
            Self::Icp(IcpFundingState::Refunded) => Some(FundingOutcome::Refunded),
            Self::Icp(IcpFundingState::Terminal) => Some(FundingOutcome::Terminal),
            _ => None,
        }
    }

    /// Whether `next` is a legal successor of `self` in the rail's own
    /// lifecycle graph:
    ///
    /// - Cycles: `PlannedReserved -> Submitted -> {Confirmed, Unknown} ->
    ///   {Complete, Terminal, Quarantined}`.
    /// - ICP/CMC: `PlannedReserved -> LedgerSubmitted -> {TransferUnknown,
    ///   TransferConfirmed} -> NotifyPending -> {Complete, Refunded,
    ///   Terminal, Quarantined}`.
    ///
    /// A same-state transition is always legal for a state that has not
    /// stopped automatic retry (a retryable/indeterminate attempt that
    /// leaves the phase unchanged). Nothing is a legal successor of a state
    /// that has already stopped automatic retry — including itself — so a
    /// resolved or quarantined operation can never be mutated by a further
    /// `record_attempt`. This also rejects any cross-rail transition, since
    /// `self` and `next` can only match a table entry when both are the
    /// same rail.
    pub fn is_valid_successor(&self, next: &FundingOperationState) -> bool {
        if self.stops_automatic_retry() {
            return false;
        }
        if self == next {
            return true;
        }
        matches!(
            (self, next),
            (
                Self::Cycles(CyclesFundingState::PlannedReserved),
                Self::Cycles(CyclesFundingState::Submitted)
            ) | (
                Self::Cycles(CyclesFundingState::Submitted),
                Self::Cycles(CyclesFundingState::Confirmed)
            ) | (
                Self::Cycles(CyclesFundingState::Submitted),
                Self::Cycles(CyclesFundingState::Unknown)
            ) | (
                Self::Cycles(CyclesFundingState::Confirmed),
                Self::Cycles(CyclesFundingState::Complete)
            ) | (
                Self::Cycles(CyclesFundingState::Confirmed),
                Self::Cycles(CyclesFundingState::Terminal)
            ) | (
                Self::Cycles(CyclesFundingState::Confirmed),
                Self::Cycles(CyclesFundingState::Quarantined)
            ) | (
                Self::Cycles(CyclesFundingState::Unknown),
                Self::Cycles(CyclesFundingState::Complete)
            ) | (
                Self::Cycles(CyclesFundingState::Unknown),
                Self::Cycles(CyclesFundingState::Terminal)
            ) | (
                Self::Cycles(CyclesFundingState::Unknown),
                Self::Cycles(CyclesFundingState::Quarantined)
            ) | (
                Self::Icp(IcpFundingState::PlannedReserved),
                Self::Icp(IcpFundingState::LedgerSubmitted)
            ) | (
                Self::Icp(IcpFundingState::LedgerSubmitted),
                Self::Icp(IcpFundingState::TransferUnknown)
            ) | (
                Self::Icp(IcpFundingState::LedgerSubmitted),
                Self::Icp(IcpFundingState::TransferConfirmed)
            ) | (
                Self::Icp(IcpFundingState::TransferUnknown),
                Self::Icp(IcpFundingState::NotifyPending)
            ) | (
                Self::Icp(IcpFundingState::TransferConfirmed),
                Self::Icp(IcpFundingState::NotifyPending)
            ) | (
                Self::Icp(IcpFundingState::NotifyPending),
                Self::Icp(IcpFundingState::Complete)
            ) | (
                Self::Icp(IcpFundingState::NotifyPending),
                Self::Icp(IcpFundingState::Refunded)
            ) | (
                Self::Icp(IcpFundingState::NotifyPending),
                Self::Icp(IcpFundingState::Terminal)
            ) | (
                Self::Icp(IcpFundingState::NotifyPending),
                Self::Icp(IcpFundingState::Quarantined)
            )
        )
    }
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FundingAttemptResultClass {
    Success,
    RetryableFailure,
    TerminalFailure,
    Indeterminate,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct FundingAttemptRecord {
    pub ordinal: u32,
    pub phase: FundingOperationState,
    pub at_secs: u64,
    pub result_class: FundingAttemptResultClass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FundingAttemptsError {
    TooMany,
}

/// Bounded at `MAX_FUNDING_ATTEMPTS`: a runaway retry loop can never grow
/// this record without bound. Ordinals are assigned internally in order, so
/// callers can never create gaps or duplicates.
#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct FundingAttempts(Vec<FundingAttemptRecord>);

#[derive(Debug)]
enum FundingAttemptsDecodeError {
    TooMany,
    /// `record` always assigns `ordinal` as `len() + 1`, so a valid
    /// `FundingAttempts` always has ordinals `1..=len()` in order.
    OrdinalsNotSequential,
}

/// Re-enforces `record`'s bound and its ordinal-assignment invariant
/// against the raw decoded vector.
impl<'de> Deserialize<'de> for FundingAttempts {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let records = Vec::<FundingAttemptRecord>::deserialize(deserializer)?;
        if records.len() > MAX_FUNDING_ATTEMPTS {
            return Err(invariant_decode_error(FundingAttemptsDecodeError::TooMany));
        }
        for (index, record) in records.iter().enumerate() {
            if record.ordinal as usize != index + 1 {
                return Err(invariant_decode_error(
                    FundingAttemptsDecodeError::OrdinalsNotSequential,
                ));
            }
        }
        Ok(Self(records))
    }
}

impl FundingAttempts {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    pub fn as_slice(&self) -> &[FundingAttemptRecord] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn record(
        &self,
        phase: FundingOperationState,
        at_secs: u64,
        result_class: FundingAttemptResultClass,
    ) -> Result<Self, FundingAttemptsError> {
        if self.0.len() >= MAX_FUNDING_ATTEMPTS {
            return Err(FundingAttemptsError::TooMany);
        }
        let mut next = self.0.clone();
        next.push(FundingAttemptRecord {
            ordinal: next.len() as u32 + 1,
            phase,
            at_secs,
            result_class,
        });
        Ok(Self(next))
    }
}

impl Default for FundingAttempts {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FundingOperationOpenError {
    /// `reserved_amount_cycles` must exactly match the amount embedded in
    /// `rail_arguments` (`CyclesWithdrawSnapshot::amount_cycles` or
    /// `IcpCmcSnapshot::expected_cycles`) — otherwise the policy-capacity
    /// reservation and the actual rail call could disagree about how much
    /// is being spent.
    ReservedAmountMismatch,
}

/// Every field except `state`, `attempts`, `confirmed_block_index`, and
/// `updated_at_secs` is an immutable snapshot taken when the operation is
/// opened: `target_registry_revision` and `funding_policy` are the exact
/// registry state at that moment, so editing or removing the target
/// afterward cannot alter this operation's recipient or policy. There is no
/// public mutator for any snapshot field — `record_attempt` advances
/// `state`/`attempts`/`updated_at_secs` and carries every snapshot field
/// forward unchanged, and `attach_confirmed_block` is the only way to set
/// `confirmed_block_index`, exactly once.
#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct FundingOperation {
    id: u64,
    target: Principal,
    target_registry_revision: u64,
    funding_policy: TargetFundingPolicy,
    trigger: FundingTrigger,
    rail_arguments: FundingRailArguments,
    reserved_amount_cycles: u128,
    state: FundingOperationState,
    attempts: FundingAttempts,
    confirmed_block_index: Option<u64>,
    created_at_secs: u64,
    updated_at_secs: u64,
}

impl FundingOperation {
    /// Checked constructor: rejects a `reserved_amount_cycles` that
    /// disagrees with the amount embedded in `rail_arguments`.
    pub fn open(
        id: u64,
        target: Principal,
        target_registry_revision: u64,
        funding_policy: TargetFundingPolicy,
        trigger: FundingTrigger,
        rail_arguments: FundingRailArguments,
        reserved_amount_cycles: u128,
        now_secs: u64,
    ) -> Result<Self, FundingOperationOpenError> {
        let state = match &rail_arguments {
            FundingRailArguments::Cycles(_) => {
                FundingOperationState::Cycles(CyclesFundingState::PlannedReserved)
            }
            FundingRailArguments::Icp(_) => {
                FundingOperationState::Icp(IcpFundingState::PlannedReserved)
            }
        };
        if reserved_amount_cycles != rail_arguments.embedded_amount_cycles() {
            return Err(FundingOperationOpenError::ReservedAmountMismatch);
        }
        Ok(Self {
            id,
            target,
            target_registry_revision,
            funding_policy,
            trigger,
            rail_arguments,
            reserved_amount_cycles,
            state,
            attempts: FundingAttempts::new(),
            confirmed_block_index: None,
            created_at_secs: now_secs,
            updated_at_secs: now_secs,
        })
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn target(&self) -> Principal {
        self.target
    }

    pub fn target_registry_revision(&self) -> u64 {
        self.target_registry_revision
    }

    pub fn funding_policy(&self) -> &TargetFundingPolicy {
        &self.funding_policy
    }

    pub fn trigger(&self) -> FundingTrigger {
        self.trigger
    }

    pub fn rail_arguments(&self) -> &FundingRailArguments {
        &self.rail_arguments
    }

    pub fn rail(&self) -> FundingRail {
        self.rail_arguments.rail()
    }

    pub fn reserved_amount_cycles(&self) -> u128 {
        self.reserved_amount_cycles
    }

    pub fn state(&self) -> FundingOperationState {
        self.state
    }

    pub fn attempts(&self) -> &FundingAttempts {
        &self.attempts
    }

    pub fn confirmed_block_index(&self) -> Option<u64> {
        self.confirmed_block_index
    }

    pub fn created_at_secs(&self) -> u64 {
        self.created_at_secs
    }

    pub fn updated_at_secs(&self) -> u64 {
        self.updated_at_secs
    }

    /// Advances `state` and appends a bounded attempt record, after
    /// checking that `new_state` is on the same rail as `rail_arguments`
    /// and is a legal successor of the current state. Every snapshot field,
    /// including `confirmed_block_index`, is carried forward unchanged.
    pub fn record_attempt(
        &self,
        new_state: FundingOperationState,
        at_secs: u64,
        result_class: FundingAttemptResultClass,
    ) -> Result<Self, FundingOperationTransitionError> {
        if at_secs < self.updated_at_secs {
            return Err(FundingOperationTransitionError::TimestampRegression);
        }
        if new_state.rail() != self.rail() {
            return Err(FundingOperationTransitionError::RailMismatch);
        }
        if !self.state.is_valid_successor(&new_state) {
            return Err(FundingOperationTransitionError::InvalidSuccessor);
        }
        let attempts = self
            .attempts
            .record(new_state, at_secs, result_class)
            .map_err(|_| FundingOperationTransitionError::TooManyAttempts)?;
        Ok(Self {
            state: new_state,
            attempts,
            updated_at_secs: at_secs,
            ..self.clone()
        })
    }

    /// Records the confirmed ICP ledger block that reconciliation proved
    /// matches this operation's immutable `IcpCmcSnapshot` arguments.
    /// Single-assignment: fails if a block index is already recorded, so
    /// the confirmed evidence can never be silently overwritten by a later
    /// call. Only meaningful for the ICP rail.
    pub fn attach_confirmed_block(
        &self,
        block_index: u64,
    ) -> Result<Self, AttachConfirmedBlockError> {
        if self.rail() != FundingRail::IcpCmc {
            return Err(AttachConfirmedBlockError::WrongRail);
        }
        if self.confirmed_block_index.is_some() {
            return Err(AttachConfirmedBlockError::AlreadySet);
        }
        Ok(Self {
            confirmed_block_index: Some(block_index),
            ..self.clone()
        })
    }
}

#[derive(Debug)]
enum FundingOperationHistoryDecodeError {
    /// `attempts` is empty, so `state` must still be the rail's own initial
    /// `PlannedReserved` variant — exactly what `open` sets before any
    /// `record_attempt` call has ever been made.
    StateNotInitialWithEmptyAttempts,
    /// `attempts` is empty, so `updated_at_secs` must still equal
    /// `created_at_secs` — `open` sets both fields to the same `now_secs`.
    UpdatedAtNotCreatedAtWithEmptyAttempts,
    /// `attempts` is nonempty, so its last entry's `phase`/`at_secs` must
    /// equal `state`/`updated_at_secs` — exactly what `record_attempt` sets
    /// in the same step, from the same `new_state`/`at_secs` values.
    FinalAttemptInconsistentWithSummary,
    /// An attempt's `at_secs` is before the previous attempt's `at_secs` (or,
    /// for the first attempt, before `created_at_secs`) — `record_attempt`
    /// rejects `TimestampRegression` before ever appending, so a legitimate
    /// history's timestamps are always non-decreasing from `created_at_secs`
    /// onward.
    AttemptTimestampRegression,
    /// An attempt's `phase` is not a legal `is_valid_successor` edge from the
    /// previous phase (the rail's own initial `PlannedReserved` state is the
    /// implicit predecessor of the first attempt) — `record_attempt` rejects
    /// `InvalidSuccessor` before ever appending. Since `is_valid_successor`
    /// only ever matches same-rail table entries (or an identical state),
    /// this also rejects any attempt phase on a different rail than the
    /// operation's own `rail_arguments`.
    InvalidAttemptSuccessor,
}

/// Re-enforces every invariant checkable from the operation's own fields
/// alone: `state`'s rail matches `rail_arguments`'s rail (the check
/// `record_attempt` performs on every transition), `reserved_amount_cycles`
/// matches `rail_arguments`'s embedded amount (the check `open` performs),
/// `confirmed_block_index` is only ever present on the ICP rail (the check
/// `attach_confirmed_block` performs), and — the invariant a bare derive
/// used to miss entirely — that `state`, `attempts`, and `updated_at_secs`
/// are mutually consistent exactly the way `open`/`record_attempt` guarantee
/// for any legitimately-constructed value: empty `attempts` implies `state`
/// is still the rail's initial `PlannedReserved` and `updated_at_secs ==
/// created_at_secs`; nonempty `attempts` implies its last entry's
/// `phase`/`at_secs` equal `state`/`updated_at_secs`, every consecutive
/// phase transition (including the implicit first transition from the
/// rail's initial `PlannedReserved` state) is a legal `is_valid_successor`
/// edge, and every attempt's timestamp is non-decreasing from
/// `created_at_secs` onward. `funding_policy` and `attempts`'s own
/// per-entry bounds are already re-validated compositionally through their
/// own custom `Deserialize` impls; this impl adds the cross-field
/// consistency between `state`/`attempts`/`updated_at_secs` that no single
/// field's own impl can see in isolation. There is still no way to
/// re-derive "does `state` truthfully match what a real reconciliation
/// produced" purely from a decoded snapshot — only that the value is
/// *internally* self-consistent, i.e. reachable by some legal sequence of
/// `open`/`record_attempt` calls. That remains `state.rs`'s responsibility
/// (see the module doc comment and Part 5 of
/// `.superpowers/sdd/2026-09-13-cycle-sentinel-telemetry/task-1b-decode-review.md`).
impl<'de> Deserialize<'de> for FundingOperation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            id: u64,
            target: Principal,
            target_registry_revision: u64,
            funding_policy: TargetFundingPolicy,
            trigger: FundingTrigger,
            rail_arguments: FundingRailArguments,
            reserved_amount_cycles: u128,
            state: FundingOperationState,
            attempts: FundingAttempts,
            confirmed_block_index: Option<u64>,
            created_at_secs: u64,
            updated_at_secs: u64,
        }

        let raw = Raw::deserialize(deserializer)?;
        if raw.state.rail() != raw.rail_arguments.rail() {
            return Err(invariant_decode_error(
                FundingOperationTransitionError::RailMismatch,
            ));
        }
        if raw.reserved_amount_cycles != raw.rail_arguments.embedded_amount_cycles() {
            return Err(invariant_decode_error(
                FundingOperationOpenError::ReservedAmountMismatch,
            ));
        }
        if raw.confirmed_block_index.is_some() && raw.rail_arguments.rail() != FundingRail::IcpCmc {
            return Err(invariant_decode_error(AttachConfirmedBlockError::WrongRail));
        }

        let initial_state = match &raw.rail_arguments {
            FundingRailArguments::Cycles(_) => {
                FundingOperationState::Cycles(CyclesFundingState::PlannedReserved)
            }
            FundingRailArguments::Icp(_) => {
                FundingOperationState::Icp(IcpFundingState::PlannedReserved)
            }
        };
        let attempts = raw.attempts.as_slice();
        if attempts.is_empty() {
            if raw.state != initial_state {
                return Err(invariant_decode_error(
                    FundingOperationHistoryDecodeError::StateNotInitialWithEmptyAttempts,
                ));
            }
            if raw.updated_at_secs != raw.created_at_secs {
                return Err(invariant_decode_error(
                    FundingOperationHistoryDecodeError::UpdatedAtNotCreatedAtWithEmptyAttempts,
                ));
            }
        } else {
            let last = attempts.last().expect("checked non-empty above");
            if last.phase != raw.state || last.at_secs != raw.updated_at_secs {
                return Err(invariant_decode_error(
                    FundingOperationHistoryDecodeError::FinalAttemptInconsistentWithSummary,
                ));
            }
            let mut previous_phase = initial_state;
            let mut previous_at_secs = raw.created_at_secs;
            for attempt in attempts {
                if !previous_phase.is_valid_successor(&attempt.phase) {
                    return Err(invariant_decode_error(
                        FundingOperationHistoryDecodeError::InvalidAttemptSuccessor,
                    ));
                }
                if attempt.at_secs < previous_at_secs {
                    return Err(invariant_decode_error(
                        FundingOperationHistoryDecodeError::AttemptTimestampRegression,
                    ));
                }
                previous_phase = attempt.phase;
                previous_at_secs = attempt.at_secs;
            }
        }

        Ok(Self {
            id: raw.id,
            target: raw.target,
            target_registry_revision: raw.target_registry_revision,
            funding_policy: raw.funding_policy,
            trigger: raw.trigger,
            rail_arguments: raw.rail_arguments,
            reserved_amount_cycles: raw.reserved_amount_cycles,
            state: raw.state,
            attempts: raw.attempts,
            confirmed_block_index: raw.confirmed_block_index,
            created_at_secs: raw.created_at_secs,
            updated_at_secs: raw.updated_at_secs,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FundingOperationTransitionError {
    /// `at_secs` is strictly before this operation's own `updated_at_secs`,
    /// which would make the attempt history's timestamps non-monotonic.
    /// This is distinct from — and does not replace — `state.rs`'s
    /// separate responsibility for a single globally monotonic
    /// `created_at_time` across all operations.
    TimestampRegression,
    RailMismatch,
    InvalidSuccessor,
    TooManyAttempts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachConfirmedBlockError {
    WrongRail,
    AlreadySet,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FundingOutcome {
    Completed,
    Refunded,
    Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalFundingSummaryError {
    OperationNotResolved,
}

/// Only constructible via `from_resolved`, which derives `outcome` from
/// `operation.state().resolved_outcome()` — there is no way to construct a
/// summary whose `outcome` disagrees with the operation's actual resolved
/// state. A `Quarantined` (stopped-but-unresolved) operation can never
/// reach the prunable terminal-summary ring while still awaiting signer
/// reconciliation, since `resolved_outcome()` is `None` for it.
#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct TerminalFundingSummary {
    operation_id: u64,
    target: Principal,
    rail: FundingRail,
    outcome: FundingOutcome,
    amount_cycles: u128,
    resolved_at_secs: u64,
}

#[derive(Debug)]
struct RefundedOnNonIcpRailError;

/// Re-enforces the one invariant checkable from a summary's own fields
/// alone: only the ICP rail can produce `FundingOutcome::Refunded`
/// (`Cycles` has no `Refunded`-equivalent state at all). Whether `outcome`
/// actually matches the real operation it was derived from cannot be
/// re-checked here — that guarantee comes from `from_resolved` deriving it
/// via `FundingOperationState::resolved_outcome`, not from anything
/// expressible as a self-contained check on the summary alone.
impl<'de> Deserialize<'de> for TerminalFundingSummary {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            operation_id: u64,
            target: Principal,
            rail: FundingRail,
            outcome: FundingOutcome,
            amount_cycles: u128,
            resolved_at_secs: u64,
        }

        let raw = Raw::deserialize(deserializer)?;
        if raw.outcome == FundingOutcome::Refunded && raw.rail != FundingRail::IcpCmc {
            return Err(invariant_decode_error(RefundedOnNonIcpRailError));
        }
        Ok(Self {
            operation_id: raw.operation_id,
            target: raw.target,
            rail: raw.rail,
            outcome: raw.outcome,
            amount_cycles: raw.amount_cycles,
            resolved_at_secs: raw.resolved_at_secs,
        })
    }
}

impl TerminalFundingSummary {
    pub fn from_resolved(
        operation: &FundingOperation,
        resolved_at_secs: u64,
    ) -> Result<Self, TerminalFundingSummaryError> {
        let outcome = operation
            .state()
            .resolved_outcome()
            .ok_or(TerminalFundingSummaryError::OperationNotResolved)?;
        Ok(Self {
            operation_id: operation.id(),
            target: operation.target(),
            rail: operation.rail(),
            outcome,
            amount_cycles: operation.reserved_amount_cycles(),
            resolved_at_secs,
        })
    }

    pub fn operation_id(&self) -> u64 {
        self.operation_id
    }

    pub fn target(&self) -> Principal {
        self.target
    }

    pub fn rail(&self) -> FundingRail {
        self.rail
    }

    pub fn outcome(&self) -> FundingOutcome {
        self.outcome
    }

    pub fn amount_cycles(&self) -> u128 {
        self.amount_cycles
    }

    pub fn resolved_at_secs(&self) -> u64 {
        self.resolved_at_secs
    }
}

// ─────────────────────── Reservation and rolling spend ───────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpendEntry {
    pub settled_at_secs: u64,
    pub amount_cycles: u128,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingReservation {
    pub operation_id: u64,
    pub amount_cycles: u128,
    pub reserved_at_secs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollingSpendReserveError {
    Overflow,
    CapExceeded,
    DuplicateOperation,
    Bound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollingSpendSettleError {
    UnknownOperation,
    /// Settling this entry would leave more than
    /// `MAX_ROLLING_SPEND_SETTLED_ENTRIES` entries still inside the window
    /// even after pruning every out-of-window entry. Fails closed instead
    /// of silently evicting real in-window spend, which would let the
    /// rolling cap undercount genuine spend.
    Bound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollingSpendReleaseError {
    UnknownOperation,
}

/// A true sliding-window 24h spend ledger — not a fixed reset-at-boundary
/// bucket. `settled` holds one timestamped entry per confirmed spend;
/// `pending` holds one entry per in-flight reservation, keyed by operation
/// id. Both are bounded so a caller can never grow either list without
/// limit.
#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct RollingSpendLedger {
    settled: Vec<SpendEntry>,
    pending: Vec<PendingReservation>,
}

#[derive(Debug)]
enum RollingSpendLedgerDecodeError {
    TooManySettled,
    TooManyPending,
    DuplicatePendingOperation,
}

/// Re-enforces every bound the ledger's own mutators (`reserve`, `settle`)
/// maintain: `settled.len() <= MAX_ROLLING_SPEND_SETTLED_ENTRIES`,
/// `pending.len() <= MAX_PENDING_RESERVATIONS`, and no duplicate
/// `operation_id` among `pending` entries.
impl<'de> Deserialize<'de> for RollingSpendLedger {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            settled: Vec<SpendEntry>,
            pending: Vec<PendingReservation>,
        }

        let raw = Raw::deserialize(deserializer)?;
        if raw.settled.len() > MAX_ROLLING_SPEND_SETTLED_ENTRIES {
            return Err(invariant_decode_error(
                RollingSpendLedgerDecodeError::TooManySettled,
            ));
        }
        if raw.pending.len() > MAX_PENDING_RESERVATIONS {
            return Err(invariant_decode_error(
                RollingSpendLedgerDecodeError::TooManyPending,
            ));
        }
        let mut seen = BTreeSet::new();
        for reservation in &raw.pending {
            if !seen.insert(reservation.operation_id) {
                return Err(invariant_decode_error(
                    RollingSpendLedgerDecodeError::DuplicatePendingOperation,
                ));
            }
        }
        Ok(Self {
            settled: raw.settled,
            pending: raw.pending,
        })
    }
}

impl RollingSpendLedger {
    pub fn new() -> Self {
        Self {
            settled: Vec::new(),
            pending: Vec::new(),
        }
    }

    pub fn settled(&self) -> &[SpendEntry] {
        &self.settled
    }

    pub fn pending(&self) -> &[PendingReservation] {
        &self.pending
    }

    pub fn pending_operation_id(&self) -> Option<u64> {
        self.pending.first().map(|p| p.operation_id)
    }

    pub fn pending_cycles(&self) -> u128 {
        self.pending
            .iter()
            .fold(0u128, |acc, p| acc.saturating_add(p.amount_cycles))
    }

    /// Prunes `settled` entries at or past `window_secs` old as of
    /// `now_secs`, then reserves `amount_cycles` against `cap_cycles`
    /// counting settled-in-window plus all pending (in-flight) amounts.
    pub fn reserve(
        &self,
        operation_id: u64,
        amount_cycles: u128,
        now_secs: u64,
        window_secs: u64,
        cap_cycles: u128,
    ) -> Result<Self, RollingSpendReserveError> {
        if self.pending.iter().any(|p| p.operation_id == operation_id) {
            return Err(RollingSpendReserveError::DuplicateOperation);
        }
        if self.pending.len() >= MAX_PENDING_RESERVATIONS {
            return Err(RollingSpendReserveError::Bound);
        }
        let settled: Vec<SpendEntry> = self
            .settled
            .iter()
            .copied()
            .filter(|entry| now_secs.saturating_sub(entry.settled_at_secs) < window_secs)
            .collect();
        let settled_sum = settled
            .iter()
            .try_fold(0u128, |acc, e| acc.checked_add(e.amount_cycles))
            .ok_or(RollingSpendReserveError::Overflow)?;
        let pending_sum = self
            .pending
            .iter()
            .try_fold(0u128, |acc, p| acc.checked_add(p.amount_cycles))
            .ok_or(RollingSpendReserveError::Overflow)?;
        let committed = settled_sum
            .checked_add(pending_sum)
            .ok_or(RollingSpendReserveError::Overflow)?;
        let total = committed
            .checked_add(amount_cycles)
            .ok_or(RollingSpendReserveError::Overflow)?;
        if total > cap_cycles {
            return Err(RollingSpendReserveError::CapExceeded);
        }
        let mut pending = self.pending.clone();
        pending.push(PendingReservation {
            operation_id,
            amount_cycles,
            reserved_at_secs: now_secs,
        });
        Ok(Self { settled, pending })
    }

    /// Moves the matching pending reservation to a timestamped settled
    /// entry, then prunes any settled entry at or past `window_secs` old —
    /// the same predicate `reserve` uses. Fails if `operation_id` has no
    /// matching pending reservation, or if more than
    /// `MAX_ROLLING_SPEND_SETTLED_ENTRIES` entries remain inside the window
    /// after pruning: an in-window entry is never evicted just to satisfy
    /// the bound, since doing so would make the rolling cap undercount real
    /// spend.
    pub fn settle(
        &self,
        operation_id: u64,
        settled_at_secs: u64,
        window_secs: u64,
    ) -> Result<Self, RollingSpendSettleError> {
        let index = self
            .pending
            .iter()
            .position(|p| p.operation_id == operation_id)
            .ok_or(RollingSpendSettleError::UnknownOperation)?;
        let mut pending = self.pending.clone();
        let matched = pending.remove(index);
        let mut settled = self.settled.clone();
        settled.push(SpendEntry {
            settled_at_secs,
            amount_cycles: matched.amount_cycles,
        });
        settled.retain(|entry| settled_at_secs.saturating_sub(entry.settled_at_secs) < window_secs);
        if settled.len() > MAX_ROLLING_SPEND_SETTLED_ENTRIES {
            return Err(RollingSpendSettleError::Bound);
        }
        Ok(Self { settled, pending })
    }

    /// Removes only the matching pending entry — used when a reservation is
    /// proven to have moved no funds, so it must never create a settled
    /// entry or otherwise count against the cap.
    pub fn release_no_spend(&self, operation_id: u64) -> Result<Self, RollingSpendReleaseError> {
        let index = self
            .pending
            .iter()
            .position(|p| p.operation_id == operation_id)
            .ok_or(RollingSpendReleaseError::UnknownOperation)?;
        let mut pending = self.pending.clone();
        pending.remove(index);
        Ok(Self {
            settled: self.settled.clone(),
            pending,
        })
    }
}

impl Default for RollingSpendLedger {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetReservationError {
    OperationInFlight,
    Cooldown,
    Overflow,
    CapExceeded,
    DuplicateOperation,
    Bound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetSettleError {
    Mismatch,
    /// Propagated from `RollingSpendLedger::settle`'s `Bound`: the settled
    /// window would hold more than `MAX_ROLLING_SPEND_SETTLED_ENTRIES`
    /// in-window entries. Fails closed rather than silently dropping real
    /// spend from the cap.
    Bound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetReleaseError {
    Mismatch,
}

/// Per-target reservation state (stable memory ID 10). At most one pending
/// reservation ever exists in `rolling_spend`: `reserve` rejects a second
/// attempt via `OperationInFlight` before it ever reaches the ledger, so
/// "one unresolved operation per target" is an invariant, not a convention
/// callers have to remember.
#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct TargetReservationState {
    rolling_spend: RollingSpendLedger,
    cooldown_until_secs: u64,
}

#[derive(Debug)]
struct MoreThanOnePendingReservationError;

/// The nested `rolling_spend: RollingSpendLedger` field is already
/// re-validated compositionally through `RollingSpendLedger`'s own custom
/// `Deserialize` impl. This impl adds the one invariant that is specific to
/// a *per-target* ledger and not to `RollingSpendLedger` in general (which
/// `GlobalRollingSpendState` also wraps, where many pending entries are
/// expected): at most one pending reservation, the same guarantee `reserve`
/// enforces via `OperationInFlight` before ever reaching the ledger.
impl<'de> Deserialize<'de> for TargetReservationState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            rolling_spend: RollingSpendLedger,
            cooldown_until_secs: u64,
        }

        let raw = Raw::deserialize(deserializer)?;
        if raw.rolling_spend.pending().len() > 1 {
            return Err(invariant_decode_error(MoreThanOnePendingReservationError));
        }
        Ok(Self {
            rolling_spend: raw.rolling_spend,
            cooldown_until_secs: raw.cooldown_until_secs,
        })
    }
}

impl TargetReservationState {
    pub fn new() -> Self {
        Self {
            rolling_spend: RollingSpendLedger::new(),
            cooldown_until_secs: 0,
        }
    }

    pub fn in_flight_operation_id(&self) -> Option<u64> {
        self.rolling_spend.pending_operation_id()
    }

    pub fn in_flight_amount_cycles(&self) -> Option<u128> {
        self.rolling_spend
            .pending()
            .first()
            .map(|p| p.amount_cycles)
    }

    pub fn rolling_spend(&self) -> &RollingSpendLedger {
        &self.rolling_spend
    }

    pub fn is_on_cooldown(&self, now_secs: u64) -> bool {
        now_secs < self.cooldown_until_secs
    }

    pub fn is_available(&self, now_secs: u64) -> bool {
        self.in_flight_operation_id().is_none() && !self.is_on_cooldown(now_secs)
    }

    /// Reserves pending capacity before any await. `in-flight` and
    /// `cooldown` are checked here, ahead of the ledger's own
    /// overflow/cap/duplicate/bound checks.
    pub fn reserve(
        &self,
        operation_id: u64,
        amount_cycles: u128,
        now_secs: u64,
        window_secs: u64,
        cap_cycles: u128,
    ) -> Result<Self, TargetReservationError> {
        if self.in_flight_operation_id().is_some() {
            return Err(TargetReservationError::OperationInFlight);
        }
        if self.is_on_cooldown(now_secs) {
            return Err(TargetReservationError::Cooldown);
        }
        let rolling_spend = self
            .rolling_spend
            .reserve(
                operation_id,
                amount_cycles,
                now_secs,
                window_secs,
                cap_cycles,
            )
            .map_err(|err| match err {
                RollingSpendReserveError::Overflow => TargetReservationError::Overflow,
                RollingSpendReserveError::CapExceeded => TargetReservationError::CapExceeded,
                RollingSpendReserveError::DuplicateOperation => {
                    TargetReservationError::DuplicateOperation
                }
                RollingSpendReserveError::Bound => TargetReservationError::Bound,
            })?;
        Ok(Self {
            rolling_spend,
            cooldown_until_secs: self.cooldown_until_secs,
        })
    }

    /// Confirmed spend: moves the reservation to a settled ledger entry and
    /// starts the cooldown from `now_secs`. `reserve` never sets cooldown.
    pub fn settle_spend(
        &self,
        operation_id: u64,
        now_secs: u64,
        cooldown_secs: u64,
        window_secs: u64,
    ) -> Result<Self, TargetSettleError> {
        let rolling_spend = self
            .rolling_spend
            .settle(operation_id, now_secs, window_secs)
            .map_err(|err| match err {
                RollingSpendSettleError::UnknownOperation => TargetSettleError::Mismatch,
                RollingSpendSettleError::Bound => TargetSettleError::Bound,
            })?;
        Ok(Self {
            rolling_spend,
            cooldown_until_secs: now_secs.saturating_add(cooldown_secs),
        })
    }

    /// Proven no-spend: releases only the matching pending reservation and
    /// leaves the prior cooldown untouched — a no-spend release must never
    /// look like a completed spend for cooldown purposes.
    pub fn release_no_spend(&self, operation_id: u64) -> Result<Self, TargetReleaseError> {
        let rolling_spend = self
            .rolling_spend
            .release_no_spend(operation_id)
            .map_err(|_| TargetReleaseError::Mismatch)?;
        Ok(Self {
            rolling_spend,
            cooldown_until_secs: self.cooldown_until_secs,
        })
    }
}

impl Default for TargetReservationState {
    fn default() -> Self {
        Self::new()
    }
}

/// Global rolling spend state (stable memory ID 11). Unlike
/// `TargetReservationState`, many targets' reservations can be pending here
/// at once — each keyed by its own operation id — since the global cap is
/// shared across the whole registry rather than one-per-target.
#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct GlobalRollingSpendState {
    rolling_spend: RollingSpendLedger,
}

/// Unlike `TargetReservationState`, many pending entries are legitimate
/// here (each target's own in-flight operation), so there is no additional
/// invariant beyond what `RollingSpendLedger`'s own custom `Deserialize`
/// impl already re-enforces on the nested field. Implemented explicitly
/// (rather than left derived) so this type is not silently relying on the
/// derive that Finding G showed bypasses validation elsewhere in this file.
impl<'de> Deserialize<'de> for GlobalRollingSpendState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            rolling_spend: RollingSpendLedger,
        }

        let raw = Raw::deserialize(deserializer)?;
        Ok(Self {
            rolling_spend: raw.rolling_spend,
        })
    }
}

impl GlobalRollingSpendState {
    pub fn new() -> Self {
        Self {
            rolling_spend: RollingSpendLedger::new(),
        }
    }

    pub fn rolling_spend(&self) -> &RollingSpendLedger {
        &self.rolling_spend
    }

    pub fn reserve(
        &self,
        operation_id: u64,
        amount_cycles: u128,
        now_secs: u64,
        window_secs: u64,
        cap_cycles: u128,
    ) -> Result<Self, RollingSpendReserveError> {
        Ok(Self {
            rolling_spend: self.rolling_spend.reserve(
                operation_id,
                amount_cycles,
                now_secs,
                window_secs,
                cap_cycles,
            )?,
        })
    }

    pub fn settle(
        &self,
        operation_id: u64,
        now_secs: u64,
        window_secs: u64,
    ) -> Result<Self, RollingSpendSettleError> {
        Ok(Self {
            rolling_spend: self
                .rolling_spend
                .settle(operation_id, now_secs, window_secs)?,
        })
    }

    pub fn release_no_spend(&self, operation_id: u64) -> Result<Self, RollingSpendReleaseError> {
        Ok(Self {
            rolling_spend: self.rolling_spend.release_no_spend(operation_id)?,
        })
    }
}

impl Default for GlobalRollingSpendState {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────── Self-recovery ───────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct SelfRecoveryPolicyArgs {
    pub protected_reserve_cycles: Nat,
    pub daily_cap_cycles: Nat,
    pub low_balance_threshold_cycles: Nat,
    pub refill_cycles: Nat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfRecoveryPolicyError {
    CyclesValueOverflow,
    ZeroLowBalanceThreshold,
    ZeroDailyCap,
    ZeroRefillCycles,
    DailyCapBelowRefill,
    ProtectedReserveBelowRefill,
}

/// Sentinel's self-recovery lane is hard-coded to fund only `ic_cdk::id()`
/// and its protected reserve is never available to ordinary targets; this
/// policy is validated and stored completely separately from
/// `TargetFundingPolicy` and `GlobalPolicy`.
#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct SelfRecoveryPolicy {
    protected_reserve_cycles: u128,
    daily_cap_cycles: u128,
    low_balance_threshold_cycles: u128,
    refill_cycles: u128,
}

/// `SelfRecoveryPolicyArgs` has the identical field set, so decoding
/// through it and re-running `validate` re-enforces every bound without
/// changing the wire shape.
impl<'de> Deserialize<'de> for SelfRecoveryPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let args = SelfRecoveryPolicyArgs::deserialize(deserializer)?;
        Self::validate(&args).map_err(invariant_decode_error)
    }
}

impl SelfRecoveryPolicy {
    pub fn validate(args: &SelfRecoveryPolicyArgs) -> Result<Self, SelfRecoveryPolicyError> {
        let protected_reserve_cycles = checked_cycles_from_nat(&args.protected_reserve_cycles)
            .ok_or(SelfRecoveryPolicyError::CyclesValueOverflow)?;
        let daily_cap_cycles = checked_cycles_from_nat(&args.daily_cap_cycles)
            .ok_or(SelfRecoveryPolicyError::CyclesValueOverflow)?;
        let low_balance_threshold_cycles =
            checked_cycles_from_nat(&args.low_balance_threshold_cycles)
                .ok_or(SelfRecoveryPolicyError::CyclesValueOverflow)?;
        let refill_cycles = checked_cycles_from_nat(&args.refill_cycles)
            .ok_or(SelfRecoveryPolicyError::CyclesValueOverflow)?;

        if low_balance_threshold_cycles == 0 {
            return Err(SelfRecoveryPolicyError::ZeroLowBalanceThreshold);
        }
        if daily_cap_cycles == 0 {
            return Err(SelfRecoveryPolicyError::ZeroDailyCap);
        }
        if refill_cycles == 0 {
            return Err(SelfRecoveryPolicyError::ZeroRefillCycles);
        }
        if daily_cap_cycles < refill_cycles {
            return Err(SelfRecoveryPolicyError::DailyCapBelowRefill);
        }
        if protected_reserve_cycles < refill_cycles {
            return Err(SelfRecoveryPolicyError::ProtectedReserveBelowRefill);
        }

        Ok(Self {
            protected_reserve_cycles,
            daily_cap_cycles,
            low_balance_threshold_cycles,
            refill_cycles,
        })
    }

    pub fn protected_reserve_cycles(&self) -> u128 {
        self.protected_reserve_cycles
    }

    pub fn daily_cap_cycles(&self) -> u128 {
        self.daily_cap_cycles
    }

    pub fn low_balance_threshold_cycles(&self) -> u128 {
        self.low_balance_threshold_cycles
    }

    pub fn refill_cycles(&self) -> u128 {
        self.refill_cycles
    }

    pub fn to_args(&self) -> SelfRecoveryPolicyArgs {
        SelfRecoveryPolicyArgs {
            protected_reserve_cycles: Nat::from(self.protected_reserve_cycles),
            daily_cap_cycles: Nat::from(self.daily_cap_cycles),
            low_balance_threshold_cycles: Nat::from(self.low_balance_threshold_cycles),
            refill_cycles: Nat::from(self.refill_cycles),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfRecoveryStateError {
    AlreadyInFlight,
}

/// Stable memory ID 12. While an operation is in flight, ordinary target
/// distribution must be suppressed entirely.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct SelfRecoveryState {
    in_flight_operation_id: Option<u64>,
    last_recovery_at_secs: Option<u64>,
}

impl SelfRecoveryState {
    pub fn new() -> Self {
        Self {
            in_flight_operation_id: None,
            last_recovery_at_secs: None,
        }
    }

    pub fn in_flight_operation_id(&self) -> Option<u64> {
        self.in_flight_operation_id
    }

    pub fn last_recovery_at_secs(&self) -> Option<u64> {
        self.last_recovery_at_secs
    }

    pub fn is_suppressing_distribution(&self) -> bool {
        self.in_flight_operation_id.is_some()
    }

    pub fn begin(&self, operation_id: u64) -> Result<Self, SelfRecoveryStateError> {
        if self.in_flight_operation_id.is_some() {
            return Err(SelfRecoveryStateError::AlreadyInFlight);
        }
        Ok(Self {
            in_flight_operation_id: Some(operation_id),
            last_recovery_at_secs: self.last_recovery_at_secs,
        })
    }

    pub fn complete(&self, completed_at_secs: u64) -> Self {
        Self {
            in_flight_operation_id: None,
            last_recovery_at_secs: Some(completed_at_secs),
        }
    }
}

impl Default for SelfRecoveryState {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────── Public read models ───────────────────────

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct PublicOverview {
    pub target_count: u64,
    pub healthy_count: u64,
    pub low_count: u64,
    pub stopped_count: u64,
    pub uninstalled_count: u64,
    pub unreachable_count: u64,
    pub unobserved_count: u64,
    pub total_observed_cycles: Nat,
    pub runtime_cycles: Nat,
    pub cycles_ledger_available_cycles: Nat,
    pub icp_available_e8s: Nat,
    pub protected_self_reserve_cycles: Nat,
    pub alarm_count: u64,
    pub last_sample_at_secs: Option<u64>,
    pub next_sample_at_secs: Option<u64>,
}

/// A public-safe projection of `TerminalFundingSummary`: no signer
/// principals, ledger subaccounts, or reconciliation evidence, and the
/// target principal is dropped since it is already the enclosing
/// `PublicTargetRow`'s own principal.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct PublicTopupSummary {
    pub operation_id: u64,
    pub rail: FundingRail,
    pub outcome: FundingOutcome,
    pub amount_cycles: Nat,
    pub resolved_at_secs: u64,
}

impl From<&TerminalFundingSummary> for PublicTopupSummary {
    fn from(summary: &TerminalFundingSummary) -> Self {
        Self {
            operation_id: summary.operation_id,
            rail: summary.rail,
            outcome: summary.outcome,
            amount_cycles: Nat::from(summary.amount_cycles),
            resolved_at_secs: summary.resolved_at_secs,
        }
    }
}

/// Bounds how many recent top-ups a single `PublicTargetRow` can carry.
#[derive(CandidType, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct RecentTopups(Vec<PublicTopupSummary>);

impl RecentTopups {
    pub fn new(entries: Vec<PublicTopupSummary>) -> Result<Self, BoundedFieldError> {
        if entries.len() > MAX_RECENT_TOPUPS_PER_TARGET {
            return Err(BoundedFieldError::TooMany);
        }
        Ok(Self(entries))
    }

    pub fn as_slice(&self) -> &[PublicTopupSummary] {
        &self.0
    }
}

/// Re-runs `new`'s bound against the raw decoded vector.
impl<'de> Deserialize<'de> for RecentTopups {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let entries = Vec::<PublicTopupSummary>::deserialize(deserializer)?;
        Self::new(entries).map_err(invariant_decode_error)
    }
}

/// Excludes signer principals, proposal payloads, operation internals,
/// ledger subaccounts, and pending reconciliation evidence, as required for
/// the anonymous `/telemetry` route. `principal` and `observation_mode` are
/// deliberately included: the principal is a canister ID needed by external
/// tooling, and the design accepts disclosing balance/threshold/burn/stale
/// age for the transparency benefit.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct PublicTargetRow {
    pub principal: Principal,
    pub display_name: String,
    pub project: String,
    pub environment: Environment,
    pub criticality: Criticality,
    pub observation_mode: ObservationMode,
    pub state: PublicTargetState,
    pub advisory_balance_cycles: Nat,
    /// True when `advisory_balance_cycles` is a clamped `u128::MAX` display
    /// value (`AdvisoryCyclesBalance::Overflow`) rather than the target's
    /// genuine reported balance — set from
    /// `AdvisoryCyclesBalance::is_overflow()` so an overflowed report is
    /// never mistaken for a truthful, enormous balance.
    pub advisory_balance_overflowed: bool,
    pub low_balance_threshold_cycles: Nat,
    pub refill_cycles: Nat,
    pub burn_cycles_per_day: Option<Nat>,
    pub runway_secs: Option<u64>,
    pub as_of_secs: u64,
    pub last_success_at_secs: Option<u64>,
    pub stale_for_secs: Option<u64>,
    pub next_sample_at_secs: Option<u64>,
    pub recent_topups: RecentTopups,
}

/// Excludes signer principals and any other private data; `Alarm`'s current
/// fields already carry none, but this keeps the public API contract
/// decoupled from `Alarm`'s internal representation so a future private
/// field on `Alarm` cannot leak here by accident.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct PublicAlarm {
    pub id: u64,
    pub target: Option<Principal>,
    pub kind: AlarmKind,
    pub status: AlarmStatus,
    pub opened_at_secs: u64,
    pub acknowledged_at_secs: Option<u64>,
    pub resolved_at_secs: Option<u64>,
}

impl From<&Alarm> for PublicAlarm {
    fn from(alarm: &Alarm) -> Self {
        Self {
            id: alarm.id,
            target: alarm.target,
            kind: alarm.kind,
            status: alarm.status,
            opened_at_secs: alarm.opened_at_secs,
            acknowledged_at_secs: alarm.acknowledged_at_secs,
            resolved_at_secs: alarm.resolved_at_secs,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageError {
    TooManyItems,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct PublicPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

impl<T> PublicPage<T> {
    pub fn try_new(items: Vec<T>, next_cursor: Option<String>) -> Result<Self, PageError> {
        if items.len() > MAX_PUBLIC_PAGE {
            return Err(PageError::TooManyItems);
        }
        Ok(Self { items, next_cursor })
    }
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct PermissionsView {
    pub is_signer: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use candid::{Decode, Encode};

    // ─── constants / reserved principals ───

    fn other_sentinel_id() -> Principal {
        // An arbitrary non-reserved principal used as Sentinel's own id in
        // tests that don't care about self-principal rejection.
        Principal::from_slice(&[9; 5])
    }

    fn signer(seed: u8) -> Principal {
        Principal::from_slice(&[seed; 10])
    }

    fn target_principal(seed: u8) -> Principal {
        Principal::from_slice(&[7, seed])
    }

    #[test]
    fn reserved_principal_kind_flags_anonymous() {
        assert_eq!(
            reserved_principal_kind(Principal::anonymous(), other_sentinel_id()),
            Some(ReservedPrincipalKind::Anonymous)
        );
    }

    #[test]
    fn reserved_principal_kind_flags_sentinel_self() {
        let sentinel = other_sentinel_id();
        assert_eq!(
            reserved_principal_kind(sentinel, sentinel),
            Some(ReservedPrincipalKind::SentinelSelf)
        );
    }

    #[test]
    fn reserved_principal_kind_flags_management_canister() {
        assert_eq!(
            reserved_principal_kind(Principal::management_canister(), other_sentinel_id()),
            Some(ReservedPrincipalKind::ManagementCanister)
        );
    }

    #[test]
    fn reserved_principal_kind_flags_icp_ledger() {
        assert_eq!(
            reserved_principal_kind(icp_ledger_principal(), other_sentinel_id()),
            Some(ReservedPrincipalKind::IcpLedger)
        );
    }

    #[test]
    fn reserved_principal_kind_flags_cmc() {
        assert_eq!(
            reserved_principal_kind(cycles_minting_canister_principal(), other_sentinel_id()),
            Some(ReservedPrincipalKind::Cmc)
        );
    }

    #[test]
    fn reserved_principal_kind_flags_cycles_ledger() {
        assert_eq!(
            reserved_principal_kind(cycles_ledger_principal(), other_sentinel_id()),
            Some(ReservedPrincipalKind::CyclesLedger)
        );
    }

    #[test]
    fn reserved_principal_kind_allows_ordinary_principal() {
        assert_eq!(
            reserved_principal_kind(target_principal(1), other_sentinel_id()),
            None
        );
    }

    // ─── Nat <-> u128 conversion ───

    #[test]
    fn checked_cycles_from_nat_converts_in_range_values() {
        assert_eq!(checked_cycles_from_nat(&Nat::from(0u64)), Some(0u128));
        assert_eq!(
            checked_cycles_from_nat(&Nat::from(u128::MAX)),
            Some(u128::MAX)
        );
    }

    #[test]
    fn checked_cycles_from_nat_rejects_oversized_value() {
        let over = Nat::from(u128::MAX) + Nat::from(1u64);
        assert_eq!(checked_cycles_from_nat(&over), None);
    }

    #[test]
    fn advisory_balance_overflow_for_oversized_nat() {
        let over = Nat::from(u128::MAX) + Nat::from(1u64);
        let advisory = AdvisoryCyclesBalance::from_nat(&over);
        assert_eq!(advisory, AdvisoryCyclesBalance::Overflow);
        assert_eq!(advisory.low_balance_value(), None);
        assert_eq!(advisory.to_nat(), Nat::from(u128::MAX));
    }

    #[test]
    fn advisory_balance_round_trips_in_range_value() {
        let advisory = AdvisoryCyclesBalance::from_nat(&Nat::from(42u64));
        assert_eq!(advisory, AdvisoryCyclesBalance::Exact(42));
        assert_eq!(advisory.low_balance_value(), Some(42u128));
        assert_eq!(advisory.to_nat(), Nat::from(42u64));
    }

    // ─── bounded fields ───

    #[test]
    fn bounded_name_accepts_exactly_max_bytes() {
        let value = "a".repeat(MAX_NAME_BYTES);
        assert!(BoundedName::new(&value).is_ok());
    }

    #[test]
    fn bounded_name_rejects_one_byte_over() {
        let value = "a".repeat(MAX_NAME_BYTES + 1);
        assert_eq!(BoundedName::new(&value), Err(BoundedFieldError::TooLong));
    }

    #[test]
    fn bounded_name_rejects_empty() {
        assert_eq!(BoundedName::new(""), Err(BoundedFieldError::Empty));
    }

    #[test]
    fn bounded_tags_accepts_exactly_max_tags_of_max_bytes() {
        let tags: Vec<String> = (0..MAX_TAGS)
            .map(|i| format!("{i:02}{}", "a".repeat(MAX_TAG_BYTES - 2)))
            .collect();
        assert!(tags.iter().all(|t| t.as_bytes().len() == MAX_TAG_BYTES));
        assert!(BoundedTags::new(tags).is_ok());
    }

    #[test]
    fn bounded_tags_rejects_one_tag_over() {
        let tags: Vec<String> = (0..=MAX_TAGS).map(|i| format!("t{i}")).collect();
        assert_eq!(BoundedTags::new(tags), Err(BoundedFieldError::TooMany));
    }

    #[test]
    fn bounded_tags_rejects_one_byte_over_per_tag() {
        let tags = vec!["a".repeat(MAX_TAG_BYTES + 1)];
        assert_eq!(BoundedTags::new(tags), Err(BoundedFieldError::TooLong));
    }

    #[test]
    fn bounded_tags_rejects_empty_tag() {
        assert_eq!(
            BoundedTags::new(vec!["".to_string()]),
            Err(BoundedFieldError::Empty)
        );
    }

    #[test]
    fn bounded_tags_rejects_duplicate_tag() {
        assert_eq!(
            BoundedTags::new(vec!["cycles".to_string(), "cycles".to_string()]),
            Err(BoundedFieldError::Duplicate)
        );
    }

    // ─── ValidatedInitArgs ───

    fn governance_timelocks_args() -> GovernanceTimelocksArgs {
        GovernanceTimelocksArgs {
            target_registry_secs: 3_600,
            spend_policy_secs: 3_600,
            signer_change_secs: 3_600,
            unpause_secs: 3_600,
        }
    }

    fn global_policy_args(cap: u128) -> GlobalPolicyArgs {
        GlobalPolicyArgs {
            global_daily_cap_cycles: Nat::from(cap),
            sample_interval_secs: 300,
            stale_after_secs: 600,
            min_icp_reserve_e8s: Nat::from(100_000_000u64),
            timelocks: governance_timelocks_args(),
            self_recovery_policy: self_recovery_args(1_000, 100, 1, 10),
        }
    }

    fn init_args(signers: Vec<Principal>, threshold: u32) -> InitArgs {
        InitArgs {
            signers,
            approval_threshold: threshold,
            global_policy: global_policy_args(1_000_000_000_000),
        }
    }

    #[test]
    fn init_args_rejects_empty_signers() {
        assert_eq!(
            ValidatedInitArgs::validate(init_args(vec![], 1)),
            Err(InitArgsError::EmptySigners)
        );
    }

    #[test]
    fn init_args_rejects_anonymous_signer() {
        assert_eq!(
            ValidatedInitArgs::validate(init_args(vec![Principal::anonymous()], 1)),
            Err(InitArgsError::AnonymousSigner)
        );
    }

    #[test]
    fn init_args_rejects_duplicate_signer() {
        let s = signer(1);
        assert_eq!(
            ValidatedInitArgs::validate(init_args(vec![s, s], 1)),
            Err(InitArgsError::DuplicateSigner(s))
        );
    }

    #[test]
    fn init_args_rejects_zero_threshold() {
        assert_eq!(
            ValidatedInitArgs::validate(init_args(vec![signer(1)], 0)),
            Err(InitArgsError::ThresholdZero)
        );
    }

    #[test]
    fn init_args_rejects_threshold_over_signer_count() {
        assert_eq!(
            ValidatedInitArgs::validate(init_args(vec![signer(1), signer(2)], 3)),
            Err(InitArgsError::ThresholdExceedsSigners {
                threshold: 3,
                signer_count: 2
            })
        );
    }

    #[test]
    fn init_args_accepts_threshold_equal_to_signer_count() {
        let validated =
            ValidatedInitArgs::validate(init_args(vec![signer(1), signer(2)], 2)).unwrap();
        assert_eq!(validated.approval_threshold(), 2);
        assert_eq!(validated.signers(), &[signer(1), signer(2)]);
    }

    #[test]
    fn init_args_accepts_threshold_of_one_with_one_signer() {
        assert!(ValidatedInitArgs::validate(init_args(vec![signer(1)], 1)).is_ok());
    }

    #[test]
    fn init_args_propagates_invalid_global_policy() {
        let mut args = init_args(vec![signer(1)], 1);
        args.global_policy.global_daily_cap_cycles = Nat::from(u128::MAX) + Nat::from(1u64);
        assert_eq!(
            ValidatedInitArgs::validate(args),
            Err(InitArgsError::InvalidGlobalPolicy(
                GlobalPolicyError::CyclesValueOverflow
            ))
        );
    }

    // ─── GlobalPolicy ───

    #[test]
    fn global_policy_rejects_oversized_cap() {
        let mut args = global_policy_args(1_000);
        args.global_daily_cap_cycles = Nat::from(u128::MAX) + Nat::from(1u64);
        assert_eq!(
            GlobalPolicy::validate(&args),
            Err(GlobalPolicyError::CyclesValueOverflow)
        );
    }

    #[test]
    fn global_policy_rejects_zero_daily_cap() {
        let mut args = global_policy_args(1_000);
        args.global_daily_cap_cycles = Nat::from(0u64);
        assert_eq!(
            GlobalPolicy::validate(&args),
            Err(GlobalPolicyError::ZeroGlobalDailyCap)
        );
    }

    #[test]
    fn global_policy_rejects_zero_sample_interval() {
        let mut args = global_policy_args(1_000);
        args.sample_interval_secs = 0;
        assert_eq!(
            GlobalPolicy::validate(&args),
            Err(GlobalPolicyError::ZeroSampleInterval)
        );
    }

    #[test]
    fn global_policy_rejects_stale_after_below_sample_interval() {
        let mut args = global_policy_args(1_000);
        args.sample_interval_secs = 300;
        args.stale_after_secs = 299;
        assert_eq!(
            GlobalPolicy::validate(&args),
            Err(GlobalPolicyError::StaleAfterBelowSampleInterval)
        );
    }

    #[test]
    fn global_policy_accepts_stale_after_equal_to_sample_interval() {
        let mut args = global_policy_args(1_000);
        args.sample_interval_secs = 300;
        args.stale_after_secs = 300;
        assert!(GlobalPolicy::validate(&args).is_ok());
    }

    #[test]
    fn global_policy_propagates_invalid_self_recovery_policy() {
        let mut args = global_policy_args(1_000);
        args.self_recovery_policy.low_balance_threshold_cycles = Nat::from(0u64);
        assert_eq!(
            GlobalPolicy::validate(&args),
            Err(GlobalPolicyError::InvalidSelfRecoveryPolicy(
                SelfRecoveryPolicyError::ZeroLowBalanceThreshold
            ))
        );
    }

    #[test]
    fn global_policy_rejects_zero_target_registry_timelock() {
        let mut args = global_policy_args(1_000);
        args.timelocks.target_registry_secs = 0;
        assert_eq!(
            GlobalPolicy::validate(&args),
            Err(GlobalPolicyError::InvalidTimelocks(
                GovernanceTimelocksError::ZeroTargetRegistrySecs
            ))
        );
    }

    #[test]
    fn global_policy_rejects_zero_spend_policy_timelock() {
        let mut args = global_policy_args(1_000);
        args.timelocks.spend_policy_secs = 0;
        assert_eq!(
            GlobalPolicy::validate(&args),
            Err(GlobalPolicyError::InvalidTimelocks(
                GovernanceTimelocksError::ZeroSpendPolicySecs
            ))
        );
    }

    #[test]
    fn global_policy_rejects_zero_signer_change_timelock() {
        let mut args = global_policy_args(1_000);
        args.timelocks.signer_change_secs = 0;
        assert_eq!(
            GlobalPolicy::validate(&args),
            Err(GlobalPolicyError::InvalidTimelocks(
                GovernanceTimelocksError::ZeroSignerChangeSecs
            ))
        );
    }

    #[test]
    fn global_policy_rejects_zero_unpause_timelock() {
        let mut args = global_policy_args(1_000);
        args.timelocks.unpause_secs = 0;
        assert_eq!(
            GlobalPolicy::validate(&args),
            Err(GlobalPolicyError::InvalidTimelocks(
                GovernanceTimelocksError::ZeroUnpauseSecs
            ))
        );
    }

    #[test]
    fn global_policy_rejects_all_zero_timelocks() {
        let mut args = global_policy_args(1_000);
        args.timelocks = GovernanceTimelocksArgs {
            target_registry_secs: 0,
            spend_policy_secs: 0,
            signer_change_secs: 0,
            unpause_secs: 0,
        };
        // The first field checked wins; the point of this test is that an
        // all-zero configuration is rejected at all, not which error fires
        // first.
        assert!(GlobalPolicy::validate(&args).is_err());
    }

    #[test]
    fn global_policy_round_trips_through_to_args() {
        let args = global_policy_args(500);
        let policy = GlobalPolicy::validate(&args).unwrap();
        assert_eq!(policy.global_daily_cap_cycles(), 500);
        assert_eq!(policy.sample_interval_secs(), 300);
        assert_eq!(policy.stale_after_secs(), 600);
        assert_eq!(policy.min_icp_reserve_e8s(), 100_000_000);
        assert_eq!(policy.timelocks().unpause_secs(), 3_600);
        assert_eq!(policy.self_recovery_policy().refill_cycles(), 10);
        assert_eq!(policy.to_args(), args);
    }

    // ─── TargetFundingPolicy ───

    fn funding_policy_args(low: u128, refill: u128, cap: u128) -> TargetFundingPolicyArgs {
        TargetFundingPolicyArgs {
            low_balance_threshold_cycles: Nat::from(low),
            refill_cycles: Nat::from(refill),
            daily_cap_cycles: Nat::from(cap),
            cooldown_secs: 60,
            burn_anomaly_limit_cycles_per_day: None,
        }
    }

    #[test]
    fn funding_policy_rejects_zero_low_balance_threshold() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let args = funding_policy_args(0, 10, 100);
        assert_eq!(
            TargetFundingPolicy::validate(&args, &global),
            Err(TargetValidationError::ZeroLowBalanceThreshold)
        );
    }

    #[test]
    fn funding_policy_rejects_zero_refill() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let args = funding_policy_args(1, 0, 100);
        assert_eq!(
            TargetFundingPolicy::validate(&args, &global),
            Err(TargetValidationError::ZeroRefillCycles)
        );
    }

    #[test]
    fn funding_policy_accepts_refill_at_exactly_max() {
        let global = GlobalPolicy::validate(&global_policy_args(MAX_REFILL_CYCLES)).unwrap();
        let args = funding_policy_args(1, MAX_REFILL_CYCLES, MAX_REFILL_CYCLES);
        assert!(TargetFundingPolicy::validate(&args, &global).is_ok());
    }

    #[test]
    fn funding_policy_rejects_refill_one_over_max() {
        let global = GlobalPolicy::validate(&global_policy_args(MAX_REFILL_CYCLES + 1)).unwrap();
        let args = funding_policy_args(1, MAX_REFILL_CYCLES + 1, MAX_REFILL_CYCLES + 1);
        assert_eq!(
            TargetFundingPolicy::validate(&args, &global),
            Err(TargetValidationError::RefillExceedsMaximum)
        );
    }

    #[test]
    fn funding_policy_accepts_daily_cap_equal_to_refill() {
        let global = GlobalPolicy::validate(&global_policy_args(100)).unwrap();
        let args = funding_policy_args(1, 100, 100);
        assert!(TargetFundingPolicy::validate(&args, &global).is_ok());
    }

    #[test]
    fn funding_policy_rejects_daily_cap_below_refill() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let args = funding_policy_args(1, 100, 99);
        assert_eq!(
            TargetFundingPolicy::validate(&args, &global),
            Err(TargetValidationError::DailyCapBelowRefill)
        );
    }

    #[test]
    fn funding_policy_accepts_global_cap_equal_to_daily_cap() {
        let global = GlobalPolicy::validate(&global_policy_args(100)).unwrap();
        let args = funding_policy_args(1, 10, 100);
        assert!(TargetFundingPolicy::validate(&args, &global).is_ok());
    }

    #[test]
    fn funding_policy_rejects_global_cap_below_daily_cap() {
        let global = GlobalPolicy::validate(&global_policy_args(99)).unwrap();
        let args = funding_policy_args(1, 10, 100);
        assert_eq!(
            TargetFundingPolicy::validate(&args, &global),
            Err(TargetValidationError::GlobalCapBelowDailyCap)
        );
    }

    #[test]
    fn funding_policy_rejects_oversized_nat_field() {
        let global = GlobalPolicy::validate(&global_policy_args(u128::MAX)).unwrap();
        let args = TargetFundingPolicyArgs {
            low_balance_threshold_cycles: Nat::from(u128::MAX) + Nat::from(1u64),
            refill_cycles: Nat::from(10u64),
            daily_cap_cycles: Nat::from(100u64),
            cooldown_secs: 60,
            burn_anomaly_limit_cycles_per_day: None,
        };
        assert_eq!(
            TargetFundingPolicy::validate(&args, &global),
            Err(TargetValidationError::CyclesValueOverflow)
        );
    }

    #[test]
    fn funding_policy_rejects_low_balance_threshold_over_max() {
        let global =
            GlobalPolicy::validate(&global_policy_args(MAX_LOW_BALANCE_THRESHOLD_CYCLES + 1))
                .unwrap();
        let args = funding_policy_args(
            MAX_LOW_BALANCE_THRESHOLD_CYCLES + 1,
            1,
            MAX_LOW_BALANCE_THRESHOLD_CYCLES + 1,
        );
        assert_eq!(
            TargetFundingPolicy::validate(&args, &global),
            Err(TargetValidationError::LowBalanceThresholdExceedsMaximum)
        );
    }

    #[test]
    fn funding_policy_accepts_low_balance_threshold_at_exactly_max() {
        let global =
            GlobalPolicy::validate(&global_policy_args(MAX_LOW_BALANCE_THRESHOLD_CYCLES)).unwrap();
        let args = funding_policy_args(
            MAX_LOW_BALANCE_THRESHOLD_CYCLES,
            1,
            MAX_LOW_BALANCE_THRESHOLD_CYCLES,
        );
        assert!(TargetFundingPolicy::validate(&args, &global).is_ok());
    }

    #[test]
    fn funding_policy_rejects_daily_cap_over_max() {
        let global =
            GlobalPolicy::validate(&global_policy_args(MAX_TARGET_DAILY_CAP_CYCLES + 1_000))
                .unwrap();
        let args = funding_policy_args(1, 1, MAX_TARGET_DAILY_CAP_CYCLES + 1);
        assert_eq!(
            TargetFundingPolicy::validate(&args, &global),
            Err(TargetValidationError::DailyCapExceedsMaximum)
        );
    }

    #[test]
    fn funding_policy_accepts_daily_cap_at_exactly_max() {
        let global =
            GlobalPolicy::validate(&global_policy_args(MAX_TARGET_DAILY_CAP_CYCLES)).unwrap();
        let args = funding_policy_args(1, 1, MAX_TARGET_DAILY_CAP_CYCLES);
        assert!(TargetFundingPolicy::validate(&args, &global).is_ok());
    }

    #[test]
    fn funding_policy_rejects_zero_burn_anomaly_limit() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let mut args = funding_policy_args(1, 10, 100);
        args.burn_anomaly_limit_cycles_per_day = Some(Nat::from(0u64));
        assert_eq!(
            TargetFundingPolicy::validate(&args, &global),
            Err(TargetValidationError::ZeroBurnAnomalyLimit)
        );
    }

    #[test]
    fn funding_policy_rejects_burn_anomaly_limit_over_max() {
        let global =
            GlobalPolicy::validate(&global_policy_args(MAX_TARGET_DAILY_CAP_CYCLES)).unwrap();
        let mut args = funding_policy_args(1, 10, 100);
        args.burn_anomaly_limit_cycles_per_day =
            Some(Nat::from(MAX_BURN_ANOMALY_CYCLES_PER_DAY + 1));
        assert_eq!(
            TargetFundingPolicy::validate(&args, &global),
            Err(TargetValidationError::BurnAnomalyLimitExceedsMaximum)
        );
    }

    #[test]
    fn funding_policy_accepts_burn_anomaly_limit_at_exactly_max() {
        let global =
            GlobalPolicy::validate(&global_policy_args(MAX_TARGET_DAILY_CAP_CYCLES)).unwrap();
        let mut args = funding_policy_args(1, 10, 100);
        args.burn_anomaly_limit_cycles_per_day = Some(Nat::from(MAX_BURN_ANOMALY_CYCLES_PER_DAY));
        let policy = TargetFundingPolicy::validate(&args, &global).unwrap();
        assert_eq!(
            policy.burn_anomaly_limit_cycles_per_day(),
            Some(MAX_BURN_ANOMALY_CYCLES_PER_DAY)
        );
    }

    #[test]
    fn funding_policy_round_trips_through_to_args() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let mut args = funding_policy_args(1, 10, 100);
        args.cooldown_secs = 120;
        args.burn_anomaly_limit_cycles_per_day = Some(Nat::from(500u64));
        let policy = TargetFundingPolicy::validate(&args, &global).unwrap();
        assert_eq!(policy.low_balance_threshold_cycles(), 1);
        assert_eq!(policy.refill_cycles(), 10);
        assert_eq!(policy.daily_cap_cycles(), 100);
        assert_eq!(policy.cooldown_secs(), 120);
        assert_eq!(policy.burn_anomaly_limit_cycles_per_day(), Some(500));
        assert_eq!(policy.to_args(), args);
    }

    // ─── TargetRecord registration ───

    fn valid_target_args(principal: Principal) -> TargetArgs {
        TargetArgs {
            principal,
            display_name: "Sample Target".to_string(),
            project: "Rumi Protocol".to_string(),
            environment: Environment::Production,
            criticality: Criticality::Standard,
            observation_mode: ObservationMode::Unobserved,
            tags: vec!["cycles".to_string()],
            funding_policy: funding_policy_args(1, 10, 100),
        }
    }

    fn registration_context<'a>(
        sentinel_id: Principal,
        existing: &'a BTreeSet<Principal>,
        global_policy: &'a GlobalPolicy,
    ) -> TargetRegistrationContext<'a> {
        TargetRegistrationContext {
            sentinel_id,
            existing_target_count: existing.len(),
            existing_target_principals: existing,
            global_policy,
        }
    }

    #[test]
    fn target_record_register_forces_disabled_defaults_and_revision_one() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let target = target_principal(1);

        let mut args = valid_target_args(target);
        // These fields do not exist on TargetArgs, but prove that whatever
        // upstream code assembles TargetArgs cannot smuggle them in: the
        // registered record is always disabled/unpaused at revision 1.
        args.display_name = "Untouched".to_string();
        let record = TargetRecord::register(args, &ctx).unwrap();

        assert_eq!(record.principal(), target);
        assert_eq!(record.revision(), 1);
        assert!(!record.enabled());
        assert!(!record.auto_topup());
        assert!(!record.paused());
    }

    #[test]
    fn target_record_register_rejects_anonymous_principal() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let err =
            TargetRecord::register(valid_target_args(Principal::anonymous()), &ctx).unwrap_err();
        assert_eq!(
            err,
            TargetValidationError::ReservedPrincipal(ReservedPrincipalKind::Anonymous)
        );
    }

    #[test]
    fn target_record_register_rejects_sentinel_self() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let sentinel = other_sentinel_id();
        let ctx = registration_context(sentinel, &existing, &global);
        let err = TargetRecord::register(valid_target_args(sentinel), &ctx).unwrap_err();
        assert_eq!(
            err,
            TargetValidationError::ReservedPrincipal(ReservedPrincipalKind::SentinelSelf)
        );
    }

    #[test]
    fn target_record_register_rejects_management_canister() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let err = TargetRecord::register(valid_target_args(Principal::management_canister()), &ctx)
            .unwrap_err();
        assert_eq!(
            err,
            TargetValidationError::ReservedPrincipal(ReservedPrincipalKind::ManagementCanister)
        );
    }

    #[test]
    fn target_record_register_rejects_icp_ledger() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let err =
            TargetRecord::register(valid_target_args(icp_ledger_principal()), &ctx).unwrap_err();
        assert_eq!(
            err,
            TargetValidationError::ReservedPrincipal(ReservedPrincipalKind::IcpLedger)
        );
    }

    #[test]
    fn target_record_register_rejects_cmc() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let err =
            TargetRecord::register(valid_target_args(cycles_minting_canister_principal()), &ctx)
                .unwrap_err();
        assert_eq!(
            err,
            TargetValidationError::ReservedPrincipal(ReservedPrincipalKind::Cmc)
        );
    }

    #[test]
    fn target_record_register_rejects_cycles_ledger() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let err =
            TargetRecord::register(valid_target_args(cycles_ledger_principal()), &ctx).unwrap_err();
        assert_eq!(
            err,
            TargetValidationError::ReservedPrincipal(ReservedPrincipalKind::CyclesLedger)
        );
    }

    #[test]
    fn target_record_register_rejects_duplicate_target() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let target = target_principal(1);
        let mut existing = BTreeSet::new();
        existing.insert(target);
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let err = TargetRecord::register(valid_target_args(target), &ctx).unwrap_err();
        assert_eq!(err, TargetValidationError::DuplicateTarget);
    }

    #[test]
    fn target_record_register_rejects_at_max_targets() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing: BTreeSet<Principal> = (0..MAX_TARGETS as u8).map(target_principal).collect();
        assert_eq!(existing.len(), MAX_TARGETS);
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let err =
            TargetRecord::register(valid_target_args(target_principal(200)), &ctx).unwrap_err();
        assert_eq!(err, TargetValidationError::TooManyTargets);
    }

    #[test]
    fn target_record_register_accepts_one_below_max_targets() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing: BTreeSet<Principal> =
            (0..(MAX_TARGETS as u8 - 1)).map(target_principal).collect();
        assert_eq!(existing.len(), MAX_TARGETS - 1);
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        assert!(TargetRecord::register(valid_target_args(target_principal(200)), &ctx).is_ok());
    }

    #[test]
    fn target_record_register_rejects_name_too_long() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let mut args = valid_target_args(target_principal(1));
        args.display_name = "a".repeat(MAX_NAME_BYTES + 1);
        assert_eq!(
            TargetRecord::register(args, &ctx).unwrap_err(),
            TargetValidationError::NameTooLong
        );
    }

    #[test]
    fn target_record_register_rejects_project_too_long() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let mut args = valid_target_args(target_principal(1));
        args.project = "a".repeat(MAX_NAME_BYTES + 1);
        assert_eq!(
            TargetRecord::register(args, &ctx).unwrap_err(),
            TargetValidationError::ProjectTooLong
        );
    }

    #[test]
    fn target_record_register_rejects_too_many_tags() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let mut args = valid_target_args(target_principal(1));
        args.tags = (0..=MAX_TAGS).map(|i| format!("t{i}")).collect();
        assert_eq!(
            TargetRecord::register(args, &ctx).unwrap_err(),
            TargetValidationError::TooManyTags
        );
    }

    #[test]
    fn target_record_register_rejects_tag_too_long() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let mut args = valid_target_args(target_principal(1));
        args.tags = vec!["a".repeat(MAX_TAG_BYTES + 1)];
        assert_eq!(
            TargetRecord::register(args, &ctx).unwrap_err(),
            TargetValidationError::TagTooLong
        );
    }

    #[test]
    fn target_record_register_rejects_invalid_funding_policy() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let mut args = valid_target_args(target_principal(1));
        args.funding_policy = funding_policy_args(0, 10, 100);
        assert_eq!(
            TargetRecord::register(args, &ctx).unwrap_err(),
            TargetValidationError::ZeroLowBalanceThreshold
        );
    }

    // ─── TargetPatch ───

    #[test]
    fn target_patch_cannot_change_principal_and_increments_revision() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let target = target_principal(1);
        let record = TargetRecord::register(valid_target_args(target), &ctx).unwrap();

        let patch = TargetPatch {
            display_name: Some("Renamed".to_string()),
            project: None,
            environment: None,
            criticality: None,
            observation_mode: None,
            tags: None,
            funding_policy: None,
            enabled: None,
            auto_topup: None,
        };
        let patched = record.apply_patch(patch, &global).unwrap();

        assert_eq!(patched.principal(), target);
        assert_eq!(patched.revision(), record.revision() + 1);
        assert_eq!(patched.display_name(), "Renamed");
        // Untouched fields survive unchanged.
        assert_eq!(patched.project(), record.project());
        assert_eq!(patched.funding_policy(), record.funding_policy());
    }

    fn no_op_patch() -> TargetPatch {
        TargetPatch {
            display_name: None,
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

    #[test]
    fn target_patch_preserves_disabled_state() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let record = TargetRecord::register(valid_target_args(target_principal(1)), &ctx).unwrap();

        let patched = record.apply_patch(no_op_patch(), &global).unwrap();
        assert!(!patched.enabled());
        assert!(!patched.auto_topup());
        assert!(!patched.paused());
    }

    #[test]
    fn target_patch_revalidates_bounds_on_changed_field() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let record = TargetRecord::register(valid_target_args(target_principal(1)), &ctx).unwrap();

        let patch = TargetPatch {
            display_name: Some("a".repeat(MAX_NAME_BYTES + 1)),
            ..no_op_patch()
        };
        assert_eq!(
            record.apply_patch(patch, &global).unwrap_err(),
            TargetValidationError::NameTooLong
        );
    }

    #[test]
    fn target_patch_can_change_funding_policy_within_bounds() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let record = TargetRecord::register(valid_target_args(target_principal(1)), &ctx).unwrap();

        let patch = TargetPatch {
            funding_policy: Some(funding_policy_args(2, 20, 200)),
            ..no_op_patch()
        };
        let patched = record.apply_patch(patch, &global).unwrap();
        assert_eq!(patched.funding_policy().refill_cycles(), 20);
    }

    #[test]
    fn target_patch_can_enable_target() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let record = TargetRecord::register(valid_target_args(target_principal(1)), &ctx).unwrap();

        let patch = TargetPatch {
            enabled: Some(true),
            ..no_op_patch()
        };
        let patched = record.apply_patch(patch, &global).unwrap();
        assert!(patched.enabled());
        assert!(!patched.auto_topup());
    }

    #[test]
    fn target_patch_can_set_auto_topup_when_enabled_and_observed() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let record = TargetRecord::register(valid_target_args(target_principal(1)), &ctx).unwrap();

        let patch = TargetPatch {
            observation_mode: Some(ObservationMode::SelfReport),
            enabled: Some(true),
            auto_topup: Some(true),
            ..no_op_patch()
        };
        let patched = record.apply_patch(patch, &global).unwrap();
        assert!(patched.enabled());
        assert!(patched.auto_topup());
    }

    #[test]
    fn target_patch_rejects_auto_topup_without_enabled() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let record = TargetRecord::register(valid_target_args(target_principal(1)), &ctx).unwrap();

        let patch = TargetPatch {
            observation_mode: Some(ObservationMode::SelfReport),
            auto_topup: Some(true),
            ..no_op_patch()
        };
        assert_eq!(
            record.apply_patch(patch, &global).unwrap_err(),
            TargetValidationError::AutoTopupRequiresEnabledAndObserved
        );
    }

    #[test]
    fn target_patch_rejects_auto_topup_when_unobserved() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        // valid_target_args registers with ObservationMode::Unobserved.
        let record = TargetRecord::register(valid_target_args(target_principal(1)), &ctx).unwrap();

        let patch = TargetPatch {
            enabled: Some(true),
            auto_topup: Some(true),
            ..no_op_patch()
        };
        assert_eq!(
            record.apply_patch(patch, &global).unwrap_err(),
            TargetValidationError::AutoTopupRequiresEnabledAndObserved
        );
    }

    #[test]
    fn target_patch_preserves_enabled_and_auto_topup_when_omitted() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let record = TargetRecord::register(valid_target_args(target_principal(1)), &ctx).unwrap();

        let enabled_patch = TargetPatch {
            observation_mode: Some(ObservationMode::SelfReport),
            enabled: Some(true),
            auto_topup: Some(true),
            ..no_op_patch()
        };
        let enabled_record = record.apply_patch(enabled_patch, &global).unwrap();

        let renamed = enabled_record
            .apply_patch(
                TargetPatch {
                    display_name: Some("Still Enabled".to_string()),
                    ..no_op_patch()
                },
                &global,
            )
            .unwrap();
        assert!(renamed.enabled());
        assert!(renamed.auto_topup());
    }

    // ─── TargetRecord pause/unpause ───

    #[test]
    fn target_record_pause_sets_paused_and_increments_revision() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let record = TargetRecord::register(valid_target_args(target_principal(1)), &ctx).unwrap();

        let paused = record.pause();
        assert!(paused.paused());
        assert_eq!(paused.revision(), record.revision() + 1);
        assert_eq!(paused.principal(), record.principal());
        assert_eq!(paused.display_name(), record.display_name());
        assert_eq!(paused.funding_policy(), record.funding_policy());
    }

    #[test]
    fn target_record_unpause_clears_paused_and_increments_revision() {
        let global = GlobalPolicy::validate(&global_policy_args(1_000)).unwrap();
        let existing = BTreeSet::new();
        let ctx = registration_context(other_sentinel_id(), &existing, &global);
        let record = TargetRecord::register(valid_target_args(target_principal(1)), &ctx).unwrap();
        let paused = record.pause();

        let unpaused = paused.unpause();
        assert!(!unpaused.paused());
        assert_eq!(unpaused.revision(), paused.revision() + 1);
        assert_eq!(unpaused.principal(), record.principal());
    }

    // ─── ProposalRecord ───

    #[test]
    fn proposal_record_new_starts_with_no_approvals() {
        let proposer = signer(1);
        let record = ProposalRecord::new(
            1,
            ProposalPayload::RemoveTarget {
                principal: target_principal(1),
            },
            proposer,
            0,
        );
        assert_eq!(record.approvals(), &[] as &[Principal]);
        assert_eq!(record.approval_count(), 0);
        assert!(!record.has_approved(proposer));
        assert_eq!(record.status, ProposalStatus::Open);
        assert_eq!(record.kind(), ProposalKind::RemoveTarget);
    }

    #[test]
    fn proposal_record_approval_is_idempotent_per_signer() {
        let proposer = signer(1);
        let mut record = ProposalRecord::new(
            1,
            ProposalPayload::AddSigner { signer: signer(2) },
            proposer,
            0,
        );
        assert!(record.record_approval(proposer));
        assert_eq!(record.approval_count(), 1);
        assert!(!record.record_approval(proposer));
        assert_eq!(record.approval_count(), 1);

        let other = signer(3);
        assert!(record.record_approval(other));
        assert_eq!(record.approval_count(), 2);
        assert!(!record.record_approval(other));
        assert_eq!(record.approval_count(), 2);
    }

    #[test]
    fn proposal_payload_kind_matches_every_variant() {
        assert_eq!(
            ProposalPayload::RegisterTarget(valid_target_args(target_principal(1))).kind(),
            ProposalKind::RegisterTarget
        );
        assert_eq!(
            ProposalPayload::UpdateTarget {
                principal: target_principal(1),
                patch: no_op_patch(),
            }
            .kind(),
            ProposalKind::UpdateTarget
        );
        assert_eq!(
            ProposalPayload::RemoveTarget {
                principal: target_principal(1)
            }
            .kind(),
            ProposalKind::RemoveTarget
        );
        assert_eq!(
            ProposalPayload::SetGlobalPolicy(global_policy_args(1)).kind(),
            ProposalKind::SetGlobalPolicy
        );
        assert_eq!(
            ProposalPayload::AddSigner { signer: signer(1) }.kind(),
            ProposalKind::AddSigner
        );
        assert_eq!(
            ProposalPayload::RemoveSigner { signer: signer(1) }.kind(),
            ProposalKind::RemoveSigner
        );
        assert_eq!(
            ProposalPayload::SetSignerThreshold { threshold: 2 }.kind(),
            ProposalKind::SetSignerThreshold
        );
        assert_eq!(
            ProposalPayload::UnpauseTarget {
                principal: target_principal(1)
            }
            .kind(),
            ProposalKind::UnpauseTarget
        );
    }

    // ─── Funding operation state ───

    fn test_funding_policy() -> TargetFundingPolicy {
        let global = GlobalPolicy::validate(&global_policy_args(1_000_000)).unwrap();
        TargetFundingPolicy::validate(&funding_policy_args(1, 10, 1_000), &global).unwrap()
    }

    fn cycles_withdraw_snapshot(destination: Principal) -> CyclesWithdrawSnapshot {
        CyclesWithdrawSnapshot {
            destination,
            from_subaccount: None,
            amount_cycles: 10,
            fee_cycles: 1,
            created_at_time_ns: 1_000,
        }
    }

    fn icp_cmc_snapshot(target_canister: Principal) -> IcpCmcSnapshot {
        IcpCmcSnapshot {
            source_subaccount: None,
            cmc_account_identifier: FixedBytes32::new(vec![1; 32]).unwrap(),
            target_canister,
            amount_e8s: 100_000_000,
            fee_e8s: 10_000,
            memo: 1_347_768_404,
            created_at_time_ns: 1_000,
            rate_xdr_permyriad_per_icp: 10_000,
            rate_timestamp_secs: 900,
            expected_cycles: 10,
        }
    }

    fn open_cycles_operation(id: u64, target: Principal, now_secs: u64) -> FundingOperation {
        FundingOperation::open(
            id,
            target,
            1,
            test_funding_policy(),
            FundingTrigger::LowBalanceAutoTopup,
            FundingRailArguments::Cycles(cycles_withdraw_snapshot(target)),
            10,
            now_secs,
        )
        .unwrap()
    }

    #[test]
    fn funding_operation_state_rail_matches_variant() {
        assert_eq!(
            FundingOperationState::Cycles(CyclesFundingState::Submitted).rail(),
            FundingRail::CyclesLedger
        );
        assert_eq!(
            FundingOperationState::Icp(IcpFundingState::NotifyPending).rail(),
            FundingRail::IcpCmc
        );
    }

    #[test]
    fn funding_operation_state_quarantined_stops_retry_but_is_not_resolved() {
        let quarantined = FundingOperationState::Cycles(CyclesFundingState::Quarantined);
        assert!(quarantined.stops_automatic_retry());
        assert!(!quarantined.is_resolved());

        let icp_quarantined = FundingOperationState::Icp(IcpFundingState::Quarantined);
        assert!(icp_quarantined.stops_automatic_retry());
        assert!(!icp_quarantined.is_resolved());
    }

    #[test]
    fn funding_operation_state_unknown_neither_stops_retry_nor_resolved() {
        let unknown = FundingOperationState::Cycles(CyclesFundingState::Unknown);
        assert!(!unknown.stops_automatic_retry());
        assert!(!unknown.is_resolved());

        let transfer_unknown = FundingOperationState::Icp(IcpFundingState::TransferUnknown);
        assert!(!transfer_unknown.stops_automatic_retry());
        assert!(!transfer_unknown.is_resolved());
    }

    #[test]
    fn funding_operation_state_resolved_states_also_stop_retry() {
        for state in [
            FundingOperationState::Cycles(CyclesFundingState::Complete),
            FundingOperationState::Cycles(CyclesFundingState::Terminal),
            FundingOperationState::Icp(IcpFundingState::Complete),
            FundingOperationState::Icp(IcpFundingState::Refunded),
            FundingOperationState::Icp(IcpFundingState::Terminal),
        ] {
            assert!(state.stops_automatic_retry());
            assert!(state.is_resolved());
        }
    }

    #[test]
    fn funding_operation_snapshot_fields_are_immutable_across_attempts() {
        let target = target_principal(1);
        let op = open_cycles_operation(1, target, 100);
        let advanced = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                110,
                FundingAttemptResultClass::Success,
            )
            .unwrap();

        assert_eq!(advanced.id(), op.id());
        assert_eq!(advanced.target(), op.target());
        assert_eq!(
            advanced.target_registry_revision(),
            op.target_registry_revision()
        );
        assert_eq!(advanced.funding_policy(), op.funding_policy());
        assert_eq!(advanced.trigger(), op.trigger());
        assert_eq!(advanced.rail_arguments(), op.rail_arguments());
        assert_eq!(
            advanced.reserved_amount_cycles(),
            op.reserved_amount_cycles()
        );
        assert_eq!(advanced.created_at_secs(), op.created_at_secs());

        assert_eq!(
            advanced.state(),
            FundingOperationState::Cycles(CyclesFundingState::Submitted)
        );
        assert_eq!(advanced.updated_at_secs(), 110);
        assert_eq!(advanced.attempts().len(), 1);
    }

    #[test]
    fn funding_operation_attempts_bounded_at_max() {
        let mut op = open_cycles_operation(1, target_principal(1), 0);
        for i in 0..MAX_FUNDING_ATTEMPTS {
            op = op
                .record_attempt(
                    FundingOperationState::Cycles(CyclesFundingState::Submitted),
                    i as u64,
                    FundingAttemptResultClass::RetryableFailure,
                )
                .unwrap();
        }
        assert_eq!(op.attempts().len(), MAX_FUNDING_ATTEMPTS);
        assert_eq!(op.attempts().as_slice().first().unwrap().ordinal, 1);
        assert_eq!(
            op.attempts().as_slice().last().unwrap().ordinal,
            MAX_FUNDING_ATTEMPTS as u32
        );
        assert_eq!(
            op.record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                999,
                FundingAttemptResultClass::RetryableFailure,
            ),
            Err(FundingOperationTransitionError::TooManyAttempts)
        );
    }

    #[test]
    fn record_attempt_rejects_cross_rail_transition() {
        let op = open_cycles_operation(1, target_principal(1), 0);
        assert_eq!(
            op.record_attempt(
                FundingOperationState::Icp(IcpFundingState::Complete),
                10,
                FundingAttemptResultClass::Success,
            ),
            Err(FundingOperationTransitionError::RailMismatch)
        );
    }

    #[test]
    fn record_attempt_rejects_direct_jump_past_intermediate_states() {
        let op = open_cycles_operation(1, target_principal(1), 0);
        assert_eq!(
            op.record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Complete),
                10,
                FundingAttemptResultClass::Success,
            ),
            Err(FundingOperationTransitionError::InvalidSuccessor)
        );
    }

    #[test]
    fn record_attempt_rejects_regression_from_resolved_state() {
        let op = open_cycles_operation(1, target_principal(1), 0)
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                20,
                FundingAttemptResultClass::Success,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Complete),
                30,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        assert_eq!(
            op.record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::PlannedReserved),
                40,
                FundingAttemptResultClass::Success,
            ),
            Err(FundingOperationTransitionError::InvalidSuccessor)
        );
    }

    #[test]
    fn record_attempt_allows_same_state_retry_of_an_indeterminate_result() {
        let op = open_cycles_operation(1, target_principal(1), 0)
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                10,
                FundingAttemptResultClass::RetryableFailure,
            )
            .unwrap();
        let retried = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                20,
                FundingAttemptResultClass::RetryableFailure,
            )
            .unwrap();
        assert_eq!(
            retried.state(),
            FundingOperationState::Cycles(CyclesFundingState::Submitted)
        );
        assert_eq!(retried.attempts().len(), 2);
    }

    #[test]
    fn record_attempt_rejects_timestamp_regression() {
        let op = open_cycles_operation(1, target_principal(1), 100)
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                110,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        assert_eq!(
            op.record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                109,
                FundingAttemptResultClass::Success,
            ),
            Err(FundingOperationTransitionError::TimestampRegression)
        );
        // Equal to the current `updated_at_secs` is allowed — this is
        // distinct from same-state retry, which is governed separately by
        // `is_valid_successor`.
        assert!(op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                110,
                FundingAttemptResultClass::RetryableFailure,
            )
            .is_ok());
    }

    #[test]
    fn terminal_funding_summary_rejects_unresolved_operation() {
        let op = open_cycles_operation(1, target_principal(1), 0);
        assert_eq!(
            TerminalFundingSummary::from_resolved(&op, 200),
            Err(TerminalFundingSummaryError::OperationNotResolved)
        );

        let quarantined = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                10,
                FundingAttemptResultClass::RetryableFailure,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Unknown),
                20,
                FundingAttemptResultClass::Indeterminate,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Quarantined),
                50,
                FundingAttemptResultClass::Indeterminate,
            )
            .unwrap();
        assert_eq!(
            TerminalFundingSummary::from_resolved(&quarantined, 200),
            Err(TerminalFundingSummaryError::OperationNotResolved)
        );
    }

    #[test]
    fn terminal_funding_summary_accepts_resolved_operation() {
        let op = open_cycles_operation(1, target_principal(1), 0);
        let completed = op
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                30,
                FundingAttemptResultClass::Success,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Complete),
                50,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        let summary = TerminalFundingSummary::from_resolved(&completed, 200).unwrap();
        assert_eq!(summary.operation_id(), 1);
        assert_eq!(summary.target(), target_principal(1));
        assert_eq!(summary.rail(), FundingRail::CyclesLedger);
        assert_eq!(summary.outcome(), FundingOutcome::Completed);
        assert_eq!(summary.amount_cycles(), 10);
        assert_eq!(summary.resolved_at_secs(), 200);
    }

    #[test]
    fn terminal_funding_summary_derives_outcome_from_icp_resolved_states() {
        let target = target_principal(1);
        for (final_state, expected_outcome) in [
            (IcpFundingState::Complete, FundingOutcome::Completed),
            (IcpFundingState::Refunded, FundingOutcome::Refunded),
            (IcpFundingState::Terminal, FundingOutcome::Terminal),
        ] {
            let op = FundingOperation::open(
                1,
                target,
                1,
                test_funding_policy(),
                FundingTrigger::LowBalanceAutoTopup,
                FundingRailArguments::Icp(icp_cmc_snapshot(target)),
                10,
                800,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Icp(IcpFundingState::LedgerSubmitted),
                810,
                FundingAttemptResultClass::Success,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Icp(IcpFundingState::TransferConfirmed),
                820,
                FundingAttemptResultClass::Success,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Icp(IcpFundingState::NotifyPending),
                830,
                FundingAttemptResultClass::Success,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Icp(final_state),
                840,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
            let summary = TerminalFundingSummary::from_resolved(&op, 900).unwrap();
            assert_eq!(summary.outcome(), expected_outcome);
        }
    }

    // ─── FundingOperation::open reserved-amount consistency ───

    #[test]
    fn open_rejects_reserved_amount_mismatched_with_cycles_snapshot() {
        let target = target_principal(1);
        assert_eq!(
            FundingOperation::open(
                1,
                target,
                1,
                test_funding_policy(),
                FundingTrigger::LowBalanceAutoTopup,
                FundingRailArguments::Cycles(cycles_withdraw_snapshot(target)),
                999,
                0,
            ),
            Err(FundingOperationOpenError::ReservedAmountMismatch)
        );
    }

    #[test]
    fn open_rejects_reserved_amount_mismatched_with_icp_snapshot() {
        let target = target_principal(1);
        assert_eq!(
            FundingOperation::open(
                1,
                target,
                1,
                test_funding_policy(),
                FundingTrigger::LowBalanceAutoTopup,
                FundingRailArguments::Icp(icp_cmc_snapshot(target)),
                999,
                0,
            ),
            Err(FundingOperationOpenError::ReservedAmountMismatch)
        );
    }

    // ─── FundingOperation::attach_confirmed_block ───

    #[test]
    fn attach_confirmed_block_sets_evidence_exactly_once() {
        let target = target_principal(1);
        let op = FundingOperation::open(
            1,
            target,
            1,
            test_funding_policy(),
            FundingTrigger::LowBalanceAutoTopup,
            FundingRailArguments::Icp(icp_cmc_snapshot(target)),
            10,
            0,
        )
        .unwrap();
        assert_eq!(op.confirmed_block_index(), None);

        let attached = op.attach_confirmed_block(42).unwrap();
        assert_eq!(attached.confirmed_block_index(), Some(42));

        // Single-assignment: a second call must never overwrite it.
        assert_eq!(
            attached.attach_confirmed_block(99),
            Err(AttachConfirmedBlockError::AlreadySet)
        );
        assert_eq!(attached.confirmed_block_index(), Some(42));
    }

    #[test]
    fn attach_confirmed_block_rejects_cycles_rail() {
        let op = open_cycles_operation(1, target_principal(1), 0);
        assert_eq!(
            op.attach_confirmed_block(1),
            Err(AttachConfirmedBlockError::WrongRail)
        );
    }

    #[test]
    fn record_attempt_never_touches_confirmed_block_index() {
        let target = target_principal(1);
        let op = FundingOperation::open(
            1,
            target,
            1,
            test_funding_policy(),
            FundingTrigger::LowBalanceAutoTopup,
            FundingRailArguments::Icp(icp_cmc_snapshot(target)),
            10,
            0,
        )
        .unwrap()
        .attach_confirmed_block(7)
        .unwrap()
        .record_attempt(
            FundingOperationState::Icp(IcpFundingState::LedgerSubmitted),
            10,
            FundingAttemptResultClass::Success,
        )
        .unwrap();
        assert_eq!(op.confirmed_block_index(), Some(7));
    }

    // ─── FixedBytes32 ───

    #[test]
    fn fixed_bytes_32_accepts_exactly_32_bytes() {
        assert!(FixedBytes32::new(vec![0u8; 32]).is_ok());
    }

    #[test]
    fn fixed_bytes_32_rejects_wrong_length() {
        assert_eq!(
            FixedBytes32::new(vec![0u8; 31]),
            Err(FixedBytes32Error::WrongLength { actual: 31 })
        );
        assert_eq!(
            FixedBytes32::new(vec![0u8; 33]),
            Err(FixedBytes32Error::WrongLength { actual: 33 })
        );
        assert_eq!(
            FixedBytes32::new(Vec::new()),
            Err(FixedBytes32Error::WrongLength { actual: 0 })
        );
    }

    // ─── RollingSpendLedger ───

    #[test]
    fn rolling_spend_ledger_reserve_counts_settled_plus_pending_against_cap() {
        let ledger = RollingSpendLedger::new()
            .reserve(1, 100, 0, 86_400, 250)
            .unwrap()
            .settle(1, 10, 86_400)
            .unwrap();
        let ledger = ledger.reserve(2, 150, 20, 86_400, 250).unwrap();
        assert_eq!(ledger.pending().len(), 1);
        assert_eq!(ledger.settled()[0].amount_cycles, 100);

        // 100 settled + 150 pending + 1 more would exceed the 250 cap.
        assert_eq!(
            ledger.reserve(3, 1, 30, 86_400, 250),
            Err(RollingSpendReserveError::CapExceeded)
        );
    }

    #[test]
    fn rolling_spend_ledger_true_sliding_window_prunes_at_exact_boundary() {
        let ledger = RollingSpendLedger::new()
            .reserve(1, 100, 0, 100, 200)
            .unwrap()
            .settle(1, 0, 100)
            .unwrap();

        // now - settled_at == window_secs: this is a true sliding window,
        // not a fixed bucket that only resets at multiples of the window,
        // so the entry is pruned exactly at the boundary.
        let at_boundary = ledger.reserve(2, 200, 100, 100, 200).unwrap();
        assert_eq!(at_boundary.settled().len(), 0);

        // One second before the boundary it still counts against the cap.
        assert_eq!(
            ledger.reserve(2, 101, 99, 100, 200),
            Err(RollingSpendReserveError::CapExceeded)
        );
    }

    #[test]
    fn rolling_spend_ledger_rejects_duplicate_operation_id() {
        let ledger = RollingSpendLedger::new()
            .reserve(1, 10, 0, 86_400, 1_000)
            .unwrap();
        assert_eq!(
            ledger.reserve(1, 10, 1, 86_400, 1_000),
            Err(RollingSpendReserveError::DuplicateOperation)
        );
    }

    #[test]
    fn rolling_spend_ledger_rejects_pending_over_bound() {
        let mut ledger = RollingSpendLedger::new();
        for i in 0..MAX_PENDING_RESERVATIONS as u64 {
            ledger = ledger.reserve(i, 1, 0, 86_400, u128::MAX).unwrap();
        }
        assert_eq!(
            ledger.reserve(MAX_PENDING_RESERVATIONS as u64, 1, 0, 86_400, u128::MAX),
            Err(RollingSpendReserveError::Bound)
        );
    }

    #[test]
    fn rolling_spend_ledger_rejects_checked_add_overflow() {
        let ledger = RollingSpendLedger::new()
            .reserve(1, u128::MAX, 0, 86_400, u128::MAX)
            .unwrap();
        assert_eq!(
            ledger.reserve(2, 1, 1, 86_400, u128::MAX),
            Err(RollingSpendReserveError::Overflow)
        );
    }

    #[test]
    fn rolling_spend_ledger_settle_matches_exact_operation_id() {
        let ledger = RollingSpendLedger::new()
            .reserve(1, 10, 0, 86_400, 1_000)
            .unwrap()
            .reserve(2, 20, 1, 86_400, 1_000)
            .unwrap();

        let settled = ledger.settle(1, 50, 86_400).unwrap();
        assert_eq!(settled.pending().len(), 1);
        assert_eq!(settled.pending()[0].operation_id, 2);
        assert_eq!(settled.settled().len(), 1);
        assert_eq!(settled.settled()[0].amount_cycles, 10);
        assert_eq!(settled.settled()[0].settled_at_secs, 50);
    }

    #[test]
    fn rolling_spend_ledger_settle_rejects_unknown_operation() {
        let ledger = RollingSpendLedger::new();
        assert_eq!(
            ledger.settle(9, 10, 86_400),
            Err(RollingSpendSettleError::UnknownOperation)
        );
    }

    #[test]
    fn rolling_spend_ledger_settle_never_evicts_an_in_window_entry() {
        // Settle more than MAX_ROLLING_SPEND_SETTLED_ENTRIES entries, all
        // within the window: the old behavior would silently drop the
        // globally-oldest entry by count once the bound was crossed, even
        // though every entry is still inside the 24h window. The corrected
        // behavior must fail closed instead, and never lose a real entry.
        let mut ledger = RollingSpendLedger::new();
        for i in 0..MAX_ROLLING_SPEND_SETTLED_ENTRIES as u64 {
            ledger = ledger
                .reserve(i, 1, i, 86_400, u128::MAX)
                .unwrap()
                .settle(i, i, 86_400)
                .unwrap();
        }
        assert_eq!(ledger.settled().len(), MAX_ROLLING_SPEND_SETTLED_ENTRIES);

        let one_more = ledger
            .reserve(
                MAX_ROLLING_SPEND_SETTLED_ENTRIES as u64,
                1,
                MAX_ROLLING_SPEND_SETTLED_ENTRIES as u64,
                86_400,
                u128::MAX,
            )
            .unwrap();
        assert_eq!(
            one_more.settle(
                MAX_ROLLING_SPEND_SETTLED_ENTRIES as u64,
                MAX_ROLLING_SPEND_SETTLED_ENTRIES as u64,
                86_400,
            ),
            Err(RollingSpendSettleError::Bound)
        );
        // The failed settle must not have silently dropped any prior entry.
        assert_eq!(one_more.settled().len(), MAX_ROLLING_SPEND_SETTLED_ENTRIES);
    }

    #[test]
    fn rolling_spend_ledger_settle_prunes_out_of_window_entries_before_bound_check() {
        // An entry that has aged out of the window is safe to prune even
        // while over the count bound, since it no longer counts toward the
        // cap either way.
        let ledger = RollingSpendLedger::new()
            .reserve(1, 10, 0, 100, 1_000)
            .unwrap()
            .settle(1, 0, 100)
            .unwrap();
        let settled_later = ledger
            .reserve(2, 10, 500, 100, 1_000)
            .unwrap()
            .settle(2, 500, 100)
            .unwrap();
        // Operation 1's entry (settled at 0) is now 500s old, past the
        // 100s window as of settle time 500, so it is pruned rather than
        // counted toward the bound.
        assert_eq!(settled_later.settled().len(), 1);
        assert_eq!(settled_later.settled()[0].amount_cycles, 10);
        assert_eq!(settled_later.settled()[0].settled_at_secs, 500);
    }

    #[test]
    fn rolling_spend_ledger_release_no_spend_removes_only_matching_pending_entry() {
        let ledger = RollingSpendLedger::new()
            .reserve(1, 10, 0, 86_400, 1_000)
            .unwrap()
            .reserve(2, 20, 1, 86_400, 1_000)
            .unwrap();

        let released = ledger.release_no_spend(1).unwrap();
        assert_eq!(released.pending().len(), 1);
        assert_eq!(released.pending()[0].operation_id, 2);
        assert!(released.settled().is_empty());
    }

    #[test]
    fn rolling_spend_ledger_release_no_spend_rejects_unknown_operation() {
        let ledger = RollingSpendLedger::new();
        assert_eq!(
            ledger.release_no_spend(9),
            Err(RollingSpendReleaseError::UnknownOperation)
        );
    }

    // ─── TargetReservationState ───

    #[test]
    fn target_reservation_reserve_then_reject_second_reservation() {
        let state = TargetReservationState::new();
        let reserved = state.reserve(1, 10, 100, 86_400, 1_000).unwrap();
        assert_eq!(reserved.in_flight_operation_id(), Some(1));
        assert_eq!(reserved.in_flight_amount_cycles(), Some(10));
        assert!(!reserved.is_available(100));

        assert_eq!(
            reserved.reserve(2, 10, 100, 86_400, 1_000),
            Err(TargetReservationError::OperationInFlight)
        );
    }

    #[test]
    fn target_reservation_reserve_does_not_start_cooldown() {
        let state = TargetReservationState::new();
        let reserved = state.reserve(1, 10, 100, 86_400, 1_000).unwrap();
        let released = reserved.release_no_spend(1).unwrap();
        // If reserve() had set a cooldown, this would still be unavailable.
        assert!(released.is_available(100));
    }

    #[test]
    fn target_reservation_settle_spend_starts_cooldown_from_confirmed_spend() {
        let state = TargetReservationState::new();
        let reserved = state.reserve(1, 10, 100, 86_400, 1_000).unwrap();
        let settled = reserved.settle_spend(1, 150, 60, 86_400).unwrap();
        assert_eq!(settled.in_flight_operation_id(), None);
        // Cooldown runs from the settle time (150), not the reserve time (100).
        assert!(!settled.is_available(200));
        assert!(settled.is_available(210));
    }

    #[test]
    fn target_reservation_release_no_spend_leaves_prior_cooldown_unchanged() {
        let state = TargetReservationState::new();
        let reserved = state.reserve(1, 10, 100, 86_400, 1_000).unwrap();
        let settled = reserved.settle_spend(1, 150, 60, 86_400).unwrap(); // cooldown_until = 210
        let reserved_again = settled.reserve(2, 5, 210, 86_400, 1_000).unwrap();
        let released = reserved_again.release_no_spend(2).unwrap();
        // The earlier confirmed spend's cooldown (210) must be untouched by
        // this proven no-spend release: neither reset nor extended.
        assert!(!released.is_available(209));
        assert!(released.is_available(210));
    }

    #[test]
    fn target_reservation_settle_and_release_reject_mismatched_operation_id() {
        let state = TargetReservationState::new();
        let reserved = state.reserve(1, 10, 100, 86_400, 1_000).unwrap();
        assert_eq!(
            reserved.settle_spend(2, 150, 60, 86_400),
            Err(TargetSettleError::Mismatch)
        );
        assert_eq!(
            reserved.release_no_spend(2),
            Err(TargetReleaseError::Mismatch)
        );
    }

    #[test]
    fn target_reservation_respects_cooldown_before_reserve_available_again() {
        let state = TargetReservationState::new();
        let reserved = state.reserve(1, 10, 100, 86_400, 1_000).unwrap();
        let settled = reserved.settle_spend(1, 100, 60, 86_400).unwrap();
        assert_eq!(
            settled.reserve(2, 10, 150, 86_400, 1_000),
            Err(TargetReservationError::Cooldown)
        );
        assert!(settled.reserve(2, 10, 160, 86_400, 1_000).is_ok());
    }

    #[test]
    fn target_reservation_settle_spend_propagates_ledger_bound_failure() {
        let mut state = TargetReservationState::new();
        for i in 0..MAX_ROLLING_SPEND_SETTLED_ENTRIES as u64 {
            state = state
                .reserve(i, 1, i, 86_400, u128::MAX)
                .unwrap()
                .settle_spend(i, i, 0, 86_400)
                .unwrap();
        }
        let next = MAX_ROLLING_SPEND_SETTLED_ENTRIES as u64;
        let reserved = state.reserve(next, 1, next, 86_400, u128::MAX).unwrap();
        assert_eq!(
            reserved.settle_spend(next, next, 0, 86_400),
            Err(TargetSettleError::Bound)
        );
    }

    // ─── GlobalRollingSpendState ───

    #[test]
    fn global_rolling_spend_state_enforces_pending_cap_across_many_targets() {
        let state = GlobalRollingSpendState::new();
        let state = state.reserve(1, 400, 0, 86_400, 1_000).unwrap();
        let state = state.reserve(2, 400, 1, 86_400, 1_000).unwrap();
        assert_eq!(state.rolling_spend().pending_cycles(), 800);

        // A third target's reservation would push total pending over cap.
        assert_eq!(
            state.reserve(3, 300, 2, 86_400, 1_000),
            Err(RollingSpendReserveError::CapExceeded)
        );
        assert!(state.reserve(3, 200, 2, 86_400, 1_000).is_ok());
    }

    #[test]
    fn global_rolling_spend_state_settle_and_release_match_exact_operation_id() {
        let state = GlobalRollingSpendState::new();
        let state = state.reserve(1, 100, 0, 86_400, 1_000).unwrap();
        let state = state.reserve(2, 200, 1, 86_400, 1_000).unwrap();

        let settled = state.settle(1, 50, 86_400).unwrap();
        assert_eq!(settled.rolling_spend().pending().len(), 1);
        assert_eq!(settled.rolling_spend().pending()[0].operation_id, 2);
        assert_eq!(settled.rolling_spend().settled().len(), 1);

        let released = settled.release_no_spend(2).unwrap();
        assert!(released.rolling_spend().pending().is_empty());
        // Only operation 1's settled entry remains; releasing operation 2
        // must never create a settled entry for it.
        assert_eq!(released.rolling_spend().settled().len(), 1);
    }

    // ─── SelfRecoveryPolicy ───

    fn self_recovery_args(
        reserve: u128,
        cap: u128,
        threshold: u128,
        refill: u128,
    ) -> SelfRecoveryPolicyArgs {
        SelfRecoveryPolicyArgs {
            protected_reserve_cycles: Nat::from(reserve),
            daily_cap_cycles: Nat::from(cap),
            low_balance_threshold_cycles: Nat::from(threshold),
            refill_cycles: Nat::from(refill),
        }
    }

    #[test]
    fn self_recovery_policy_rejects_zero_low_balance_threshold() {
        assert_eq!(
            SelfRecoveryPolicy::validate(&self_recovery_args(1_000, 100, 0, 10)),
            Err(SelfRecoveryPolicyError::ZeroLowBalanceThreshold)
        );
    }

    #[test]
    fn self_recovery_policy_rejects_zero_daily_cap() {
        assert_eq!(
            SelfRecoveryPolicy::validate(&self_recovery_args(1_000, 0, 1, 1)),
            Err(SelfRecoveryPolicyError::ZeroDailyCap)
        );
    }

    #[test]
    fn self_recovery_policy_rejects_zero_refill() {
        assert_eq!(
            SelfRecoveryPolicy::validate(&self_recovery_args(1_000, 100, 1, 0)),
            Err(SelfRecoveryPolicyError::ZeroRefillCycles)
        );
    }

    #[test]
    fn self_recovery_policy_rejects_daily_cap_below_refill() {
        assert_eq!(
            SelfRecoveryPolicy::validate(&self_recovery_args(1_000, 10, 1, 100)),
            Err(SelfRecoveryPolicyError::DailyCapBelowRefill)
        );
    }

    #[test]
    fn self_recovery_policy_accepts_daily_cap_equal_to_refill() {
        assert!(SelfRecoveryPolicy::validate(&self_recovery_args(1_000, 100, 1, 100)).is_ok());
    }

    #[test]
    fn self_recovery_policy_rejects_protected_reserve_below_refill() {
        assert_eq!(
            SelfRecoveryPolicy::validate(&self_recovery_args(10, 1_000, 1, 100)),
            Err(SelfRecoveryPolicyError::ProtectedReserveBelowRefill)
        );
    }

    #[test]
    fn self_recovery_policy_accepts_protected_reserve_equal_to_refill() {
        assert!(SelfRecoveryPolicy::validate(&self_recovery_args(100, 1_000, 1, 100)).is_ok());
    }

    #[test]
    fn self_recovery_policy_rejects_oversized_field() {
        let mut args = self_recovery_args(1_000, 100, 1, 10);
        args.protected_reserve_cycles = Nat::from(u128::MAX) + Nat::from(1u64);
        assert_eq!(
            SelfRecoveryPolicy::validate(&args),
            Err(SelfRecoveryPolicyError::CyclesValueOverflow)
        );
    }

    #[test]
    fn self_recovery_policy_accepts_valid_values_and_round_trips() {
        let args = self_recovery_args(1_000, 100, 1, 10);
        let policy = SelfRecoveryPolicy::validate(&args).unwrap();
        assert_eq!(policy.protected_reserve_cycles(), 1_000);
        assert_eq!(policy.daily_cap_cycles(), 100);
        assert_eq!(policy.low_balance_threshold_cycles(), 1);
        assert_eq!(policy.refill_cycles(), 10);
        assert_eq!(policy.to_args(), args);
    }

    // ─── SelfRecoveryState ───

    #[test]
    fn self_recovery_state_begin_suppresses_distribution() {
        let state = SelfRecoveryState::new();
        assert!(!state.is_suppressing_distribution());
        let started = state.begin(1).unwrap();
        assert!(started.is_suppressing_distribution());
        assert_eq!(started.in_flight_operation_id(), Some(1));
    }

    #[test]
    fn self_recovery_state_begin_rejects_second_start_while_unresolved() {
        let state = SelfRecoveryState::new().begin(1).unwrap();
        assert_eq!(state.begin(2), Err(SelfRecoveryStateError::AlreadyInFlight));
    }

    #[test]
    fn self_recovery_state_complete_clears_suppression_and_records_time() {
        let state = SelfRecoveryState::new().begin(1).unwrap();
        let completed = state.complete(500);
        assert!(!completed.is_suppressing_distribution());
        assert_eq!(completed.in_flight_operation_id(), None);
        assert_eq!(completed.last_recovery_at_secs(), Some(500));
    }

    // ─── Public read models ───

    #[test]
    fn public_page_accepts_exactly_max_items() {
        let items: Vec<u32> = (0..MAX_PUBLIC_PAGE as u32).collect();
        assert!(PublicPage::try_new(items, None).is_ok());
    }

    #[test]
    fn public_page_rejects_one_item_over_max() {
        let items: Vec<u32> = (0..=(MAX_PUBLIC_PAGE as u32)).collect();
        assert_eq!(
            PublicPage::try_new(items, None).unwrap_err(),
            PageError::TooManyItems
        );
    }

    fn public_target_row(target: Principal, recent_topups: RecentTopups) -> PublicTargetRow {
        let advisory = AdvisoryCyclesBalance::from_nat(&Nat::from(7u64));
        PublicTargetRow {
            principal: target,
            display_name: "Sample".to_string(),
            project: "Rumi Protocol".to_string(),
            environment: Environment::Production,
            criticality: Criticality::Standard,
            observation_mode: ObservationMode::SelfReport,
            state: PublicTargetState::Healthy,
            advisory_balance_cycles: advisory.to_nat(),
            advisory_balance_overflowed: advisory.is_overflow(),
            low_balance_threshold_cycles: Nat::from(1u64),
            refill_cycles: Nat::from(10u64),
            burn_cycles_per_day: Some(Nat::from(50u64)),
            runway_secs: Some(3_600),
            as_of_secs: 100,
            last_success_at_secs: Some(90),
            stale_for_secs: None,
            next_sample_at_secs: Some(400),
            recent_topups,
        }
    }

    #[test]
    fn public_target_row_carries_nat_balance_at_boundary() {
        let target = target_principal(1);
        let row = public_target_row(target, RecentTopups::new(Vec::new()).unwrap());
        assert_eq!(row.principal, target);
        assert_eq!(row.advisory_balance_cycles, Nat::from(7u64));
        assert_eq!(row.observation_mode, ObservationMode::SelfReport);
    }

    #[test]
    fn public_target_row_truthfully_labels_overflowed_balance() {
        let overflowed = AdvisoryCyclesBalance::from_nat(&(Nat::from(u128::MAX) + Nat::from(1u64)));
        assert!(overflowed.is_overflow());
        assert_eq!(overflowed.to_nat(), Nat::from(u128::MAX));

        let exact = AdvisoryCyclesBalance::from_nat(&Nat::from(u128::MAX));
        assert!(!exact.is_overflow());
        // A genuine u128::MAX report renders identically to a clamped
        // overflow report, which is exactly why the flag must exist
        // alongside it rather than being inferred from the Nat value.
        assert_eq!(exact.to_nat(), overflowed.to_nat());
    }

    fn topup_summary(operation_id: u64) -> PublicTopupSummary {
        PublicTopupSummary {
            operation_id,
            rail: FundingRail::CyclesLedger,
            outcome: FundingOutcome::Completed,
            amount_cycles: Nat::from(1_000u64),
            resolved_at_secs: 500,
        }
    }

    #[test]
    fn public_topup_summary_from_terminal_funding_summary_drops_target() {
        let target = target_principal(1);
        let op = FundingOperation::open(
            7,
            target,
            1,
            test_funding_policy(),
            FundingTrigger::LowBalanceAutoTopup,
            FundingRailArguments::Icp(icp_cmc_snapshot(target)),
            10,
            800,
        )
        .unwrap()
        .record_attempt(
            FundingOperationState::Icp(IcpFundingState::LedgerSubmitted),
            820,
            FundingAttemptResultClass::Success,
        )
        .unwrap()
        .record_attempt(
            FundingOperationState::Icp(IcpFundingState::TransferConfirmed),
            850,
            FundingAttemptResultClass::Success,
        )
        .unwrap()
        .record_attempt(
            FundingOperationState::Icp(IcpFundingState::NotifyPending),
            870,
            FundingAttemptResultClass::Success,
        )
        .unwrap()
        .record_attempt(
            FundingOperationState::Icp(IcpFundingState::Refunded),
            900,
            FundingAttemptResultClass::Success,
        )
        .unwrap();
        let summary = TerminalFundingSummary::from_resolved(&op, 900).unwrap();
        let public = PublicTopupSummary::from(&summary);
        assert_eq!(public.operation_id, 7);
        assert_eq!(public.rail, FundingRail::IcpCmc);
        assert_eq!(public.outcome, FundingOutcome::Refunded);
        assert_eq!(public.amount_cycles, Nat::from(10u64));
        assert_eq!(public.resolved_at_secs, 900);
    }

    #[test]
    fn recent_topups_accepts_exactly_max_entries() {
        let entries: Vec<PublicTopupSummary> = (0..MAX_RECENT_TOPUPS_PER_TARGET as u64)
            .map(topup_summary)
            .collect();
        assert!(RecentTopups::new(entries).is_ok());
    }

    #[test]
    fn recent_topups_rejects_one_entry_over_max() {
        let entries: Vec<PublicTopupSummary> = (0..=MAX_RECENT_TOPUPS_PER_TARGET as u64)
            .map(topup_summary)
            .collect();
        assert_eq!(
            RecentTopups::new(entries).unwrap_err(),
            BoundedFieldError::TooMany
        );
    }

    #[test]
    fn public_target_row_carries_bounded_recent_topups() {
        let recent_topups = RecentTopups::new(vec![topup_summary(1)]).unwrap();
        let row = public_target_row(target_principal(1), recent_topups);
        assert_eq!(row.recent_topups.as_slice().len(), 1);
    }

    #[test]
    fn public_alarm_from_alarm_excludes_nothing_but_stays_a_distinct_type() {
        let alarm = Alarm {
            id: 1,
            target: Some(target_principal(1)),
            kind: AlarmKind::LowBalance,
            status: AlarmStatus::Open,
            opened_at_secs: 100,
            acknowledged_at_secs: None,
            resolved_at_secs: None,
        };
        let public = PublicAlarm::from(&alarm);
        assert_eq!(public.id, alarm.id);
        assert_eq!(public.target, alarm.target);
        assert_eq!(public.kind, alarm.kind);
        assert_eq!(public.status, alarm.status);
        assert_eq!(public.opened_at_secs, alarm.opened_at_secs);
    }

    #[test]
    fn public_overview_carries_all_six_state_counts_and_aggregates() {
        let overview = PublicOverview {
            target_count: 10,
            healthy_count: 4,
            low_count: 2,
            stopped_count: 1,
            uninstalled_count: 1,
            unreachable_count: 1,
            unobserved_count: 1,
            total_observed_cycles: Nat::from(1_000_000u64),
            runtime_cycles: Nat::from(500_000u64),
            cycles_ledger_available_cycles: Nat::from(2_000_000u64),
            icp_available_e8s: Nat::from(100_000_000u64),
            protected_self_reserve_cycles: Nat::from(300_000u64),
            alarm_count: 3,
            last_sample_at_secs: Some(100),
            next_sample_at_secs: Some(400),
        };
        assert_eq!(
            overview.healthy_count
                + overview.low_count
                + overview.stopped_count
                + overview.uninstalled_count
                + overview.unreachable_count
                + overview.unobserved_count,
            overview.target_count
        );
    }

    // ─── Finding G: decode-time invariant re-validation ───
    //
    // Every test below builds wire bytes for a value a checked constructor
    // would reject, then asserts `Decode!` itself rejects it too — proving
    // decode can no longer bypass the invariant the way a struct-literal
    // derive used to. Mirror ("Raw*ForTest") structs exist only where the
    // real type has no already-public identically-shaped counterpart to
    // encode directly (e.g. `*Args`); they carry the exact same field names
    // and types as the real type's own fields, matching what its custom
    // `Deserialize` impl decodes through, so the wire bytes they produce are
    // the same bytes a legacy/malicious/hand-restored blob would carry.

    #[test]
    fn governance_timelocks_decode_rejects_all_zero_timelocks() {
        // This is the exact scenario the Task 1b Finding G proof-of-concept
        // demonstrated: `validate` rejects all-zero timelocks, but the
        // pre-fix derived `Deserialize` accepted them anyway.
        let zero_args = GovernanceTimelocksArgs {
            target_registry_secs: 0,
            spend_policy_secs: 0,
            signer_change_secs: 0,
            unpause_secs: 0,
        };
        assert!(GovernanceTimelocks::validate(&zero_args).is_err());
        let bytes = Encode!(&zero_args).unwrap();
        assert!(Decode!(&bytes, GovernanceTimelocks).is_err());
    }

    #[test]
    fn governance_timelocks_decode_accepts_valid_round_trip() {
        let args = governance_timelocks_args();
        let bytes = Encode!(&args).unwrap();
        let timelocks = Decode!(&bytes, GovernanceTimelocks).unwrap();
        assert_eq!(timelocks.unpause_secs(), args.unpause_secs);
    }

    #[test]
    fn global_policy_decode_rejects_zero_daily_cap() {
        let mut args = global_policy_args(1_000);
        args.global_daily_cap_cycles = Nat::from(0u64);
        let bytes = Encode!(&args).unwrap();
        assert!(Decode!(&bytes, GlobalPolicy).is_err());
    }

    #[test]
    fn global_policy_decode_accepts_valid_round_trip() {
        let args = global_policy_args(500);
        let bytes = Encode!(&args).unwrap();
        let policy = Decode!(&bytes, GlobalPolicy).unwrap();
        assert_eq!(policy, GlobalPolicy::validate(&args).unwrap());
    }

    #[test]
    fn target_funding_policy_decode_rejects_zero_refill() {
        let args = funding_policy_args(1, 0, 100);
        let bytes = Encode!(&args).unwrap();
        assert!(Decode!(&bytes, TargetFundingPolicy).is_err());
    }

    #[test]
    fn target_funding_policy_decode_accepts_valid_round_trip() {
        let args = funding_policy_args(1, 10, 100);
        let bytes = Encode!(&args).unwrap();
        let policy = Decode!(&bytes, TargetFundingPolicy).unwrap();
        assert_eq!(policy.refill_cycles(), 10);
    }

    #[test]
    fn bounded_name_decode_rejects_empty() {
        let bytes = Encode!(&String::new()).unwrap();
        assert!(Decode!(&bytes, BoundedName).is_err());
    }

    #[test]
    fn bounded_tags_decode_rejects_duplicate_tag() {
        let bytes = Encode!(&vec!["cycles".to_string(), "cycles".to_string()]).unwrap();
        assert!(Decode!(&bytes, BoundedTags).is_err());
    }

    #[test]
    fn bounded_name_decode_accepts_valid_round_trip() {
        let bytes = Encode!(&"Sample Target".to_string()).unwrap();
        let name = Decode!(&bytes, BoundedName).unwrap();
        assert_eq!(name.as_str(), "Sample Target");
    }

    #[test]
    fn bounded_tags_decode_accepts_valid_round_trip() {
        let bytes = Encode!(&vec!["cycles".to_string(), "sentinel".to_string()]).unwrap();
        let tags = Decode!(&bytes, BoundedTags).unwrap();
        assert_eq!(
            tags.as_slice(),
            &["cycles".to_string(), "sentinel".to_string()]
        );
    }

    #[test]
    fn self_recovery_policy_decode_rejects_zero_refill() {
        let args = self_recovery_args(1_000, 100, 1, 0);
        let bytes = Encode!(&args).unwrap();
        assert!(Decode!(&bytes, SelfRecoveryPolicy).is_err());
    }

    #[test]
    fn self_recovery_policy_decode_accepts_valid_round_trip() {
        let args = self_recovery_args(1_000, 100, 1, 10);
        let bytes = Encode!(&args).unwrap();
        let policy = Decode!(&bytes, SelfRecoveryPolicy).unwrap();
        assert_eq!(policy.refill_cycles(), 10);
    }

    #[test]
    fn recent_topups_decode_rejects_oversized_vector() {
        let entries: Vec<PublicTopupSummary> = (0..=MAX_RECENT_TOPUPS_PER_TARGET as u64)
            .map(topup_summary)
            .collect();
        let bytes = Encode!(&entries).unwrap();
        assert!(Decode!(&bytes, RecentTopups).is_err());
    }

    #[test]
    fn recent_topups_decode_accepts_valid_round_trip() {
        let entries: Vec<PublicTopupSummary> = vec![topup_summary(1)];
        let bytes = Encode!(&entries).unwrap();
        let topups = Decode!(&bytes, RecentTopups).unwrap();
        assert_eq!(topups.as_slice().len(), 1);
    }

    fn attempt_record(ordinal: u32) -> FundingAttemptRecord {
        FundingAttemptRecord {
            ordinal,
            phase: FundingOperationState::Cycles(CyclesFundingState::Submitted),
            at_secs: ordinal as u64,
            result_class: FundingAttemptResultClass::RetryableFailure,
        }
    }

    #[test]
    fn funding_attempts_decode_rejects_oversized_vector() {
        let records: Vec<FundingAttemptRecord> = (1..=(MAX_FUNDING_ATTEMPTS as u32 + 1))
            .map(attempt_record)
            .collect();
        let bytes = Encode!(&records).unwrap();
        assert!(Decode!(&bytes, FundingAttempts).is_err());
    }

    #[test]
    fn funding_attempts_decode_rejects_non_sequential_ordinals() {
        let records = vec![attempt_record(1), attempt_record(3)];
        let bytes = Encode!(&records).unwrap();
        assert!(Decode!(&bytes, FundingAttempts).is_err());
    }

    #[test]
    fn funding_attempts_decode_accepts_valid_round_trip() {
        let records = vec![attempt_record(1), attempt_record(2)];
        let bytes = Encode!(&records).unwrap();
        let attempts = Decode!(&bytes, FundingAttempts).unwrap();
        assert_eq!(attempts.len(), 2);
    }

    #[derive(CandidType, Serialize)]
    struct RawTargetRecordForTest {
        principal: Principal,
        revision: u64,
        display_name: BoundedName,
        project: BoundedName,
        environment: Environment,
        criticality: Criticality,
        observation_mode: ObservationMode,
        tags: BoundedTags,
        funding_policy: TargetFundingPolicy,
        enabled: bool,
        auto_topup: bool,
        paused: bool,
    }

    fn raw_target_record_for_test(
        revision: u64,
        enabled: bool,
        auto_topup: bool,
        observation_mode: ObservationMode,
    ) -> RawTargetRecordForTest {
        RawTargetRecordForTest {
            principal: target_principal(1),
            revision,
            display_name: BoundedName::new("Sample Target").unwrap(),
            project: BoundedName::new("Rumi Protocol").unwrap(),
            environment: Environment::Production,
            criticality: Criticality::Standard,
            observation_mode,
            tags: BoundedTags::new(vec!["cycles".to_string()]).unwrap(),
            funding_policy: test_funding_policy(),
            enabled,
            auto_topup,
            paused: false,
        }
    }

    #[test]
    fn target_record_decode_rejects_zero_revision() {
        let raw = raw_target_record_for_test(0, false, false, ObservationMode::Unobserved);
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, TargetRecord).is_err());
    }

    #[test]
    fn target_record_decode_rejects_auto_topup_without_enabled() {
        let raw = raw_target_record_for_test(1, false, true, ObservationMode::SelfReport);
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, TargetRecord).is_err());
    }

    #[test]
    fn target_record_decode_rejects_auto_topup_while_unobserved() {
        let raw = raw_target_record_for_test(1, true, true, ObservationMode::Unobserved);
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, TargetRecord).is_err());
    }

    #[test]
    fn target_record_decode_accepts_valid_round_trip() {
        let raw = raw_target_record_for_test(3, true, true, ObservationMode::SelfReport);
        let bytes = Encode!(&raw).unwrap();
        let record = Decode!(&bytes, TargetRecord).unwrap();
        assert_eq!(record.revision(), 3);
        assert!(record.enabled());
        assert!(record.auto_topup());
    }

    #[derive(CandidType, Serialize)]
    struct RawProposalRecordForTest {
        id: u64,
        payload: ProposalPayload,
        proposer: Principal,
        approvals: Vec<Principal>,
        status: ProposalStatus,
        created_at_secs: u64,
    }

    #[test]
    fn proposal_record_decode_rejects_duplicate_approval() {
        let dup = signer(1);
        let raw = RawProposalRecordForTest {
            id: 1,
            payload: ProposalPayload::AddSigner { signer: signer(2) },
            proposer: signer(3),
            approvals: vec![dup, dup],
            status: ProposalStatus::Open,
            created_at_secs: 0,
        };
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, ProposalRecord).is_err());
    }

    #[test]
    fn proposal_record_decode_accepts_valid_round_trip_with_approvals() {
        let raw = RawProposalRecordForTest {
            id: 1,
            payload: ProposalPayload::AddSigner { signer: signer(2) },
            proposer: signer(3),
            approvals: vec![signer(1), signer(4)],
            status: ProposalStatus::Open,
            created_at_secs: 0,
        };
        let bytes = Encode!(&raw).unwrap();
        let record = Decode!(&bytes, ProposalRecord).unwrap();
        assert_eq!(record.approval_count(), 2);
        assert!(record.has_approved(signer(1)));
    }

    #[derive(CandidType, Serialize)]
    struct RawRollingSpendLedgerForTest {
        settled: Vec<SpendEntry>,
        pending: Vec<PendingReservation>,
    }

    #[test]
    fn rolling_spend_ledger_decode_rejects_oversized_pending_vector() {
        let pending: Vec<PendingReservation> = (0..(MAX_PENDING_RESERVATIONS as u64 + 1))
            .map(|i| PendingReservation {
                operation_id: i,
                amount_cycles: 1,
                reserved_at_secs: 0,
            })
            .collect();
        let raw = RawRollingSpendLedgerForTest {
            settled: Vec::new(),
            pending,
        };
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, RollingSpendLedger).is_err());
    }

    #[test]
    fn rolling_spend_ledger_decode_rejects_duplicate_pending_operation_id() {
        let raw = RawRollingSpendLedgerForTest {
            settled: Vec::new(),
            pending: vec![
                PendingReservation {
                    operation_id: 1,
                    amount_cycles: 5,
                    reserved_at_secs: 0,
                },
                PendingReservation {
                    operation_id: 1,
                    amount_cycles: 5,
                    reserved_at_secs: 1,
                },
            ],
        };
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, RollingSpendLedger).is_err());
    }

    #[test]
    fn rolling_spend_ledger_decode_accepts_valid_round_trip() {
        let raw = RawRollingSpendLedgerForTest {
            settled: vec![SpendEntry {
                settled_at_secs: 0,
                amount_cycles: 10,
            }],
            pending: vec![PendingReservation {
                operation_id: 1,
                amount_cycles: 5,
                reserved_at_secs: 1,
            }],
        };
        let bytes = Encode!(&raw).unwrap();
        let ledger = Decode!(&bytes, RollingSpendLedger).unwrap();
        assert_eq!(ledger.settled().len(), 1);
        assert_eq!(ledger.pending().len(), 1);
    }

    #[derive(CandidType, Serialize)]
    struct RawTargetReservationStateForTest {
        rolling_spend: RawRollingSpendLedgerForTest,
        cooldown_until_secs: u64,
    }

    #[test]
    fn target_reservation_state_decode_rejects_more_than_one_pending() {
        // A per-target ledger with two pending entries stays within
        // `RollingSpendLedger`'s own `MAX_PENDING_RESERVATIONS` bound, so
        // only `TargetReservationState`'s own "at most one pending"
        // decode check — never reachable through
        // `TargetReservationState::reserve`'s `OperationInFlight` guard —
        // can catch this.
        let raw = RawTargetReservationStateForTest {
            rolling_spend: RawRollingSpendLedgerForTest {
                settled: Vec::new(),
                pending: vec![
                    PendingReservation {
                        operation_id: 1,
                        amount_cycles: 10,
                        reserved_at_secs: 0,
                    },
                    PendingReservation {
                        operation_id: 2,
                        amount_cycles: 5,
                        reserved_at_secs: 1,
                    },
                ],
            },
            cooldown_until_secs: 0,
        };
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, TargetReservationState).is_err());
    }

    #[test]
    fn target_reservation_state_decode_accepts_valid_round_trip() {
        let raw = RawTargetReservationStateForTest {
            rolling_spend: RawRollingSpendLedgerForTest {
                settled: vec![SpendEntry {
                    settled_at_secs: 0,
                    amount_cycles: 10,
                }],
                pending: vec![PendingReservation {
                    operation_id: 1,
                    amount_cycles: 5,
                    reserved_at_secs: 1,
                }],
            },
            cooldown_until_secs: 42,
        };
        let bytes = Encode!(&raw).unwrap();
        let state = Decode!(&bytes, TargetReservationState).unwrap();
        assert_eq!(state.in_flight_operation_id(), Some(1));
        assert_eq!(state.rolling_spend().settled().len(), 1);
    }

    #[derive(CandidType, Serialize)]
    struct RawGlobalRollingSpendStateForTest {
        rolling_spend: RawRollingSpendLedgerForTest,
    }

    #[test]
    fn global_rolling_spend_state_decode_accepts_valid_round_trip() {
        // Unlike `TargetReservationState`, many pending entries are
        // legitimate here — this is the positive coverage that
        // distinguishes the two wrappers' invariants.
        let raw = RawGlobalRollingSpendStateForTest {
            rolling_spend: RawRollingSpendLedgerForTest {
                settled: Vec::new(),
                pending: vec![
                    PendingReservation {
                        operation_id: 1,
                        amount_cycles: 5,
                        reserved_at_secs: 0,
                    },
                    PendingReservation {
                        operation_id: 2,
                        amount_cycles: 7,
                        reserved_at_secs: 1,
                    },
                ],
            },
        };
        let bytes = Encode!(&raw).unwrap();
        let state = Decode!(&bytes, GlobalRollingSpendState).unwrap();
        assert_eq!(state.rolling_spend().pending().len(), 2);
    }

    #[derive(CandidType, Serialize)]
    struct RawFundingOperationForTest {
        id: u64,
        target: Principal,
        target_registry_revision: u64,
        funding_policy: TargetFundingPolicy,
        trigger: FundingTrigger,
        rail_arguments: FundingRailArguments,
        reserved_amount_cycles: u128,
        state: FundingOperationState,
        attempts: FundingAttempts,
        confirmed_block_index: Option<u64>,
        created_at_secs: u64,
        updated_at_secs: u64,
    }

    fn raw_funding_operation_for_test(op: &FundingOperation) -> RawFundingOperationForTest {
        RawFundingOperationForTest {
            id: op.id(),
            target: op.target(),
            target_registry_revision: op.target_registry_revision(),
            funding_policy: op.funding_policy().clone(),
            trigger: op.trigger(),
            rail_arguments: op.rail_arguments().clone(),
            reserved_amount_cycles: op.reserved_amount_cycles(),
            state: op.state(),
            attempts: op.attempts().clone(),
            confirmed_block_index: op.confirmed_block_index(),
            created_at_secs: op.created_at_secs(),
            updated_at_secs: op.updated_at_secs(),
        }
    }

    #[test]
    fn funding_operation_decode_rejects_rail_mismatch() {
        let op = open_cycles_operation(1, target_principal(1), 0);
        let mut raw = raw_funding_operation_for_test(&op);
        raw.state = FundingOperationState::Icp(IcpFundingState::PlannedReserved);
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, FundingOperation).is_err());
    }

    #[test]
    fn funding_operation_decode_rejects_reserved_amount_mismatch() {
        let op = open_cycles_operation(1, target_principal(1), 0);
        let mut raw = raw_funding_operation_for_test(&op);
        raw.reserved_amount_cycles = op.reserved_amount_cycles() + 1;
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, FundingOperation).is_err());
    }

    #[test]
    fn funding_operation_decode_rejects_confirmed_block_on_cycles_rail() {
        let op = open_cycles_operation(1, target_principal(1), 0);
        let mut raw = raw_funding_operation_for_test(&op);
        raw.confirmed_block_index = Some(5);
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, FundingOperation).is_err());
    }

    #[test]
    fn funding_operation_decode_accepts_valid_round_trip() {
        let op = open_cycles_operation(1, target_principal(1), 0);
        let raw = raw_funding_operation_for_test(&op);
        let bytes = Encode!(&raw).unwrap();
        let decoded = Decode!(&bytes, FundingOperation).unwrap();
        assert_eq!(decoded, op);
    }

    // Finding H regression tests: `state`/`attempts`/`updated_at_secs` must
    // be mutually consistent, exactly as `open`/`record_attempt` guarantee.

    #[test]
    fn funding_operation_decode_rejects_complete_state_with_empty_attempts() {
        // Finding H, Case 1: a decoded operation could previously claim
        // `state == Complete` with an empty `attempts` vector — no evidence
        // any attempt was ever made.
        let op = open_cycles_operation(1, target_principal(1), 0);
        let mut raw = raw_funding_operation_for_test(&op);
        raw.state = FundingOperationState::Cycles(CyclesFundingState::Complete);
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, FundingOperation).is_err());
    }

    #[test]
    fn funding_operation_decode_rejects_state_disagreeing_with_last_attempt_phase() {
        // Finding H, Case 2: `state` claims `Complete` while the attempt
        // log's own last entry says `Submitted`.
        let op = open_cycles_operation(1, target_principal(1), 0)
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        let mut raw = raw_funding_operation_for_test(&op);
        raw.state = FundingOperationState::Cycles(CyclesFundingState::Complete);
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, FundingOperation).is_err());
    }

    #[test]
    fn funding_operation_decode_rejects_updated_at_ahead_of_created_at_with_empty_attempts() {
        // Finding H, Case 3: `updated_at_secs` far in the future of
        // `created_at_secs` with zero attempts recorded.
        let op = open_cycles_operation(1, target_principal(1), 1_000);
        let mut raw = raw_funding_operation_for_test(&op);
        raw.updated_at_secs = 5_000;
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, FundingOperation).is_err());
    }

    #[test]
    fn funding_operation_decode_rejects_terminal_state_skipping_intermediate_phases() {
        // Finding H, Case 4: `Complete` reached through a single attempt
        // that skips every intermediate `Submitted`/`Confirmed` step
        // `is_valid_successor` would have required through the real state
        // machine.
        let op = open_cycles_operation(1, target_principal(1), 0);
        let mut raw = raw_funding_operation_for_test(&op);
        raw.attempts = FundingAttempts::new()
            .record(
                FundingOperationState::Cycles(CyclesFundingState::Complete),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        raw.state = FundingOperationState::Cycles(CyclesFundingState::Complete);
        raw.updated_at_secs = 10;
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, FundingOperation).is_err());
    }

    #[test]
    fn funding_operation_decode_rejects_attempt_on_a_different_rail() {
        // A middle attempt entry switches to the ICP rail's phase enum
        // while the operation itself stays on the Cycles rail throughout —
        // `state` and the final attempt still agree, so only the
        // successor-chain replay (not the top-level rail check or the
        // final-attempt check) can catch this.
        let op = open_cycles_operation(1, target_principal(1), 0);
        let mut raw = raw_funding_operation_for_test(&op);
        raw.attempts = FundingAttempts::new()
            .record(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                10,
                FundingAttemptResultClass::Success,
            )
            .unwrap()
            .record(
                FundingOperationState::Icp(IcpFundingState::LedgerSubmitted),
                20,
                FundingAttemptResultClass::Success,
            )
            .unwrap()
            .record(
                FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                30,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        raw.state = FundingOperationState::Cycles(CyclesFundingState::Confirmed);
        raw.updated_at_secs = 30;
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, FundingOperation).is_err());
    }

    #[test]
    fn funding_operation_decode_rejects_decreasing_attempt_timestamps() {
        let op = open_cycles_operation(1, target_principal(1), 0);
        let mut raw = raw_funding_operation_for_test(&op);
        raw.attempts = FundingAttempts::new()
            .record(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                100,
                FundingAttemptResultClass::Success,
            )
            .unwrap()
            .record(
                FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                50,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        raw.state = FundingOperationState::Cycles(CyclesFundingState::Confirmed);
        raw.updated_at_secs = 50;
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, FundingOperation).is_err());
    }

    #[test]
    fn funding_operation_decode_rejects_attempt_before_created_at() {
        let op = open_cycles_operation(1, target_principal(1), 1_000);
        let mut raw = raw_funding_operation_for_test(&op);
        raw.attempts = FundingAttempts::new()
            .record(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                500,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        raw.state = FundingOperationState::Cycles(CyclesFundingState::Submitted);
        raw.updated_at_secs = 500;
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, FundingOperation).is_err());
    }

    #[test]
    fn funding_operation_decode_accepts_multi_step_round_trip_with_same_state_retry() {
        let op = open_cycles_operation(1, target_principal(1), 0)
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                10,
                FundingAttemptResultClass::RetryableFailure,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Submitted),
                20,
                FundingAttemptResultClass::RetryableFailure,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Confirmed),
                30,
                FundingAttemptResultClass::Success,
            )
            .unwrap()
            .record_attempt(
                FundingOperationState::Cycles(CyclesFundingState::Complete),
                40,
                FundingAttemptResultClass::Success,
            )
            .unwrap();
        let raw = raw_funding_operation_for_test(&op);
        let bytes = Encode!(&raw).unwrap();
        let decoded = Decode!(&bytes, FundingOperation).unwrap();
        assert_eq!(decoded, op);
        assert_eq!(decoded.attempts().len(), 4);
    }

    #[derive(CandidType, Serialize)]
    struct RawTerminalFundingSummaryForTest {
        operation_id: u64,
        target: Principal,
        rail: FundingRail,
        outcome: FundingOutcome,
        amount_cycles: u128,
        resolved_at_secs: u64,
    }

    #[test]
    fn terminal_funding_summary_decode_rejects_refunded_on_cycles_rail() {
        let raw = RawTerminalFundingSummaryForTest {
            operation_id: 1,
            target: target_principal(1),
            rail: FundingRail::CyclesLedger,
            outcome: FundingOutcome::Refunded,
            amount_cycles: 10,
            resolved_at_secs: 100,
        };
        let bytes = Encode!(&raw).unwrap();
        assert!(Decode!(&bytes, TerminalFundingSummary).is_err());
    }

    #[test]
    fn terminal_funding_summary_decode_accepts_valid_round_trip() {
        let raw = RawTerminalFundingSummaryForTest {
            operation_id: 1,
            target: target_principal(1),
            rail: FundingRail::IcpCmc,
            outcome: FundingOutcome::Refunded,
            amount_cycles: 10,
            resolved_at_secs: 100,
        };
        let bytes = Encode!(&raw).unwrap();
        let summary = Decode!(&bytes, TerminalFundingSummary).unwrap();
        assert_eq!(summary.outcome(), FundingOutcome::Refunded);
    }
}
