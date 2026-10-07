use candid::{CandidType, Deserialize, Principal};
use serde::Serialize;

/// Types of deposits that can be made to the treasury
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum DepositType {
    /// Fee collected when users borrow/mint icUSD
    #[serde(alias = "MintingFee")]
    BorrowingFee,
    /// Fee collected when users redeem icUSD
    RedemptionFee,
    /// Protocol's share of liquidation bonus (in collateral)
    #[serde(alias = "LiquidationSurplus")]
    LiquidationFee,
    /// Interest revenue accrued on vault debt
    #[serde(alias = "StabilityFee")]
    InterestRevenue,
    /// Stable evidence for a record whose historical Candid variant is not
    /// recognized by this binary. The value is raw-wire hex, not an amount.
    LegacyUnknown { candid_hex: String },
}

/// Asset types that can be held in treasury
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub enum AssetType {
    /// icUSD stablecoin
    ICUSD,
    /// ICP collateral
    ICP,
    /// ckBTC collateral (for future Bitcoin support)
    CKBTC,
    /// ckUSDT stablecoin (for vault repayment/liquidation)
    CKUSDT,
    /// ckUSDC stablecoin (for vault repayment/liquidation)
    CKUSDC,
}

/// A record of a deposit to the treasury
#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct DepositRecord {
    /// Unique ID for this deposit
    pub id: u64,
    /// Type of deposit (minting fee, liquidation surplus, etc.)
    pub deposit_type: DepositType,
    /// Asset type (icUSD, ICP, ckBTC)
    pub asset_type: AssetType,
    /// Amount deposited (in e8s)
    pub amount: u64,
    /// Block index of the transfer that funded this deposit
    pub block_index: u64,
    /// Timestamp when deposit was made
    pub timestamp: u64,
    /// Optional memo/description
    pub memo: Option<String>,
}

/// Version-one query projection. Its variants stay frozen for existing Candid
/// clients; unknown stable records are returned by the V2 evidence endpoint.
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum DepositTypeV1 {
    BorrowingFee,
    RedemptionFee,
    LiquidationFee,
    InterestRevenue,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct DepositRecordV1 {
    pub id: u64,
    pub deposit_type: DepositTypeV1,
    pub asset_type: AssetType,
    pub amount: u64,
    pub block_index: u64,
    pub timestamp: u64,
    pub memo: Option<String>,
}

impl From<DepositRecord> for DepositRecordV1 {
    fn from(record: DepositRecord) -> Self {
        let deposit_type = match record.deposit_type {
            DepositType::BorrowingFee => DepositTypeV1::BorrowingFee,
            DepositType::RedemptionFee => DepositTypeV1::RedemptionFee,
            DepositType::LiquidationFee => DepositTypeV1::LiquidationFee,
            DepositType::InterestRevenue => DepositTypeV1::InterestRevenue,
            DepositType::LegacyUnknown { .. } => unreachable!("unknown records are filtered"),
        };
        Self {
            id: record.id,
            deposit_type,
            asset_type: record.asset_type,
            amount: record.amount,
            block_index: record.block_index,
            timestamp: record.timestamp,
            memo: record.memo,
        }
    }
}

/// Treasury balance for a specific asset
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, Default)]
pub struct AssetBalance {
    /// Total amount of this asset
    pub total: u64,
    /// Amount reserved/locked
    pub reserved: u64,
    /// Amount available for withdrawal
    pub available: u64,
}

/// Treasury status overview
#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct TreasuryStatus {
    /// Total number of deposits
    pub total_deposits: u64,
    /// Balances by asset type
    pub balances: Vec<(AssetType, AssetBalance)>,
    /// Controller principal (pre-SNS) or governance canister (post-SNS)
    pub controller: Principal,
    /// Whether treasury is paused
    pub is_paused: bool,
}

/// Arguments for initializing treasury
#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct TreasuryInitArgs {
    /// Initial controller (usually protocol backend canister)
    pub controller: Principal,
    /// icUSD ledger principal
    pub icusd_ledger: Principal,
    /// ICP ledger principal
    pub icp_ledger: Principal,
    /// ckBTC ledger principal (for future use)
    pub ckbtc_ledger: Option<Principal>,
    /// ckUSDT ledger principal (for vault repayment)
    pub ckusdt_ledger: Option<Principal>,
    /// ckUSDC ledger principal (for vault repayment)
    pub ckusdc_ledger: Option<Principal>,
}

/// Arguments for making a deposit
#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct DepositArgs {
    /// Type of deposit
    pub deposit_type: DepositType,
    /// Asset being deposited
    pub asset_type: AssetType,
    /// Amount to deposit (in e8s)
    pub amount: u64,
    /// Block index of the funding transfer
    pub block_index: u64,
    /// Optional memo
    pub memo: Option<String>,
}

/// Arguments for withdrawing from treasury
#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct WithdrawArgs {
    /// Asset to withdraw
    pub asset_type: AssetType,
    /// Amount to withdraw (in e8s)
    pub amount: u64,
    /// Destination principal
    pub to: Principal,
    /// Optional memo for the transfer
    pub memo: Option<String>,
    /// Caller-supplied idempotency token. The treasury persists the first
    /// attempt's `created_at_time` per `request_id` and reuses it on retries,
    /// so re-submitting with the same `request_id` lets the ledger
    /// deduplicate (audit ICRC-003). If omitted, treasury derives one from
    /// `(caller, asset_type, amount, to, floor(now / 60s))` so a same-minute
    /// repeat still dedups.
    #[serde(default)]
    pub request_id: Option<u64>,
}

/// Result of a successful withdrawal
#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct WithdrawResult {
    /// Block index of the transfer
    pub block_index: u64,
    /// Amount actually transferred (after fees)
    pub amount_transferred: u64,
    /// Fee deducted
    pub fee: u64,
}

/// Snapshot of all asset balances, persisted to stable memory via `StableCell`.
/// Survives canister upgrades (unlike the in-memory `HashMap`).
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, Default)]
pub struct BalancesSnapshot {
    pub entries: Vec<(AssetType, AssetBalance)>,
}

// ─── Treasury Events (audit trail) ───

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub enum TreasuryAction {
    Deposit {
        deposit_type: DepositType,
        asset_type: AssetType,
        amount: u64,
    },
    Withdraw {
        asset_type: AssetType,
        amount: u64,
        to: Principal,
    },
    SetPaused {
        paused: bool,
    },
    /// Preserves stable audit evidence for an unrecognized future/legacy
    /// action without mislabeling it as a known financial event.
    LegacyUnknown {
        candid_value: String,
    },
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct TreasuryEvent {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub action: TreasuryAction,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub enum TreasuryActionV1 {
    Deposit {
        deposit_type: DepositTypeV1,
        asset_type: AssetType,
        amount: u64,
    },
    Withdraw {
        asset_type: AssetType,
        amount: u64,
        to: Principal,
    },
    SetPaused {
        paused: bool,
    },
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct TreasuryEventV1 {
    pub id: u64,
    pub timestamp: u64,
    pub caller: Principal,
    pub action: TreasuryActionV1,
}

impl From<TreasuryEvent> for TreasuryEventV1 {
    fn from(event: TreasuryEvent) -> Self {
        let action = match event.action {
            TreasuryAction::Deposit {
                deposit_type,
                asset_type,
                amount,
            } => TreasuryActionV1::Deposit {
                deposit_type: match deposit_type {
                    DepositType::BorrowingFee => DepositTypeV1::BorrowingFee,
                    DepositType::RedemptionFee => DepositTypeV1::RedemptionFee,
                    DepositType::LiquidationFee => DepositTypeV1::LiquidationFee,
                    DepositType::InterestRevenue => DepositTypeV1::InterestRevenue,
                    DepositType::LegacyUnknown { .. } => {
                        unreachable!("unknown events are filtered")
                    }
                },
                asset_type,
                amount,
            },
            TreasuryAction::Withdraw {
                asset_type,
                amount,
                to,
            } => TreasuryActionV1::Withdraw {
                asset_type,
                amount,
                to,
            },
            TreasuryAction::SetPaused { paused } => TreasuryActionV1::SetPaused { paused },
            TreasuryAction::LegacyUnknown { .. } => unreachable!("unknown events are filtered"),
        };
        Self {
            id: event.id,
            timestamp: event.timestamp,
            caller: event.caller,
            action,
        }
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct UnknownTreasuryEvidenceV2 {
    pub record_kind: String,
    pub id: u64,
    pub raw_candid_hex: String,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct UnknownTreasuryEvidencePageV2 {
    pub deposits: Vec<UnknownTreasuryEvidenceV2>,
    pub events: Vec<UnknownTreasuryEvidenceV2>,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct PendingWithdrawalV2 {
    pub request_id: u64,
    pub caller: Principal,
    pub asset_type: AssetType,
    pub ledger: Principal,
    pub amount: u64,
    pub to: Principal,
    pub memo: Option<String>,
    pub created_at_time: u64,
    pub send_amount: u64,
    pub fee: u64,
    pub dispatch_attempts: Option<u32>,
    /// "pending" or "legacy_unknown"; completed rows are omitted.
    pub status: String,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct PendingWithdrawalsPageV2 {
    pub withdrawals: Vec<PendingWithdrawalV2>,
    /// First stable request key to inspect on the next page.
    pub next_start: Option<u64>,
}

#[cfg(test)]
mod v1_wire_compat_tests {
    use super::*;

    #[derive(CandidType, Deserialize)]
    enum OldDepositType {
        BorrowingFee,
        RedemptionFee,
        LiquidationFee,
        InterestRevenue,
    }

    #[derive(CandidType, Deserialize)]
    struct OldDepositRecord {
        id: u64,
        deposit_type: OldDepositType,
        asset_type: AssetType,
        amount: u64,
        block_index: u64,
        timestamp: u64,
        memo: Option<String>,
    }

    #[test]
    fn v1_deposit_projection_decodes_with_predecessor_variant_set() {
        let response = vec![DepositRecordV1 {
            id: 7,
            deposit_type: DepositTypeV1::BorrowingFee,
            asset_type: AssetType::ICP,
            amount: 123,
            block_index: 456,
            timestamp: 789,
            memo: None,
        }];
        let bytes = candid::encode_one(response).unwrap();
        let decoded: Vec<OldDepositRecord> = candid::decode_one(&bytes).unwrap();
        assert_eq!(decoded[0].id, 7);
        assert_eq!(decoded[0].amount, 123);
        assert!(matches!(
            decoded[0].deposit_type,
            OldDepositType::BorrowingFee
        ));
    }

    #[test]
    fn v1_event_projection_decodes_with_predecessor_action_set() {
        #[derive(CandidType, Deserialize)]
        enum OldAction {
            SetPaused { paused: bool },
        }
        #[derive(CandidType, Deserialize)]
        struct OldEvent {
            id: u64,
            timestamp: u64,
            caller: Principal,
            action: OldAction,
        }
        let bytes = candid::encode_one(TreasuryEventV1 {
            id: 9,
            timestamp: 10,
            caller: Principal::from_slice(&[1]),
            action: TreasuryActionV1::SetPaused { paused: true },
        })
        .unwrap();
        let decoded: OldEvent = candid::decode_one(&bytes).unwrap();
        assert_eq!(decoded.id, 9);
        assert!(matches!(
            decoded.action,
            OldAction::SetPaused { paused: true }
        ));
    }
}
