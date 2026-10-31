//! Candid-facing types of the Sui bridge instance (BB-52 skeleton).

use crate::graphql::MirrorSettings;
use crate::ids::{SuiNetwork, SuiRole};
use candid::{CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use serde::Serialize;

pub use crate::accounting::Transition;

/// Install and upgrade argument. Decoded once per install, so the size difference between the
/// variants costs nothing.
#[derive(CandidType, Deserialize, Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum BridgeArg {
    Init(InitArgs),
    Upgrade(UpgradeArgs),
}

/// One canister serves one network. Both fields are required and must agree: the 64-hex
/// chain identifier must be the genesis checkpoint digest of the declared network (only Sui
/// mainnet and testnet are accepted; anything else is refused, never guessed).
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct InitArgs {
    pub network: SuiNetwork,
    /// 64 lowercase hex digits: the genesis checkpoint digest.
    pub chain_identifier: String,
    /// Provider set and quorum. Optional, and disabled by default.
    pub mirror: Option<MirrorSettings>,
    /// Canonical icUSD ICRC-1/2/3 ledger (or a synthetic test ledger on testnet, D9).
    pub icusd_ledger: Principal,
    /// Principals that may pause and tune limits and fees. They cannot move escrow, change the
    /// ledger or the network, or touch the role keys.
    pub admins: Vec<Principal>,
    /// Destination for swept fees. When absent, sweeps are refused.
    pub fee_recipient: Option<Account>,
    /// Starting fees and limits. Absent: no fees and the default limits.
    pub fees: Option<BridgeFees>,
    pub limits: Option<BridgeLimits>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct UpgradeArgs {
    /// Replaces the provider set when present. Nothing else can change at upgrade except the
    /// admin list: the network, the ledger and the gate binding are fixed for the life of the
    /// instance.
    pub mirror: Option<MirrorSettings>,
    /// Optional replacement admin list. Upgrades are controller-only, so this is a controller
    /// action.
    pub admins: Option<Vec<Principal>>,
}

/// The exact identities of the deployed gate (`move/rumi_icusd_gate`), as the operator states
/// them. They are configuration, not facts the canister has observed: the bind check that
/// reads the chain belongs to BB-53.
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct GateBindingArg {
    /// Each id is `0x` followed by 64 lowercase hex digits.
    pub package_id: String,
    pub gate_id: String,
    pub mint_cap_id: String,
    pub admin_cap_id: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct GateBindingView {
    pub package_id: String,
    pub gate_id: String,
    pub mint_cap_id: String,
    pub admin_cap_id: String,
    /// `<package>::rumi_icusd_gate::Gate`
    pub gate_type: String,
    /// `<package>::rumi_icusd_gate::RUMI_ICUSD_GATE`
    pub coin_type: String,
    pub decimals: u8,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct RoleView {
    pub role: SuiRole,
    /// The threshold-Schnorr derivation path, each element in hex.
    pub derivation_path: Vec<String>,
    /// 32-byte Ed25519 public key in hex, once derived.
    pub public_key: Option<String>,
    /// The Sui address (`0x` plus 64 hex), once derived.
    pub address: Option<String>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ConfigView {
    pub initialized: bool,
    pub network: SuiNetwork,
    pub chain_identifier: String,
    /// JSON-RPC's form, display only: the first four bytes of the chain identifier, in hex.
    pub short_chain_id: String,
    pub schnorr_key_name: String,
    pub instance_salt: String,
    pub binding: Option<GateBindingView>,
    pub mirror: MirrorSettings,
    pub bridge_version: String,
    pub schema_version: u32,
    /// LEGACY and NON-AUTHORITATIVE: a placeholder from the BB-52 foundation that is always false. It does
    /// not reflect whether the mint lane can sign and it enforces nothing. The readiness seam is
    /// `get_mint_lane` (`ready` and `blocked_by`); use that, not this field.
    pub signing_enabled: bool,
    pub icusd_ledger: Principal,
    pub admins: Vec<Principal>,
    pub fee_recipient: Option<Account>,
    pub fees: BridgeFees,
    pub limits: BridgeLimits,
    pub pauses: Pauses,
    /// Why the bridge is halted, if it is.
    pub halted: Option<String>,
    /// Hex of the escrow subaccount on the icUSD ledger.
    pub escrow_subaccount: String,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum SuiBridgeError {
    Anonymous,
    Unauthorized,
    InvalidArgument(String),
    /// A set-once value is already set (to something else).
    Conflict(String),
    NotConfigured(String),
    SignerFailed(String),
    Paused(String),
    Halted(String),
    /// The Sui recipient is not an acceptable wallet.
    InvalidRecipient(String),
    AmountBelowMinimum { min_e8s: u64 },
    AmountAboveMaximum { max_e8s: u64 },
    CapacityExceeded { available_e8s: u64 },
    RateLimited { available_e8s: u64, window_resets_at_ns: u64 },
    FeeChanged { current_fee_e8s: u64 },
    /// The caller's ICRC-2 allowance to the bridge does not cover amount plus ledger fee.
    InsufficientAllowance { allowance_e8s: u64, required_e8s: u64 },
    /// The caller's icUSD balance does not cover amount plus ledger fee.
    InsufficientFunds { balance_e8s: u64, required_e8s: u64 },
    TooManyActiveOperations,
    RequestIdConflict,
    NotFound,
    NotOwner,
    InvalidState(String),
    Ledger(String),
    /// The Sui providers could not give an agreed answer (too few usable, or they disagree).
    Unavailable(String),
    /// What the providers agree on is not the configured deployment.
    DeploymentMismatch(String),
    Internal(String),
}

/// The saga modules (copied from the Solana instance) call the error `BridgeError`.
pub type BridgeError = SuiBridgeError;
pub type BridgeResult<T> = Result<T, SuiBridgeError>;
pub type SuiResult<T> = Result<T, SuiBridgeError>;

// ---------------------------------------------------------------------------
// Fees, limits and pauses (53a)
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct BridgeFees {
    /// Deducted from the minted amount on ICP -> Sui.
    pub deposit_fee_e8s: u64,
    /// Deducted from the released amount on Sui -> ICP (in addition to the ledger fee).
    pub release_fee_e8s: u64,
}

/// Deposit, capacity and rate limits, e8s.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeLimits {
    pub min_deposit_e8s: u64,
    /// Also the per-operation cap of one Sui mint.
    pub max_deposit_e8s: u64,
    /// Cap on `O + PM + H + reserved pulls`: everything that is, or may become, supply.
    pub max_outstanding_e8s: u64,
    /// Maximum deposits admitted per window.
    pub mint_window_limit_e8s: u64,
    /// Maximum releases started per window.
    pub release_window_limit_e8s: u64,
    pub window_seconds: u64,
    pub max_active_per_caller: u32,
    pub max_active_global: u32,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Pauses {
    pub deposits: bool,
    pub mints: bool,
    pub releases: bool,
    pub observer: bool,
}

// ---------------------------------------------------------------------------
// Argument shapes (decoded by ICRC-21 and by the endpoints)
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeToSuiArgs {
    /// Amount pulled into escrow, in e8s (`A`). The ledger fee is charged to the caller on top.
    pub amount_e8s: u64,
    /// `0x` and 64 lowercase hex digits: the Sui address that receives the coin.
    pub sui_recipient: String,
    pub from_subaccount: Option<serde_bytes::ByteBuf>,
    /// 16 to 32 bytes, unique per caller. Repeating it with identical arguments returns the
    /// existing operation.
    pub client_request_id: serde_bytes::ByteBuf,
    /// The deposit fee the caller was shown. Rejected if it no longer matches.
    pub expected_deposit_fee_e8s: u64,
}

// ---------------------------------------------------------------------------
// Deposits and releases
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DepositKindView {
    /// ICP -> Sui deposit by `owner`.
    UserDeposit,
    /// Re-mint of a held return claim to its Sui sender (64 hex `transfer_id`).
    Remint { return_id: String },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DepositStatus {
    PullPending,
    PullUnknown,
    PullFailed { reason: String },
    Escrowed,
    MintInFlight,
    Delivered,
    MintHeld { reason: String },
    Refunding,
    Refunded,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SuiInclusion {
    /// Base58 typed transaction digest of the finalized mint transaction; `None` when delivery was
    /// proven by the consumed mint-id record alone.
    pub tx_digest: Option<String>,
    pub checkpoint: u64,
    /// The Gate's mint sequence number.
    pub sequence: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DepositView {
    pub id: u64,
    pub kind: DepositKindView,
    pub owner: Principal,
    pub from: Option<Account>,
    /// `0x` and 64 hex.
    pub sui_recipient: String,
    pub amount_e8s: u64,
    pub deposit_fee_e8s: u64,
    pub mint_amount_e8s: u64,
    /// 64 hex, once the pull is proven.
    pub mint_id: Option<String>,
    pub status: DepositStatus,
    pub client_request_id: serde_bytes::ByteBuf,
    pub pull_block_index: Option<u64>,
    pub refund_block_index: Option<u64>,
    pub refund_amount_e8s: Option<u64>,
    pub mint_op_id: Option<u64>,
    pub delivered: Option<SuiInclusion>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ReleaseStatus {
    Pending,
    TransferUnknown,
    Released,
    Held { reason: String },
    Reminting { deposit_id: u64 },
    Reminted { deposit_id: u64 },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReleaseView {
    /// 64 hex: the Gate's `transfer_id` (also the memo of the ICP transfer).
    pub return_id: String,
    pub seq: u64,
    /// The Gate's `bridge_out` sequence of this return (first is 1).
    pub gate_sequence: u64,
    /// `0x` and 64 hex: the Sui sender of the return.
    pub sui_sender: String,
    pub client_nonce: u64,
    pub amount_e8s: u64,
    pub release_fee_e8s: u64,
    pub ledger_fee_e8s: Option<u64>,
    pub amount_to_receive_e8s: Option<u64>,
    /// The ICP account the Gate saved (principal bytes and subaccount), immutable.
    pub destination: Account,
    /// Base58 typed digest of the `bridge_out` transaction.
    pub tx_digest: String,
    pub checkpoint: u64,
    pub status: ReleaseStatus,
    pub release_block_index: Option<u64>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ListDepositsArgs {
    pub start_after: Option<u64>,
    pub limit: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ListDepositsForOwnerArgs {
    pub owner: Principal,
    pub start_after: Option<u64>,
    pub limit: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ListReleasesArgs {
    /// The ICP principal the releases are paid to.
    pub owner: Principal,
    pub start_after_seq: Option<u64>,
    pub limit: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ListReleasesForSenderArgs {
    /// `0x` and 64 hex: the Sui address that sent the returns.
    pub sui_sender: String,
    pub start_after_seq: Option<u64>,
    pub limit: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AuditEntry {
    pub seq: u64,
    pub at_ns: u64,
    pub transition: Option<Transition>,
    pub deposit_id: Option<u64>,
    /// 64 hex.
    pub return_id: Option<String>,
    pub op_id: Option<u64>,
    pub note: String,
}

/// The accounting buckets, e8s (`Nat`: the Rust type is `u128`).
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BucketsView {
    pub outstanding: Nat,
    pub pending_mint: Nat,
    pub pending_release: Nat,
    pub pending_refund: Nat,
    pub held: Nat,
    pub fees: Nat,
    pub reserved_pulls: Nat,
    pub unknown_outflow: Nat,
}

impl From<&crate::accounting::Buckets> for BucketsView {
    fn from(b: &crate::accounting::Buckets) -> Self {
        BucketsView {
            outstanding: Nat::from(b.outstanding),
            pending_mint: Nat::from(b.pending_mint),
            pending_release: Nat::from(b.pending_release),
            pending_refund: Nat::from(b.pending_refund),
            held: Nat::from(b.held),
            fees: Nat::from(b.fees),
            reserved_pulls: Nat::from(b.reserved_pulls),
            unknown_outflow: Nat::from(b.unknown_outflow),
        }
    }
}

// ---------------------------------------------------------------------------
// Observed deployment and mint lane views (53b)
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ObservedDeploymentView {
    pub checkpoint: u64,
    /// Base58 digest of the checkpoint the observation was made at.
    pub checkpoint_digest: String,
    pub epoch: u64,
    pub observed_at_ns: u64,
    pub gate_version: u64,
    pub initial_shared_version: u64,
    pub supply_cap: u64,
    pub min_bridge_out: u64,
    pub total_supply: u64,
    pub minted_total: u128,
    pub burned_total: u128,
    pub mint_count: u64,
    pub bridge_out_count: u64,
    pub decimals: u8,
    pub symbol: String,
    pub mint_cap_id: String,
    pub mint_cap_version: u64,
    /// `0x` and 64 hex: who owns the `MintCap` (it must be the treasury role).
    pub mint_cap_owner: String,
    pub admin_cap_id: String,
    pub admin_cap_owner: String,
    /// The role whose address owns the `AdminCap`, if it is one of this canister's.
    pub admin_cap_role: Option<SuiRole>,
    /// False: the published module bytes were not compared with a build.
    pub module_digest_observed: bool,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum AttemptStatusView {
    Open,
    NeverSigned { reason: String },
    FailedFinal { checkpoint: u64, error: String },
    LandedFinal { checkpoint: u64 },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MintAttemptView {
    pub generation: u32,
    /// Base58 typed transaction digest (what `transaction(digest:)` takes).
    pub tx_digest: String,
    /// Hex of the 32-byte signing digest.
    pub signing_digest: String,
    pub epoch: u64,
    pub gas_price: u64,
    pub gas_budget: u64,
    pub mint_cap_version: u64,
    pub gas_coin_version: u64,
    pub treasury_signed: bool,
    pub gas_signed: bool,
    /// A signing reply was lost: a signature for that role may exist that we never saw.
    pub treasury_sig_unknown: bool,
    pub gas_sig_unknown: bool,
    pub sends: u32,
    pub status: AttemptStatusView,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DeliveryViaView {
    Receipt,
    ConsumedRecord,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum MintOpStatusView {
    Active,
    Delivered { tx_digest: Option<String>, checkpoint: u64, sequence: u64, via: DeliveryViaView },
    Failed { reason: String },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MintOpView {
    pub id: u64,
    pub deposit_id: u64,
    pub mint_id: String,
    pub sui_recipient: String,
    pub amount_e8s: u64,
    pub status: MintOpStatusView,
    pub hold_reason: Option<String>,
    pub attempts: Vec<MintAttemptView>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MintLaneView {
    /// Whether a pass would run the lane now. Empty `blocked_by` means yes.
    pub ready: bool,
    pub blocked_by: Vec<String>,
    pub active_op: Option<u64>,
    pub lane_halt: Option<String>,
    /// `0x` and 64 hex.
    pub gas_coin: Option<String>,
    pub gas_budget: u64,
    pub max_attempts: u32,
    pub last_wait: Option<String>,
    pub last_final_checkpoint: u64,
    pub observed: Option<ObservedDeploymentView>,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LaneSettingsArg {
    pub gas_budget: u64,
    pub max_attempts: u32,
}
