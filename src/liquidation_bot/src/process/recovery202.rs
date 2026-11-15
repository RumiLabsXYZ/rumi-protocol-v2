//! One historical recovery. No caller-selected vault, generation, receipt or amount.
use super::*;
use icrc_ledger_types::icrc1::account::Account;

const ID: u64 = 202;
const GENERATION: u64 = 3;
const RECORD: u64 = 231;
const GROSS: u64 = 5_152_859;
const NET: u64 = 5_142_859;
const DEBT: u64 = 10_090_702;
const INPUT: u64 = 4_918_638;
const OUTPUT: u64 = 95_482;
const BEFORE: u64 = 26_621_238;
const FEE: u64 = 10_000;
const POOL_MEMO: &[u8] = &[0, 0, 0, 0, 0, 0x12, 0x6b, 0x37];
const CLAIM_MEMO: &[u8] = &[
    0x18, 0xe8, 0x05, 0x51, 0x75, 0xd4, 0x01, 0x34, 0, 0, 0, 0, 0, 0, 0x5b, 0x20,
];

fn identity(
    config: &BotConfig,
    claim: &state::BotClaimJournal,
    record: &LiquidationRecordV1,
    bot: candid::Principal,
) -> Result<(), String> {
    if bot != vault158_principal("nygob-3qaaa-aaaap-qttcq-cai")
        || config.backend_principal != vault158_principal("tfesu-vyaaa-aaaap-qrd7a-cai")
        || config.icpswap_pool != vault158_principal("mohjv-bqaaa-aaaag-qjyia-cai")
        || config.icp_ledger != vault158_principal("ryjl3-tyaaa-aaaaa-aaaba-cai")
        || config.ckusdc_ledger != vault158_principal("xevnm-gaaaa-aaaar-qafnq-cai")
        || config.treasury_principal != vault158_principal("tlg74-oiaaa-aaaap-qrd6a-cai")
        || claim.vault_id != ID
        || claim.claim_generation != GENERATION
        || claim.status != state::BotClaimJournalStatus::PaymentShortfall
        || claim.collateral_amount_e8s != GROSS
        || claim.collateral_received_amount_e8s != Some(NET)
        || claim.collateral_outbound_fee_e8s != Some(FEE)
        || claim.debt_covered_e8s != DEBT
        || claim.collateral_return.is_some()
        || !claim.failed_return_attempts.is_empty()
        || claim.payment_memo != payment_memo(ID, GENERATION)
        || claim.collateral_return_memo != {
            let mut m = b"RUMI-BRET-V2:".to_vec();
            m.extend_from_slice(&ID.to_be_bytes());
            m.extend_from_slice(&GENERATION.to_be_bytes());
            m
        }
        || claim.swap_intents.len() != 1
        || record.id != RECORD
        || record.vault_id != ID
        || record.status != LiquidationStatus::TransferFailed
        || record.collateral_claimed_e8s != GROSS
        || record.debt_to_cover_e8s != DEBT
        || record.icp_swapped_e8s != INPUT
        || record.ckusdc_received_e6 != OUTPUT
        || record.ckusdc_transferred_e6 != 0
        || record.icp_to_treasury_e8s != 0
    {
        return Err("state differs from pinned vault202/record231/generation3".into());
    }
    let i = &claim.swap_intents[0];
    if i.pool_principal != config.icpswap_pool
        || i.input_ledger_principal != config.icp_ledger
        || i.output_ledger_principal != config.ckusdc_ledger
        || i.amount_in_e8s != INPUT
        || i.input_fee_e8s != FEE
        || i.output_fee_e6 != FEE
        || !i.zero_for_one
        || i.pre_swap_ckusdc_balance_e6 != BEFORE
        || i.attempt_ordinal != 1
        || i.amount_out_minimum_e6 != 103_373
        || i.created_at_time != 1_794_690_309_467_401_255
    {
        return Err("saved202 swap intent differs from verified receipt scope".into());
    }
    Ok(())
}

fn validate_output(
    block: &rumi_protocol_backend::icrc3_proof::DecodedBlock,
    config: &BotConfig,
    bot: candid::Principal,
) -> Result<(), String> {
    rumi_protocol_backend::icrc3_proof::validate_icrc3_transfer_block(
        block,
        Some(Account {
            owner: config.icpswap_pool,
            subaccount: None,
        }),
        Account {
            owner: bot,
            subaccount: None,
        },
        OUTPUT,
        Some(POOL_MEMO),
        None,
    )?;
    if block.fee != Some(FEE) || block.created_at_time.is_some() {
        return Err("output fee/timestamp differs from pinned202 block815252".into());
    }
    Ok(())
}

fn journal_matches(j: &state::BotPaymentJournal, config: &BotConfig) -> bool {
    j.record_id == Some(RECORD)
        && j.vault_id == ID
        && j.claim_generation == GENERATION
        && j.backend_principal == config.backend_principal
        && j.ledger_principal == config.ckusdc_ledger
        && j.collateral_amount_e8s == GROSS
        && j.collateral_received_amount_e8s == Some(NET)
        && j.debt_covered_e8s == DEBT
        && j.icp_swapped_e8s == INPUT
        && j.swap_input_fee_e8s == FEE
        && j.ckusdc_received_e6 == OUTPUT
        && j.gross_amount_e6 == 110_908
        && j.amount_e6 == 100_908
        && j.fee_e6 == FEE
        && j.memo == payment_memo(ID, GENERATION)
        && j.created_at_time != 0
        && j.held_surplus_e6 == 0
}

pub fn protected_claim_output(config: &BotConfig, s: &BotState) -> Result<u64, String> {
    if s.pending_claims.is_empty() {
        return Ok(0);
    }
    if s.pending_claims.len() != 1 {
        return Err("unrelated claims prevent reserve initialization".into());
    }
    let claim = s
        .pending_claims
        .get(&ID)
        .ok_or("unrelated claim prevents reserve initialization")?;
    let Some(LiquidationRecordVersioned::V1(record)) = history::get_record(RECORD) else {
        return Err("record231 missing".into());
    };
    identity(config, claim, &record, ic_cdk::id())?;
    Ok(OUTPUT)
}

pub async fn admin_execute_vault202_recovery(config: &BotConfig) -> Result<(), String> {
    if !state::read_state(|s| s.processing_paused) {
        return Err("keep worker paused through202 recovery".into());
    }
    if let Some(j) = state::read_state(|s| s.pending_payments.get(&ID).cloned()) {
        if !journal_matches(&j, config) {
            return Err("existing202 payment tuple does not match".into());
        }
        // Existing continuation preserves its original timestamp/memo and never reallocates.
        let _ = resume_pending_payment_for_vault(config, Some(ID)).await;
        return finish_treasury(config).await;
    }
    if state::read_state(|s| s.pending_treasury.contains_key(&RECORD)) {
        return finish_treasury(config).await;
    }
    if state::read_state(|s| {
        s.operator_reserve
            .as_ref()
            .is_some_and(|r| r.allocations.contains_key(&RECORD))
    }) {
        return match history::get_record(RECORD) {
            Some(LiquidationRecordVersioned::V1(r))
                if r.status == LiquidationStatus::Completed
                    && r.vault_id == ID
                    && r.ckusdc_transferred_e6 == 100_908
                    && r.icp_to_treasury_e8s == 214_221 =>
            {
                Ok(())
            }
            _ => Err(
                "202 allocation already consumed; reconcile existing journals rather than recreate"
                    .into(),
            ),
        };
    }
    let claim =
        state::read_state(|s| s.pending_claims.get(&ID).cloned()).ok_or("no held202 claim")?;
    let Some(LiquidationRecordVersioned::V1(record)) = history::get_record(RECORD) else {
        return Err("record231 missing".into());
    };
    identity(config, &claim, &record, ic_cdk::id())?;
    if !state::read_state(|s| {
        s.pending_claims.len() == 1
            && s.pending_payments.is_empty()
            && s.pending_treasury.is_empty()
    }) {
        return Err("unrelated recovery prevents exact202 continuation".into());
    }
    // Verify the backend collateral delivery and both swap legs, each by its exact ledger block.
    let delivery = swap::fetch_native_block(config.icp_ledger, 40_431_390).await?;
    crate::native_icp_blocks::verify_treasury_transfer(
        &delivery,
        config.backend_principal,
        ic_cdk::id(),
        NET,
        FEE,
        CLAIM_MEMO,
        1_794_690_298_934_460_724,
    )?;
    swap::verify_vault158_swap_input_receipt(
        config.icp_ledger,
        ic_cdk::id(),
        config.icpswap_pool,
        40_431_407,
        INPUT,
        FEE,
        POOL_MEMO,
        1_794_690_311_992_321_267,
    )
    .await?;
    let output =
        rumi_protocol_backend::icrc3_proof::fetch_icrc3_block(config.ckusdc_ledger, 815_252)
            .await?;
    validate_output(&output, config, ic_cdk::id())?;
    let fee = swap::fetch_ledger_fee(config.ckusdc_ledger).await?;
    let icp_fee = swap::fetch_ledger_fee(config.icp_ledger).await?;
    if fee != FEE
        || icp_fee != FEE
        || config.icp_fee_e8s != Some(FEE)
        || config.ckusdc_fee_e6 != Some(FEE)
    {
        return Err("live/cached fees changed; keep202 held for a revised exact decision".into());
    }
    let balance = swap::balance_of_self_ckusdc(config).await?;
    if balance != BEFORE + OUTPUT {
        return Err("wallet changed from exact202 baseline plus verified output".into());
    }
    let journal = state::BotPaymentJournal {
        record_id: Some(RECORD),
        vault_id: ID,
        backend_principal: config.backend_principal,
        ledger_principal: config.ckusdc_ledger,
        claim_generation: GENERATION,
        debt_covered_e8s: DEBT,
        collateral_amount_e8s: GROSS,
        collateral_received_amount_e8s: Some(NET),
        collateral_price_e8s: claim.collateral_price_e8s,
        icp_swapped_e8s: INPUT,
        swap_input_fee_e8s: FEE,
        ckusdc_received_e6: OUTPUT,
        held_surplus_e6: 0,
        gross_amount_e6: 110_908,
        amount_e6: 100_908,
        fee_e6: FEE,
        created_at_time: ic_cdk::api::time(),
        memo: payment_memo(ID, GENERATION),
        status: state::BotPaymentStatus::Prepared,
        receipt: None,
        shortfall_receipt_observed: false,
        shortfall_topup: None,
    };
    state::mutate_state(|s| {
        let current = s.pending_claims.get(&ID).ok_or("202 claim disappeared")?;
        identity(config, current, &record, ic_cdk::id())?;
        if !s.processing_paused
            || s.pending_claims.len() != 1
            || !s.pending_payments.is_empty()
            || !s.pending_treasury.is_empty()
        {
            return Err("recovery state changed while receipts were checked".into());
        }
        crate::reserve::commit_payment(
            s,
            journal,
            balance,
            crate::reserve::economic_subsidy_cap(DEBT, FEE, config.max_slippage_bps),
        )
    })?;
    state::save_config_to_stable();
    let _ = resume_pending_payment_for_vault(config, Some(ID)).await;
    finish_treasury(config).await
}

async fn finish_treasury(config: &BotConfig) -> Result<(), String> {
    if state::read_state(|s| s.pending_payments.contains_key(&ID)) {
        return Err(
            "202 payment/confirmation remains held; inspect/reconcile its original tuple".into(),
        );
    }
    if let Some(j) = state::read_state(|s| s.pending_treasury.get(&RECORD).cloned()) {
        if j.vault_id != ID
            || j.claim_generation != GENERATION
            || j.gross_amount_e8s != 214_221
            || j.treasury_principal != config.treasury_principal
            || j.ledger_principal != config.icp_ledger
            || j.sender_principal != ic_cdk::id()
            || j.fee_e8s != FEE
            || j.amount_e8s != 204_221
            || j.memo != swap::icp_treasury_memo(RECORD, GENERATION)
        {
            return Err("202 treasury tuple differs from approved residual".into());
        }
        let _ = resume_pending_treasury_for_record(config, Some(RECORD)).await;
    }
    match history::get_record(RECORD) {
        Some(LiquidationRecordVersioned::V1(r))
            if r.vault_id == ID
                && r.status == LiquidationStatus::Completed
                && r.ckusdc_transferred_e6 == 100_908
                && r.icp_to_treasury_e8s == 214_221 =>
        {
            Ok(())
        }
        _ => Err("202 treasury settlement remains held; inspect the saved tuple".into()),
    }
}

#[cfg(test)]
pub(crate) fn fixture202() -> (BotConfig, state::BotClaimJournal, LiquidationRecordV1) {
    fn bytes(s: &str) -> Vec<u8> {
        s.trim()
            .as_bytes()
            .chunks_exact(2)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
            .collect()
    }
    let mut claims: Vec<state::BotClaimJournal> =
        candid::decode_one(&bytes(include_str!("../../fixtures/claim202.hex"))).unwrap();
    let record: Option<LiquidationRecordVersioned> =
        candid::decode_one(&bytes(include_str!("../../fixtures/record231.hex"))).unwrap();
    let Some(LiquidationRecordVersioned::V1(record)) = record else {
        panic!("missing real record231 fixture")
    };
    let config = BotConfig {
        backend_principal: vault158_principal("tfesu-vyaaa-aaaap-qrd7a-cai"),
        treasury_principal: vault158_principal("tlg74-oiaaa-aaaap-qrd6a-cai"),
        admin: candid::Principal::from_slice(&[1]),
        max_slippage_bps: 250,
        icp_ledger: vault158_principal("ryjl3-tyaaa-aaaaa-aaaba-cai"),
        ckusdc_ledger: vault158_principal("xevnm-gaaaa-aaaar-qafnq-cai"),
        icpswap_pool: vault158_principal("mohjv-bqaaa-aaaag-qjyia-cai"),
        icpswap_zero_for_one: Some(true),
        icp_fee_e8s: Some(FEE),
        ckusdc_fee_e6: Some(FEE),
        three_pool_principal: None,
        kong_swap_principal: None,
        ckusdt_ledger: None,
        icusd_ledger: None,
    };
    (config, claims.remove(0), record)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_202_identity_rejects_changed_claim_generation_amount_and_destination() {
        let (config, claim, record) = fixture202();
        let bot = vault158_principal("nygob-3qaaa-aaaap-qttcq-cai");
        assert!(identity(&config, &claim, &record, bot).is_ok());
        let mut wrong = claim.clone();
        wrong.claim_generation += 1;
        assert!(identity(&config, &wrong, &record, bot).is_err());
        wrong = claim.clone();
        wrong.collateral_amount_e8s += 1;
        assert!(identity(&config, &wrong, &record, bot).is_err());
        wrong = claim.clone();
        wrong.swap_intents[0].attempt_ordinal += 1;
        assert!(identity(&config, &wrong, &record, bot).is_err());
        let mut wrong_config = config.clone();
        wrong_config.treasury_principal = candid::Principal::anonymous();
        assert!(identity(&wrong_config, &claim, &record, bot).is_err());
        let mut wrong_record = record.clone();
        wrong_record.ckusdc_transferred_e6 = 1;
        assert!(identity(&config, &claim, &wrong_record, bot).is_err());
    }

    #[test]
    fn real_output_receipt_requires_exact_fee_amount_sender_recipient_and_memo() {
        let bytes: Vec<u8> = include_str!("../../fixtures/output815252.hex")
            .trim()
            .as_bytes()
            .chunks_exact(2)
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
            .collect();
        let response: icrc_ledger_types::icrc3::blocks::GetBlocksResult =
            candid::decode_one(&bytes).unwrap();
        assert_eq!(response.blocks[0].id, candid::Nat::from(815_252u64));
        let block =
            rumi_protocol_backend::icrc3_proof::decode_block(&response.blocks[0].block).unwrap();
        let (config, _, _) = fixture202();
        let bot = vault158_principal("nygob-3qaaa-aaaap-qttcq-cai");
        assert!(validate_output(&block, &config, bot).is_ok());
        for mutate in [0, 1, 2, 3] {
            let mut b = block.clone();
            match mutate {
                0 => b.amount += 1,
                1 => b.fee = Some(1),
                2 => b.memo = Some(vec![0]),
                _ => b.from = None,
            }
            assert!(validate_output(&b, &config, bot).is_err());
        }
        assert!(validate_output(&block, &config, candid::Principal::anonymous()).is_err());
    }
}
