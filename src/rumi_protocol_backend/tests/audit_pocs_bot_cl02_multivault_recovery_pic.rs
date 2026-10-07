//! CL-02 multi-vault claim-return/reconciliation regression.
//!
//! Two claims share one collateral ledger. A return block for claim B must
//! not authorize cancellation of claim A. After expiry, reconciliation
//! must retain both claims and the reserved bot budget.

include!("common/bot_claim_fixture.rs");

#[test]
fn cl02_multivault_return_proof_is_generation_bound_and_expiry_is_reconciled() {
    let f = setup_fixture();
    let (budget_before, claim_a_timestamp, claim_a) = seed_bot_claim(&f);

    // A distinct timestamp ensures that the two return subaccounts are truly
    // claim-generation-specific even when both claims use the same bot.
    f.pic.advance_time(Duration::from_nanos(1));
    let claim_b = bot_claim_call(&f, f.bot_id, f.second_vault_id, 2)
        .expect("second bot claim should succeed");
    let claim_b_timestamp = claim_b
        .claim_timestamp
        .expect("second claim result must expose its persisted generation");
    assert_ne!(claim_a_timestamp, claim_b_timestamp);

    let budget_after_claims = get_bot_stats(&f.pic, f.protocol_id).budget_remaining_e8s;
    assert!(budget_after_claims < budget_before);
    let active = get_active_claim_ids(&f.pic, f.protocol_id);
    assert!(active.contains(&f.vault_id) && active.contains(&f.second_vault_id));

    // B returns its own exact memo-bound block into B's isolated return
    // subaccount. A receives no return. This positive block must not be
    // accepted as A's cancellation evidence.
    let (block_b, created_at_b, fee_b) = transfer_exact_claim_return(
        &f,
        f.second_vault_id,
        claim_b_timestamp,
        claim_b.collateral_amount,
    );
    let cross_cancel = cancel_with_generation(
        &f,
        f.vault_id,
        claim_a_timestamp,
        block_b,
        created_at_b,
        fee_b,
    );
    assert!(
        cross_cancel.is_err(),
        "claim B return block must not cancel claim A"
    );
    assert_eq!(
        get_active_claim_ids(&f.pic, f.protocol_id),
        active,
        "cross-generation return evidence must leave both claims open"
    );
    assert_eq!(
        get_bot_stats(&f.pic, f.protocol_id).budget_remaining_e8s,
        budget_after_claims,
        "rejected cross-claim evidence must not restore bot budget"
    );

    // Simulate unrelated collateral in the backend's pooled default account.
    // A pooled balance large enough for A still cannot stand in for an exact
    // return block from A's bot claim generation.
    icrc1_transfer_call(
        &f.pic,
        f.icp_ledger,
        f.test_user,
        f.protocol_id,
        claim_a.collateral_amount as u128,
    );
    let pooled_balance = icrc1_balance_of_call(&f.pic, f.icp_ledger, f.protocol_id);
    assert!(
        pooled_balance >= claim_a.collateral_amount.saturating_sub(10_000),
        "test requires unrelated pooled collateral sufficient for A"
    );

    assert!(get_reconciliation_events(&f.pic, f.protocol_id).is_empty());
    f.pic.advance_time(Duration::from_secs(700));
    for _ in 0..15 {
        f.pic.tick();
    }

    let active_after_expiry = get_active_claim_ids(&f.pic, f.protocol_id);
    assert!(active_after_expiry.contains(&f.vault_id));
    assert!(active_after_expiry.contains(&f.second_vault_id));
    assert_eq!(
        get_bot_stats(&f.pic, f.protocol_id).budget_remaining_e8s,
        budget_after_claims,
        "expired claims must remain reserved until their own recovery is proven"
    );

    let reconciliation = get_reconciliation_events(&f.pic, f.protocol_id);
    for expected_vault_id in [f.vault_id, f.second_vault_id] {
        assert!(
            reconciliation.iter().any(|event| matches!(event,
                Event::BotClaimReconciliationNeeded { vault_id, .. }
                    if *vault_id == expected_vault_id
            )),
            "expected a reconciliation event for vault #{expected_vault_id}; got {reconciliation:?}"
        );
    }

    // The same block is valid for B itself. A successful post-callback
    // recovery must clear only B and leave A's unpaid claim/budget held.
    cancel_with_generation(
        &f,
        f.second_vault_id,
        claim_b_timestamp,
        block_b,
        created_at_b,
        fee_b,
    )
    .expect("B's exact return receipt should cancel only B");
    let active_after_b = get_active_claim_ids(&f.pic, f.protocol_id);
    assert!(!active_after_b.contains(&f.second_vault_id));
    assert!(active_after_b.contains(&f.vault_id));
    assert!(
        get_bot_stats(&f.pic, f.protocol_id).budget_remaining_e8s > budget_after_claims,
        "valid B cancellation should restore B's budget while A remains reserved"
    );
}

/// The same generation-bound claim-return proof must remain verifiable after
/// the official native ICP ledger moves its block behind an archive callback.
#[test]
fn cl02_native_archived_return_receipt_cancels_exact_claim() {
    // A two-block threshold exercises the official ledger archive path with a
    // small number of ordinary transactions and avoids an artificial mock.
    let f = setup_fixture_with_archive_threshold(2);
    let (_, claim_timestamp, claim) = seed_bot_claim(&f);
    // The return argument includes one fee in `amount`, and the ledger debits
    // a second fee separately. Top up the bot by two fees before returning.
    icrc1_transfer_call(&f.pic, f.icp_ledger, f.test_user, f.bot_id, 20_000);
    let (return_block, created_at_time, fee) =
        transfer_exact_claim_return(&f, f.vault_id, claim_timestamp, claim.collateral_amount);

    let mut archived = native_block_is_archived(&f, return_block);
    for _ in 0..8 {
        if archived {
            break;
        }
        // Advance the official ledger beyond its tiny archive window. These
        // unrelated transfers cannot satisfy the claim's memo/subaccount proof.
        icrc1_transfer_call(&f.pic, f.icp_ledger, f.test_user, f.bot_id, 1_000_000);
        archived = native_block_is_archived(&f, return_block);
    }
    assert!(
        archived,
        "official query_blocks never exposed return block {return_block} through an archive descriptor"
    );

    cancel_with_generation(
        &f,
        f.vault_id,
        claim_timestamp,
        return_block,
        created_at_time,
        fee,
    )
    .expect(
        "exact archived return block should cancel its claim through the native archive callback",
    );
    assert!(
        !get_active_claim_ids(&f.pic, f.protocol_id).contains(&f.vault_id),
        "successful archived-receipt cancellation must clear only the matching claim"
    );
}
