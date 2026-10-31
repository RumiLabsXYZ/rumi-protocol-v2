//! Public Candid types. Internal stable records live in `state.rs`.
//!
//! Skeleton surface only (BB-47): install arguments, errors, the wallet-standard argument
//! shapes the ICRC-21 consent messages decode, and two small views. The deposit, return and
//! admin endpoints that use the remaining argument types arrive with BB-48 and BB-49.

use crate::records::MintState;
use crate::tx::TokenProgram;
use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use serde::{Deserialize, Serialize};

pub use crate::accounting::Transition;

// ---------------------------------------------------------------------------
// Install arguments
// ---------------------------------------------------------------------------

// The install argument is decoded once per install or upgrade; boxing buys nothing.
#[allow(clippy::large_enum_variant)]
#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum BridgeArg {
    Init(InitArgs),
    Upgrade(UpgradeArgs),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct InitArgs {
    /// Canonical icUSD ICRC-1/2/3 ledger (or a synthetic test ledger on devnet/testnet, D9).
    pub icusd_ledger: Principal,
    /// SOL RPC canister. Production id `tghme-zyaaa-aaaar-qarca-cai`; its re-verification is an
    /// activation step (blocker `sol-rpc-canister-id-conflict`), so it is an install argument.
    pub sol_rpc: Principal,
    /// Threshold-Schnorr key: `key_1` for mainnet-beta, `test_key_1` otherwise.
    pub schnorr_key_name: String,
    /// Base58 genesis hash of the cluster: the network identity in every id and key path.
    pub genesis_hash: String,
    pub admins: Vec<Principal>,
    /// Destination for swept fees. When absent, sweeps are refused.
    pub fee_recipient: Option<Account>,
    /// Profile `token_program`: exactly two allowed values.
    pub token_program: TokenProgram,
    /// Profile `memo_program`, base58 (U: which Memo version is deployed).
    pub memo_program: String,
    /// SOL RPC settings (cluster, consensus, cycles). Absent: defaults, with the cluster taken
    /// from the genesis hash on mainnet-beta and REQUIRED otherwise. (BB-48; optional so a
    /// BB-47 style install argument still decodes.)
    pub rpc: Option<RpcSettings>,
    /// Nonce lane settings. Absent: the defaults in [`LaneSettings`].
    pub lane: Option<LaneSettings>,
}

#[derive(CandidType, Deserialize, Clone, Debug, Default)]
pub struct UpgradeArgs {
    /// Optional replacement admin list. Upgrades are controller-only, so this is a
    /// controller action.
    pub admins: Option<Vec<Principal>>,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct BridgeFees {
    /// Deducted from the minted amount on ICP -> Solana.
    pub deposit_fee_e8s: u64,
    /// Deducted from the released amount on Solana -> ICP (in addition to the ledger fee).
    pub release_fee_e8s: u64,
}

/// Deposit, capacity and rate limits (49b), e8s.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeLimits {
    pub min_deposit_e8s: u64,
    /// Also the per-operation cap of one Solana mint.
    pub max_deposit_e8s: u64,
    /// Cap on `O + PM + RC + BU + H + reserved pulls`: everything that is, or may become, supply.
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
// SOL RPC and nonce lane settings (BB-48)
// ---------------------------------------------------------------------------

/// Which cluster the SOL RPC canister's default providers are asked for.
#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SolRpcCluster {
    Mainnet,
    Devnet,
    Testnet,
}

/// Response consensus for one class of SOL RPC call (mirrors the SOL RPC canister's
/// `ConsensusStrategy`).
#[derive(CandidType, Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsensusSetting {
    /// Every queried provider must return the same response.
    Equality,
    /// At least `min` of `total` providers must agree (`total` absent: all configured).
    Threshold { total: Option<u8>, min: u8 },
}

/// SOL RPC canister call settings. The canister id itself is `BridgeConfig::sol_rpc`, an install
/// argument and never a default.
///
/// Discovery is lenient and verification is strict (note section 4, P4): candidate lists
/// (`getSignaturesForAddress`) may use a threshold, but every candidate that can change a
/// decision is then fetched singly with `getTransaction` under `verify`.
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct RpcSettings {
    /// `None` until configured; every call is refused while it is `None`.
    pub cluster: Option<SolRpcCluster>,
    /// `getAccountInfo`, `getTransaction`, `getSignatureStatuses`, `sendTransaction`,
    /// `getSlot`, `getBlock`.
    pub verify: ConsensusSetting,
    /// `getSignaturesForAddress`.
    pub discover: ConsensusSetting,
    /// Cycles attached to each call (unused cycles are refunded).
    pub cycles_per_call: u64,
    pub response_size_estimate: Option<u64>,
}

impl Default for RpcSettings {
    fn default() -> Self {
        RpcSettings {
            cluster: None,
            verify: ConsensusSetting::Equality,
            // 2 of 3, as the note suggests for candidate lists.
            discover: ConsensusSetting::Threshold { total: Some(3), min: 2 },
            // About 1 to 5B per small request (source S10); 10B leaves headroom.
            cycles_per_call: 10_000_000_000,
            response_size_estimate: None,
        }
    }
}

/// Default of [`LaneSettings::history_horizon_secs`]. **UNMEASURED PLACEHOLDER.** The note sets
/// the real value after the retention probe, "strictly below the retention measured for the
/// providers that form the quorum" (blocker `solana-history-availability-unmeasured`). Six hours
/// is deliberately far below the roughly two days a secondary guide mentions for standard nodes;
/// it is not a fact about any provider.
pub const DEFAULT_HISTORY_HORIZON_SECS: u64 = 6 * 3600;
/// Blocker carried by `history_horizon_secs`.
pub const HISTORY_HORIZON_BLOCKER: &str = "solana-history-availability-unmeasured";

/// Durable-nonce lane settings (note sections 4 and 9).
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct LaneSettings {
    /// Nonce accounts per lane (the mint lane and the burn lane each get this many). Start 4.
    pub nonce_pool_size: u8,
    /// After this long without the attempt landing, persist and sign the advance-only cancel.
    pub cancel_after_secs: u64,
    /// Minimum time between two broadcasts of the same signed bytes.
    pub rebroadcast_interval_secs: u64,
    /// Raise a reconcile alarm for an operation unresolved this long (R: 15 minutes).
    pub reconcile_alarm_secs: u64,
    /// Halt the lane when the nonce advanced and no consumer was found within this time.
    /// UNMEASURED, see [`DEFAULT_HISTORY_HORIZON_SECS`].
    pub history_horizon_secs: u64,
    /// Fresh-nonce re-issues of one operation before it is held for review.
    pub max_reissues: u8,
    /// Most candidates one reconcile tick fetches from a nonce account's signature list.
    pub discover_limit: u16,
}

impl Default for LaneSettings {
    fn default() -> Self {
        LaneSettings {
            nonce_pool_size: 4,
            cancel_after_secs: 300,
            rebroadcast_interval_secs: 30,
            reconcile_alarm_secs: 15 * 60,
            history_horizon_secs: DEFAULT_HISTORY_HORIZON_SECS,
            max_reissues: 3,
            discover_limit: 10,
        }
    }
}

impl LaneSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.nonce_pool_size == 0 || self.nonce_pool_size > 16 {
            return Err("nonce_pool_size must be in 1..=16".into());
        }
        if self.rebroadcast_interval_secs == 0 || self.cancel_after_secs < self.rebroadcast_interval_secs {
            return Err("cancel_after_secs must be at least rebroadcast_interval_secs, which must be > 0".into());
        }
        if self.reconcile_alarm_secs == 0 || self.history_horizon_secs == 0 {
            return Err("reconcile_alarm_secs and history_horizon_secs must be > 0".into());
        }
        if self.max_reissues > 10 {
            return Err("max_reissues must be at most 10".into());
        }
        if self.discover_limit == 0 || self.discover_limit > 100 {
            return Err("discover_limit must be in 1..=100".into());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Argument shapes (decoded by ICRC-21 and by the endpoints)
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeToSolanaArgs {
    /// Amount pulled into escrow, in e8s (`A`). The ledger fee is charged to the caller on top.
    pub amount_e8s: u64,
    /// Base58 32-byte Solana wallet address (the recipient's ATA is derived and created by the
    /// bridge).
    pub solana_recipient: String,
    pub from_subaccount: Option<serde_bytes::ByteBuf>,
    /// 16 to 32 bytes, unique per caller. Repeating it with identical arguments returns the
    /// existing operation.
    pub client_request_id: serde_bytes::ByteBuf,
    /// The deposit fee the caller was shown. Rejected if it no longer matches.
    pub expected_deposit_fee_e8s: u64,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum BridgeError {
    Anonymous,
    Unauthorized,
    NotConfigured(String),
    Paused(String),
    Halted(String),
    InvalidArgument(String),
    /// The Solana recipient is not an acceptable wallet.
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
    Rpc(String),
    Internal(String),
}

pub type BridgeResult<T> = Result<T, BridgeError>;

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BridgeInfo {
    pub schema_version: u32,
    pub initialized: bool,
    /// Base58 genesis hash (cluster identity).
    pub genesis_hash: String,
    pub token_program: TokenProgram,
    pub memo_program: String,
    pub icusd_ledger: Principal,
    pub sol_rpc: Principal,
    pub schnorr_key_name: String,
    pub admins: Vec<Principal>,
    pub pauses: Pauses,
    pub fees: BridgeFees,
    pub limits: BridgeLimits,
    pub rpc: RpcSettings,
    pub lane: LaneSettings,
    pub halted: Option<String>,
    /// Hex instance salt (identity of this instance in every mint id).
    pub instance_salt: String,
    /// Hex of the escrow subaccount on the icUSD ledger.
    pub escrow_subaccount: String,
}

/// Where the bootstrap stands (49a): the addresses an operator must fund and check, the claim on the
/// escrow mint, and the nonce pool.
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BootstrapStatus {
    pub keys_derived: bool,
    /// Base58. The fee payer needs SOL for fees, account rent and nonce rent (blocker `solana-fee-payer-funding`).
    pub fee_payer: Option<String>,
    pub mint_authority: Option<String>,
    pub return_owner: Option<String>,
    /// The escrow mint address (the one-time `mint-account` key).
    pub mint_account: Option<String>,
    pub return_account: Option<String>,
    pub mint_state: MintState,
    /// Why the last bind attempt did not bind.
    pub bind_note: Option<String>,
    pub pool_registered: u32,
    pub pool_unverified: u32,
    pub pool_free: u32,
    pub pool_reserved: u32,
    pub pending_bootstrap_txs: u32,
}

// ---- deposits and releases (49b) ----

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum DepositKindView {
    /// ICP -> Solana deposit by `owner`.
    UserDeposit,
    /// Re-mint of a held return claim to its Solana sender (64 hex).
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
pub struct SolanaInclusion {
    /// Base58 first signature of the finalized mint transaction.
    pub signature: String,
    pub slot: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DepositView {
    pub id: u64,
    pub kind: DepositKindView,
    pub owner: Principal,
    pub from: Option<Account>,
    /// Base58 Solana wallet.
    pub solana_recipient: String,
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
    pub delivered: Option<SolanaInclusion>,
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
    /// 64 hex.
    pub return_id: String,
    pub seq: u64,
    pub burn_op_id: u64,
    /// Base58 Solana wallet that sent the return.
    pub solana_sender: String,
    pub amount_e8s: u64,
    pub release_fee_e8s: u64,
    pub ledger_fee_e8s: Option<u64>,
    pub amount_to_receive_e8s: Option<u64>,
    pub destination: Account,
    pub return_signature: String,
    pub return_slot: u64,
    pub burn_signature: String,
    pub burn_slot: u64,
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
    /// Base58 Solana wallet that sent the returns.
    pub solana_sender: String,
    pub start_after_seq: Option<u64>,
    pub limit: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HintView {
    /// Base58 transaction signature.
    pub signature: String,
    pub submitted_by: Principal,
    pub submitted_at_ns: u64,
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

// ---- lane operations (49b: the admin's view of held operations) ----

#[derive(CandidType, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKindView {
    Mint,
    Burn,
    Refund,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum OpStateView {
    Active,
    Delivered { signature: String, slot: u64, adopted: bool },
    NeedsReissue { reason: String },
    Held { reason: String },
    /// Abandoned by an admin, accepted only because the lane's own records showed that no signed
    /// original can land (never signed or sent, or every signed generation proven not delivered).
    /// The reason is a record, not the proof.
    Abandoned { reason: String },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OpView {
    pub id: u64,
    pub kind: OpKindView,
    pub lane: crate::records::Lane,
    /// 64 hex: the mint, burn or refund id (also the memo content).
    pub external_id: String,
    pub amount: u64,
    pub state: OpStateView,
    pub generation: u32,
    pub slot_index: Option<u8>,
    /// Base58 transaction ids of every attempt, oldest first.
    pub tx_ids: Vec<String>,
    /// For a mint: whether its deposit's backing could be released again, judged only from recorded
    /// signatures, transaction ids and typed non-delivery proofs. `None` for burns and refunds.
    pub refund_eligibility: Option<crate::records::RefundEligibility>,
    pub note: Option<String>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ListOpsArgs {
    pub start_after: Option<u64>,
    pub limit: u32,
}

// ---- returns, strays, the observer and the monitors (49c) ----

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ReturnStatus {
    /// Verified and finalized; the tokens sit in the return account.
    Custodied,
    Burning { burn_op_id: u64 },
    Burned { burn_op_id: u64 },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReturnView {
    /// 64 hex: the dedup key and the ICP transfer memo.
    pub return_id: String,
    pub seq: u64,
    /// Base58 first signature of the user's return transaction.
    pub signature: String,
    pub slot: u64,
    /// Base58 token account the tokens left.
    pub source_account: String,
    /// Base58 verified sender (owner of the source account, and the signer).
    pub sender: String,
    pub amount_e8s: u64,
    pub destination: Account,
    pub client_nonce: u64,
    pub status: ReturnStatus,
    pub created_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StrayView {
    pub signature: String,
    pub slot: u64,
    pub amount_e8s: u64,
    pub source_account: Option<String>,
    pub verified_sender: Option<String>,
    pub reason: String,
    pub created_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ListReturnsArgs {
    pub start_after_seq: Option<u64>,
    pub limit: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct ListStraysArgs {
    /// Base58 signature of the last stray of the previous page.
    pub after: Option<String>,
    pub limit: u32,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ObserverStatus {
    /// Base58 return account the observer lists.
    pub return_account: Option<String>,
    /// Base58 newest signature whose transactions are all processed.
    pub cursor: Option<String>,
    pub walk_active: bool,
    pub queued: u64,
    pub caught_up: bool,
    pub processed: u64,
    pub last_run_ns: Option<u64>,
    pub last_progress_ns: Option<u64>,
    pub last_note: Option<String>,
    pub returns_active: u64,
    pub strays: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EscrowReading {
    pub balance: Nat,
    pub required: Nat,
    pub ok: bool,
    pub at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SupplyReading {
    pub supply: Nat,
    pub bound: Nat,
    pub inflight_mint: Nat,
    pub within_bound: bool,
    pub quiescent: bool,
    /// `O - supply` when quiescent and positive: REPORTED, never acted on (blocker
    /// `solana-unattributed-burn-policy`).
    pub unattributed_gap: Option<Nat>,
    pub at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MonitorStatus {
    pub c1: Option<EscrowReading>,
    pub c2: Option<SupplyReading>,
    pub c1_strikes: u8,
    pub c2_strikes: u8,
    pub last_run_ns: Option<u64>,
    pub last_note: Option<String>,
}

/// What an admin does with a held operation (note section 4: "resolved by hand").
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum HeldOpResolution {
    /// Look again at the SAME stored bytes (for example after the history horizon was raised or an
    /// RPC outage ended). Nothing is assumed: adoption still needs the exact message.
    Recheck,
    /// Abandon a held mint so its deposit can be refunded. Accepted ONLY when the lane's own records
    /// prove no signed original can land: the operation was never signed or sent, or every signed
    /// generation is proven not delivered by a typed proof (a finalized owned cancel, or a failed
    /// original); see `OpView::refund_eligibility`. Otherwise it is refused and nothing changes. The
    /// reason is stored in the audit log as a record and authorizes nothing: a reason, a timeout,
    /// missing history or an advanced nonce is not proof. On success the nonce slot is re-read from
    /// the chain before it is used again and the deposit is never minted again.
    Abandon { reason: String },
}

/// The accounting buckets, e8s (`Nat`: the Rust type is `u128`).
#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BucketsView {
    pub outstanding: Nat,
    pub pending_mint: Nat,
    pub return_custodied: Nat,
    pub burn_unknown: Nat,
    pub pending_release: Nat,
    pub pending_refund: Nat,
    pub held: Nat,
    pub fees: Nat,
    pub reserved_pulls: Nat,
    pub unknown_outflow: Nat,
    /// Memo bucket (subset of `outstanding`, not a liability).
    pub stray: Nat,
}

impl From<&crate::accounting::Buckets> for BucketsView {
    fn from(b: &crate::accounting::Buckets) -> Self {
        BucketsView {
            outstanding: Nat::from(b.outstanding),
            pending_mint: Nat::from(b.pending_mint),
            return_custodied: Nat::from(b.return_custodied),
            burn_unknown: Nat::from(b.burn_unknown),
            pending_release: Nat::from(b.pending_release),
            pending_refund: Nat::from(b.pending_refund),
            held: Nat::from(b.held),
            fees: Nat::from(b.fees),
            reserved_pulls: Nat::from(b.reserved_pulls),
            unknown_outflow: Nat::from(b.unknown_outflow),
            stray: Nat::from(b.stray),
        }
    }
}
