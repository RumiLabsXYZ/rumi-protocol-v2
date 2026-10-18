//! Task 4 tests: the XRPL borrow lane against a FULL `State` (installed as the
//! thread-local state so the shared legacy admission, guards and event log are the
//! real ones). Ledger and XRPL outcomes are injected exactly as in `tests.rs`.

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use super::borrow::{self, BorrowAdvance, NewBorrow, Precheck};
use super::{get_operations, plan_tick, resume_operation, TickItem};
use super::ledger::*;
use super::settlement::*;
use super::state::*;
use super::tests::{backend, holder_addr, owner, prepared_payment, ready_state, sign_and_submit, TESTNET};
use super::types::*;
use crate::chains::xrp::issued_amount::ExactIssuedAmount;
use crate::chains::xrp::issuer_rpc::ExpiryEvidence;
use crate::numeric::{Ratio, UsdIcp, ICUSD};
use crate::state::{
    mutate_state, read_state, replace_state, xrp_collateral_principal, CollateralStatus,
    CustodyKind, Mode, State,
};
use crate::vault::{admit_borrow, BorrowReserve, Vault};
use crate::ProtocolError;

pub(super) const E8: u64 = 100_000_000;
pub(super) const NOW: u64 = 1_000_000_000_000;

pub(super) fn treasury() -> Principal {
    Principal::from_slice(&[0x77; 10])
}
pub(super) fn sp() -> Principal {
    Principal::from_slice(&[0x53; 10])
}
pub(super) fn xrp() -> Principal {
    xrp_collateral_principal()
}

pub(super) fn xrp_vault(id: u64, owner_p: Principal, drops: u64) -> Vault {
    Vault {
        owner: owner_p,
        vault_id: id,
        borrowed_icusd_amount: ICUSD::new(0),
        collateral_amount: drops,
        collateral_type: xrp(),
        last_accrual_time: NOW,
        accrued_interest: ICUSD::new(0),
        bot_processing: false,
    }
}

/// A full state: native-XRP collateral at $0.50 (zero interest, 0.5% borrow fee,
/// 1,000 icUSD ceiling), vault 1 (owner 1, 1,000 XRP), vault 3 (owner 3) and vault 4
/// (owner 4) of the same size, an ICP vault 2 (owner 2), a ready XRPL lane, a treasury
/// and a registered stability pool.
pub(super) fn world() -> State {
    let mut state = State::from(crate::InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::from_slice(&[0x10; 10]),
        icp_ledger_principal: Principal::from_slice(&[0x11; 10]),
        fee_e8s: 0,
        developer_principal: Principal::from_slice(&[0xdd; 10]),
        treasury_principal: Some(treasury()),
        stability_pool_principal: Some(sp()),
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    state.mode = Mode::GeneralAvailability;
    state.min_icusd_amount = ICUSD::new(E8 / 10);
    state.liquidation_protocol_share = Ratio::from(Decimal::ZERO);
    state.xrp_schnorr_key_name = if cfg!(feature = "xrpl-testnet-fixture") {
        "test_key_1".to_string()
    } else {
        "key_1".to_string()
    };
    let mut cfg = crate::state::xrp_collateral_config(
        Ratio::new(dec!(0.005)),
        Ratio::from(Decimal::ZERO),
        Ratio::new(dec!(1.0333)),
    );
    cfg.last_price = Some(0.5);
    cfg.status = CollateralStatus::Active;
    cfg.custody_kind = Some(CustodyKind::NativeXrp);
    cfg.debt_ceiling = 1_000 * E8;
    state.collateral_configs.insert(xrp(), cfg);
    let icp = state.icp_collateral_type();
    if let Some(c) = state.collateral_configs.get_mut(&icp) {
        c.last_price = Some(10.0);
        c.debt_ceiling = 10_000 * E8;
    }
    for (id, o) in [(1u64, owner(1)), (3, owner(3)), (4, owner(4))] {
        state.open_vault(xrp_vault(id, o, 1_000_000_000));
    }
    state.open_vault(Vault {
        owner: owner(2),
        vault_id: 2,
        borrowed_icusd_amount: ICUSD::new(0),
        collateral_amount: 100 * E8,
        collateral_type: icp,
        last_accrual_time: NOW,
        accrued_interest: ICUSD::new(0),
        bot_processing: false,
    });
    state.xrpl_icusd = ready_state();
    state
}

fn arg(vault: u64, gross: u64, byte: u8) -> BorrowToXrplArg {
    BorrowToXrplArg {
        vault_id: vault,
        gross_e8s: gross,
        recipient: holder_addr(9),
        network: TESTNET,
        request_nonce: vec![byte; 32],
    }
}

fn nonce_of(a: &BorrowToXrplArg) -> [u8; 32] {
    <[u8; 32]>::try_from(a.request_nonce.as_slice()).unwrap()
}

fn plan(caller: Principal, a: &BorrowToXrplArg) -> NewBorrow {
    match borrow::precheck(caller, a, &nonce_of(a), NOW).expect("precheck") {
        Precheck::New(p) => *p,
        Precheck::Existing(id) => panic!("unexpected existing op {id}"),
    }
}

fn commit(
    caller: Principal,
    a: &BorrowToXrplArg,
    planned: &NewBorrow,
) -> Result<(OperationId, bool), XrplIcusdError> {
    borrow::commit_reservation(caller, a, &nonce_of(a), planned, 100, backend(), NOW)
}

/// precheck + commit in one go (no state change between the two).
fn open_borrow(caller: Principal, a: &BorrowToXrplArg) -> OperationId {
    let p = plan(caller, a);
    commit(caller, a, &p).expect("commit").0
}

fn prove_mint(id: OperationId, block: u64) {
    mutate_state(|s| {
        s.xrpl_icusd.begin_job_call(id, 0).unwrap();
        s.xrpl_icusd
            .finish_job_call(id, 0, LedgerCallResult::Ok(block), NOW + 1)
            .unwrap();
        s.xrpl_icusd.mark_proof_verified(id, 0).unwrap();
    });
}

fn advance(id: OperationId) -> BorrowAdvance {
    mutate_state(|s| borrow::advance_borrow(s, id, NOW + 10)).unwrap()
}

fn borrow_terms(id: OperationId) -> (u64, u64, u64) {
    read_state(|s| match &s.xrpl_icusd.op(id).unwrap().kind {
        OperationKind::Borrow {
            gross_e8s,
            fee_e8s,
            net_e8s,
            ..
        } => (*gross_e8s, *fee_e8s, *net_e8s),
        k => panic!("{k:?}"),
    })
}

pub(super) fn set_xrp_cfg(f: impl FnOnce(&mut crate::state::CollateralConfig)) {
    mutate_state(|s| f(s.collateral_configs.get_mut(&xrp()).unwrap()));
}

pub(super) fn debt_of(vault: u64) -> u64 {
    read_state(|s| s.vault_id_to_vaults[&vault].borrowed_icusd_amount.to_u64())
}

fn borrow_events(vault: u64) -> Vec<(u64, u64, u64, Option<Principal>)> {
    crate::storage::events()
        .filter_map(|e| match e {
            crate::event::Event::BorrowFromVault {
                vault_id,
                block_index,
                fee_amount,
                borrowed_amount,
                caller,
                ..
            } if vault_id == vault => {
                Some((borrowed_amount.to_u64(), fee_amount.to_u64(), block_index, caller))
            }
            _ => None,
        })
        .collect()
}

fn deficit_events() -> usize {
    crate::storage::events()
        .filter(|e| matches!(e, crate::event::Event::DeficitRepaid { .. }))
        .count()
}

fn no_side_effects(ops: usize) {
    read_state(|s| {
        assert_eq!(s.xrpl_icusd.operations.len(), ops, "no operation created");
        assert!(s.xrpl_icusd.vault_exclusions.is_empty(), "no exclusion");
        assert_eq!(s.xrpl_icusd.durable_reserved_total(), 0, "no reservation");
    });
}

// ---- core accounting ----------------------------------------------------------------

#[test]
fn borrow_fee_net_backing_matches_payout() {
    let mut w = world();
    w.protocol_deficit_icusd = ICUSD::new(10 * E8);
    w.deficit_repayment_fraction = Ratio::new(dec!(0.5));
    replace_state(w);
    let a = arg(1, 100 * E8, 1);
    let id = open_borrow(owner(1), &a);
    let (gross, fee, net) = borrow_terms(id);
    assert_eq!(gross, 100 * E8);
    assert!(fee > 0 && gross == fee + net, "net = gross - fee");
    // The escrow mint is exactly the net, from the minting account into escrow.
    read_state(|s| {
        let op = s.xrpl_icusd.op(id).unwrap();
        assert_eq!(op.phase, Phase::EscrowMintPending);
        assert_eq!(op.owner, owner(1));
        let j = &op.ic_jobs[0];
        assert_eq!(j.purpose, IcJobPurpose::EscrowMint);
        assert_eq!((j.amount_e8s, j.from_subaccount, j.to), (net, None, escrow_account(backend())));
        assert!(s.xrpl_icusd.vault_exclusions.contains(&1));
        assert_eq!(s.xrpl_icusd.durable_reserved_total(), gross);
        assert_eq!(debt_of(1), 0, "no debt before the proven mint");
    });
    prove_mint(id, 55);
    assert_eq!(advance(id), BorrowAdvance::Committed);
    read_state(|s| {
        let st = &s.xrpl_icusd;
        let op = st.op(id).unwrap();
        assert_eq!(op.phase, Phase::IssuePrepared);
        assert_eq!(debt_of(1), gross, "debt is the GROSS amount");
        // The fee split is recorded once and conserves the fee.
        let OperationKind::Borrow { fee_split, .. } = &op.kind else { panic!() };
        let split = fee_split.clone().expect("split stored");
        assert_eq!(split.to_deficit_e8s + split.to_treasury_remainder_e8s, fee);
        assert_eq!(split.to_deficit_e8s, fee / 2);
        assert_eq!(split.treasury, Some(treasury()));
        assert_eq!(s.protocol_deficit_icusd.to_u64(), 10 * E8 - split.to_deficit_e8s);
        // A persisted fee-remainder mint to the treasury (never to escrow).
        assert_eq!(op.ic_jobs.len(), 2);
        let f = &op.ic_jobs[1];
        assert_eq!(f.purpose, IcJobPurpose::FeeMint);
        assert_eq!(f.amount_e8s, split.to_treasury_remainder_e8s);
        assert_eq!(f.to, Account { owner: treasury(), subaccount: None });
        assert_ne!(f.op_nonce, op.ic_jobs[0].op_nonce, "its own persisted nonce");
        // Backing == payout: D is exactly the net, and the XRPL Payment pays the net.
        let b = st.buckets().unwrap();
        assert_eq!(b.d, WideAmount::from_e8s(net));
        let pay = prepared_payment(st, id, 7);
        let AttemptTx::Payment { amount, destination, .. } = &pay.tx else { panic!() };
        assert_eq!(*amount, ExactIssuedAmount::try_from_e8s(net).unwrap());
        assert_eq!(destination, &holder_addr(9));
        // Released: durable reservation and exclusion are gone in the same transition.
        assert_eq!(st.durable_reserved_total(), 0);
        assert!(st.vault_exclusions.is_empty());
    });
}

#[test]
fn escrow_mint_commits_debt_once() {
    let mut w = world();
    w.protocol_deficit_icusd = ICUSD::new(10 * E8);
    w.deficit_repayment_fraction = Ratio::new(dec!(0.5));
    replace_state(w);
    let id = open_borrow(owner(1), &arg(1, 100 * E8, 1));
    let (gross, fee, _net) = borrow_terms(id);
    // Unknown mint: everything is retained, no debt, exclusion stays.
    mutate_state(|s| {
        s.xrpl_icusd.begin_job_call(id, 0).unwrap();
        s.xrpl_icusd
            .finish_job_call(id, 0, classify_transfer(Err((4, "no reply".into()))), NOW + 1)
            .unwrap();
    });
    assert_eq!(advance(id), BorrowAdvance::Waiting);
    read_state(|s| {
        assert!(s.xrpl_icusd.vault_exclusions.contains(&1));
        assert_eq!(s.xrpl_icusd.durable_reserved_total(), gross);
    });
    assert_eq!(debt_of(1), 0);
    assert!(borrow_events(1).is_empty());
    // The retry re-sends the same tuple and then proves it.
    let first_nonce = read_state(|s| s.xrpl_icusd.op(id).unwrap().ic_jobs[0].op_nonce);
    mutate_state(|s| {
        let j = s.xrpl_icusd.begin_job_call(id, 0).unwrap();
        assert_eq!(j.op_nonce, first_nonce);
        s.xrpl_icusd
            .finish_job_call(id, 0, LedgerCallResult::Duplicate(70), NOW + 2)
            .unwrap();
        s.xrpl_icusd.mark_proof_verified(id, 0).unwrap();
    });
    assert_eq!(advance(id), BorrowAdvance::Committed);
    let deficit_after_first = read_state(|s| s.protocol_deficit_icusd);
    // A second and third drive never repeat any effect.
    assert_eq!(advance(id), BorrowAdvance::Waiting);
    assert_eq!(advance(id), BorrowAdvance::Waiting);
    assert_eq!(debt_of(1), gross);
    let ev = borrow_events(1);
    assert_eq!(ev.len(), 1, "exactly one BorrowFromVault event");
    // Caller attribution is the SAVED owner (there is no timer/resume caller here).
    assert_eq!(ev[0], (gross, fee, 70, Some(owner(1))));
    assert_eq!(deficit_events(), 1, "deficit allocation planned once");
    read_state(|s| {
        assert_eq!(s.protocol_deficit_icusd, deficit_after_first);
        assert_eq!(s.xrpl_icusd.op(id).unwrap().ic_jobs.len(), 2, "no extra jobs");
    });
    // A lost fee-mint reply reuses its persisted tuple and never re-plans the split.
    let (n1, split_before) = read_state(|s| {
        let op = s.xrpl_icusd.op(id).unwrap();
        let OperationKind::Borrow { fee_split, .. } = &op.kind else { panic!() };
        (op.ic_jobs[1].op_nonce, fee_split.clone())
    });
    mutate_state(|s| {
        s.xrpl_icusd.begin_job_call(id, 1).unwrap();
        s.xrpl_icusd
            .finish_job_call(id, 1, classify_transfer(Err((4, "no reply".into()))), NOW + 3)
            .unwrap();
        let again = s.xrpl_icusd.begin_job_call(id, 1).unwrap();
        assert_eq!(again.op_nonce, n1);
    });
    assert_eq!(advance(id), BorrowAdvance::Waiting);
    assert_eq!(deficit_events(), 1);
    read_state(|s| {
        let OperationKind::Borrow { fee_split, .. } = &s.xrpl_icusd.op(id).unwrap().kind else {
            panic!()
        };
        assert_eq!(fee_split, &split_before);
    });
}

#[test]
fn definite_mint_failure_releases_without_debt() {
    replace_state(world());
    let id = open_borrow(owner(1), &arg(1, 100 * E8, 1));
    mutate_state(|s| {
        s.xrpl_icusd.begin_job_call(id, 0).unwrap();
        s.xrpl_icusd
            .finish_job_call(
                id,
                0,
                classify_transfer(Ok(Err(
                    icrc_ledger_types::icrc1::transfer::TransferError::GenericError {
                        error_code: candid::Nat::from(1u8),
                        message: "rejected".into(),
                    },
                ))),
                NOW + 1,
            )
            .unwrap();
    });
    assert_eq!(advance(id), BorrowAdvance::Cancelled);
    read_state(|s| {
        assert_eq!(s.xrpl_icusd.op(id).unwrap().phase, Phase::Cancelled);
        assert!(s.xrpl_icusd.vault_exclusions.is_empty());
        assert_eq!(s.xrpl_icusd.durable_reserved_total(), 0);
        assert_eq!(s.xrpl_icusd.buckets().unwrap().d, WideAmount::ZERO, "nothing owed");
    });
    assert_eq!(debt_of(1), 0);
    assert!(borrow_events(1).is_empty());
    // The reservation event pair replays to the same released state.
    let again = advance(id);
    assert_eq!(again, BorrowAdvance::Waiting);
}

#[test]
fn policy_change_before_mint_refuses() {
    for variant in 0..5 {
        replace_state(world());
        let a = arg(1, 100 * E8, 1);
        let planned = plan(owner(1), &a);
        let next_id = read_state(|s| s.xrpl_icusd.next_operation_id);
        match variant {
            // issuance paused by the operator
            0 => mutate_state(|s| s.xrpl_icusd.set_pauses(Some(true), None)),
            // any policy/limit reconfiguration bumps the config version
            1 => mutate_state(|s| s.xrpl_icusd.config.config_version += 1),
            // a price move changes the quote's fingerprint
            2 => set_xrp_cfg(|c| c.last_price = Some(0.45)),
            // the borrowing fee changes
            3 => set_xrp_cfg(|c| c.borrowing_fee = Ratio::new(dec!(0.02))),
            // the protocol freezes
            _ => mutate_state(|s| s.frozen = true),
        }
        let r = commit(owner(1), &a, &planned);
        assert!(r.is_err(), "variant {variant}: {r:?}");
        no_side_effects(0);
        read_state(|s| assert_eq!(s.xrpl_icusd.next_operation_id, next_id));
        assert_eq!(debt_of(1), 0);
    }
}

#[test]
fn policy_or_price_change_during_mint_commits_frozen_terms() {
    let mut w = world();
    w.protocol_deficit_icusd = ICUSD::new(10 * E8);
    w.deficit_repayment_fraction = Ratio::new(dec!(0.5));
    replace_state(w);
    let id = open_borrow(owner(1), &arg(1, 100 * E8, 1));
    let (gross, fee, net) = borrow_terms(id);
    // Everything an operator or the market can change while the mint is unresolved:
    // a price crash that would fail the CR check, a 10x fee, recovery mode, issuance
    // and settlement pauses, a freeze, the sunset of the collateral.
    set_xrp_cfg(|c| {
        c.last_price = Some(0.0001);
        c.borrowing_fee = Ratio::new(dec!(0.5));
        c.status = CollateralStatus::Sunset;
    });
    mutate_state(|s| {
        s.mode = Mode::Recovery;
        s.xrpl_icusd.set_pauses(Some(true), Some(true));
        s.frozen = true;
    });
    prove_mint(id, 91);
    assert_eq!(advance(id), BorrowAdvance::Committed);
    // The debt, fee and net are EXACTLY the frozen ones.
    assert_eq!(borrow_terms(id), (gross, fee, net));
    assert_eq!(debt_of(1), gross);
    assert_eq!(borrow_events(1), vec![(gross, fee, 91, Some(owner(1)))]);
    read_state(|s| {
        let OperationKind::Borrow { fee_split, .. } = &s.xrpl_icusd.op(id).unwrap().kind else {
            panic!()
        };
        let sp = fee_split.clone().unwrap();
        assert_eq!(sp.to_deficit_e8s + sp.to_treasury_remainder_e8s, fee, "split of the frozen fee");
        assert_eq!(s.xrpl_icusd.op(id).unwrap().phase, Phase::IssuePrepared);
        // The pause stops NEW attempts only; the owned backed claim persists.
        assert_eq!(s.xrpl_icusd.buckets().unwrap().d, WideAmount::from_e8s(net));
    });
}

// ---- admission, ownership, refusals -------------------------------------------------

#[test]
fn wrong_owner_refused() {
    replace_state(world());
    let a = arg(1, 100 * E8, 1);
    // Another principal cannot borrow against vault 1, with any nonce.
    assert_eq!(
        borrow::precheck(owner(2), &a, &nonce_of(&a), NOW).unwrap_err(),
        XrplIcusdError::Unauthorized
    );
    assert_eq!(
        borrow::precheck(Principal::anonymous(), &a, &nonce_of(&a), NOW).unwrap_err(),
        XrplIcusdError::AnonymousCaller
    );
    // The commit step re-checks ownership too (a stale plan cannot be replayed by a
    // different principal).
    let planned = plan(owner(1), &a);
    assert!(commit(owner(2), &a, &planned).is_err());
    no_side_effects(0);
    // The v1 restriction: the caller's native-XRP vault only.
    let icp_arg = arg(2, 10 * E8, 2);
    assert!(matches!(
        borrow::precheck(owner(2), &icp_arg, &nonce_of(&icp_arg), NOW),
        Err(XrplIcusdError::NotApplicable(_))
    ));
    assert_eq!(
        borrow::precheck(owner(1), &arg(99, 10 * E8, 3), &[3; 32], NOW).unwrap_err(),
        XrplIcusdError::NotFound
    );
    // Operations are invisible to other principals: resume/view say NotFound.
    let id = open_borrow(owner(1), &a);
    assert_eq!(
        futures::executor::block_on(resume_operation(owner(2), id)).unwrap_err(),
        XrplIcusdError::NotFound
    );
    assert!(get_operations(owner(2), None, 50).items.is_empty());
    assert_eq!(get_operations(owner(1), None, 50).items.len(), 1);
    // The same nonce from another principal is a NEW request (and refused as above);
    // from the owner it returns the SAME operation and creates nothing.
    assert!(matches!(
        borrow::precheck(owner(1), &a, &nonce_of(&a), NOW).unwrap(),
        Precheck::Existing(i) if i == id
    ));
    assert_eq!(read_state(|s| s.xrpl_icusd.operations.len()), 1);
    // A different body under the same nonce conflicts.
    let mut other = arg(1, 50 * E8, 1);
    other.request_nonce = a.request_nonce.clone();
    assert_eq!(
        borrow::precheck(owner(1), &other, &nonce_of(&other), NOW).unwrap_err(),
        XrplIcusdError::NonceConflict
    );
}

#[test]
fn frozen_or_cap_or_cr_refused() {
    type Mutator = Box<dyn Fn()>;
    let cases: Vec<(&str, u64, Mutator)> = vec![
        ("frozen", 100 * E8, Box::new(|| mutate_state(|s| s.frozen = true))),
        ("read only", 100 * E8, Box::new(|| mutate_state(|s| s.mode = Mode::ReadOnly))),
        (
            "per-operation cap",
            100 * E8,
            Box::new(|| mutate_state(|s| s.xrpl_icusd.config.per_operation_cap_e8s = 10 * E8)),
        ),
        (
            "outstanding cap",
            100 * E8,
            Box::new(|| mutate_state(|s| s.xrpl_icusd.config.outstanding_cap_e8s = 50 * E8)),
        ),
        ("collateral ceiling", 100 * E8, Box::new(|| set_xrp_cfg(|c| c.debt_ceiling = 50 * E8))),
        ("global mint cap", 100 * E8, Box::new(|| mutate_state(|s| s.global_icusd_mint_cap = 50 * E8))),
        // 1,000 XRP at $0.50 = $500; 150% => at most ~333 icUSD.
        ("collateral ratio", 400 * E8, Box::new(|| {})),
        ("below minimum", E8 / 100, Box::new(|| {})),
        ("collateral status", 100 * E8, Box::new(|| set_xrp_cfg(|c| c.status = CollateralStatus::Sunset))),
        ("issuance paused", 100 * E8, Box::new(|| mutate_state(|s| s.xrpl_icusd.set_pauses(Some(true), None)))),
        ("lane disabled", 100 * E8, Box::new(|| mutate_state(|s| s.xrpl_icusd.config.enabled = false))),
    ];
    for (name, gross, mutator) in cases {
        replace_state(world());
        mutator();
        let a = arg(1, gross, 1);
        let r = borrow::precheck(owner(1), &a, &nonce_of(&a), NOW);
        assert!(r.is_err(), "{name}: {r:?}");
        no_side_effects(0);
        assert_eq!(debt_of(1), 0, "{name}");
    }
    // Typed errors for the headline cases.
    replace_state(world());
    mutate_state(|s| s.frozen = true);
    let a = arg(1, 100 * E8, 1);
    assert!(matches!(
        borrow::precheck(owner(1), &a, &nonce_of(&a), NOW),
        Err(XrplIcusdError::Paused(_))
    ));
    replace_state(world());
    mutate_state(|s| s.xrpl_icusd.config.per_operation_cap_e8s = 10 * E8);
    assert!(matches!(
        borrow::precheck(owner(1), &a, &nonce_of(&a), NOW),
        Err(XrplIcusdError::LimitExceeded(_))
    ));
    replace_state(world());
    let big = arg(1, 400 * E8, 2);
    assert!(matches!(
        borrow::precheck(owner(1), &big, &nonce_of(&big), NOW),
        Err(XrplIcusdError::Policy(m)) if m.contains("max borrowable")
    ));
}

#[test]
fn trustline_or_network_mismatch_refused() {
    replace_state(world());
    // Wrong network.
    let mut a = arg(1, 100 * E8, 1);
    a.network = crate::chains::xrp::network::XrplNetwork::Mainnet;
    assert!(matches!(
        borrow::precheck(owner(1), &a, &nonce_of(&a), NOW),
        Err(XrplIcusdError::WrongNetwork(_))
    ));
    // The recipient: classic address only, and never the issuer.
    for bad in [
        "not an address".to_string(),
        read_state(|s| s.xrpl_icusd.config.issuer_address.clone().unwrap()),
        String::new(),
    ] {
        let mut b = arg(1, 100 * E8, 2);
        b.recipient = bad;
        assert!(matches!(
            borrow::precheck(owner(1), &b, &nonce_of(&b), NOW),
            Err(XrplIcusdError::InvalidArgument(_))
        ));
    }
    no_side_effects(0);
    // The trust-line gate (the async step feeds it the proof read from the XRPL).
    use crate::chains::xrp::issued_rpc::TrustLine;
    assert!(matches!(
        check_trust_line(None, 99 * E8),
        Err(XrplIcusdError::TrustLineMissing(_))
    ));
    let line = |limit: &str, bal: &str, frozen: bool| TrustLine {
        holder: holder_addr(9),
        issuer: super::tests::issuer_addr(),
        balance: ExactIssuedAmount::parse(bal).unwrap(),
        limit: ExactIssuedAmount::parse(limit).unwrap(),
        limit_peer: ExactIssuedAmount::ZERO,
        no_ripple: false,
        no_ripple_peer: false,
        freeze: false,
        freeze_peer: frozen,
        authorized: false,
        peer_authorized: false,
    };
    // Too small a limit for the net payout, or a line frozen by the issuer, refuses.
    assert!(check_trust_line(Some(&line("1", "0", false)), 99 * E8).is_err());
    assert!(check_trust_line(Some(&line("1000", "0", true)), 99 * E8).is_err());
    assert!(check_trust_line(Some(&line("1000", "0", false)), 99 * E8).is_ok());
    // A network switch between the pre-check and the commit also refuses, with nothing
    // persisted.
    let ok = arg(1, 100 * E8, 3);
    let planned = plan(owner(1), &ok);
    mutate_state(|s| s.xrpl_icusd.config.network = Some(crate::chains::xrp::network::XrplNetwork::Mainnet));
    assert!(commit(owner(1), &ok, &planned).is_err());
    no_side_effects(0);
}

// ---- shared cap reservations ----------------------------------------------------------

fn legacy(owner_p: Principal, vault: u64, gross: u64) -> Result<crate::guard::BorrowReservationGuard, ProtocolError> {
    admit_borrow(owner_p, vault, ICUSD::new(gross), NOW, BorrowReserve::Transient)
        .map(|(_, g)| g.expect("guard"))
}

#[test]
fn legacy_and_xrpl_borrows_share_cap_reservations() {
    let mut w = world();
    w.collateral_configs.get_mut(&xrp()).unwrap().debt_ceiling = 150 * E8;
    replace_state(w);
    // A durable XRPL reservation of 100 (gross) is counted by the LEGACY admission.
    let op1 = open_borrow(owner(1), &arg(1, 100 * E8, 1));
    assert!(legacy(owner(3), 3, 60 * E8).is_err(), "100 durable + 60 > 150");
    let g50 = legacy(owner(3), 3, 50 * E8).expect("100 durable + 50 == 150 fits");
    // A legacy borrow racing (heap reservation in flight) a NEW XRPL borrow: the XRPL
    // plan was made before the legacy reservation, the commit must still refuse.
    drop(g50);
    let a2 = arg(4, 10 * E8, 2);
    let planned2 = plan(owner(4), &a2);
    let g50 = legacy(owner(3), 3, 50 * E8).unwrap();
    let r = commit(owner(4), &a2, &planned2);
    assert!(
        matches!(&r, Err(XrplIcusdError::Policy(m)) if m.contains("debt ceiling")),
        "legacy in-flight reservation must block the XRPL commit: {r:?}"
    );
    read_state(|s| {
        assert_eq!(s.xrpl_icusd.operations.len(), 1, "the refused commit created nothing");
        assert_eq!(s.xrpl_icusd.durable_reserved_total(), 100 * E8);
    });
    // And the other direction: an XRPL precheck sees the heap reservation too.
    let a3 = arg(4, 51 * E8, 3);
    assert!(borrow::precheck(owner(4), &a3, &nonce_of(&a3), NOW).is_err());
    // Release the legacy reservation: now the XRPL commit fits (100 + 10 <= 150).
    drop(g50);
    let id2 = commit(owner(4), &a2, &planned2).expect("fits").0;
    read_state(|s| assert_eq!(s.xrpl_icusd.durable_reserved_total(), 110 * E8));
    // No double counting once op1 commits: durable drops by its gross exactly as the
    // committed debt rises by it, so 110 stays 110: 40 fits, 41 does not.
    prove_mint(op1, 5);
    assert_eq!(advance(op1), BorrowAdvance::Committed);
    read_state(|s| {
        assert_eq!(s.xrpl_icusd.durable_reserved_total(), 10 * E8);
        assert_eq!(s.total_debt_for_collateral(&xrp()).to_u64(), 100 * E8);
    });
    assert!(legacy(owner(3), 3, 41 * E8).is_err(), "100 + 10 + 41 > 150");
    let _g40 = legacy(owner(3), 3, 40 * E8).expect("100 + 10 + 40 == 150: counted exactly once");
    let _ = id2;
}

#[test]
fn mixed_collateral_global_cap_and_upgrade() {
    let mut w = world();
    w.global_icusd_mint_cap = 150 * E8;
    replace_state(w);
    // An XRPL borrow on XRP (100 durable) consumes GLOBAL headroom seen by a legacy
    // borrow on a different collateral (ICP), whose own ceiling is far away.
    let op = open_borrow(owner(1), &arg(1, 100 * E8, 1));
    let icp_borrow = |gross: u64| legacy(owner(2), 2, gross);
    let r = icp_borrow(60 * E8);
    assert!(
        matches!(&r, Err(ProtocolError::GenericError(m)) if m.contains("global icUSD mint cap")),
        "{:?}",
        r.err()
    );
    drop(icp_borrow(50 * E8).expect("100 + 50 == 150 fits globally"));
    // Upgrade: the whole state round-trips through its snapshot encoding. The durable
    // reservation survives; heap reservations need not (none are held here).
    let snapshot = read_state(|s| {
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(s, &mut bytes).unwrap();
        bytes
    });
    let restored: State = ciborium::de::from_reader(snapshot.as_slice()).unwrap();
    replace_state(restored);
    read_state(|s| {
        assert_eq!(s.xrpl_icusd.durable_reserved_total(), 100 * E8);
        assert_eq!(s.xrpl_icusd.durable_reserved_for(xrp()), 100 * E8);
        assert!(s.xrpl_icusd.vault_exclusions.contains(&1));
        assert!(s.xrpl_icusd.op(op).is_some());
    });
    assert!(icp_borrow(60 * E8).is_err(), "still counted after the upgrade");
    drop(icp_borrow(50 * E8).expect("fits after the upgrade"));
    // The restored operation proves and commits exactly as before the upgrade.
    prove_mint(op, 8);
    assert_eq!(advance(op), BorrowAdvance::Committed);
    assert_eq!(debt_of(1), 100 * E8);
    assert!(icp_borrow(51 * E8).is_err(), "committed debt now counts: 100 + 51 > 150");
    drop(icp_borrow(50 * E8).unwrap());
}

// ---- exclusion hooks -------------------------------------------------------------------

fn underwater_xrp_price() {
    // 1,000 XRP at $0.05 = $50 against 100 icUSD of debt.
    set_xrp_cfg(|c| c.last_price = Some(0.05));
}

fn sp_preflight() -> Result<crate::XrpSpAbsorbPreflight, ProtocolError> {
    mutate_state(|s| crate::vault::stability_pool_preflight_xrp_absorb_in_state(s, sp(), 1, 100 * E8, NOW))
}

#[test]
fn exclusion_blocks_legacy_mutations_until_release() {
    replace_state(world());
    let id = open_borrow(owner(1), &arg(1, 100 * E8, 1));
    let vault = read_state(|s| s.vault_id_to_vaults[&1].clone());
    let is_xrpl = |e: &ProtocolError| {
        matches!(e, ProtocolError::TemporarilyUnavailable(m) if m.contains("XRPL"))
    };
    // 1. legacy borrow refused (the factored admission)
    let r = admit_borrow(owner(1), 1, ICUSD::new(E8), NOW, BorrowReserve::Transient);
    assert!(is_xrpl(&r.err().expect("legacy borrow refused")));
    // 2. every owner write-op entry (repay, withdraw, close, add margin) and
    //    liquidation entry funnels through these two checks
    assert!(is_xrpl(&crate::vault::require_vault_not_processing(&vault).unwrap_err()));
    assert!(is_xrpl(&crate::vault::reject_if_bot_processing(1).unwrap_err()));
    // 3. the per-vault operation lock constructors (manual/SP liquidation, owner ops,
    //    borrow retry, push-sweep retry) fail clearly
    use crate::guard::VaultLiquidationGuard as G;
    assert!(is_xrpl(&G::new(1).err().unwrap()));
    assert!(is_xrpl(&G::new_for_borrow_retry(1).err().unwrap()));
    assert!(is_xrpl(&G::new_for_push_sweep_retry(1, owner(1)).err().unwrap()));
    // an unrelated vault is unaffected
    drop(G::new(3).expect("other vaults are free"));
    // 4. the XRP SP absorb preflight is refused BEFORE the SP burns anything
    underwater_xrp_price();
    assert!(is_xrpl(&sp_preflight().err().expect("SP preflight refused")));
    // 5. release by proven mint: all of the above are allowed again
    prove_mint(id, 12);
    assert_eq!(advance(id), BorrowAdvance::Committed);
    let vault = read_state(|s| s.vault_id_to_vaults[&1].clone());
    assert!(crate::vault::require_vault_not_processing(&vault).is_ok());
    assert!(crate::vault::reject_if_bot_processing(1).is_ok());
    drop(G::new(1).expect("liquidation lock available after the debt commit"));
    drop(G::new_for_borrow_retry(1).unwrap());
    drop(G::new_for_push_sweep_retry(1, owner(1)).unwrap());
    sp_preflight().expect("a liquidatable vault is absorbable after release");
}

#[test]
fn exclusion_also_covers_the_post_burn_sp_submit() {
    use crate::icrc3_proof::{SpProofLedger, SpWritedownProof};
    replace_state(world());
    let id = open_borrow(owner(1), &arg(1, 100 * E8, 1));
    prove_mint(id, 12);
    assert_eq!(advance(id), BorrowAdvance::Committed);
    underwater_xrp_price();
    let pf = sp_preflight().expect("preflight");
    // An unresolved XRPL operation re-excludes the vault after the burn: the submit
    // is refused with NO mutation and the persisted reservation stays.
    mutate_state(|s| s.xrpl_icusd.vault_exclusions.insert(1));
    let req = crate::XrpSpAbsorbRequest {
        vault_id: 1,
        icusd_burned_e8s: 100 * E8,
        proof: SpWritedownProof {
            block_index: 9,
            ledger_kind: SpProofLedger::IcusdBurn,
            vault_id_memo: 1,
        },
        allocations: vec![crate::XrpSpPayoutAllocation {
            claimant: owner(5),
            payout_address: "rA".into(),
            destination_tag: None,
            drops: pf.collateral_received_drops,
        }],
    };
    let r = mutate_state(|s| crate::vault::stability_pool_liquidate_xrp_vault_in_state(s, sp(), req.clone(), NOW));
    assert!(matches!(r, Err(ProtocolError::TemporarilyUnavailable(m)) if m.contains("XRPL")));
    read_state(|s| {
        assert!(s.sp_xrp_absorb_preflights.contains_key(&1));
        assert!(s.xrp_claims.is_empty());
        assert_eq!(s.vault_id_to_vaults[&1].borrowed_icusd_amount.to_u64(), 100 * E8);
    });
    // Lifted: the same submit now succeeds.
    mutate_state(|s| s.xrpl_icusd.vault_exclusions.remove(&1));
    let ok = mutate_state(|s| crate::vault::stability_pool_liquidate_xrp_vault_in_state(s, sp(), req, NOW));
    assert!(ok.is_ok(), "{ok:?}");
}

#[test]
fn xrpl_reservation_is_refused_while_an_sp_preflight_exists() {
    replace_state(world());
    // Debt is needed for a preflight: borrow legacy-style by setting it directly.
    mutate_state(|s| {
        s.vault_id_to_vaults.get_mut(&1).unwrap().borrowed_icusd_amount = ICUSD::new(100 * E8);
    });
    underwater_xrp_price();
    sp_preflight().expect("preflight");
    set_xrp_cfg(|c| c.last_price = Some(0.5));
    // Even an EXPIRED persisted reservation (honored by the post-burn submit) blocks.
    mutate_state(|s| s.sp_xrp_absorb_preflights.get_mut(&1).unwrap().expires_at_ns = 1);
    let a = arg(1, 10 * E8, 1);
    let planned = {
        // Plan with the reservation gone, then restore it: the commit-time check
        // (stricter than the legacy borrow) refuses.
        let saved = mutate_state(|s| s.sp_xrp_absorb_preflights.remove(&1)).unwrap();
        let p = plan(owner(1), &a);
        mutate_state(|s| s.sp_xrp_absorb_preflights.insert(1, saved));
        p
    };
    let r = commit(owner(1), &a, &planned);
    assert!(matches!(r, Err(XrplIcusdError::Unavailable(_))), "{r:?}");
    no_side_effects(0);
}

#[test]
fn exclusion_skips_redemption_and_unhealthy_scans() {
    let mut w = world();
    let icp = w.icp_collateral_type();
    // Two ICP vaults with debt; vault 10 is excluded by a durable XRPL operation.
    for id in [10u64, 11] {
        w.open_vault(Vault {
            owner: owner(id as u8),
            vault_id: id,
            borrowed_icusd_amount: ICUSD::new(180 * E8),
            collateral_amount: 20 * E8,
            collateral_type: icp,
            last_accrual_time: NOW,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });
    }
    replace_state(w);
    let price = UsdIcp::from(dec!(10));
    let before = read_state(|s| s.total_redeemable_debt_for(&icp));
    assert_eq!(before.to_u64(), 360 * E8);
    mutate_state(|s| s.xrpl_icusd.vault_exclusions.insert(10));
    read_state(|s| {
        assert_eq!(s.total_redeemable_debt_for(&icp).to_u64(), 180 * E8, "excluded vault skipped");
        assert!(s.redemption_runs().iter().all(|r| !r.vault_ids.contains(&10)));
        // CR = 20 ICP * $10 / 180 = 111% < 133%: unhealthy, but not scanned while excluded.
        let scan = s.scan_unhealthy_vaults(price, true);
        let ids: Vec<u64> = scan.unhealthy_vaults.iter().map(|v| v.vault_id).collect();
        assert!(ids.contains(&11) && !ids.contains(&10), "{ids:?}");
    });
    assert!(read_state(|s| crate::vault::redemption_candidate_types(s).contains(&icp)));
    // A redemption water-fill never touches the excluded vault.
    let redeemed = mutate_state(|s| s.redeem_on_vaults(ICUSD::new(E8 * 50), price, &icp));
    assert!(redeemed.iter().all(|r| r.vault_id != 10), "{redeemed:?}");
    assert_eq!(debt_of(10), 180 * E8);
    // Excluding the only candidate leaves nothing redeemable.
    mutate_state(|s| s.xrpl_icusd.vault_exclusions.insert(11));
    read_state(|s| {
        assert_eq!(s.total_redeemable_debt_for(&icp).to_u64(), 0);
        assert!(s.redemption_runs().is_empty());
        assert!(crate::vault::redemption_candidate_types(s).is_empty());
    });
    // Released: both are visible again.
    mutate_state(|s| s.xrpl_icusd.vault_exclusions.clear());
    assert_eq!(read_state(|s| s.total_redeemable_debt_for(&icp)).to_u64(), 360 * E8 - redeemed.iter().map(|r| r.icusd_redeemed_e8s).sum::<u64>());
    read_state(|s| {
        let ids: Vec<u64> = s
            .scan_unhealthy_vaults(price, true)
            .unhealthy_vaults
            .iter()
            .map(|v| v.vault_id)
            .collect();
        assert!(ids.contains(&10) && ids.contains(&11));
    });
}

// ---- delivery claim survives liquidation / issuer outage ---------------------------------

fn funded_claim() -> OperationId {
    replace_state(world());
    let id = open_borrow(owner(1), &arg(1, 100 * E8, 1));
    prove_mint(id, 31);
    assert_eq!(advance(id), BorrowAdvance::Committed);
    id
}

#[test]
fn liquidation_does_not_confiscate_delivery_claim() {
    let id = funded_claim();
    let (_, _, net) = borrow_terms(id);
    // After DebtCommitted the vault is NOT held for the delivery: normal liquidation
    // paths admit it.
    use crate::guard::VaultLiquidationGuard as G;
    drop(G::new(1).expect("liquidation lock is available"));
    assert!(crate::vault::reject_if_bot_processing(1).is_ok());
    let before = read_state(|s| s.xrpl_icusd.buckets().unwrap());
    // Liquidate the vault in full through the normal state transition.
    let price = UsdIcp::from(dec!(0.05));
    let debt = read_state(|s| s.vault_id_to_vaults[&1].borrowed_icusd_amount);
    mutate_state(|s| {
        s.liquidate_vault(1, Mode::GeneralAvailability, price, Some(debt));
    });
    read_state(|s| assert!(!s.vault_id_to_vaults.contains_key(&1), "vault liquidated"));
    // The holder's claim, its escrow backing and the owner's options are untouched.
    read_state(|s| {
        let st = &s.xrpl_icusd;
        assert_eq!(st.op(id).unwrap().phase, Phase::IssuePrepared);
        assert_eq!(st.buckets().unwrap(), before);
        assert_eq!(st.buckets().unwrap().d, WideAmount::from_e8s(net));
        assert!(st.vault_exclusions.is_empty());
        // Required backing is unchanged by the vault's disappearance.
        assert_eq!(st.required_backing_e8s(0).unwrap(), u128::from(net));
        // The owner can still take the canonical backing.
        assert!(matches!(
            super::issuance::plan_resolution(
                st,
                &owner(1),
                &ResolveClaimArg { operation_id: id, action: ClaimAction::ReceiveCanonical }
            ),
            Ok(super::issuance::ResolutionPlan::ReceiveCanonical { amount_e8s, .. }) if amount_e8s == net
        ));
    });
    // Delivery does not need the vault: an attempt can still be prepared.
    mutate_state(|s| {
        let a = prepared_payment(&s.xrpl_icusd, id, 3);
        s.xrpl_icusd.begin_attempt(LeaseHolder::Operation(id), a).unwrap();
    });
}

#[test]
fn issuer_outage_retains_owned_delivery() {
    let id = funded_claim();
    let (_, _, net) = borrow_terms(id);
    let h = LeaseHolder::Operation(id);
    // Before any attempt the issuance is definitively unexecuted: the owner may take
    // the canonical claim (ReceiveCanonical is available).
    let plan_of = |st: &XrplIcusdState| {
        super::issuance::plan_resolution(
            st,
            &owner(1),
            &ResolveClaimArg { operation_id: id, action: ClaimAction::ReceiveCanonical },
        )
    };
    read_state(|s| assert!(plan_of(&s.xrpl_icusd).is_ok()));
    // The issuer prepares, signs and submits; then the outage: not found, not expired.
    mutate_state(|s| {
        let st = &mut s.xrpl_icusd;
        let a = prepared_payment(st, id, 5);
        st.begin_attempt(h, a).unwrap();
        sign_and_submit(st, id);
    });
    let att = read_state(|s| s.xrpl_icusd.lease_attempt(h).unwrap().clone());
    let v = reconcile(&att, &Observation { tx: TxView::NotFound, expiry: None });
    assert_eq!(v, Reconciled::Pending);
    mutate_state(|s| s.xrpl_icusd.apply_reconcile(h, v, NOW + 5).unwrap());
    read_state(|s| {
        let st = &s.xrpl_icusd;
        // Still owned, still backed (counted as in-flight), never cancelled; the
        // canonical return is refused while the signed issuance may still land.
        assert_eq!(st.op(id).unwrap().phase, Phase::Submitted);
        assert_eq!(st.buckets().unwrap().d, WideAmount::from_e8s(net));
        assert!(matches!(plan_of(st), Err(XrplIcusdError::Ambiguous(_))));
        assert!(st.has_work(), "the tick keeps reconciling during the outage");
    });
    // Only a complete non-inclusion proof past LastLedgerSequence frees the attempt.
    let proof = Observation {
        tx: TxView::NotFound,
        expiry: Some(ExpiryEvidence {
            validated_ledger: att.last_ledger_sequence + 1,
            history_from: att.prepared_ledger,
            history_to: att.last_ledger_sequence + 1,
            providers: vec!["a".into(), "b".into()],
        }),
    };
    let v = reconcile(&att, &proof);
    assert!(matches!(v, Reconciled::Expired { .. }));
    mutate_state(|s| s.xrpl_icusd.apply_reconcile(h, v, NOW + 9).unwrap());
    read_state(|s| {
        let st = &s.xrpl_icusd;
        let op = st.op(id).unwrap();
        // The claim stays owned and backed; it is not cancelled by the expiry...
        assert_eq!(op.phase, Phase::IssuePrepared);
        assert_eq!(op.owner, owner(1));
        assert_eq!(st.buckets().unwrap().d, WideAmount::from_e8s(net));
        // ...and now, with issuance definitively unexecuted, ReceiveCanonical is open.
        assert!(matches!(
            plan_of(st),
            Ok(super::issuance::ResolutionPlan::ReceiveCanonical { amount_e8s, .. }) if amount_e8s == net
        ));
        // Another principal never gets it.
        assert_eq!(
            super::issuance::plan_resolution(
                st,
                &owner(2),
                &ResolveClaimArg { operation_id: id, action: ClaimAction::ReceiveCanonical }
            ),
            Err(XrplIcusdError::NotFound)
        );
    });
}

#[test]
fn borrow_ops_are_driven_resumed_and_ticked() {
    replace_state(world());
    let id = open_borrow(owner(1), &arg(1, 100 * E8, 1));
    read_state(|s| {
        let op = s.xrpl_icusd.op(id).unwrap();
        assert!(op.needs_work(), "an unresolved borrow is background work");
        assert!(s.xrpl_icusd.has_work());
        assert_eq!(plan_tick(&s.xrpl_icusd, 6, 0), vec![TickItem::Op(id)]);
    });
    prove_mint(id, 3);
    assert_eq!(advance(id), BorrowAdvance::Committed);
    // After the debt commit, a terminal delivery still keeps driving an unresolved
    // fee-remainder mint.
    mutate_state(|s| {
        let st = &mut s.xrpl_icusd;
        let a = prepared_payment(st, id, 2);
        st.begin_attempt(LeaseHolder::Operation(id), a).unwrap();
        sign_and_submit(st, id);
        let att = st.lease_attempt(LeaseHolder::Operation(id)).unwrap().clone();
        let r = super::tests::receipt_for(&att);
        let obs = Observation {
            tx: TxView::Succeeded {
                ledger_index: r.ledger_index,
                tx_type: Some("Payment".into()),
                account: Some(r.account.clone()),
                payment: Some(Box::new(r)),
            },
            expiry: None,
        };
        let v = reconcile(&att, &obs);
        assert_eq!(
            st.apply_reconcile(LeaseHolder::Operation(id), v, NOW + 20).unwrap(),
            ReconcileEffect::Delivered
        );
    });
    read_state(|s| {
        let op = s.xrpl_icusd.op(id).unwrap();
        assert_eq!(op.phase, Phase::Delivered);
        assert!(op.needs_work(), "the unresolved fee mint outlives the delivery");
    });
    // A replayed DebtCommitted claim (no jobs) is promoted to the delivery pipeline.
    mutate_state(|s| {
        let op = s.xrpl_icusd.op_mut(id).unwrap();
        op.phase = Phase::DebtCommitted;
    });
    assert_eq!(advance(id), BorrowAdvance::Promoted);
}

#[test]
fn borrow_events_replay_the_same_debt_and_release() {
    // Live path -> event log -> replay reproduces vault debt, deficit and the
    // released reservation/exclusion (the SAME BorrowFromVault event as the legacy).
    let mut w = world();
    w.protocol_deficit_icusd = ICUSD::new(10 * E8);
    w.deficit_repayment_fraction = Ratio::new(dec!(0.5));
    replace_state(w);
    let id = open_borrow(owner(1), &arg(1, 100 * E8, 1));
    prove_mint(id, 40);
    assert_eq!(advance(id), BorrowAdvance::Committed);
    let live_debt = debt_of(1);
    let live_repaid = read_state(|s| s.total_deficit_repaid_icusd);
    let init = crate::event::Event::Init(crate::InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::from_slice(&[0x10; 10]),
        icp_ledger_principal: Principal::from_slice(&[0x11; 10]),
        fee_e8s: 0,
        developer_principal: Principal::from_slice(&[0xdd; 10]),
        treasury_principal: Some(treasury()),
        stability_pool_principal: Some(sp()),
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    });
    // Seed the replayed log with the vault open (world() opened it out of band).
    let vault_open = crate::event::Event::OpenVault {
        vault: Vault {
            owner: owner(1),
            vault_id: 1,
            borrowed_icusd_amount: ICUSD::new(0),
            collateral_amount: 1_000_000_000,
            collateral_type: xrp(),
            last_accrual_time: NOW,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        },
        block_index: 0,
        timestamp: Some(NOW),
    };
    let lane_and_debt: Vec<crate::event::Event> = crate::storage::events().collect();
    let events: Vec<_> = std::iter::once(init)
        .chain(std::iter::once(vault_open))
        .chain(lane_and_debt)
        .collect();
    let replayed = crate::event::replay_with_nonce_time(events.into_iter(), || 7).unwrap();
    assert_eq!(replayed.vault_id_to_vaults[&1].borrowed_icusd_amount.to_u64(), live_debt);
    assert!(live_repaid.to_u64() > 0);
    assert_eq!(replayed.total_deficit_repaid_icusd, live_repaid, "the deficit allocation replays once");
    assert!(replayed.xrpl_icusd.vault_exclusions.is_empty());
    assert_eq!(replayed.xrpl_icusd.durable_reserved_total(), 0);
    assert_eq!(replayed.xrpl_icusd.op(id).unwrap().phase, Phase::DebtCommitted);
    assert_eq!(replayed.xrpl_icusd.op(id).unwrap().owner, owner(1));
}
