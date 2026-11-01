//! One-vault, debt-preserving WaterNeuron redemption. No minting or personal payout.
//! Every external spend is preceded by a durable journal transition. An unknown
//! WaterNeuron conversion is only reconciled, never dispatched a second time.
use candid::{CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use num_traits::ToPrimitive;
use serde::Serialize;
use sha2::{Digest, Sha224, Sha256};

use crate::guard::VaultLiquidationGuard;
use crate::management::{nonce_to_created_at_time, nonce_to_memo, transfer_idempotent_pinned_fee};
use crate::state::{mutate_state, read_state};

pub const VAULT_ID: u64 = 51;
const WN: &str = "tsbvt-pyaaa-aaaar-qafva-cai";
const NICP: &str = "buwm7-7yaaa-aaaar-qagva-cai";
const ICP: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";
const E8S: u128 = 100_000_000;

#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum ClaimStage {
    Prepared,
    FundingSubmitted,
    Funded,
    ApprovalSubmitted,
    Approved,
    WithdrawalSubmitted,
    ClaimConfirmed,
    Settled,
}

#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct NicpRedemptionClaim {
    pub vault_id: u64,
    pub owner: Principal,
    pub stage: ClaimStage,
    pub original_nicp_e8s: u64,
    pub redeem_nicp_e8s: u64,
    pub icp_due_e8s: u64,
    pub received_icp_e8s: Option<u64>,
    pub fee_e8s: u64,
    pub escrow_subaccount: [u8; 32],
    pub funding_nonce: u128,
    pub approval_nonce: u128,
    pub funding_block: Option<u64>,
    pub approval_block: Option<u64>,
    pub withdrawal_id: Option<u64>,
    pub burn_block: Option<u64>,
    pub settlement_block: Option<u64>,
    pub created_at_ns: u64,
    pub updated_at_ns: u64,
    pub last_error: Option<String>,
}

impl NicpRedemptionClaim {
    // Settled ICP stays in the isolated account under the same nICP terms.
    // An ordinary nICP payout can never spend or release this collateral.
    pub fn blocks_vault(&self) -> bool {
        true
    }
    pub fn fixed_icp_amount_e8s(&self) -> u64 {
        self.received_icp_e8s.unwrap_or(self.icp_due_e8s)
    }
    pub fn account(&self, protocol: Principal) -> Account {
        Account {
            owner: protocol,
            subaccount: Some(self.escrow_subaccount),
        }
    }
}

#[derive(CandidType, Deserialize)]
pub struct PrepareNicpRedemption {
    pub expected_owner: Principal,
    pub expected_nicp_e8s: u64,
    pub max_debt_e8s: u64,
}

#[derive(CandidType, Deserialize)]
struct WnInfo {
    exchange_rate: u64,
    tracked_6m_stake: u64,
}
#[derive(CandidType, Deserialize)]
struct ConversionArg {
    maybe_subaccount: Option<[u8; 32]>,
    amount_e8s: u64,
}
#[derive(CandidType, Deserialize)]
struct WithdrawalSuccess {
    block_index: Nat,
    withdrawal_id: u64,
    icp_amount: Option<u64>,
}
#[derive(CandidType, Deserialize, Clone)]
struct NeuronId {
    id: u64,
}
#[derive(CandidType, Deserialize, Clone)]
enum WithdrawalStatus {
    ConversionDone { transfer_block_height: u64 },
    NotFound,
    Cancelled,
    WaitingToSplitNeuron,
    WaitingDissolvement { neuron_id: NeuronId },
    WaitingToStartDissolving { neuron_id: NeuronId },
}
#[derive(CandidType, Deserialize, Clone)]
struct WithdrawalRequest {
    nicp_burned: u64,
    withdrawal_id: u64,
    icp_due: u64,
    nicp_burn_index: u64,
    timestamp: u64,
    receiver: Account,
    neuron_id: Option<NeuronId>,
}
#[derive(CandidType, Deserialize, Clone)]
struct WithdrawalDetails {
    status: WithdrawalStatus,
    request: WithdrawalRequest,
}

fn principal(text: &str) -> Principal {
    Principal::from_text(text).expect("constant principal")
}
fn require_operator() -> Result<(), String> {
    let caller = ic_cdk::caller();
    if caller == Principal::anonymous() || !read_state(|s| s.developer_principal == caller) {
        return Err("Only the protocol developer may operate the vault 51 migration".into());
    }
    Ok(())
}
pub fn current() -> Option<NicpRedemptionClaim> {
    read_state(|s| s.nicp_redemption_claim.clone())
}
fn save(mut claim: NicpRedemptionClaim) -> NicpRedemptionClaim {
    claim.updated_at_ns = ic_cdk::api::time();
    mutate_state(|s| crate::event::record_nicp_redemption_claim(s, claim.clone()));
    claim
}
fn hold(mut claim: NicpRedemptionClaim, message: String) -> Result<NicpRedemptionClaim, String> {
    claim.last_error = Some(message.chars().take(400).collect());
    save(claim);
    Err(message)
}
fn quote_icp(nicp: u64, rate: u64) -> Result<u64, String> {
    if rate == 0 {
        return Err("WaterNeuron returned a zero exchange rate".into());
    }
    u64::try_from(u128::from(nicp) * E8S / u128::from(rate))
        .map_err(|_| "ICP quote overflow".into())
}
fn subaccount(nonce: u128) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"rumi/vault51/nicp-redemption/v1");
    h.update(nonce.to_be_bytes());
    h.finalize().into()
}
async fn balance(ledger: Principal, account: Account) -> Result<u64, String> {
    let (n,): (Nat,) = ic_cdk::call(ledger, "icrc1_balance_of", (account,))
        .await
        .map_err(|e| format!("Balance read failed: {e:?}"))?;
    n.0.to_u64().ok_or_else(|| "Balance exceeds u64".into())
}

/// Preparation fences the vault but moves no tokens. Exactly one lifetime claim.
pub async fn prepare(arg: PrepareNicpRedemption) -> Result<NicpRedemptionClaim, String> {
    require_operator()?;
    let _guard = VaultLiquidationGuard::new(VAULT_ID).map_err(|e| format!("{e:?}"))?;
    if current().is_some() {
        return Err("Vault 51 already has a durable redemption claim".into());
    }
    let vault =
        read_state(|s| s.vault_id_to_vaults.get(&VAULT_ID).cloned()).ok_or("Vault 51 not found")?;
    ensure_no_prior_obligation()?;
    if vault.owner != arg.expected_owner
        || vault.collateral_type != principal(NICP)
        || vault.collateral_amount != arg.expected_nicp_e8s
        || vault.bot_processing
        || vault.borrowed_icusd_amount.to_u64() > arg.max_debt_e8s
    {
        return Err(
            "Vault 51 does not match the approved owner, collateral or debt ceiling".into(),
        );
    }
    let (minting,): (Option<Account>,) = ic_cdk::call(principal(NICP), "icrc1_minting_account", ())
        .await
        .map_err(|e| format!("Cannot verify nICP burn destination: {e:?}"))?;
    if minting
        != Some(Account {
            owner: principal(WN),
            subaccount: None,
        })
    {
        return Err("Unexpected nICP minting account; do not assume zero-fee burn".into());
    }
    let fee = crate::management::get_ledger_fee(principal(NICP)).await?;
    let redeem = vault
        .collateral_amount
        .checked_sub(fee.checked_mul(2).ok_or("Fee overflow")?)
        .filter(|v| *v > 0)
        .ok_or("Collateral cannot cover funding and approval fees")?;
    let (info,): (WnInfo,) = ic_cdk::call(principal(WN), "get_info", ())
        .await
        .map_err(|e| format!("WaterNeuron information unavailable: {e:?}"))?;
    let due = quote_icp(redeem, info.exchange_rate)?;
    if due < 10 * E8S as u64 || due > info.tracked_6m_stake {
        return Err("Redemption is below minimum or exceeds reported backing".into());
    }
    let now = ic_cdk::api::time();
    // Accrue under the existing nICP rules before changing the valuation basis.
    mutate_state(|s| crate::event::record_accrue_interest(s, now));
    let unchanged = read_state(|s| {
        s.vault_id_to_vaults.get(&VAULT_ID).is_some_and(|v| {
            v.owner == vault.owner
                && v.collateral_amount == vault.collateral_amount
                && v.collateral_type == vault.collateral_type
                && !v.bot_processing
                && v.borrowed_icusd_amount.to_u64() <= arg.max_debt_e8s
        })
    });
    if !unchanged || current().is_some() {
        return Err("Vault changed during preparation".into());
    }
    let (funding_nonce, approval_nonce) = mutate_state(|s| (s.next_op_nonce(), s.next_op_nonce()));
    let claim = NicpRedemptionClaim {
        vault_id: VAULT_ID,
        owner: vault.owner,
        stage: ClaimStage::Prepared,
        original_nicp_e8s: vault.collateral_amount,
        redeem_nicp_e8s: redeem,
        icp_due_e8s: due,
        received_icp_e8s: None,
        fee_e8s: fee,
        escrow_subaccount: subaccount(funding_nonce),
        funding_nonce,
        approval_nonce,
        funding_block: None,
        approval_block: None,
        withdrawal_id: None,
        burn_block: None,
        settlement_block: None,
        created_at_ns: now,
        updated_at_ns: now,
        last_error: None,
    };
    if balance(principal(NICP), claim.account(ic_cdk::id())).await? != 0
        || balance(principal(ICP), claim.account(ic_cdk::id())).await? != 0
    {
        return Err("Dedicated custody account must be empty before preparation".into());
    }
    ensure_no_prior_obligation()?;
    // Operator corrections are synchronous and can run during the balance
    // reads above; recheck the complete approved snapshot at the commit point.
    let unchanged = read_state(|s| s.vault_id_to_vaults.get(&VAULT_ID).is_some_and(|v|
        v.owner == vault.owner && v.collateral_amount == vault.collateral_amount
        && v.collateral_type == vault.collateral_type && !v.bot_processing
        && v.borrowed_icusd_amount.to_u64() <= arg.max_debt_e8s));
    if !unchanged || current().is_some() {
        return Err("Vault changed before preparation commit".into());
    }
    Ok(save(claim))
}

fn ensure_no_prior_obligation() -> Result<(), String> {
    let reserved = read_state(|s| {
        s.bot_claims.contains_key(&VAULT_ID)
            || s.pending_margin_transfers
                .iter()
                .any(|(key, row)| key.0 == VAULT_ID || row.vault_id == VAULT_ID)
            || s.pending_excess_transfers
                .iter()
                .any(|(key, row)| key.0 == VAULT_ID || row.vault_id == VAULT_ID)
            || s.pending_redemption_transfer
                .values()
                .any(|row| row.vault_id == VAULT_ID)
    });
    if reserved {
        Err("Vault 51 has an existing liquidation or payout obligation; reconcile it before migration".into())
    } else {
        Ok(())
    }
}

/// Advance at most one external spend. Calling this in WithdrawalSubmitted
/// only reconciles the existing request; it cannot submit another burn.
pub async fn advance() -> Result<NicpRedemptionClaim, String> {
    require_operator()?;
    let _guard =
        VaultLiquidationGuard::new_for_nicp_claim(VAULT_ID).map_err(|e| format!("{e:?}"))?;
    let mut claim = current().ok_or("Prepare vault 51 first")?;
    claim.last_error = None;
    match claim.stage {
        ClaimStage::Prepared | ClaimStage::FundingSubmitted => {
            claim.stage = ClaimStage::FundingSubmitted;
            claim = save(claim);
            let amount = claim
                .original_nicp_e8s
                .checked_sub(claim.fee_e8s)
                .ok_or("Invalid funding amount")?;
            match transfer_idempotent_pinned_fee(
                principal(NICP),
                None,
                claim.account(ic_cdk::id()),
                u128::from(amount),
                claim.funding_nonce,
                claim.fee_e8s,
            )
            .await
            {
                Ok(block) => {
                    claim.funding_block = Some(block);
                    claim.stage = ClaimStage::Funded;
                }
                Err(e) => {
                    return hold(
                        claim,
                        format!("Funding held; only the exact tuple may be retried: {e:?}"),
                    )
                }
            }
        }
        ClaimStage::Funded | ClaimStage::ApprovalSubmitted => {
            claim.stage = ClaimStage::ApprovalSubmitted;
            claim = save(claim);
            let result: Result<(Result<Nat, ApproveError>,), _> = ic_cdk::call(
                principal(NICP),
                "icrc2_approve",
                (ApproveArgs {
                    from_subaccount: Some(claim.escrow_subaccount),
                    spender: Account {
                        owner: principal(WN),
                        subaccount: None,
                    },
                    amount: Nat::from(claim.redeem_nicp_e8s),
                    expected_allowance: Some(Nat::from(0u64)),
                    expires_at: None,
                    fee: Some(Nat::from(claim.fee_e8s)),
                    created_at_time: Some(nonce_to_created_at_time(claim.approval_nonce)),
                    memo: Some(nonce_to_memo(claim.approval_nonce)),
                },),
            )
            .await;
            let block = match result {
                Ok((Ok(block),)) => block,
                Ok((Err(ApproveError::Duplicate { duplicate_of }),)) => duplicate_of,
                other => {
                    return hold(
                        claim,
                        format!("Approval held; do not create a new allowance: {other:?}"),
                    )
                }
            };
            claim.approval_block = Some(block.0.to_u64().ok_or("Approval block overflow")?);
            claim.stage = ClaimStage::Approved;
        }
        ClaimStage::Approved => {
            // The third-party API does not have an idempotency parameter. This
            // durable phase is never rolled back, even on a typed error.
            claim.stage = ClaimStage::WithdrawalSubmitted;
            claim = save(claim);
            let result: Result<(Result<WithdrawalSuccess, candid::Reserved>,), _> = ic_cdk::call(
                principal(WN),
                "nicp_to_icp",
                (ConversionArg {
                    maybe_subaccount: Some(claim.escrow_subaccount),
                    amount_e8s: claim.redeem_nicp_e8s,
                },),
            )
            .await;
            match result {
                Ok((Ok(success),)) => {
                    claim.withdrawal_id = Some(success.withdrawal_id);
                    claim.burn_block = Some(success.block_index.0.to_u64().ok_or("Burn block overflow")?);
                    if let Some(due) = success.icp_amount { claim.icp_due_e8s = due; }
                    // Verify the independently retrievable request on the next call.
                }
                _ => return hold(claim, "Withdrawal outcome needs reconciliation. No automatic resubmission is permitted.".into()),
            }
        }
        ClaimStage::WithdrawalSubmitted | ClaimStage::ClaimConfirmed => {
            return reconcile_claim(claim).await
        }
        ClaimStage::Settled => return Ok(claim),
    }
    Ok(save(claim))
}

async fn reconcile_claim(mut claim: NicpRedemptionClaim) -> Result<NicpRedemptionClaim, String> {
    let receiver = claim.account(ic_cdk::id());
    let (requests,): (Vec<WithdrawalDetails>,) =
        ic_cdk::call(principal(WN), "get_withdrawal_requests", (Some(receiver),))
            .await
            .map_err(|e| format!("Withdrawal read failed; keep the claim fenced: {e:?}"))?;
    // Unique subaccount has only this migration. More than one request signals
    // an invariant violation; choosing a convenient row would hide it.
    if requests.len() != 1 {
        return hold(
            claim,
            "Expected exactly one withdrawal for the dedicated account; held for reconciliation"
                .into(),
        );
    }
    let details = requests.into_iter().next().unwrap();
    validate_request(&claim, &details.request, receiver)?;
    if matches!(
        details.status,
        WithdrawalStatus::NotFound | WithdrawalStatus::Cancelled
    ) {
        return hold(
            claim,
            "Withdrawal missing or cancelled; collateral remains fenced".into(),
        );
    }
    claim.withdrawal_id = Some(details.request.withdrawal_id);
    claim.burn_block = Some(details.request.nicp_burn_index);
    claim.icp_due_e8s = details.request.icp_due;
    claim.stage = ClaimStage::ClaimConfirmed;
    claim = save(claim);
    if let WithdrawalStatus::ConversionDone {
        transfer_block_height,
    } = details.status
    {
        let block =
            crate::native_icp_proof::query_block(principal(ICP), transfer_block_height).await?;
        let received = verify_receipt(&claim, &block, ic_cdk::id())?;
        if balance(principal(ICP), receiver).await? < received {
            return hold(
                claim,
                "Settlement receipt exists but dedicated ICP balance is short".into(),
            );
        }
        claim.received_icp_e8s = Some(received);
        claim.settlement_block = Some(transfer_block_height);
        claim.stage = ClaimStage::Settled;
        return Ok(save(claim));
    }
    Ok(claim)
}

fn validate_request(
    claim: &NicpRedemptionClaim,
    request: &WithdrawalRequest,
    receiver: Account,
) -> Result<(), String> {
    if request.receiver != receiver
        || request.nicp_burned != claim.redeem_nicp_e8s
        || request.icp_due == 0
        || request.timestamp < claim.created_at_ns
        || claim
            .withdrawal_id
            .is_some_and(|id| id != request.withdrawal_id)
        || claim
            .burn_block
            .is_some_and(|id| id != request.nicp_burn_index)
    {
        return Err("WaterNeuron request does not match the dedicated migration".into());
    }
    Ok(())
}

fn account_identifier(owner: Principal, sub: [u8; 32]) -> [u8; 32] {
    let mut h = Sha224::new();
    h.update(b"\x0Aaccount-id");
    h.update(owner.as_slice());
    h.update(sub);
    let hash = h.finalize();
    let mut crc = !0u32;
    for byte in &hash {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    let mut result = [0u8; 32];
    result[..4].copy_from_slice(&(!crc).to_be_bytes());
    result[4..].copy_from_slice(&hash);
    result
}

fn verify_receipt(
    claim: &NicpRedemptionClaim,
    block: &crate::native_icp_proof::Block,
    protocol: Principal,
) -> Result<u64, String> {
    match &block.transaction.operation {
        Some(crate::native_icp_proof::Operation::Transfer { to, amount, .. })
            if to.as_slice() == account_identifier(protocol, claim.escrow_subaccount)
                && block.timestamp.timestamp_nanos >= claim.created_at_ns
                && amount.e8s > 0
                && amount.e8s <= claim.icp_due_e8s =>
        {
            Ok(amount.e8s)
        }
        _ => Err("Settlement block does not credit this claim's isolated ICP account".into()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn fixture() -> NicpRedemptionClaim {
        NicpRedemptionClaim {
            vault_id: 51,
            owner: Principal::from_slice(&[1]),
            stage: ClaimStage::ClaimConfirmed,
            original_nicp_e8s: 150_000_000_000,
            redeem_nicp_e8s: 149_999_980_000,
            icp_due_e8s: 194_285_494_567,
            received_icp_e8s: None,
            fee_e8s: 10_000,
            escrow_subaccount: [9; 32],
            funding_nonce: 1,
            approval_nonce: 2,
            funding_block: Some(3),
            approval_block: Some(4),
            withdrawal_id: Some(5),
            burn_block: Some(6),
            settlement_block: None,
            created_at_ns: 100,
            updated_at_ns: 100,
            last_error: None,
        }
    }
    #[test]
    fn quote_is_fixed_integer_icp_not_round_2000() {
        assert_eq!(
            quote_icp(150_000_000_000, 77_205_959).unwrap(),
            194_285_521_406
        );
        assert!(quote_icp(1, 0).is_err());
    }
    #[test]
    fn dedicated_accounts_are_generation_bound() {
        assert_ne!(subaccount(1), subaccount(2));
    }
    #[test]
    fn only_exact_request_matches_and_settled_remains_fenced() {
        let c = fixture();
        let receiver = c.account(Principal::from_slice(&[2]));
        let mut r = WithdrawalRequest {
            nicp_burned: c.redeem_nicp_e8s,
            withdrawal_id: 5,
            icp_due: c.icp_due_e8s,
            nicp_burn_index: 6,
            timestamp: 101,
            receiver,
            neuron_id: None,
        };
        assert!(validate_request(&c, &r, receiver).is_ok());
        r.nicp_burned += 1;
        assert!(validate_request(&c, &r, receiver).is_err());
        let mut settled = c;
        settled.stage = ClaimStage::Settled;
        settled.received_icp_e8s = Some(123);
        assert!(settled.blocks_vault());
        assert_eq!(settled.fixed_icp_amount_e8s(), 123);
    }
    #[test]
    fn default_account_identifier_matches_existing_native_proof() {
        let p = Principal::from_slice(&[5]);
        assert_eq!(
            account_identifier(p, [0; 32]),
            crate::native_icp_proof::account_identifier(p)
        );
    }
    #[test]
    fn journal_roundtrips_every_phase() {
        for stage in [
            ClaimStage::Prepared,
            ClaimStage::FundingSubmitted,
            ClaimStage::Funded,
            ClaimStage::ApprovalSubmitted,
            ClaimStage::Approved,
            ClaimStage::WithdrawalSubmitted,
            ClaimStage::ClaimConfirmed,
            ClaimStage::Settled,
        ] {
            let mut c = fixture();
            c.stage = stage;
            let bytes = serde_json::to_vec(&c).unwrap();
            assert_eq!(
                serde_json::from_slice::<NicpRedemptionClaim>(&bytes).unwrap(),
                c
            );
        }
    }
}
