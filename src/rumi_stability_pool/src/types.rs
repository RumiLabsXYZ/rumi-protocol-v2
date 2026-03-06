use candid::{CandidType, Deserialize, Nat, Principal};
use ic_stable_structures::{Storable, storable::Bound};
use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::borrow::Cow;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct UserDeposit {
    pub user: Principal,
    pub icusd_amount: u64,
    pub deposit_time: u64,
    pub pending_collateral: Vec<CollateralReward>,
}

impl Storable for UserDeposit {
    const BOUND: Bound = Bound::Unbounded;

    fn to_bytes(&self) -> Cow<[u8]> {
        let bytes = candid::encode_one(self).unwrap();
        Cow::Owned(bytes)
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).unwrap()
    }
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct CollateralReward {
    pub collateral_type: CollateralType,
    pub amount: u64,
    pub liquidation_id: u64,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub enum CollateralType {
    ICP,
    CkBTC,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct LiquidationRecord {
    pub liquidation_id: u64,
    pub vault_id: u64,
    pub liquidated_debt: u64,
    pub collateral_received: u64,
    pub collateral_type: CollateralType,
    pub liquidation_time: u64,
    pub pool_size_at_liquidation: u64,
}

impl Storable for LiquidationRecord {
    const BOUND: Bound = Bound::Unbounded;

    fn to_bytes(&self) -> Cow<[u8]> {
        let bytes = candid::encode_one(self).unwrap();
        Cow::Owned(bytes)
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        candid::decode_one(&bytes).unwrap()
    }
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug)]
pub struct PoolInfo {
    pub total_icusd_deposited: u64,
    pub total_depositors: u64,
    pub pool_utilization: f64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct InitArgs {
    pub protocol_owner: Principal,
    pub liquidation_discount: u8, // Percentage (e.g., 10 for 10%)
    pub max_ltv_ratio: u8,        // Percentage (e.g., 80 for 80%)
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct DepositResult {
    pub success: bool,
    pub new_balance: u64,
    pub message: String,
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct WithdrawResult {
    pub success: bool,
    pub remaining_balance: u64,
    pub message: String,
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct ClaimResult {
    pub success: bool,
    pub claimed_collateral: Vec<CollateralReward>,
    pub message: String,
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct ManualLiquidationResult {
    pub success: bool,
    pub liquidations_executed: u64,
    pub message: String,
}

// ── ICRC-21 Canister Call Consent Messages ──

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Icrc21ConsentMessageRequest {
    pub method: String,
    pub arg: Vec<u8>,
    pub user_preferences: Icrc21ConsentMessageMetadata,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct Icrc21ConsentMessageMetadata {
    pub language: String,
    pub utc_offset_minutes: Option<i16>,
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub enum Icrc21ConsentMessageResponse {
    #[serde(rename = "Ok")]
    Ok(Icrc21ConsentInfo),
    #[serde(rename = "Err")]
    Err(Icrc21Error),
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct Icrc21ConsentInfo {
    pub consent_message: Icrc21ConsentMessage,
    pub metadata: Icrc21ConsentMessageResponseMetadata,
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct Icrc21ConsentMessageResponseMetadata {
    pub language: String,
    pub utc_offset_minutes: Option<i16>,
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub enum Icrc21ConsentMessage {
    GenericDisplayMessage(String),
    LineDisplayMessage { pages: Vec<Icrc21LineDisplayPage> },
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct Icrc21LineDisplayPage {
    pub lines: Vec<Icrc21LineDisplayLine>,
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct Icrc21LineDisplayLine {
    pub line: String,
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub enum Icrc21Error {
    UnsupportedCanisterCall(Icrc21ErrorInfo),
    ConsentMessageUnavailable(Icrc21ErrorInfo),
    GenericError { error_code: Nat, description: String },
}

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct Icrc21ErrorInfo {
    pub description: String,
}

// ── ICRC-10 Supported Standards ──

#[derive(CandidType, Serialize, Clone, Debug)]
pub struct Icrc10SupportedStandard {
    pub name: String,
    pub url: String,
}

// Use simple in-memory storage for now
thread_local! {
    pub static DEPOSITS: RefCell<HashMap<Principal, UserDeposit>> = RefCell::new(HashMap::new());
    pub static LIQUIDATIONS: RefCell<HashMap<u64, LiquidationRecord>> = RefCell::new(HashMap::new());
    pub static STATE: RefCell<PoolState> = RefCell::new(PoolState::default());
}

#[derive(Clone, Debug, CandidType, Deserialize)]
pub struct PoolState {
    pub protocol_owner: Principal,
    pub liquidation_discount: u8,  // Percentage (e.g., 10 for 10%)
    pub max_ltv_ratio: u8,         // Percentage (e.g., 80 for 80%)
    pub next_liquidation_id: u64,
    pub paused: bool,
}

impl Default for PoolState {
    fn default() -> Self {
        Self {
            protocol_owner: Principal::anonymous(),
            liquidation_discount: 10,  // 10%
            max_ltv_ratio: 66,         // 66%
            next_liquidation_id: 1,
            paused: false,
        }
    }
}