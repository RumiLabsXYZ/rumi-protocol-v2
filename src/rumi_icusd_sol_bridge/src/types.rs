//! Public Candid types. Internal stable records live in `state.rs`.
//!
//! Skeleton surface only (BB-47): install arguments, errors, the wallet-standard argument
//! shapes the ICRC-21 consent messages decode, and two small views. The deposit, return and
//! admin endpoints that use the remaining argument types arrive with BB-48 and BB-49.

use crate::tx::TokenProgram;
use candid::{CandidType, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use serde::{Deserialize, Serialize};

pub use crate::accounting::Transition;

// ---------------------------------------------------------------------------
// Install arguments
// ---------------------------------------------------------------------------

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

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Pauses {
    pub deposits: bool,
    pub mints: bool,
    pub releases: bool,
    pub observer: bool,
}

// ---------------------------------------------------------------------------
// Wallet-standard argument shapes (decoded by ICRC-21; the endpoints land in BB-48/BB-49)
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
    InvalidState(String),
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
    pub halted: Option<String>,
    /// Hex instance salt (identity of this instance in every mint id).
    pub instance_salt: String,
    /// Hex of the escrow subaccount on the icUSD ledger.
    pub escrow_subaccount: String,
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
