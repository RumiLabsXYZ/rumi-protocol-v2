use super::deposit_watch::{apply_burn_to_state, credit_deposit_to_state};
use super::chain_vault::{ChainVaultStatus, ChainVaultV1};
use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainStateV2;
use crate::chains::monad::evm_rpc::BurnLog;
use candid::Principal;

fn seeded() -> MultiChainStateV2 {
    let mut s = MultiChainStateV2::default();
    s.chain_supplies.insert(ChainId(10143), 0);
    s.chain_vaults.insert(1, ChainVaultV1 {
        vault_id: 1, owner: Principal::anonymous(), collateral_chain: ChainId(10143),
        custody_address: "0xcustody".into(), collateral_amount_e18: 0, debt_e8s: 0,
        mint_recipient: "0xr".into(), pending_mint_e8s: 0,
        status: ChainVaultStatus::Open, opened_at_ns: 0,
    });
    s
}

#[test]
fn credit_deposit_increments_collateral() {
    let mut s = seeded();
    credit_deposit_to_state(&mut s, 1, 5_000_000_000_000_000_000).expect("credit");
    assert_eq!(s.chain_vaults[&1].collateral_amount_e18, 5_000_000_000_000_000_000);
}

#[test]
fn burn_decrements_supply_and_debt_preserving_invariant() {
    let mut s = seeded();
    s.chain_vaults.get_mut(&1).unwrap().debt_e8s = 10_000_000_000;
    s.chain_supplies.insert(ChainId(10143), 10_000_000_000);
    let total_debt = 10_000_000_000u128;
    let burn = BurnLog { vault_id: 1, amount_e8s: 4_000_000_000, tx_hash: "0xb".into(), block_number: 110 };
    apply_burn_to_state(&mut s, &burn, total_debt).expect("burn");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 6_000_000_000);
    assert_eq!(s.chain_supplies[&ChainId(10143)], 6_000_000_000);
}

#[test]
fn burn_exceeding_debt_is_rejected_without_mutation() {
    let mut s = seeded();
    s.chain_vaults.get_mut(&1).unwrap().debt_e8s = 1_000_000_000;
    s.chain_supplies.insert(ChainId(10143), 1_000_000_000);
    let burn = BurnLog { vault_id: 1, amount_e8s: 9_999_999_999, tx_hash: "0xb".into(), block_number: 1 };
    let res = apply_burn_to_state(&mut s, &burn, 1_000_000_000);
    assert!(res.is_err());
    assert_eq!(s.chain_vaults[&1].debt_e8s, 1_000_000_000); // unchanged
    assert_eq!(s.chain_supplies[&ChainId(10143)], 1_000_000_000); // unchanged
}

#[test]
fn burn_for_unknown_vault_is_rejected() {
    let mut s = seeded();
    let burn = BurnLog { vault_id: 999, amount_e8s: 1, tx_hash: "0xb".into(), block_number: 1 };
    assert!(apply_burn_to_state(&mut s, &burn, 0).is_err());
}
