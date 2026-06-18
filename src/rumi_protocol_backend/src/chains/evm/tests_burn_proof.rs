use crate::chains::config::ChainId;
use crate::chains::monad::burn_proof::apply_receipt_burns_to_state;
use crate::chains::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
use crate::chains::monad::evm_rpc::{TxReceiptWithLogs, BURN_EVENT_TOPIC0};
use crate::chains::multi_chain_state::MultiChainStateV4;
use candid::Principal;

fn word(v: u128) -> String {
    format!("0x{:064x}", v)
}

fn state_with_open_vault(debt: u128) -> MultiChainStateV4 {
    let mut s = MultiChainStateV4::default();
    s.chain_supplies.insert(ChainId(10143), debt);
    s.chain_vaults.insert(
        1,
        ChainVaultV1 {
            vault_id: 1,
            owner: Principal::anonymous(),
            collateral_chain: ChainId(10143),
            custody_address: "0xc".into(),
            collateral_amount_native: 0,
            debt_e8s: debt,
            mint_recipient: "0xr".into(),
            pending_mint_e8s: 0,
            status: ChainVaultStatus::Open,
            opened_at_ns: 0,
            last_interest_accrual_ns: 0,
            pending_interest_mint_e8s: 0,
        },
    );
    s
}

#[test]
fn applies_burn_log_from_correct_contract_and_dedups() {
    let mut s = state_with_open_vault(100);
    let contract = "0xcafe";
    let receipt = TxReceiptWithLogs {
        success: true,
        block_number: 10,
        logs: vec![(
            contract.to_string(),
            vec![BURN_EVENT_TOPIC0.to_string(), word(1), word(0xdead)],
            word(40),
            3,
        )],
    };
    let applied =
        apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt)
            .expect("apply");
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].vault_id, 1);
    assert_eq!(applied[0].amount_e8s, 40);
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
    // Re-apply same receipt → deduped, no change.
    let again =
        apply_receipt_burns_to_state(&mut s, ChainId(10143), contract, "0xtx", &receipt)
            .expect("apply again");
    assert_eq!(again.len(), 0);
    assert_eq!(s.chain_vaults[&1].debt_e8s, 60);
}

#[test]
fn rejects_log_from_wrong_contract() {
    let mut s = state_with_open_vault(100);
    let receipt = TxReceiptWithLogs {
        success: true,
        block_number: 10,
        logs: vec![(
            "0xnotthecontract".to_string(),
            vec![BURN_EVENT_TOPIC0.to_string(), word(1), word(0xdead)],
            word(40),
            0,
        )],
    };
    let applied =
        apply_receipt_burns_to_state(&mut s, ChainId(10143), "0xcafe", "0xtx", &receipt)
            .expect("apply");
    assert_eq!(applied.len(), 0, "log from a non-icUSD contract is ignored");
    assert_eq!(s.chain_vaults[&1].debt_e8s, 100);
}
