//! Durable, owner-bound ICP escrow for ckSOL processing.
pub const ATTACHED_CYCLES: u64 = 1_000_000_000_000;
pub const OPERATING_CYCLES: u64 = 1_000_000_000;
pub const SERVICE_BPS: u64 = 500;
const INTERRUPTED_MESSAGE: &str =
    "Processing was interrupted by an upgrade; SOL status remains unknown";
fn ceil_div(n: u128, d: u128) -> Option<u64> {
    if d == 0 {
        return None;
    }
    u64::try_from(n / d + u128::from(n % d != 0)).ok()
}
pub fn deposit_cap(rate: u64, ledger_fee: u64) -> Option<u64> {
    if ledger_fee == 0 {
        return None;
    }
    let max_service = ceil_div(ATTACHED_CYCLES as u128 * SERVICE_BPS as u128, 10_000)?;
    let max_charge = ceil_div(
        (ATTACHED_CYCLES + max_service + OPERATING_CYCLES) as u128,
        rate as u128,
    )?;
    max_charge.checked_add(ledger_fee.checked_mul(2)?)
}
pub fn settlement(
    net_cycles: u64,
    rate: u64,
    deposit: u64,
    refund_fee: u64,
) -> Option<(u64, u64, u64)> {
    if net_cycles > ATTACHED_CYCLES {
        return None;
    }
    let service = ceil_div(net_cycles as u128 * SERVICE_BPS as u128, 10_000)?;
    let charge = ceil_div(
        net_cycles as u128 + service as u128 + OPERATING_CYCLES as u128,
        rate as u128,
    )?;
    let refund = deposit.checked_sub(charge)?.checked_sub(refund_fee)?;
    if refund == 0 {
        return None;
    }
    Some((service, charge, refund))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cap_covers_full_charge_and_positive_refund() {
        for rate in [1, 3, 10_000, 42_001, u64::MAX] {
            let fee = 10_000;
            let deposit = deposit_cap(rate, fee).unwrap();
            let (service, charge, refund) =
                settlement(ATTACHED_CYCLES, rate, deposit, fee).unwrap();
            assert_eq!(service, 50_000_000_000);
            assert_eq!(refund, fee);
            assert_eq!(charge + refund + fee, deposit);
        }
    }
    #[test]
    fn proportional_fee_rounds_up_and_rate_is_frozen() {
        let deposit = deposit_cap(31_337, 10_000).unwrap();
        let (service, charge, refund) = settlement(21, 31_337, deposit, 10_000).unwrap();
        assert_eq!(service, 2);
        assert_eq!(charge, 31_912);
        assert_eq!(charge + refund + 10_000, deposit);
        assert!(settlement(ATTACHED_CYCLES + 1, 31_337, deposit, 10_000).is_none());
        assert!(deposit_cap(0, 10_000).is_none());
    }
}
use crate::cksol::{CKSOL_RELAY_CANISTER_RESERVE, CKSOL_RELAY_OWNER_CAP};
use crate::cksol_paid::{CkSolPaymentPhase, DepositOutcome};
use crate::state::{mutate_state, read_state, CkSolRelayAttempt, CkSolRelayStatus, State};
use candid::{CandidType, Deserialize, Nat, Principal};
use icrc_ledger_types::icrc1::{
    account::Account,
    transfer::{TransferArg, TransferError},
};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use serde::Serialize;
use sha2::{Digest, Sha256};
const ICP: &str = "ryjl3-tyaaa-aaaaa-aaaba-cai";
const CMC: &str = "rkp4c-7iaaa-aaaaa-aaaca-cai";
fn icp() -> Principal {
    Principal::from_text(ICP).unwrap()
}
fn cmc() -> Principal {
    Principal::from_text(CMC).unwrap()
}
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct CkSolIcpPayment {
    pub sequence: u64,
    pub phase: CkSolPaymentPhase,
    pub created_at_time: u64,
    pub deposit_e8s: u64,
    pub rate_permyriad: u64,
    pub rate_timestamp_seconds: u64,
    pub ledger_fee_e8s: u64,
    pub funding_block: Option<Nat>,
    pub deposit_id: Option<u64>,
    pub net_cycles: u64,
    pub service_fee_cycles: u64,
    pub operating_fee_cycles: u64,
    pub charged_e8s: u64,
    pub refundable_e8s: u64,
    pub refund_block: Option<Nat>,
    pub message: Option<String>,
    pub actual_refund_fee_e8s: u64,
}
#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Journal {
    pub payment: CkSolIcpPayment,
    pub subaccount: [u8; 32],
    pub refund_time: u64,
    #[serde(default)]
    pub refund_blocked: bool,
    /// A minter call was outstanding across a successful canister upgrade.
    #[serde(default)]
    pub orphaned_minter_call: bool,
}
#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct CkSolIcpTerms {
    pub next_sequence: u64,
    pub attached_cycles: u64,
    pub operating_fee_cycles: u64,
    pub service_fee_bps: u64,
    pub reserve_available_cycles: u64,
    pub available: bool,
    pub icp_ledger: Principal,
    pub cmc: Principal,
}
pub fn terms() -> CkSolIcpTerms {
    read_state(|s| CkSolIcpTerms {
        next_sequence: s.cksol_icp_sequence.saturating_add(1),
        attached_cycles: ATTACHED_CYCLES,
        operating_fee_cycles: OPERATING_CYCLES,
        service_fee_bps: SERVICE_BPS,
        reserve_available_cycles: s.cksol_icp_reserve,
        available: s.cksol_icp_reserve >= ATTACHED_CYCLES
            && transport::balance() >= total_protected(s)
            && s.cksol_icp_sequence < u64::MAX
            && s.cksol_relay_attempts.len() < CKSOL_RELAY_OWNER_CAP
            && (s.cksol_icp_payments.len() < CKSOL_RELAY_OWNER_CAP
                || s.cksol_icp_payments.values().any(|j| {
                    !j.orphaned_minter_call
                        && matches!(
                            j.payment.phase,
                            CkSolPaymentPhase::Complete | CkSolPaymentPhase::FundingRejected
                        )
                })),
        icp_ledger: icp(),
        cmc: cmc(),
    })
}

/// Called exclusively while restoring state in post_upgrade. Calls marked here
/// may have executed at the minter; never redispatch or credit their cycles.
pub fn mark_interrupted_minter_calls(s: &mut State, now: u64) {
    s.cksol_icp_upgrade_epoch = s
        .cksol_icp_upgrade_epoch
        .checked_add(1)
        .expect("ckSOL upgrade epoch exhausted");
    for journal in s.cksol_icp_payments.values_mut() {
        if journal.payment.phase == CkSolPaymentPhase::DepositPending {
            journal.orphaned_minter_call = true;
            if journal.payment.funding_block.is_none() {
                continue;
            }
            if let Some(refund) = journal
                .payment
                .deposit_e8s
                .checked_sub(journal.payment.ledger_fee_e8s)
            {
                journal.payment.net_cycles = 0;
                journal.payment.service_fee_cycles = 0;
                journal.payment.operating_fee_cycles = 0;
                journal.payment.charged_e8s = 0;
                journal.payment.refundable_e8s = refund;
                journal.payment.actual_refund_fee_e8s = journal.payment.ledger_fee_e8s;
                journal.payment.message = Some(INTERRUPTED_MESSAGE.into());
                journal.refund_time = now;
                journal.payment.phase = CkSolPaymentPhase::RefundReady;
            }
        }
    }
}

pub(crate) fn blocks_cycles_payment(j: &Journal) -> bool {
    j.orphaned_minter_call
        || !matches!(
            j.payment.phase,
            CkSolPaymentPhase::Complete | CkSolPaymentPhase::FundingRejected
        )
}

pub fn payment(owner: Principal) -> Option<CkSolIcpPayment> {
    read_state(|s| s.cksol_icp_payments.get(&owner).map(|j| j.payment.clone()))
}
pub fn reserved(s: &State) -> u128 {
    s.cksol_icp_payments
        .values()
        .filter(|j| {
            matches!(
                j.payment.phase,
                CkSolPaymentPhase::FundingPending
                    | CkSolPaymentPhase::FundingUnknown
                    | CkSolPaymentPhase::Funded
                    | CkSolPaymentPhase::DepositPending
            ) || j.orphaned_minter_call
        })
        .count() as u128
        * ATTACHED_CYCLES as u128
}
// DepositPending has already attached its cycles to the minter call. Keep it
// in reserved() for the allocation cap, but exclude it from liquid backing.
fn liquid_reserved(s: &State) -> u128 {
    s.cksol_icp_payments
        .values()
        .filter(|j| {
            matches!(
                j.payment.phase,
                CkSolPaymentPhase::FundingPending
                    | CkSolPaymentPhase::FundingUnknown
                    | CkSolPaymentPhase::Funded
            )
        })
        .count() as u128
        * ATTACHED_CYCLES as u128
}
pub fn icp_protected(s: &State) -> u128 {
    s.cksol_icp_reserve as u128 + liquid_reserved(s)
}
fn legacy_liabilities(s: &State) -> u128 {
    s.cksol_paid_payments
        .values()
        .map(|p| p.refundable_cycles as u128)
        .sum()
}
fn total_protected(s: &State) -> u128 {
    CKSOL_RELAY_CANISTER_RESERVE
        .saturating_add(legacy_liabilities(s))
        .saturating_add(icp_protected(s))
}
pub fn set_reserve(caller: Principal, amount: u64) -> Result<(), String> {
    mutate_state(|s| {
        if caller == Principal::anonymous() || caller != s.developer_principal {
            return Err("Only the developer can set the ckSOL ICP reserve".into());
        }
        if (amount as u128).saturating_add(reserved(s)) > 100_000_000_000_000 {
            return Err("Reserve exceeds limit".into());
        }
        let need = CKSOL_RELAY_CANISTER_RESERVE
            .saturating_add(legacy_liabilities(s))
            .saturating_add(liquid_reserved(s))
            .saturating_add(amount as u128);
        if transport::balance() < need {
            return Err("Insufficient reserve backing".into());
        }
        s.cksol_icp_reserve = amount;
        Ok(())
    })
}
fn hash(domain: &[u8], owner: Principal, seq: u64) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(domain);
    h.update(owner.as_slice());
    h.update(seq.to_be_bytes());
    h.finalize().into()
}
fn message(s: String) -> String {
    s.chars().take(512).collect()
}
struct Guard(Principal, u64);
impl Guard {
    fn acquire(p: Principal) -> Result<Self, String> {
        if p == Principal::anonymous() {
            return Err("Connect an ICP wallet first".into());
        }
        mutate_state(|s| {
            if s.cksol_relay_in_flight.contains(&p)
                || s.cksol_relay_in_flight.len() >= CKSOL_RELAY_OWNER_CAP
            {
                return Err("ckSOL account processing or capacity reached".into());
            }
            s.cksol_relay_in_flight.insert(p);
            Ok(Self(p, s.cksol_icp_upgrade_epoch))
        })
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        mutate_state(|s| {
            if s.cksol_icp_upgrade_epoch == self.1 {
                s.cksol_relay_in_flight.remove(&self.0);
            }
        });
    }
}
#[derive(CandidType, Deserialize)]
struct Rate {
    timestamp_seconds: u64,
    xdr_permyriad_per_icp: u64,
}
#[derive(CandidType, Deserialize)]
struct RateResponse {
    data: Rate,
}
async fn current_terms() -> Result<(u64, u64, u64), String> {
    let (r,): (RateResponse,) = ic_cdk::call(cmc(), "get_icp_xdr_conversion_rate", ())
        .await
        .map_err(|e| format!("CMC rate unavailable: {e:?}"))?;
    let (fee,): (Nat,) = ic_cdk::call(icp(), "icrc1_fee", ())
        .await
        .map_err(|e| format!("ICP fee unavailable: {e:?}"))?;
    Ok((
        r.data.xdr_permyriad_per_icp,
        r.data.timestamp_seconds,
        fee.0.try_into().map_err(|_| "ICP fee overflow")?,
    ))
}
async fn reconcile(owner: Principal) -> Result<(), String> {
    if read_state(|s| {
        s.cksol_paid_payments.get(&owner).is_some_and(|p| {
            p.refundable_cycles != 0
                || !matches!(
                    p.phase,
                    CkSolPaymentPhase::Complete | CkSolPaymentPhase::FundingRejected
                )
        })
    }) {
        return Err("Resolve previous cycles payment first".into());
    }
    let old = read_state(|s| s.cksol_relay_attempts.get(&owner).cloned());
    if let Some(a) = old {
        match &a.status {
            CkSolRelayStatus::Queued(id) if !crate::cksol::deposit_terminal(*id).await? => {
                return Err("Previous SOL deposit still processing".into())
            }
            CkSolRelayStatus::SubmittedOrUnknown => {
                return Err("Previous SOL deposit outcome unknown".into())
            }
            _ => {}
        }
        mutate_state(|s| -> Result<(), String> {
            if s.cksol_relay_attempts.get(&owner) != Some(&a) {
                return Err("Previous deposit changed".into());
            }
            s.cksol_relay_attempts.remove(&owner);
            Ok(())
        })?;
    }
    Ok(())
}
pub async fn start(
    owner: Principal,
    seq: u64,
    deposit: u64,
    rate: u64,
    rate_time: u64,
    fee: u64,
) -> Result<CkSolIcpPayment, String> {
    let _guard = Guard::acquire(owner)?;
    if let Some(p) = payment(owner) {
        if p.sequence == seq {
            return Ok(p);
        }
        if read_state(|s| {
            s.cksol_icp_payments
                .get(&owner)
                .is_some_and(|j| j.orphaned_minter_call)
        }) {
            return Err("Previous interrupted minter call is held for reconciliation".into());
        }
        if !matches!(
            p.phase,
            CkSolPaymentPhase::Complete | CkSolPaymentPhase::FundingRejected
        ) {
            return Err("Resolve previous ICP payment".into());
        }
    }
    reconcile(owner).await?;
    let current = transport::terms().await?;
    let now = transport::time();
    let secs = now / 1_000_000_000;
    if current != (rate, rate_time, fee)
        || rate_time > secs
        || secs - rate_time > 600
        || deposit_cap(rate, fee) != Some(deposit)
    {
        return Err("Payment terms changed or expired; refresh quote".into());
    }
    let sub = hash(b"rumi-cksol-icp-escrow-v1", owner, seq);
    mutate_state(|s| -> Result<(), String> {
        if s.cksol_icp_sequence.checked_add(1) != Some(seq) {
            return Err("Refresh sequence".into());
        }
        if s.cksol_relay_attempts.len() >= CKSOL_RELAY_OWNER_CAP {
            return Err("Deposit capacity reached".into());
        }
        if s.cksol_icp_reserve < ATTACHED_CYCLES || transport::balance() < total_protected(s) {
            return Err("Processing reserve unavailable".into());
        }
        if !s.cksol_icp_payments.contains_key(&owner)
            && s.cksol_icp_payments.len() >= CKSOL_RELAY_OWNER_CAP
        {
            let old = s
                .cksol_icp_payments
                .iter()
                .filter(|(_, j)| {
                    !j.orphaned_minter_call
                        && matches!(
                            j.payment.phase,
                            CkSolPaymentPhase::Complete | CkSolPaymentPhase::FundingRejected
                        )
                })
                .min_by_key(|(_, j)| j.payment.sequence)
                .map(|(o, _)| *o)
                .ok_or("Payment capacity reached")?;
            s.cksol_icp_payments.remove(&old);
        }
        s.cksol_icp_reserve -= ATTACHED_CYCLES;
        s.cksol_relay_attempts.insert(
            owner,
            CkSolRelayAttempt {
                started_at_ns: now,
                status: CkSolRelayStatus::SubmittedOrUnknown,
            },
        );
        s.cksol_icp_sequence = seq;
        s.cksol_icp_payments.insert(
            owner,
            Journal {
                subaccount: sub,
                refund_time: 0,
                refund_blocked: false,
                orphaned_minter_call: false,
                payment: CkSolIcpPayment {
                    sequence: seq,
                    phase: CkSolPaymentPhase::FundingPending,
                    created_at_time: now,
                    deposit_e8s: deposit,
                    rate_permyriad: rate,
                    rate_timestamp_seconds: rate_time,
                    ledger_fee_e8s: fee,
                    funding_block: None,
                    deposit_id: None,
                    net_cycles: 0,
                    service_fee_cycles: 0,
                    operating_fee_cycles: 0,
                    charged_e8s: 0,
                    refundable_e8s: 0,
                    refund_block: None,
                    message: None,
                    actual_refund_fee_e8s: 0,
                },
            },
        );
        Ok(())
    })?;
    let args = TransferFromArgs {
        spender_subaccount: None,
        from: Account {
            owner,
            subaccount: None,
        },
        to: Account {
            owner: transport::canister(),
            subaccount: Some(sub),
        },
        amount: Nat::from(deposit),
        fee: Some(Nat::from(fee)),
        memo: Some(hash(b"rumi-cksol-icp-funding", owner, seq).to_vec().into()),
        created_at_time: Some(now),
    };
    let result = transport::fund(args).await;
    let applied = mutate_state(|s| {
        if s.cksol_icp_upgrade_epoch != _guard.1
            || !s.cksol_icp_payments.get(&owner).is_some_and(|j| {
                j.payment.sequence == seq && j.payment.phase == CkSolPaymentPhase::FundingPending
            })
        {
            return false;
        }
        funding_result(s, owner, result);
        true
    });
    if !applied {
        return payment(owner).ok_or("No ICP payment".into());
    }
    advance(owner, seq).await
}
fn funding_result(
    s: &mut State,
    owner: Principal,
    result: Result<Result<Nat, TransferFromError>, String>,
) {
    let p = &mut s.cksol_icp_payments.get_mut(&owner).unwrap().payment;
    match result {
        Ok(Ok(b)) => {
            p.funding_block = Some(b);
            p.phase = CkSolPaymentPhase::Funded
        }
        Ok(Err(e)) => {
            let definite = matches!(
                e,
                TransferFromError::BadFee { .. }
                    | TransferFromError::BadBurn { .. }
                    | TransferFromError::InsufficientFunds { .. }
                    | TransferFromError::InsufficientAllowance { .. }
                    | TransferFromError::CreatedInFuture { .. }
                    | TransferFromError::TooOld
            );
            p.phase = if definite {
                s.cksol_icp_reserve = s.cksol_icp_reserve.saturating_add(ATTACHED_CYCLES);
                s.cksol_relay_attempts.remove(&owner);
                CkSolPaymentPhase::FundingRejected
            } else {
                CkSolPaymentPhase::FundingUnknown
            };
            p.message = Some(message(format!("Funding not confirmed: {e:?}")));
        }
        Err(e) => {
            p.phase = CkSolPaymentPhase::FundingUnknown;
            p.message = Some(message(format!("Funding response unavailable: {e:?}")));
        }
    }
}
async fn advance(owner: Principal, seq: u64) -> Result<CkSolIcpPayment, String> {
    let p = payment(owner).ok_or("No ICP payment")?;
    if p.sequence != seq {
        return Err("Payment sequence mismatch".into());
    }
    if p.phase == CkSolPaymentPhase::Funded
        && read_state(|s| transport::balance() < total_protected(s))
    {
        mutate_state(|s| {
            let j = s.cksol_icp_payments.get_mut(&owner).unwrap();
            j.payment.operating_fee_cycles = 0;
            j.payment.actual_refund_fee_e8s = j.payment.ledger_fee_e8s;
            j.payment.refundable_e8s = j.payment.deposit_e8s - j.payment.ledger_fee_e8s;
            j.payment.message =
                Some("Processing reserve unavailable; returning the unused deposit".into());
            j.refund_time = transport::time();
            j.payment.phase = CkSolPaymentPhase::RefundReady;
            s.cksol_icp_reserve = s.cksol_icp_reserve.saturating_add(ATTACHED_CYCLES);
            s.cksol_relay_attempts.remove(&owner);
        });
    }
    if payment(owner).ok_or("No ICP payment")?.phase == CkSolPaymentPhase::Funded {
        mutate_state(|s| -> Result<(), String> {
            if !s.cksol_relay_attempts.contains_key(&owner) {
                return Err("Reserved deposit slot missing; payment held".into());
            }
            if transport::balance() < total_protected(s) {
                return Err("Protected cycle backing unavailable".into());
            }
            s.cksol_icp_payments.get_mut(&owner).unwrap().payment.phase =
                CkSolPaymentPhase::DepositPending;
            s.cksol_relay_attempts.insert(
                owner,
                CkSolRelayAttempt {
                    started_at_ns: transport::time(),
                    status: CkSolRelayStatus::SubmittedOrUnknown,
                },
            );
            Ok(())
        })?;
        let (outcome, returned) = transport::mint(owner).await;
        let applied = mutate_state(|s| {
            let Some(j) = s.cksol_icp_payments.get_mut(&owner) else {
                return false;
            };
            if j.payment.sequence != seq
                || j.orphaned_minter_call
                || j.payment.phase != CkSolPaymentPhase::DepositPending
            {
                return false;
            }
            let net = match (ATTACHED_CYCLES as u128).checked_sub(returned) {
                Some(n) => n as u64,
                None => {
                    j.payment.message = Some("Invalid minter refund".into());
                    return false;
                }
            };
            let Some((service, charge, refund)) = settlement(
                net,
                j.payment.rate_permyriad,
                j.payment.deposit_e8s,
                j.payment.ledger_fee_e8s,
            ) else {
                j.payment.message = Some("Escrow settlement held".into());
                return false;
            };
            j.payment.net_cycles = net;
            j.payment.service_fee_cycles = service;
            j.payment.operating_fee_cycles = OPERATING_CYCLES;
            j.payment.charged_e8s = charge;
            j.payment.refundable_e8s = refund;
            j.payment.actual_refund_fee_e8s = j.payment.ledger_fee_e8s;
            j.refund_time = transport::time();
            j.payment.phase = CkSolPaymentPhase::RefundReady;
            s.cksol_icp_reserve = s.cksol_icp_reserve.saturating_add(returned as u64);
            let status = match outcome {
                DepositOutcome::Queued(id) => {
                    j.payment.deposit_id = Some(id);
                    CkSolRelayStatus::Queued(id)
                }
                DepositOutcome::Rejected(e) => {
                    j.payment.message = Some(message(e.clone()));
                    CkSolRelayStatus::Rejected(message(e))
                }
                DepositOutcome::Unknown(e) => {
                    j.payment.message = Some(message(e));
                    CkSolRelayStatus::SubmittedOrUnknown
                }
            };
            s.cksol_relay_attempts.get_mut(&owner).unwrap().status = status;
            true
        });
        if !applied {
            return payment(owner).ok_or("No ICP payment".into());
        }
    }
    let j = read_state(|s| s.cksol_icp_payments.get(&owner).cloned()).unwrap();
    if matches!(
        j.payment.phase,
        CkSolPaymentPhase::RefundReady
            | CkSolPaymentPhase::RefundUnknown
            | CkSolPaymentPhase::RefundPending
    ) && !j.refund_blocked
    {
        mutate_state(|s| {
            s.cksol_icp_payments.get_mut(&owner).unwrap().payment.phase =
                CkSolPaymentPhase::RefundPending
        });
        let args = refund_args(owner, &j);
        let result = transport::refund(args).await;
        mutate_state(|s| {
            if s.cksol_icp_payments
                .get(&owner)
                .is_some_and(|j| j.payment.sequence == seq)
            {
                refund_result(s, owner, result);
            }
        });
    }
    payment(owner).ok_or("No ICP payment".into())
}
fn refund_args(owner: Principal, j: &Journal) -> TransferArg {
    TransferArg {
        from_subaccount: Some(j.subaccount),
        to: Account {
            owner,
            subaccount: None,
        },
        amount: Nat::from(j.payment.refundable_e8s),
        fee: Some(Nat::from(j.payment.actual_refund_fee_e8s)),
        created_at_time: Some(j.refund_time),
        memo: Some(
            hash(b"rumi-cksol-icp-refund", owner, j.payment.sequence)
                .to_vec()
                .into(),
        ),
    }
}
fn refund_result(
    s: &mut State,
    owner: Principal,
    result: Result<Result<Nat, TransferError>, String>,
) {
    let Some(journal) = s.cksol_icp_payments.get_mut(&owner) else {
        return;
    };
    let p = &mut journal.payment;
    if p.phase != CkSolPaymentPhase::RefundPending {
        return;
    }
    match result {
        Ok(Ok(b)) | Ok(Err(TransferError::Duplicate { duplicate_of: b })) => {
            p.refund_block = Some(b);
            p.phase = CkSolPaymentPhase::Complete
        }
        Ok(Err(e)) => {
            p.phase = if matches!(e, TransferError::BadFee { .. } | TransferError::TooOld) {
                journal.refund_blocked = true;
                CkSolPaymentPhase::RefundPending
            } else {
                CkSolPaymentPhase::RefundUnknown
            };
            let prefix = if journal.orphaned_minter_call {
                format!("{INTERRUPTED_MESSAGE}; ")
            } else {
                String::new()
            };
            p.message = Some(message(format!("{prefix}Refund held: {e:?}")));
        }
        Err(e) => {
            p.phase = CkSolPaymentPhase::RefundUnknown;
            let prefix = if journal.orphaned_minter_call {
                format!("{INTERRUPTED_MESSAGE}; ")
            } else {
                String::new()
            };
            p.message = Some(message(format!(
                "{prefix}Refund response unavailable: {e:?}"
            )));
        }
    }
}
/// Recover only a proven original funding debit. The supplied index is a hint;
/// the canonical ledger is queried directly and every transfer field is checked.
/// This prepares a refund and never submits a new debit or minter request.
pub async fn reconcile_funding_from_block(
    owner: Principal,
    seq: u64,
    block_index: u64,
) -> Result<CkSolIcpPayment, String> {
    let guard = Guard::acquire(owner)?;
    let (journal, relay) = read_state(|s| {
        (
            s.cksol_icp_payments.get(&owner).cloned(),
            s.cksol_relay_attempts.get(&owner).cloned(),
        )
    });
    let journal = journal.ok_or("No ICP payment")?;
    if journal.payment.sequence != seq
        || !matches!(
            journal.payment.phase,
            CkSolPaymentPhase::FundingPending | CkSolPaymentPhase::FundingUnknown
        )
        || journal.payment.funding_block.is_some()
        || journal.orphaned_minter_call
    {
        return Err("Payment is not awaiting funding reconciliation".into());
    }
    let expected_relay = relay.ok_or("Reserved deposit slot missing; payment held")?;
    if expected_relay.status != CkSolRelayStatus::SubmittedOrUnknown {
        return Err("Reserved deposit slot changed; payment held".into());
    }
    let block = crate::native_icp_proof::query_direct_block(icp(), block_index).await?;
    crate::native_icp_proof::verify_transfer_from_block_to_subaccount(
        &block,
        owner,
        transport::canister(),
        transport::canister(),
        journal.subaccount,
        journal.payment.deposit_e8s,
        journal.payment.ledger_fee_e8s,
        &hash(b"rumi-cksol-icp-funding", owner, seq),
        journal.payment.created_at_time,
    )?;
    mutate_state(|s| -> Result<(), String> {
        if s.cksol_icp_upgrade_epoch != guard.1
            || s.cksol_icp_payments.get(&owner) != Some(&journal)
            || s.cksol_relay_attempts.get(&owner) != Some(&expected_relay)
        {
            return Err("Payment changed during proof verification; payment held".into());
        }
        let refund = journal
            .payment
            .deposit_e8s
            .checked_sub(journal.payment.ledger_fee_e8s)
            .filter(|n| *n > 0)
            .ok_or("Deposit too small to refund")?;
        let reserve = s
            .cksol_icp_reserve
            .checked_add(ATTACHED_CYCLES)
            .ok_or("Reserve overflow; payment held")?;
        let j = s.cksol_icp_payments.get_mut(&owner).unwrap();
        j.payment.funding_block = Some(Nat::from(block_index));
        j.payment.charged_e8s = 0;
        j.payment.operating_fee_cycles = 0;
        j.payment.service_fee_cycles = 0;
        j.payment.refundable_e8s = refund;
        j.payment.actual_refund_fee_e8s = j.payment.ledger_fee_e8s;
        j.payment.message = Some(
            "Original ICP deposit verified; return available. No SOL processing was submitted."
                .into(),
        );
        j.refund_time = transport::time();
        j.payment.phase = CkSolPaymentPhase::RefundReady;
        s.cksol_icp_reserve = reserve;
        s.cksol_relay_attempts.remove(&owner);
        Ok(())
    })?;
    payment(owner).ok_or("No ICP payment".into())
}

pub async fn resume(owner: Principal, seq: u64) -> Result<CkSolIcpPayment, String> {
    let _guard = Guard::acquire(owner)?;
    advance(owner, seq).await
}

#[cfg(test)]
mod state_tests {
    use super::*;
    fn journal(phase: CkSolPaymentPhase) -> Journal {
        Journal {
            subaccount: [42; 32],
            refund_time: 123,
            refund_blocked: false,
            orphaned_minter_call: false,
            payment: CkSolIcpPayment {
                sequence: 1,
                phase,
                created_at_time: 99,
                deposit_e8s: 100_000,
                rate_permyriad: 50_000,
                rate_timestamp_seconds: 10,
                ledger_fee_e8s: 10_000,
                funding_block: Some(Nat::from(5u64)),
                deposit_id: None,
                net_cycles: 0,
                service_fee_cycles: 0,
                operating_fee_cycles: OPERATING_CYCLES,
                charged_e8s: 0,
                refundable_e8s: 0,
                refund_block: None,
                message: None,
                actual_refund_fee_e8s: 0,
            },
        }
    }
    #[test]
    fn late_refund_failure_cannot_overwrite_complete_receipt() {
        let owner = Principal::from_slice(&[7]);
        let mut s = State::default();
        let mut j = journal(CkSolPaymentPhase::Complete);
        j.payment.refund_block = Some(Nat::from(9u8));
        s.cksol_icp_payments.insert(owner, j.clone());
        refund_result(&mut s, owner, Err("late error".into()));
        assert_eq!(s.cksol_icp_payments[&owner], j);
    }
    #[test]
    fn legacy_journal_defaults_orphan_flag_to_false() {
        let j = journal(CkSolPaymentPhase::DepositPending);
        let mut value = serde_json::to_value(&j).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("orphaned_minter_call");
        let decoded: Journal = serde_json::from_value(value).unwrap();
        assert!(!decoded.orphaned_minter_call);
        assert_eq!(decoded.payment.phase, CkSolPaymentPhase::DepositPending);
    }
    #[test]
    fn orphan_refund_errors_preserve_interruption_message() {
        let owner = Principal::from_slice(&[7]);
        let mut s = State::default();
        let mut j = journal(CkSolPaymentPhase::RefundPending);
        j.orphaned_minter_call = true;
        s.cksol_icp_payments.insert(owner, j);
        refund_result(&mut s, owner, Err("lost reply".into()));
        assert!(s.cksol_icp_payments[&owner]
            .payment
            .message
            .as_deref()
            .unwrap()
            .starts_with(INTERRUPTED_MESSAGE));
        s.cksol_icp_payments.get_mut(&owner).unwrap().payment.phase =
            CkSolPaymentPhase::RefundPending;
        refund_result(
            &mut s,
            owner,
            Ok(Err(TransferError::BadFee {
                expected_fee: Nat::from(20_000u64),
            })),
        );
        assert!(s.cksol_icp_payments[&owner]
            .payment
            .message
            .as_deref()
            .unwrap()
            .starts_with(INTERRUPTED_MESSAGE));
    }
    #[test]
    fn completed_orphan_blocks_cycles_payment() {
        let mut j = journal(CkSolPaymentPhase::Complete);
        assert!(!blocks_cycles_payment(&j));
        j.orphaned_minter_call = true;
        assert!(blocks_cycles_payment(&j));
    }
    #[test]
    fn protected_balance_counts_free_reserve_and_unresolved_reservations() {
        let mut s = State::default();
        s.cksol_icp_reserve = ATTACHED_CYCLES * 2;
        s.cksol_icp_payments.insert(
            Principal::from_slice(&[1]),
            journal(CkSolPaymentPhase::FundingUnknown),
        );
        s.cksol_icp_payments.insert(
            Principal::from_slice(&[2]),
            journal(CkSolPaymentPhase::DepositPending),
        );
        assert_eq!(reserved(&s), ATTACHED_CYCLES as u128 * 2);
        assert_eq!(icp_protected(&s), ATTACHED_CYCLES as u128 * 3);
        s.cksol_icp_payments
            .get_mut(&Principal::from_slice(&[1]))
            .unwrap()
            .payment
            .phase = CkSolPaymentPhase::RefundReady;
        assert_eq!(icp_protected(&s), ATTACHED_CYCLES as u128 * 2);
    }
    #[test]
    fn dispatched_attachment_does_not_block_another_funded_owner() {
        let a = Principal::from_slice(&[1]);
        let b = Principal::from_slice(&[2]);
        let mut s = State::default();
        s.cksol_icp_payments
            .insert(a, journal(CkSolPaymentPhase::Funded));
        s.cksol_icp_payments
            .insert(b, journal(CkSolPaymentPhase::Funded));
        s.cksol_paid_payments.insert(
            Principal::from_slice(&[3]),
            crate::cksol_paid::CkSolPayment {
                sequence: 1,
                created_at_time: 1,
                phase: CkSolPaymentPhase::RefundUnknown,
                funding_block: None,
                deposit_id: None,
                refundable_cycles: 123,
                refund_block: None,
                message: None,
            },
        );
        let initial = CKSOL_RELAY_CANISTER_RESERVE + 2 * ATTACHED_CYCLES as u128 + 123;
        assert_eq!(total_protected(&s), initial);
        s.cksol_icp_payments.get_mut(&a).unwrap().payment.phase = CkSolPaymentPhase::DepositPending;
        let after_dispatch = initial - ATTACHED_CYCLES as u128;
        assert_eq!(total_protected(&s), after_dispatch);
        assert!(after_dispatch >= total_protected(&s));
        assert!(after_dispatch - 1 < total_protected(&s));
        assert_eq!(reserved(&s), 2 * ATTACHED_CYCLES as u128);
        assert_eq!(s.cksol_icp_reserve, 0);
    }
    #[test]
    fn legacy_refund_liability_is_added_to_new_reservations() {
        let owner = Principal::from_slice(&[7]);
        let mut s = State::default();
        s.cksol_icp_reserve = ATTACHED_CYCLES;
        s.cksol_paid_payments.insert(
            owner,
            crate::cksol_paid::CkSolPayment {
                sequence: 1,
                created_at_time: 1,
                phase: CkSolPaymentPhase::RefundUnknown,
                funding_block: None,
                deposit_id: None,
                refundable_cycles: 123,
                refund_block: None,
                message: None,
            },
        );
        assert_eq!(
            total_protected(&s),
            CKSOL_RELAY_CANISTER_RESERVE + ATTACHED_CYCLES as u128 + 123
        );
    }
    #[test]
    fn upgrade_preserves_ambiguous_effect_and_default_old_fields() {
        let owner = Principal::from_slice(&[7]);
        let mut s = State::default();
        let j = journal(CkSolPaymentPhase::RefundPending);
        s.cksol_icp_payments.insert(owner, j.clone());
        s.cksol_icp_sequence = 1;
        let mut bytes = vec![];
        ciborium::ser::into_writer(&s, &mut bytes).unwrap();
        let restored = crate::storage::decode_state_body(&bytes).unwrap();
        assert_eq!(restored.cksol_icp_payments.get(&owner), Some(&j));
        let mut old = vec![];
        ciborium::ser::into_writer(&std::collections::BTreeMap::<String, u8>::new(), &mut old)
            .unwrap();
        let restored = crate::storage::decode_state_body(&old).unwrap();
        assert_eq!(restored.cksol_icp_reserve, 0);
        assert!(restored.cksol_icp_payments.is_empty());
    }
    #[test]
    fn funding_reply_loss_holds_reserve_and_no_second_debit_is_scheduled() {
        let owner = Principal::from_slice(&[7]);
        let mut s = State::default();
        let mut pending = journal(CkSolPaymentPhase::FundingPending);
        pending.payment.funding_block = None;
        s.cksol_icp_payments.insert(owner, pending);
        funding_result(&mut s, owner, Err("lost reply".into()));
        let p = &s.cksol_icp_payments[&owner].payment;
        assert_eq!(p.phase, CkSolPaymentPhase::FundingUnknown);
        assert_eq!(reserved(&s), ATTACHED_CYCLES as u128);
        assert_eq!(p.funding_block, None);
    }
    #[test]
    fn refund_reply_loss_reuses_identical_tuple_and_duplicate_completes() {
        let owner = Principal::from_slice(&[7]);
        let mut s = State::default();
        let mut j = journal(CkSolPaymentPhase::RefundPending);
        j.payment.refundable_e8s = 50_000;
        j.payment.actual_refund_fee_e8s = 10_000;
        let before = refund_args(owner, &j);
        s.cksol_icp_payments.insert(owner, j);
        refund_result(&mut s, owner, Err("lost reply".into()));
        assert_eq!(
            s.cksol_icp_payments[&owner].payment.phase,
            CkSolPaymentPhase::RefundUnknown
        );
        assert_eq!(refund_args(owner, &s.cksol_icp_payments[&owner]), before);
        s.cksol_icp_payments.get_mut(&owner).unwrap().payment.phase =
            CkSolPaymentPhase::RefundPending;
        refund_result(
            &mut s,
            owner,
            Ok(Err(TransferError::Duplicate {
                duplicate_of: Nat::from(77u64),
            })),
        );
        assert_eq!(
            s.cksol_icp_payments[&owner].payment.refund_block,
            Some(Nat::from(77u64))
        );
        assert_eq!(
            s.cksol_icp_payments[&owner].payment.phase,
            CkSolPaymentPhase::Complete
        );
    }
    #[test]
    fn changed_refund_fee_holds_the_original_tuple() {
        let owner = Principal::from_slice(&[7]);
        let mut s = State::default();
        let j = journal(CkSolPaymentPhase::RefundPending);
        let before = refund_args(owner, &j);
        s.cksol_icp_payments.insert(owner, j);
        refund_result(
            &mut s,
            owner,
            Ok(Err(TransferError::BadFee {
                expected_fee: Nat::from(20_000u64),
            })),
        );
        assert!(s.cksol_icp_payments[&owner].refund_blocked);
        assert_eq!(refund_args(owner, &s.cksol_icp_payments[&owner]), before);
    }
    #[test]
    fn recovery_cannot_bind_an_id_before_minter_dispatch() {
        let owner = Principal::from_slice(&[7]);
        let developer = Principal::from_slice(&[8]);
        let mut s = State::default();
        s.developer_principal = developer;
        s.cksol_icp_payments
            .insert(owner, journal(CkSolPaymentPhase::FundingUnknown));
        s.cksol_relay_attempts.insert(
            owner,
            CkSolRelayAttempt {
                started_at_ns: 1,
                status: CkSolRelayStatus::SubmittedOrUnknown,
            },
        );
        crate::state::replace_state(s);
        assert!(crate::cksol::bind_observed_id(developer, owner, 42).is_err());
        assert_eq!(
            crate::cksol::attempt(owner).unwrap().status,
            CkSolRelayStatus::SubmittedOrUnknown
        );
    }
    #[test]
    fn anonymous_owner_cannot_acquire_guard() {
        assert!(Guard::acquire(Principal::anonymous()).is_err());
    }
}

#[cfg(test)]
#[path = "cksol_icp_lifecycle_tests.rs"]
mod lifecycle_tests;

mod transport {
    #[cfg(test)]
    pub(super) use super::lifecycle_tests::mock;
    use super::*;
    pub fn time() -> u64 {
        #[cfg(test)]
        if let Some(v) = mock::time() {
            return v;
        }
        ic_cdk::api::time()
    }
    pub fn balance() -> u128 {
        #[cfg(test)]
        if let Some(v) = mock::balance() {
            return v;
        }
        ic_cdk::api::canister_balance128()
    }
    pub fn canister() -> Principal {
        #[cfg(test)]
        if mock::enabled() {
            return Principal::from_slice(&[42]);
        }
        ic_cdk::id()
    }
    pub async fn terms() -> Result<(u64, u64, u64), String> {
        #[cfg(test)]
        if mock::enabled() {
            return Ok((50_000, 1000, 10_000));
        }
        current_terms().await
    }
    pub async fn fund(args: TransferFromArgs) -> Result<Result<Nat, TransferFromError>, String> {
        #[cfg(test)]
        if mock::enabled() {
            let r = mock::fund(args);
            if mock::pending("fund") {
                futures::future::poll_fn(|_| {
                    if mock::pending("fund") {
                        std::task::Poll::Pending
                    } else {
                        std::task::Poll::Ready(())
                    }
                })
                .await;
            }
            return r;
        }
        let r: Result<(Result<Nat, TransferFromError>,), _> =
            ic_cdk::call(icp(), "icrc2_transfer_from", (args,)).await;
        r.map(|(r,)| r).map_err(|e| format!("{e:?}"))
    }
    pub async fn mint(owner: Principal) -> (DepositOutcome, u128) {
        #[cfg(test)]
        if mock::enabled() {
            let r = mock::mint(owner);
            if mock::pending("mint") {
                futures::future::poll_fn(|_| {
                    if mock::pending("mint") {
                        std::task::Poll::Pending
                    } else {
                        std::task::Poll::Ready(())
                    }
                })
                .await;
            }
            return r;
        }
        crate::cksol::call_paid_minter(owner).await
    }
    pub async fn refund(args: TransferArg) -> Result<Result<Nat, TransferError>, String> {
        #[cfg(test)]
        if mock::enabled() {
            let r = mock::refund(args);
            if mock::pending("refund") {
                futures::future::poll_fn(|_| {
                    if mock::pending("refund") {
                        std::task::Poll::Pending
                    } else {
                        std::task::Poll::Ready(())
                    }
                })
                .await;
            }
            return r;
        }
        let r: Result<(Result<Nat, TransferError>,), _> =
            ic_cdk::call(icp(), "icrc1_transfer", (args,)).await;
        r.map(|(r,)| r).map_err(|e| format!("{e:?}"))
    }
}
