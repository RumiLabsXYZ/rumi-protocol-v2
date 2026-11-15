//! Rob's exact vault-51 direction: adopt verified settlement as ordinary ICP
//! collateral, preserving the vault and every liability. Never pay the owner.
use super::*;
use crate::state::{CollateralStatus, State};
use std::cell::Cell;
use std::time::Duration;

const OWNER: &str = "zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae";
const ESCROW: [u8; 32] = [
    0x6b, 0xbd, 0x6b, 0xa3, 0xf7, 0xaa, 0x00, 0xa1, 0x2f, 0x21, 0x3c, 0xc9, 0x33, 0x01, 0x39, 0x5c,
    0x88, 0x2b, 0xeb, 0x8b, 0xd5, 0x61, 0x5f, 0x31, 0x77, 0xd6, 0xeb, 0x33, 0x36, 0xf1, 0x79, 0x6a,
];
const MAX_SWEEP_FEE_E8S: u64 = 10_000;

#[derive(CandidType, Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CollateralAdoption {
    pub nonce: u128,
    pub fee_e8s: u64,
    pub amount_e8s: u64,
    pub block: Option<u64>,
    pub completed: bool,
}

pub fn is_automatic_target(claim: &NicpRedemptionClaim) -> bool {
    claim.vault_id == VAULT_ID
        && claim.owner == principal(OWNER)
        && claim.withdrawal_id == Some(1696)
        && claim.burn_block == Some(3_435_761)
        && claim.escrow_subaccount == ESCROW
        && claim.original_nicp_e8s == 150_000_000_000
        && claim.redeem_nicp_e8s == 149_998_000_000
}

thread_local! {
    static TIMER: Cell<Option<ic_cdk_timers::TimerId>> = const { Cell::new(None) };
}

/// Timers are transient. Re-register after upgrade without replacing any journal.
pub fn register_timer() {
    TIMER.with(|timer| {
        if let Some(old) = timer.take() {
            ic_cdk_timers::clear_timer(old);
        }
        timer.set(Some(ic_cdk_timers::set_timer_interval(
            Duration::from_secs(3600),
            spawn_tick,
        )));
    });
    ic_cdk_timers::set_timer(Duration::from_secs(10), spawn_tick);
}

fn spawn_tick() {
    ic_cdk::spawn(async {
        if let Err(error) = process_automatic().await {
            ic_canister_log::log!(crate::INFO, "[vault51-adoption] held: {}", error);
        }
    });
}

fn validate_original_vault(state: &State, claim: &NicpRedemptionClaim) -> Result<(), String> {
    if !is_automatic_target(claim) || claim.release.is_some() {
        return Err(
            "Claim is not the approved automatic target or has an owner payout obligation".into(),
        );
    }
    if state.icp_ledger_principal != principal(ICP) {
        return Err("Ordinary custody is not the expected native ICP ledger".into());
    }
    let vault = state
        .vault_id_to_vaults
        .get(&VAULT_ID)
        .ok_or("Vault 51 is missing")?;
    if vault.owner != claim.owner
        || vault.collateral_type != principal(NICP)
        // Raw nICP remains as the claim's historical source reference; only
        // CandidVault projects zero while the separate claim owns custody.
        || vault.collateral_amount != claim.original_nicp_e8s
        || vault.bot_processing
    {
        return Err(
            "Vault 51 changed owner, collateral or liquidation state; keep the claim fenced".into(),
        );
    }
    Ok(())
}

fn validate_icp_config(state: &State) -> Result<(), String> {
    let config = state
        .get_collateral_config(&principal(ICP))
        .ok_or("ICP collateral configuration missing")?;
    if config.status != CollateralStatus::Active
        || config.decimals != 8
        || config.ledger_canister_id != principal(ICP)
    {
        return Err("Ordinary ICP collateral configuration is not active native ICP".into());
    }
    // Adoption moves existing liabilities; it neither borrows nor mints. Future
    // operations use ordinary ICP caps, price, interest and liquidation rules.
    Ok(())
}

fn validate_journal(
    claim: &NicpRedemptionClaim,
    adoption: &CollateralAdoption,
) -> Result<(), String> {
    if claim.stage != ClaimStage::Settled
        || claim.settlement_block.is_none()
        || claim.release.is_some()
        || adoption.fee_e8s > MAX_SWEEP_FEE_E8S
        || adoption.amount_e8s == 0
        || adoption.amount_e8s.checked_add(adoption.fee_e8s) != claim.received_icp_e8s
    {
        return Err("Adoption tuple must consume exactly the verified settlement, including its bounded fee".into());
    }
    Ok(())
}

/// Applied from the same private event that records completion. Replay changes
/// custody attribution and its tombstone atomically; debt fields are untouched.
pub(crate) fn apply_completed(
    state: &mut State,
    claim: &NicpRedemptionClaim,
) -> Result<(), String> {
    let adoption = claim.adoption.as_ref().ok_or("Missing adoption journal")?;
    validate_journal(claim, adoption)?;
    if !adoption.completed || adoption.block.is_none() {
        return Err("Cannot adopt collateral before a verified sweep receipt".into());
    }
    let previous = state
        .nicp_redemption_claim
        .as_ref()
        .ok_or("Missing previous claim")?;
    let mut expected = previous.clone();
    expected
        .adoption
        .as_mut()
        .ok_or("Missing prepared adoption")?
        .completed = true;
    expected.updated_at_ns = claim.updated_at_ns;
    expected.last_error = claim.last_error.clone();
    if expected != *claim {
        return Err("Completion does not match the retained claim and sweep tuple".into());
    }
    if previous.adoption_completed() {
        return Ok(()); // Replayed completion must not overwrite later vault activity.
    }
    validate_original_vault(state, claim)?;
    let old_type = state.vault_id_to_vaults[&VAULT_ID].collateral_type;
    let vault = state.vault_id_to_vaults.get_mut(&VAULT_ID).unwrap();
    vault.collateral_type = principal(ICP);
    vault.collateral_amount = adoption.amount_e8s;
    state.unindex_vault_by_collateral(&old_type, VAULT_ID);
    state.index_vault_by_collateral(principal(ICP), VAULT_ID);
    // Existing claim-event application installs the terminal claim and then
    // reindexes CR / TCR, removing the old receivable valuation in that step.
    Ok(())
}

fn finish(mut claim: NicpRedemptionClaim) -> Result<NicpRedemptionClaim, String> {
    read_state(|state| {
        validate_original_vault(state, &claim)?;
        validate_icp_config(state)
    })?;
    let journal = claim.adoption.as_ref().ok_or("Missing adoption journal")?;
    validate_journal(&claim, journal)?;
    if journal.block.is_none() {
        return Err("Sweep receipt must be verified before collateral credit".into());
    }
    // Finish the old interest period before changing the collateral's rate.
    let cutover = ic_cdk::api::time();
    mutate_state(|state| crate::event::record_accrue_interest(state, cutover));
    if !read_state(|state| {
        state
            .vault_id_to_vaults
            .get(&VAULT_ID)
            .is_some_and(|vault| {
                vault.borrowed_icusd_amount.to_u64() == 0 || vault.last_accrual_time == cutover
            })
    }) {
        return Err(
            "Old collateral interest accrual was deferred; keep the adoption fenced".into(),
        );
    }
    claim.adoption.as_mut().unwrap().completed = true;
    claim.last_error = None;
    Ok(save(claim))
}

async fn verify_sweep_and_finish(
    mut claim: NicpRedemptionClaim,
    block_index: u64,
) -> Result<NicpRedemptionClaim, String> {
    if claim
        .settlement_block
        .is_none_or(|settlement| block_index <= settlement)
    {
        return Err("Sweep must follow the retained settlement block".into());
    }
    let journal = claim.adoption.as_ref().ok_or("Missing adoption journal")?;
    let block = crate::native_icp_proof::query_block(principal(ICP), block_index).await?;
    verify_sweep(&claim, journal, &block, ic_cdk::id())?;
    claim.adoption.as_mut().unwrap().block = Some(block_index);
    claim.last_error = None;
    claim = save(claim);
    finish(claim)
}

fn verify_sweep(
    claim: &NicpRedemptionClaim,
    adoption: &CollateralAdoption,
    block: &crate::native_icp_proof::Block,
    protocol: Principal,
) -> Result<(), String> {
    use crate::native_icp_proof::Operation;
    validate_journal(claim, adoption)?;
    let Some(Operation::Transfer {
        from,
        to,
        spender,
        amount,
        fee,
    }) = &block.transaction.operation
    else {
        return Err("Adoption receipt is not a transfer".into());
    };
    if from.as_slice() != account_identifier(protocol, claim.escrow_subaccount)
        || to.as_slice() != account_identifier(protocol, [0; 32])
        || spender.is_some()
        || amount.e8s != adoption.amount_e8s
        || fee.e8s != adoption.fee_e8s
        || block.transaction.icrc1_memo.as_deref() != Some(adoption.nonce.to_be_bytes().as_slice())
        || block.transaction.created_at_time.timestamp_nanos
            != nonce_to_created_at_time(adoption.nonce)
    {
        return Err(
            "Adoption receipt does not match the immutable escrow-to-ordinary ICP tuple".into(),
        );
    }
    Ok(())
}

/// Internal timer path; there is no caller-supplied vault, recipient or amount.
async fn process_automatic() -> Result<(), String> {
    let Some(claim) = current().filter(is_automatic_target) else {
        return Ok(());
    };
    if claim.adoption_completed() {
        return Ok(());
    }
    let _guard =
        VaultLiquidationGuard::new_for_nicp_claim(VAULT_ID).map_err(|e| format!("{e:?}"))?;
    let mut claim = current().ok_or("Claim disappeared")?;
    read_state(|state| {
        validate_original_vault(state, &claim)?;
        validate_icp_config(state)
    })?;
    match claim.stage {
        ClaimStage::WithdrawalSubmitted | ClaimStage::ClaimConfirmed => {
            claim = reconcile_claim(claim).await?;
        }
        ClaimStage::Settled => {}
        _ => return Err("Automatic adoption cannot initiate a new nICP withdrawal".into()),
    }
    if claim.stage != ClaimStage::Settled {
        return Ok(());
    }
    if claim.adoption.is_none() {
        // Recheck the retained settlement, not a quote or wallet-wide balance.
        let settlement = claim.settlement_block.ok_or("Missing settlement block")?;
        let block = crate::native_icp_proof::query_block(principal(ICP), settlement).await?;
        let received = verify_receipt(&claim, &block, ic_cdk::id())?;
        if Some(received) != claim.received_icp_e8s
            || balance(principal(ICP), claim.account(ic_cdk::id())).await? < received
        {
            return hold(
                claim,
                "Verified settlement amount and dedicated custody do not agree".into(),
            )
            .map(|_| ());
        }
        let fee = crate::management::get_ledger_fee(principal(ICP)).await?;
        if fee > MAX_SWEEP_FEE_E8S {
            return hold(
                claim,
                "ICP adoption fee exceeds the fixed 10,000 e8s budget".into(),
            )
            .map(|_| ());
        }
        let amount = received
            .checked_sub(fee)
            .filter(|n| *n > 0)
            .ok_or("Settlement cannot cover sweep fee")?;
        read_state(|state| validate_original_vault(state, &claim))?;
        let nonce = mutate_state(|state| state.next_op_nonce());
        claim.adoption = Some(CollateralAdoption {
            nonce,
            fee_e8s: fee,
            amount_e8s: amount,
            block: None,
            completed: false,
        });
        claim.last_error = None;
        claim = save(claim); // Exact tuple is durable before any spend.
    }
    let journal = claim.adoption.clone().ok_or("Missing sweep journal")?;
    validate_journal(&claim, &journal)?;
    if journal.completed {
        return Ok(());
    }
    if journal.block.is_some() {
        return finish(claim).map(|_| ());
    }
    match transfer_idempotent_pinned_fee(
        principal(ICP),
        Some(claim.escrow_subaccount),
        Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },
        u128::from(journal.amount_e8s),
        journal.nonce,
        journal.fee_e8s,
    )
    .await
    {
        Ok(block) => verify_sweep_and_finish(claim, block).await.map(|_| ()),
        Err(error) => hold(
            claim,
            format!("ICP adoption held; retain and reconcile the original sweep tuple: {error:?}"),
        )
        .map(|_| ()),
    }
}

/// Verifies a candidate after an expired/lost reply; cannot send tokens or
/// replace delivery parameters. Owner/operator authority remains explicit.
pub async fn reconcile(block_index: u64) -> Result<NicpRedemptionClaim, String> {
    let claim = current().ok_or("Missing vault 51 claim")?;
    let caller = ic_cdk::caller();
    if caller == Principal::anonymous()
        || (caller != claim.owner && !read_state(|state| state.developer_principal == caller))
    {
        return Err("Only the claim owner or protocol operator may reconcile adoption".into());
    }
    if !is_automatic_target(&claim) {
        return Err("Not the approved adoption claim".into());
    }
    let _guard =
        VaultLiquidationGuard::new_for_nicp_claim(VAULT_ID).map_err(|e| format!("{e:?}"))?;
    if let Some(existing) = claim.adoption.as_ref().and_then(|a| a.block) {
        if existing != block_index {
            return Err("Adoption already has a different receipt".into());
        }
        if claim.adoption_completed() {
            return Ok(claim);
        }
    }
    verify_sweep_and_finish(claim, block_index).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_icp_proof::{Block, Operation, Timestamp, Tokens, Transaction};

    fn retained_original_state() -> (State, NicpRedemptionClaim) {
        let mut state = State::default();
        state.icp_ledger_principal = principal(ICP);
        let mut claim = super::super::tests::approved_adoption_fixture();
        claim.stage = ClaimStage::ClaimConfirmed;
        claim.received_icp_e8s = None;
        claim.settlement_block = None;
        state.nicp_redemption_claim = Some(claim.clone());
        state.open_vault(crate::vault::Vault {
            owner: claim.owner, vault_id: VAULT_ID, collateral_type: principal(NICP),
            collateral_amount: claim.original_nicp_e8s,
            borrowed_icusd_amount: crate::numeric::ICUSD::new(287_535_228_114),
            accrued_interest: crate::numeric::ICUSD::new(99), last_accrual_time: 1234,
            bot_processing: false,
        });
        (state, claim)
    }

    #[test]
    fn nicp_retained_raw_collateral_passes_despite_zero_public_projection() {
        let (state, claim) = retained_original_state();
        let raw = state.vault_id_to_vaults[&VAULT_ID].clone();
        assert_eq!(raw.collateral_amount, 150_000_000_000);
        let public = crate::vault::CandidVault::from_state(raw.clone(), &state);
        assert_eq!(public.collateral_amount, 0);
        assert!(validate_original_vault(&state, &claim).is_ok());
        assert_eq!(state.vault_id_to_vaults[&VAULT_ID], raw);
        assert!(state.has_pending_nicp_claim(VAULT_ID));
    }

    #[test]
    fn nicp_retained_raw_collateral_still_refuses_changed_vault_identity_and_bot_state() {
        for change in ["owner", "type", "less", "more", "zero", "bot"] {
            let (mut state, claim) = retained_original_state();
            let vault = state.vault_id_to_vaults.get_mut(&VAULT_ID).unwrap();
            match change {
                "owner" => vault.owner = Principal::anonymous(),
                "type" => vault.collateral_type = principal(ICP),
                "less" => vault.collateral_amount -= 1,
                "more" => vault.collateral_amount += 1,
                "zero" => vault.collateral_amount = 0,
                "bot" => vault.bot_processing = true,
                _ => unreachable!(),
            }
            assert!(validate_original_vault(&state, &claim).is_err(), "{change}");
        }
    }

    fn journal() -> CollateralAdoption {
        CollateralAdoption {
            nonce: (100u128 << 64) | 41,
            fee_e8s: 10_000,
            amount_e8s: 194_282_921_553,
            block: None,
            completed: false,
        }
    }

    #[test]
    fn nicp_automatic_target_requires_the_approved_claim_identity() {
        let claim = super::super::tests::approved_adoption_fixture();
        assert!(is_automatic_target(&claim));
        let mut other = claim.clone();
        other.owner = Principal::anonymous();
        assert!(!is_automatic_target(&other));
        let mut other = claim.clone();
        other.withdrawal_id = Some(1697);
        assert!(!is_automatic_target(&other));
        let mut other = claim.clone();
        other.burn_block = Some(3_435_762);
        assert!(!is_automatic_target(&other));
        let mut other = claim.clone();
        other.escrow_subaccount[0] ^= 1;
        assert!(!is_automatic_target(&other));
        let mut other = claim;
        other.vault_id = 52;
        assert!(!is_automatic_target(&other));
    }

    #[test]
    fn nicp_sweep_budget_is_exact_and_cannot_coexist_with_owner_payout() {
        let mut claim = super::super::tests::approved_adoption_fixture();
        let mut tuple = journal();
        assert!(validate_journal(&claim, &tuple).is_ok());
        tuple.amount_e8s += 1;
        assert!(validate_journal(&claim, &tuple).is_err());
        tuple = journal();
        tuple.fee_e8s += 1;
        tuple.amount_e8s -= 1;
        assert!(validate_journal(&claim, &tuple).is_err());
        tuple = journal();
        claim.release = Some(ClaimRelease {
            nonce: 1,
            fee_e8s: 10_000,
            amount_e8s: 1,
            block: None,
        });
        assert!(validate_journal(&claim, &tuple).is_err());
        claim.release = None;
        claim.settlement_block = None;
        assert!(validate_journal(&claim, &tuple).is_err());
    }

    #[test]
    fn nicp_adoption_receipt_requires_the_full_escrow_to_ordinary_tuple() {
        let claim = super::super::tests::approved_adoption_fixture();
        let tuple = journal();
        let protocol = Principal::from_slice(&[2]);
        let block = Block {
            parent_hash: None,
            timestamp: Timestamp {
                timestamp_nanos: 101,
            },
            transaction: Transaction {
                memo: 0,
                icrc1_memo: Some(tuple.nonce.to_be_bytes().to_vec()),
                created_at_time: Timestamp {
                    timestamp_nanos: 100,
                },
                operation: Some(Operation::Transfer {
                    from: account_identifier(protocol, claim.escrow_subaccount).to_vec(),
                    to: account_identifier(protocol, [0; 32]).to_vec(),
                    spender: None,
                    amount: Tokens {
                        e8s: tuple.amount_e8s,
                    },
                    fee: Tokens { e8s: tuple.fee_e8s },
                }),
            },
        };
        assert!(verify_sweep(&claim, &tuple, &block, protocol).is_ok());
        for field in ["from", "to", "spender", "amount", "fee", "memo", "time"] {
            let mut wrong = block.clone();
            if let Some(Operation::Transfer {
                from,
                to,
                spender,
                amount,
                fee,
            }) = &mut wrong.transaction.operation
            {
                match field {
                    "from" => from[0] ^= 1,
                    "to" => *to = account_identifier(claim.owner, [0; 32]).to_vec(),
                    "spender" => *spender = Some(vec![1; 32]),
                    "amount" => amount.e8s += 1,
                    "fee" => fee.e8s += 1,
                    _ => {}
                }
            }
            if field == "memo" {
                wrong.transaction.icrc1_memo = None;
            }
            if field == "time" {
                wrong.transaction.created_at_time.timestamp_nanos += 1;
            }
            assert!(
                verify_sweep(&claim, &tuple, &wrong, protocol).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn nicp_unreceipted_adoption_stays_fenced_and_reserves_only_net_backing() {
        let mut claim = super::super::tests::approved_adoption_fixture();
        claim.adoption = Some(journal());
        assert!(claim.blocks_vault());
        assert_eq!(claim.fixed_icp_amount_e8s(), journal().amount_e8s);
        claim.adoption.as_mut().unwrap().completed = true;
        assert!(
            claim.blocks_vault(),
            "a marker without its receipt cannot un-fence collateral"
        );
    }

    #[test]
    fn nicp_adoption_survives_cbor_and_legacy_claims_default_to_none() {
        let mut claim = super::super::tests::approved_adoption_fixture();
        claim.adoption = Some(journal());
        let mut bytes = Vec::new();
        ciborium::into_writer(&claim, &mut bytes).unwrap();
        let roundtrip: NicpRedemptionClaim = ciborium::from_reader(bytes.as_slice()).unwrap();
        assert_eq!(roundtrip, claim);
        let mut value: ciborium::value::Value = ciborium::from_reader(bytes.as_slice()).unwrap();
        if let ciborium::value::Value::Map(ref mut map) = value {
            map.retain(
                |(key, _)| !matches!(key, ciborium::value::Value::Text(name) if name == "adoption"),
            );
        } else {
            panic!("claim is not a map")
        }
        bytes.clear();
        ciborium::into_writer(&value, &mut bytes).unwrap();
        let legacy: NicpRedemptionClaim = ciborium::from_reader(bytes.as_slice()).unwrap();
        assert!(legacy.adoption.is_none());
        assert!(legacy.blocks_vault());
        assert_eq!(legacy.withdrawal_id, Some(1696));
        assert_eq!(legacy.settlement_block, Some(100));
    }
}
