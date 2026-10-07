//! Test-only protocol canister used by `sp_burn_refund_caller_pic.rs`.
//! It returns a valid native-XRP preflight, rejects settlement after the SP
//! commits its real ledger burn, and issues the exact compensation mint from
//! the ledger's configured minting account.

use candid::{CandidType, Nat, Principal};
use ic_cdk::api::management_canister::http_request::{
    http_request, CanisterHttpRequestArgument, HttpMethod,
};
use ic_cdk::{init, query, update};
use serde::Deserialize;
use std::cell::RefCell;

#[derive(CandidType, Deserialize, Clone)]
struct Config {
    ledger: Principal,
    stability_pool: Principal,
    wrong_first_receipt: bool,
    fail_first_refund_before_mint: bool,
    fail_first_refund_after_mint: bool,
    trap_first_absorb_reply: bool,
    status_reports_accepted: bool,
}

#[derive(CandidType, Deserialize, Clone)]
struct StoredRefund {
    receipt: RefundReceipt,
    proof: SpWritedownProof,
}

thread_local! {
    static CONFIG: RefCell<Option<Config>> = const { RefCell::new(None) };
    static LAST_BURN_PROOF: RefCell<Option<SpWritedownProof>> = const { RefCell::new(None) };
    static LAST_REFUND: RefCell<Option<StoredRefund>> = const { RefCell::new(None) };
    static WRONG_RECEIPT_USED: RefCell<bool> = const { RefCell::new(false) };
    static FIRST_REFUND_FAILURE_USED: RefCell<bool> = const { RefCell::new(false) };
    static FIRST_REFUND_POST_MINT_ERROR_USED: RefCell<bool> = const { RefCell::new(false) };
    static FIRST_ABSORB_TRAP_USED: RefCell<bool> = const { RefCell::new(false) };
    static STATUS_CALL_COUNT: RefCell<u64> = const { RefCell::new(0) };
    static STATUS_GATE_HELD: RefCell<bool> = const { RefCell::new(false) };
    static TEST_GATE_CALL_COUNT: RefCell<u64> = const { RefCell::new(0) };
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum MockProtocolError {
    GenericError(String),
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct XrpSpAbsorbPreflight {
    vault_id: u64,
    icusd_burn_e8s: u64,
    collateral_received_drops: u64,
    collateral_price_e8s: u64,
    expires_at_ns: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct SpWritedownProof {
    block_index: u64,
    ledger_kind: SpProofLedger,
    vault_id_memo: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq, Eq)]
enum SpProofLedger {
    IcusdBurn,
    ThreePoolTransfer,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct XrpSpPayoutAllocation {
    claimant: Principal,
    payout_address: String,
    destination_tag: Option<u32>,
    drops: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct XrpSpAbsorbRequest {
    vault_id: u64,
    icusd_burned_e8s: u64,
    proof: SpWritedownProof,
    allocations: Vec<XrpSpPayoutAllocation>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct XrpSpAbsorbResult {
    success: bool,
    vault_id: u64,
    icusd_burned_e8s: u64,
    liquidated_debt_e8s: u64,
    collateral_received_drops: u64,
    payout_claims: Vec<XrpSpPayoutClaim>,
    block_index: u64,
    collateral_price_e8s: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct XrpSpPayoutClaim {
    claimant: Principal,
    claim_id: u64,
    payout_address: String,
    destination_tag: Option<u32>,
    drops: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum XrpSpAbsorbStatus {
    Accepted(XrpSpAbsorbResult),
    Unseen,
    ConsumedWithoutResult,
    RefundJournaled,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct RefundReceipt {
    vault_id: u64,
    amount_e8s: u64,
    ledger: Principal,
    recipient: Principal,
    burn_block_index: u64,
    refund_block_index: u64,
    refund_created_at_time: u64,
    refund_memo: Vec<u8>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct Account {
    owner: Principal,
    subaccount: Option<[u8; 32]>,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
struct TransferArg {
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    fee: Option<Nat>,
    created_at_time: Option<u64>,
    memo: Option<Vec<u8>>,
    amount: Nat,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
enum TransferError {
    BadFee { expected_fee: Nat },
    BadBurn { min_burn_amount: Nat },
    InsufficientFunds { balance: Nat },
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: Nat },
    TemporarilyUnavailable,
    GenericError { error_code: Nat, message: String },
}

#[init]
fn init(config: Config) {
    CONFIG.with(|slot| *slot.borrow_mut() = Some(config));
}

#[update]
fn stability_pool_preflight_xrp_absorb(
    vault_id: u64,
    expected_icusd_burn_e8s: u64,
) -> Result<XrpSpAbsorbPreflight, MockProtocolError> {
    let config = CONFIG
        .with(|slot| slot.borrow().clone())
        .expect("init config");
    if ic_cdk::api::caller() != config.stability_pool {
        return Err(MockProtocolError::GenericError("unexpected caller".into()));
    }
    Ok(XrpSpAbsorbPreflight {
        vault_id,
        icusd_burn_e8s: expected_icusd_burn_e8s,
        collateral_received_drops: 75_000,
        collateral_price_e8s: 50_000_000,
        expires_at_ns: ic_cdk::api::time() + 60_000_000_000,
    })
}

#[update]
fn stability_pool_liquidate_xrp_vault(
    request: XrpSpAbsorbRequest,
) -> Result<XrpSpAbsorbResult, MockProtocolError> {
    LAST_BURN_PROOF.with(|slot| *slot.borrow_mut() = Some(request.proof));
    let _ = request.allocations;
    let trap_first = CONFIG.with(|slot| {
        slot.borrow().as_ref().is_some_and(|config| config.trap_first_absorb_reply)
    });
    let already_trapped = FIRST_ABSORB_TRAP_USED.with(|used| *used.borrow());
    if trap_first && !already_trapped {
        FIRST_ABSORB_TRAP_USED.with(|used| *used.borrow_mut() = true);
        ic_cdk::trap("injected backend reply loss after the committed SP burn");
    }
    Err(MockProtocolError::GenericError(
        "XRP absorb reservation expired after the committed SP burn".into(),
    ))
}

#[update]
async fn stability_pool_xrp_absorb_status(
    request: XrpSpAbsorbRequest,
) -> Result<XrpSpAbsorbStatus, MockProtocolError> {
    let config = CONFIG
        .with(|slot| slot.borrow().clone())
        .expect("init config");
    if ic_cdk::api::caller() != config.stability_pool {
        return Err(MockProtocolError::GenericError("unexpected caller".into()));
    }
    STATUS_CALL_COUNT.with(|count| *count.borrow_mut() += 1);
    if STATUS_GATE_HELD.with(|held| *held.borrow()) {
        http_request(
            CanisterHttpRequestArgument {
                url: "https://status-gate.test/hold".into(),
                max_response_bytes: Some(1),
                method: HttpMethod::GET,
                headers: vec![],
                body: None,
                transform: None,
            },
            1_000_000_000,
        )
        .await
        .map_err(|(_, message)| MockProtocolError::GenericError(message))?;
        STATUS_GATE_HELD.with(|held| *held.borrow_mut() = false);
    }
    if !config.status_reports_accepted {
        return Ok(XrpSpAbsorbStatus::Unseen);
    }
    let payout_claims = request
        .allocations
        .iter()
        .enumerate()
        .map(|(index, allocation)| XrpSpPayoutClaim {
            claimant: allocation.claimant,
            claim_id: request.proof.block_index.saturating_mul(100).saturating_add(index as u64 + 1),
            payout_address: allocation.payout_address.clone(),
            destination_tag: allocation.destination_tag,
            drops: allocation.drops,
        })
        .collect();
    Ok(XrpSpAbsorbStatus::Accepted(XrpSpAbsorbResult {
        success: true,
        vault_id: request.vault_id,
        icusd_burned_e8s: request.icusd_burned_e8s,
        liquidated_debt_e8s: request.icusd_burned_e8s,
        collateral_received_drops: 75_000,
        payout_claims,
        block_index: request.proof.block_index,
        collateral_price_e8s: 50_000_000,
    }))
}

#[query]
fn get_mock_status_call_count() -> u64 {
    STATUS_CALL_COUNT.with(|count| *count.borrow())
}

#[update]
fn hold_mock_status_reply() {
    STATUS_GATE_HELD.with(|held| *held.borrow_mut() = true);
}

/// Test-only cross-canister barrier used by the real-backend refund fixture.
/// PocketIC intercepts this HTTPS request; the ledger awaits this canister,
/// which makes the ledger's already-committed transfer reply genuinely
/// pending across canister messages.
#[update]
async fn await_test_gate() {
    TEST_GATE_CALL_COUNT.with(|count| *count.borrow_mut() += 1);
    http_request(
        CanisterHttpRequestArgument {
            url: "https://refund-gate.test/hold".into(),
            max_response_bytes: Some(1),
            method: HttpMethod::GET,
            headers: vec![],
            body: None,
            transform: None,
        },
        1_000_000_000,
    )
    .await
    .expect("PocketIC must release the controlled refund gate");
}

#[query]
fn get_test_gate_call_count() -> u64 {
    TEST_GATE_CALL_COUNT.with(|count| *count.borrow())
}

#[update]
async fn refund_stability_pool_burn(
    vault_id: u64,
    amount_e8s: u64,
    proof: SpWritedownProof,
) -> Result<RefundReceipt, MockProtocolError> {
    let config = CONFIG
        .with(|slot| slot.borrow().clone())
        .expect("init config");
    if ic_cdk::api::caller() != config.stability_pool {
        return Err(MockProtocolError::GenericError("unexpected caller".into()));
    }
    if proof.vault_id_memo != vault_id || !matches!(proof.ledger_kind, SpProofLedger::IcusdBurn) {
        return Err(MockProtocolError::GenericError("wrong burn proof".into()));
    }

    if let Some(stored) = LAST_REFUND.with(|slot| slot.borrow().clone()) {
        if stored.receipt.vault_id != vault_id
            || stored.receipt.amount_e8s != amount_e8s
            || stored.proof.block_index != proof.block_index
            || stored.proof.ledger_kind != proof.ledger_kind
            || stored.proof.vault_id_memo != proof.vault_id_memo
        {
            return Err(MockProtocolError::GenericError(
                "refund tuple conflicts with the persisted receipt".into(),
            ));
        }
        return Ok(stored.receipt);
    }

    if config.fail_first_refund_before_mint
        && !FIRST_REFUND_FAILURE_USED.with(|used| *used.borrow())
    {
        FIRST_REFUND_FAILURE_USED.with(|used| *used.borrow_mut() = true);
        return Err(MockProtocolError::GenericError(
            "injected definite first refund failure before ledger dispatch".into(),
        ));
    }

    let mut memo = b"RSPRFND:".to_vec();
    memo.extend_from_slice(&proof.block_index.to_be_bytes());
    memo.extend_from_slice(&vault_id.to_be_bytes());
    let created_at_time = ic_cdk::api::time();
    let transfer: Result<(Result<Nat, TransferError>,), _> = ic_cdk::call(
        config.ledger,
        "icrc1_transfer",
        (TransferArg {
            from_subaccount: None,
            to: Account {
                owner: config.stability_pool,
                subaccount: None,
            },
            fee: None,
            created_at_time: Some(created_at_time),
            memo: Some(memo.clone()),
            amount: Nat::from(amount_e8s),
        },),
    )
    .await;
    let (transfer_result,) =
        transfer.map_err(|(_, message)| MockProtocolError::GenericError(message))?;
    let block_index = transfer_result.map_err(|error| {
        MockProtocolError::GenericError(format!("refund mint transfer failed: {error:?}"))
    })?;
    let refund_block_index: u64 = block_index
        .0
        .try_into()
        .map_err(|_| MockProtocolError::GenericError("block index exceeds u64".into()))?;

    let receipt = RefundReceipt {
        vault_id,
        amount_e8s,
        ledger: config.ledger,
        recipient: config.stability_pool,
        burn_block_index: proof.block_index,
        refund_block_index,
        refund_created_at_time: created_at_time,
        refund_memo: memo,
    };
    LAST_REFUND.with(|slot| {
        *slot.borrow_mut() = Some(StoredRefund {
            receipt: receipt.clone(),
            proof: proof.clone(),
        })
    });

    if config.fail_first_refund_after_mint
        && !FIRST_REFUND_POST_MINT_ERROR_USED.with(|used| *used.borrow())
    {
        FIRST_REFUND_POST_MINT_ERROR_USED.with(|used| *used.borrow_mut() = true);
        return Err(MockProtocolError::GenericError(
            "injected response error after refund mint was persisted".into(),
        ));
    }

    let return_wrong_receipt =
        config.wrong_first_receipt && !WRONG_RECEIPT_USED.with(|used| *used.borrow());
    if return_wrong_receipt {
        WRONG_RECEIPT_USED.with(|used| *used.borrow_mut() = true);
        let mut wrong = receipt;
        wrong.vault_id = wrong.vault_id.saturating_add(1);
        return Ok(wrong);
    }
    Ok(receipt)
}

#[query]
fn get_mock_burn_block_index() -> Option<u64> {
    LAST_BURN_PROOF.with(|slot| slot.borrow().as_ref().map(|proof| proof.block_index))
}

#[query]
fn get_mock_refund_block_index() -> Option<u64> {
    LAST_REFUND.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|stored| stored.receipt.refund_block_index)
    })
}

#[update]
fn reconcile_stability_pool_burn_refund(
    vault_id: u64,
    amount_e8s: u64,
    proof: SpWritedownProof,
    refund_block_index: u64,
) -> Result<RefundReceipt, MockProtocolError> {
    let config = CONFIG
        .with(|slot| slot.borrow().clone())
        .expect("init config");
    if ic_cdk::api::caller() != config.stability_pool {
        return Err(MockProtocolError::GenericError("unexpected caller".into()));
    }
    LAST_REFUND.with(|slot| {
        let stored = slot.borrow();
        let stored = stored.as_ref().ok_or_else(|| {
            MockProtocolError::GenericError("refund has not already landed".into())
        })?;
        if stored.receipt.vault_id != vault_id
            || stored.receipt.amount_e8s != amount_e8s
            || stored.proof.block_index != proof.block_index
            || stored.proof.ledger_kind != proof.ledger_kind
            || stored.proof.vault_id_memo != proof.vault_id_memo
            || stored.receipt.refund_block_index != refund_block_index
        {
            return Err(MockProtocolError::GenericError(
                "refund reconciliation tuple mismatch".into(),
            ));
        }
        Ok(stored.receipt.clone())
    })
}

#[update]
fn reconcile_stability_pool_burn_refund_from_history(
    vault_id: u64,
    amount_e8s: u64,
    proof: SpWritedownProof,
) -> Result<RefundReceipt, MockProtocolError> {
    let config = CONFIG
        .with(|slot| slot.borrow().clone())
        .expect("init config");
    if ic_cdk::api::caller() != config.stability_pool {
        return Err(MockProtocolError::GenericError("unexpected caller".into()));
    }
    LAST_REFUND.with(|slot| {
        let stored = slot.borrow();
        let stored = stored.as_ref().ok_or_else(|| {
            MockProtocolError::GenericError("refund has not already landed".into())
        })?;
        if stored.receipt.vault_id != vault_id
            || stored.receipt.amount_e8s != amount_e8s
            || stored.proof.block_index != proof.block_index
            || stored.proof.ledger_kind != proof.ledger_kind
            || stored.proof.vault_id_memo != proof.vault_id_memo
        {
            return Err(MockProtocolError::GenericError(
                "refund history reconciliation tuple mismatch".into(),
            ));
        }
        Ok(stored.receipt.clone())
    })
}

candid::export_service!();
