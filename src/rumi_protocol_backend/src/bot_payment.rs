//! ckUSDC payment proof checks for bot liquidations.
//!
//! A bot claim is not finalized until one real transfer on the configured
//! ckUSDC ledger pays at least one ckUSDC e6 unit for each 100 icUSD e8s,
//! rounding up (a 1:1 value conversion), from the registered bot
//! claim-generation-isolated account to this backend's default account. New
//! claims bind that transfer to a durable per-claim memo. Legacy claims remain
//! held because they lack an isolated sender authorization.

use crate::state::{mutate_state, BotClaim, BotPaymentReceipt};
use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

const BOT_PAYMENT_MEMO_PREFIX: &[u8] = b"RUMI-BOT-LIQ:";

/// Allocate and persist a globally unique memo nonce before collateral is
/// transferred out to the bot. A failed claim consumes its nonce permanently.
pub fn allocate_claim_payment_memo() -> Result<Vec<u8>, String> {
    let nonce = mutate_state(|state| {
        let next = state
            .bot_claim_payment_nonce
            .checked_add(1)
            .ok_or_else(|| "Bot claim payment memo nonce is exhausted".to_string())?;
        state.bot_claim_payment_nonce = next;
        Ok::<u64, String>(next)
    })?;

    let mut memo = Vec::with_capacity(BOT_PAYMENT_MEMO_PREFIX.len() + 8);
    memo.extend_from_slice(BOT_PAYMENT_MEMO_PREFIX);
    memo.extend_from_slice(&nonce.to_be_bytes());
    Ok(memo)
}

/// Convert icUSD e8s debt to the minimum ckUSDC e6 amount at 1:1 value,
/// rounding up without an addition that could overflow.
pub fn minimum_ckusdc_e6_for_debt(debt_e8s: u64) -> u64 {
    debt_e8s / 100 + u64::from(debt_e8s % 100 != 0)
}

pub fn registered_bot_matches(caller: Principal, registered_bot: Option<Principal>) -> bool {
    registered_bot == Some(caller)
}

/// Validate all payment fields decoded from a configured ckUSDC ICRC-3 block.
/// The decoded ledger amount is authoritative; callers cannot supply or alter
/// it. Returns the verified amount in ckUSDC e6s.
pub fn validate_payment_block(
    block: &crate::icrc3_proof::DecodedBlock,
    bot: Principal,
    backend: Principal,
    claim: &BotClaim,
) -> Result<u64, String> {
    if block.op != "transfer" && block.op != "xfer" {
        return Err(format!(
            "ckUSDC proof block operation is {}, expected a transfer",
            block.op
        ));
    }

    let expected_from = claim_payment_account(bot, claim)?;
    if block.from.as_ref() != Some(&expected_from) {
        return Err(
            "ckUSDC proof sender is not the exact claim-generation bot account".to_string(),
        );
    }

    let expected_to = Account {
        owner: backend,
        subaccount: None,
    };
    if block.to.as_ref() != Some(&expected_to) {
        return Err("ckUSDC proof recipient is not the backend default account".to_string());
    }

    match (
        claim.payment_memo.as_deref(),
        claim.claim_payment_subaccount.as_deref(),
    ) {
        (Some(expected_memo), Some(_)) => {
            if block.memo.as_deref() != Some(expected_memo) {
                return Err("ckUSDC proof memo does not match the active claim".to_string());
            }
        }
        _ => return Err(
            "legacy claim has no claim-specific payment authorization; payment recovery is held"
                .to_string(),
        ),
    }

    let minimum = minimum_ckusdc_e6_for_debt(claim.debt_amount);
    let amount_e6 = u64::try_from(block.amount)
        .map_err(|_| "ckUSDC proof amount does not fit in u64".to_string())?;
    if amount_e6 < minimum {
        return Err(format!(
            "ckUSDC payment {} is below the required minimum {}",
            amount_e6, minimum
        ));
    }
    if amount_e6 == 0 {
        return Err("ckUSDC payment amount must be greater than zero".to_string());
    }

    Ok(amount_e6)
}

/// Validate a memo-bound block's transfer identity without applying the
/// single-block minimum. Aggregate confirmation applies the minimum to the
/// checked sum only; legacy memo-less claims remain single-block only.
pub fn validate_memo_payment_block(
    block: &crate::icrc3_proof::DecodedBlock,
    bot: Principal,
    backend: Principal,
    claim: &BotClaim,
) -> Result<u64, String> {
    if claim.payment_memo.is_none() {
        return Err("aggregate payment recovery requires a memo-bound claim".to_string());
    }
    if block.op != "transfer" && block.op != "xfer" {
        return Err(format!(
            "ckUSDC proof block operation is {}, expected a transfer",
            block.op
        ));
    }
    if block.from.as_ref() != Some(&claim_payment_account(bot, claim)?) {
        return Err(
            "ckUSDC proof sender is not the exact claim-generation bot account".to_string(),
        );
    }
    if block.to.as_ref()
        != Some(&Account {
            owner: backend,
            subaccount: None,
        })
    {
        return Err("ckUSDC proof recipient is not the backend default account".to_string());
    }
    if block.memo.as_deref() != claim.payment_memo.as_deref() {
        return Err("ckUSDC proof memo does not match the active claim".to_string());
    }
    let amount = u64::try_from(block.amount)
        .map_err(|_| "ckUSDC proof amount does not fit in u64".to_string())?;
    if amount == 0 {
        return Err("ckUSDC payment amount must be greater than zero".to_string());
    }
    Ok(amount)
}

fn claim_payment_account(bot: Principal, claim: &BotClaim) -> Result<Account, String> {
    let expected = crate::bot_claim_payment_subaccount(claim.vault_id, claim.claimed_at);
    if claim.claim_payment_subaccount.as_deref() != Some(expected.as_slice()) {
        return Err(
            "claim has no valid generation-specific payment subaccount; payment recovery is held"
                .to_string(),
        );
    }
    Ok(Account {
        owner: bot,
        subaccount: Some(expected),
    })
}

pub fn sum_memo_payments(amounts: &[u64], debt_e8s: u64) -> Result<u64, String> {
    if amounts.is_empty() {
        return Err("at least one ckUSDC payment block is required".to_string());
    }
    let total = amounts
        .iter()
        .try_fold(0u64, |sum, amount| sum.checked_add(*amount))
        .ok_or_else(|| "aggregate ckUSDC payment amount overflow".to_string())?;
    let minimum = minimum_ckusdc_e6_for_debt(debt_e8s);
    if total < minimum {
        return Err(format!(
            "aggregate ckUSDC payment {} is below required minimum {}",
            total, minimum
        ));
    }
    Ok(total)
}

pub fn exact_block_set_matches(
    receipt: &BotPaymentReceipt,
    block_indexes: &[u64],
    keyed_block_index: u64,
) -> bool {
    receipt.payment_block_indexes == block_indexes
        || (receipt.payment_block_indexes.is_empty() && block_indexes == [keyed_block_index])
}

pub fn canonical_payment_block_indexes(
    mut indexes: Vec<u64>,
    maximum: usize,
) -> Result<Vec<u64>, String> {
    if indexes.is_empty() || indexes.len() > maximum {
        return Err(format!(
            "payment proof must contain between 1 and {} block indexes",
            maximum
        ));
    }
    indexes.sort_unstable();
    if indexes.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("payment proof contains duplicate block indexes".to_string());
    }
    Ok(indexes)
}

pub fn partial_set_can_extend(receipt: &BotPaymentReceipt, proposed: &[u64]) -> bool {
    !receipt.aggregate_complete
        && receipt
            .payment_block_indexes
            .iter()
            .all(|index| proposed.contains(index))
}

pub fn partial_receipt_matches_claim(
    receipt: &BotPaymentReceipt,
    caller: Principal,
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: Option<&[u8]>,
) -> bool {
    receipt.caller == caller
        && partial_receipt_matches_generation(receipt, vault_id, claim_timestamp, payment_memo)
}

pub fn partial_receipt_matches_generation(
    receipt: &BotPaymentReceipt,
    vault_id: u64,
    claim_timestamp: u64,
    payment_memo: Option<&[u8]>,
) -> bool {
    !receipt.aggregate_complete
        && receipt.vault_id == vault_id
        && receipt.claim_timestamp == claim_timestamp
        && receipt.payment_memo.as_deref() == payment_memo
}

/// Checks whether an already-consumed block receipt names this exact claim
/// generation. The memo remains part of the identity when the claim has one.
pub fn receipt_matches_claim(
    receipt: &BotPaymentReceipt,
    caller: Principal,
    vault_id: u64,
    claim_timestamp: u64,
    claim_memo: Option<&[u8]>,
) -> bool {
    receipt.caller == caller
        && receipt.vault_id == vault_id
        && receipt.claim_timestamp == claim_timestamp
        && receipt.payment_memo.as_deref() == claim_memo
}

/// Compare a live claim against the generation captured before an async proof
/// fetch. Full equality catches claim cancellation/recreation (including memo
/// changes) even if a timestamp were ever reused.
pub fn active_claim_matches(
    active: Option<&BotClaim>,
    expected: &BotClaim,
    claim_timestamp: u64,
) -> bool {
    expected.claimed_at == claim_timestamp && active == Some(expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icrc3_proof::DecodedBlock;

    #[test]
    fn legacy_bot_claim_decodes_without_recovery_identity() {
        #[derive(serde::Serialize)]
        struct LegacyBotClaim {
            vault_id: u64,
            collateral_amount: u64,
            debt_amount: u64,
            collateral_type: Principal,
            claimed_at: u64,
            collateral_price_e8s: u64,
            payment_memo: Option<Vec<u8>>,
        }
        let legacy = LegacyBotClaim {
            vault_id: 7,
            collateral_amount: 900,
            debt_amount: 12_345,
            collateral_type: Principal::from_slice(&[9]),
            claimed_at: 500,
            collateral_price_e8s: 200_000_000,
            payment_memo: Some(b"legacy-memo".to_vec()),
        };
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&legacy, &mut bytes).unwrap();
        let restored: BotClaim = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        assert_eq!(restored.request_id, None);
        assert_eq!(restored.claiming_bot, None);
        assert_eq!(restored.payment_memo, legacy.payment_memo);
    }

    #[test]
    fn old_payment_receipt_snapshot_remains_settled_single_block_receipt() {
        #[derive(serde::Serialize)]
        struct LegacyReceipt {
            caller: Principal,
            vault_id: u64,
            claim_timestamp: u64,
            payment_memo: Option<Vec<u8>>,
        }
        let legacy = LegacyReceipt {
            caller: Principal::from_slice(&[1]),
            vault_id: 7,
            claim_timestamp: 500,
            payment_memo: Some(b"memo".to_vec()),
        };
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&legacy, &mut bytes).unwrap();
        let restored: BotPaymentReceipt = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        assert!(restored.aggregate_complete);
        assert!(restored.payment_block_indexes.is_empty());
        assert!(exact_block_set_matches(&restored, &[42], 42));
        assert!(!exact_block_set_matches(&restored, &[42, 43], 42));
    }

    fn claim(payment_memo: Option<Vec<u8>>) -> BotClaim {
        BotClaim {
            vault_id: 7,
            collateral_amount: 900,
            debt_amount: 12_345,
            collateral_type: Principal::from_slice(&[9]),
            claimed_at: 500,
            collateral_price_e8s: 200_000_000,
            payment_memo: payment_memo.clone(),
            request_id: None,
            claiming_bot: None,
            claim_transfer: None,
            claim_payment_subaccount: payment_memo
                .as_ref()
                .map(|_| crate::bot_claim_payment_subaccount(7, 500).to_vec()),
        }
    }

    fn block(from: Principal, to: Principal, amount: u128, memo: Option<Vec<u8>>) -> DecodedBlock {
        DecodedBlock {
            btype: None,
            op: "xfer".to_string(),
            from: Some(Account {
                owner: from,
                subaccount: None,
            }),
            to: Some(Account {
                owner: to,
                subaccount: None,
            }),
            spender: None,
            amount,
            fee: None,
            transaction_fee: None,
            memo,
            created_at_time: None,
            expected_allowance: None,
            expires_at: None,
        }
    }

    #[test]
    fn registered_bot_auth_requires_exact_principal() {
        let bot = Principal::from_slice(&[1]);
        assert!(registered_bot_matches(bot, Some(bot)));
        assert!(!registered_bot_matches(
            Principal::from_slice(&[2]),
            Some(bot)
        ));
        assert!(!registered_bot_matches(bot, None));
    }

    #[test]
    fn validates_sender_recipient_memo_and_minimum_amount() {
        let bot = Principal::from_slice(&[1]);
        let backend = Principal::from_slice(&[2]);
        let memo = b"RUMI-BOT-LIQ:claim1".to_vec();
        let claim = claim(Some(memo.clone()));
        let mut valid = block(bot, backend, 124, Some(memo.clone()));
        valid.from = Some(Account {
            owner: bot,
            subaccount: Some(crate::bot_claim_payment_subaccount(7, 500)),
        });
        assert_eq!(
            validate_payment_block(&valid, bot, backend, &claim),
            Ok(124)
        );
        let mut overpayment = block(bot, backend, 200, Some(memo.clone()));
        overpayment.from = valid.from.clone();
        assert_eq!(
            validate_payment_block(&overpayment, bot, backend, &claim),
            Ok(200)
        );

        assert!(validate_payment_block(
            &block(
                Principal::from_slice(&[3]),
                backend,
                124,
                Some(memo.clone())
            ),
            bot,
            backend,
            &claim
        )
        .is_err());
        assert!(validate_payment_block(
            &block(bot, Principal::from_slice(&[3]), 124, Some(memo.clone())),
            bot,
            backend,
            &claim
        )
        .is_err());
        assert!(validate_payment_block(
            &block(bot, backend, 124, Some(b"wrong".to_vec())),
            bot,
            backend,
            &claim
        )
        .is_err());
        assert!(validate_payment_block(
            &block(bot, backend, 123, Some(memo)),
            bot,
            backend,
            &claim
        )
        .is_err());
    }

    #[test]
    fn legacy_claim_without_isolated_account_fails_closed() {
        let bot = Principal::from_slice(&[1]);
        let backend = Principal::from_slice(&[2]);
        let claim = claim(None);
        let mut payment = block(bot, backend, 124, None);
        payment.created_at_time = Some(500);
        assert!(validate_payment_block(&payment, bot, backend, &claim).is_err());
        payment.created_at_time = Some(499);
        assert!(validate_payment_block(&payment, bot, backend, &claim).is_err());
        payment.created_at_time = None;
        assert!(validate_payment_block(&payment, bot, backend, &claim).is_err());
        payment.created_at_time = Some(500);
        payment.memo = Some(b"unbound".to_vec());
        assert!(validate_payment_block(&payment, bot, backend, &claim).is_err());
    }

    #[test]
    fn payment_sender_is_bound_to_exact_claim_generation_subaccount() {
        let bot = Principal::from_slice(&[1]);
        let backend = Principal::from_slice(&[2]);
        let memo = b"RUMI-BOT-LIQ:claim1".to_vec();
        let claim = claim(Some(memo.clone()));
        let mut valid = block(bot, backend, 124, Some(memo.clone()));
        valid.from = Some(Account {
            owner: bot,
            subaccount: Some(crate::bot_claim_payment_subaccount(7, 500)),
        });
        assert!(validate_payment_block(&valid, bot, backend, &claim).is_ok());

        let default_account = block(bot, backend, 124, Some(memo.clone()));
        assert!(validate_payment_block(&default_account, bot, backend, &claim).is_err());

        let mut other_generation = valid;
        other_generation.from = Some(Account {
            owner: bot,
            subaccount: Some(crate::bot_claim_payment_subaccount(7, 501)),
        });
        assert!(validate_payment_block(&other_generation, bot, backend, &claim).is_err());
    }

    #[test]
    fn claim_replay_identity_includes_generation_and_memo() {
        let receipt = BotPaymentReceipt {
            caller: Principal::from_slice(&[1]),
            vault_id: 7,
            claim_timestamp: 500,
            payment_memo: Some(b"first".to_vec()),
            payment_block_indexes: vec![3],
            total_amount_e6: 124,
            aggregate_complete: true,
        };
        assert!(receipt_matches_claim(
            &receipt,
            Principal::from_slice(&[1]),
            7,
            500,
            Some(b"first")
        ));
        assert!(!receipt_matches_claim(
            &receipt,
            Principal::from_slice(&[2]),
            7,
            500,
            Some(b"first")
        ));
        assert!(!receipt_matches_claim(
            &receipt,
            Principal::from_slice(&[1]),
            7,
            501,
            Some(b"first")
        ));
        assert!(!receipt_matches_claim(
            &receipt,
            Principal::from_slice(&[1]),
            8,
            500,
            Some(b"first")
        ));
        assert!(!receipt_matches_claim(
            &receipt,
            Principal::from_slice(&[1]),
            7,
            500,
            Some(b"second")
        ));

        let original = claim(Some(b"first".to_vec()));
        let replacement = claim(Some(b"second".to_vec()));
        assert!(active_claim_matches(Some(&original), &original, 500));
        assert!(!active_claim_matches(Some(&replacement), &original, 500));
    }

    #[test]
    fn minimum_payment_rounds_up_without_overflow() {
        assert_eq!(minimum_ckusdc_e6_for_debt(0), 0);
        assert_eq!(minimum_ckusdc_e6_for_debt(100), 1);
        assert_eq!(minimum_ckusdc_e6_for_debt(101), 2);
        assert_eq!(minimum_ckusdc_e6_for_debt(u64::MAX), u64::MAX / 100 + 1);
    }

    #[test]
    fn aggregate_payment_requires_checked_sum_and_rounds_debt_minimum_up() {
        assert_eq!(sum_memo_payments(&[1, 1], 101), Ok(2));
        assert!(sum_memo_payments(&[1], 101).is_err());
        assert!(sum_memo_payments(&[u64::MAX, 1], 1).is_err());
        assert!(sum_memo_payments(&[], 1).is_err());
    }

    #[test]
    fn bounded_indexes_reject_duplicates_and_partial_receipts_require_monotonic_topups() {
        assert_eq!(
            canonical_payment_block_indexes(vec![9, 3], 16),
            Ok(vec![3, 9])
        );
        assert!(canonical_payment_block_indexes(vec![3, 3], 16).is_err());
        assert!(canonical_payment_block_indexes(vec![], 16).is_err());
        assert!(canonical_payment_block_indexes(vec![1, 2, 3], 2).is_err());
        let partial = BotPaymentReceipt {
            caller: Principal::from_slice(&[1]),
            vault_id: 7,
            claim_timestamp: 500,
            payment_memo: Some(b"m".to_vec()),
            payment_block_indexes: vec![3, 9],
            total_amount_e6: 1,
            aggregate_complete: false,
        };
        assert!(partial_set_can_extend(&partial, &[3, 9]));
        assert!(partial_set_can_extend(&partial, &[3, 9, 12]));
        assert!(!partial_set_can_extend(&partial, &[3, 12]));
        let mut complete = partial.clone();
        complete.aggregate_complete = true;
        assert!(!partial_set_can_extend(&complete, &[3, 9, 12]));
        assert!(partial_receipt_matches_claim(
            &partial,
            Principal::from_slice(&[1]),
            7,
            500,
            Some(b"m")
        ));
        // Cancellation fencing is generation-based, so ledger or bot
        // principal rotation cannot make this verified partial disappear.
        assert!(partial_receipt_matches_generation(
            &partial,
            7,
            500,
            Some(b"m")
        ));
        assert!(!partial_receipt_matches_claim(
            &partial,
            Principal::from_slice(&[1]),
            7,
            501,
            Some(b"m")
        ));
    }

    #[test]
    fn legacy_claim_and_state_snapshots_default_new_payment_fields() {
        let collateral = Principal::from_slice(&[9]);
        let old_claim = serde_json::json!({
            "vault_id": 7,
            "collateral_amount": 900,
            "debt_amount": 12_345,
            "collateral_type": serde_json::to_value(collateral).unwrap(),
            "claimed_at": 500,
            "collateral_price_e8s": 200_000_000
        });
        let decoded_claim: BotClaim = serde_json::from_value(old_claim).unwrap();
        assert_eq!(decoded_claim.payment_memo, None);

        let old_state: crate::state::State =
            serde_json::from_value(serde_json::json!({ "bot_claims": {} })).unwrap();
        assert_eq!(old_state.bot_claim_payment_nonce, 0);
        assert!(old_state.consumed_bot_payment_blocks.is_empty());
        assert!(old_state.bot_payment_aggregate_receipts.is_empty());
        let mut state = old_state;
        let aggregate = crate::state::BotPaymentAggregateReceipt {
            caller: Principal::from_slice(&[1]),
            vault_id: 7,
            claim_timestamp: 500,
            payment_memo: Some(b"memo".to_vec()),
            payment_block_indexes: vec![10, 11],
            total_amount_e6: 124,
        };
        state
            .bot_payment_aggregate_receipts
            .insert((aggregate.caller, 7, 500), aggregate.clone());
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&state, &mut bytes).unwrap();
        let restored: crate::state::State = ciborium::de::from_reader(bytes.as_slice()).unwrap();
        assert_eq!(
            restored.bot_payment_aggregate_receipts[&(aggregate.caller, 7, 500)],
            aggregate
        );
    }
}
