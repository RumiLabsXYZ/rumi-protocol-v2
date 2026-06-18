use super::chain_vault::{collateral_ratio_e4, ChainVaultStatus, ChainVaultV1};
use crate::chains::config::ChainId;
use candid::{Decode, Encode};

#[test]
fn collateral_ratio_18_decimals_125_percent() {
    // 5 MON (5e18 wei) at $2.00, 8 icUSD debt -> 5*2/8 = 1.25 = 12500 e4.
    let cr = collateral_ratio_e4(5_000_000_000_000_000_000, 18, 2_0000_0000, 8_0000_0000);
    assert_eq!(cr, 12_500);
}

#[test]
fn collateral_ratio_9_decimals_solana_150_percent() {
    // 10 SOL (10e9 lamports) at $150.00, 1000 icUSD debt -> 10*150/1000 = 1.5 = 15000 e4.
    let cr = collateral_ratio_e4(10_000_000_000, 9, 150_0000_0000, 1000_0000_0000);
    assert_eq!(cr, 15_000);
}

#[test]
fn collateral_ratio_zero_debt_is_unbounded() {
    assert_eq!(collateral_ratio_e4(1, 9, 1, 0), u64::MAX);
}

#[test]
fn chain_vault_round_trips_via_candid() {
    let v = ChainVaultV1 {
        vault_id: 42,
        owner: candid::Principal::anonymous(),
        collateral_chain: ChainId(10143),
        custody_address: "0xabc0000000000000000000000000000000000001".into(),
        collateral_amount_native: 5_000_000_000_000_000_000, // 5 MON
        debt_e8s: 0,
        mint_recipient: "0xrecipient".into(),
        pending_mint_e8s: 10_000_000_000, // 100 icUSD pending
        status: ChainVaultStatus::MintPending,
        opened_at_ns: 1_700_000_000_000_000_000,
        owner_evm: None,
        last_interest_accrual_ns: 0,
        pending_interest_mint_e8s: 0,
    };
    let bytes = Encode!(&v).expect("encode");
    let back: ChainVaultV1 = Decode!(&bytes, ChainVaultV1).expect("decode");
    assert_eq!(back.vault_id, 42);
    assert!(matches!(back.status, ChainVaultStatus::MintPending));
}

#[test]
fn chain_vault_round_trips_via_cbor() {
    let v = ChainVaultV1 {
        vault_id: 1,
        owner: candid::Principal::anonymous(),
        collateral_chain: ChainId(10143),
        custody_address: "0xa".into(),
        collateral_amount_native: 1,
        debt_e8s: 2,
        mint_recipient: "0xb".into(),
        pending_mint_e8s: 0,
        status: ChainVaultStatus::Open,
        opened_at_ns: 0,
        owner_evm: None,
        last_interest_accrual_ns: 0,
        pending_interest_mint_e8s: 0,
    };
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&v, &mut buf).expect("cbor encode");
    let back: ChainVaultV1 = ciborium::de::from_reader(buf.as_slice()).expect("cbor decode");
    assert_eq!(back.debt_e8s, 2);
}
