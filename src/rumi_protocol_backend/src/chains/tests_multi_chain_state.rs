use super::multi_chain_state::MultiChainStateV1;
use super::config::ChainId;
use candid::{Decode, Encode};

#[test]
fn default_is_empty() {
    let s = MultiChainStateV1::default();
    assert!(s.chain_configs.is_empty());
    assert!(s.chain_supplies.is_empty());
    assert!(s.settlement_queues.is_empty());
    assert_eq!(s.total_supply_all_chains_e8s(), 0u128);
}

#[test]
fn total_supply_sums_across_chains() {
    let mut s = MultiChainStateV1::default();
    s.chain_supplies.insert(ChainId(1), 10_000_000);
    s.chain_supplies.insert(ChainId(2), 25_000_000);
    s.chain_supplies.insert(ChainId(3), 5_000_000);
    assert_eq!(s.total_supply_all_chains_e8s(), 40_000_000u128);
}

#[test]
fn round_trips_via_candid() {
    let mut s = MultiChainStateV1::default();
    s.chain_supplies.insert(ChainId(7), 99);
    let bytes = Encode!(&s).expect("encode");
    let back: MultiChainStateV1 = Decode!(&bytes, MultiChainStateV1).expect("decode");
    assert_eq!(back.chain_supplies.get(&ChainId(7)), Some(&99u128));
}

#[test]
fn round_trips_via_cbor() {
    // The whole State is persisted via ciborium CBOR in `storage::save_state_to_stable`.
    // A multi_chain field that survives Candid but trips up CBOR would still
    // wipe state across an upgrade, so test CBOR directly.
    let mut s = MultiChainStateV1::default();
    s.chain_supplies.insert(ChainId(11), 1234);
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&s, &mut buf).expect("cbor encode");
    let back: MultiChainStateV1 = ciborium::de::from_reader(buf.as_slice()).expect("cbor decode");
    assert_eq!(back.chain_supplies.get(&ChainId(11)), Some(&1234u128));
}

// ── M2: additive owner_evm + evm_owner_nonces serde-default migration ──────────

/// Recursively drop the M2 keys from a CBOR value, simulating a snapshot written
/// before they existed (at any nesting level: top-level `evm_owner_nonces`, and
/// `owner_evm` inside each `chain_vaults` vault map).
fn strip_m2_keys(v: ciborium::Value) -> ciborium::Value {
    use ciborium::Value;
    match v {
        Value::Map(entries) => Value::Map(
            entries
                .into_iter()
                .filter(|(k, _)| !matches!(k, Value::Text(s) if s == "evm_owner_nonces" || s == "owner_evm"))
                .map(|(k, val)| (k, strip_m2_keys(val)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(strip_m2_keys).collect()),
        other => other,
    }
}

#[test]
fn pre_m2_snapshot_decodes_with_defaulted_evm_fields() {
    use super::config::ChainId;
    use super::monad::chain_vault::{ChainVaultStatus, ChainVaultV1};
    use super::multi_chain_state::MultiChainStateV4;
    use candid::Principal;

    let mut s = MultiChainStateV4::default();
    s.chain_supplies.insert(ChainId(71), 100_000_000);
    s.evm_owner_nonces.insert(Principal::from_slice(&[7, 7, 7]), 3);
    s.chain_vaults.insert(
        1,
        ChainVaultV1 {
            vault_id: 1,
            owner: Principal::from_slice(&[1, 2, 3]),
            collateral_chain: ChainId(71),
            custody_address: "0xabc".into(),
            collateral_amount_native: 1_000,
            debt_e8s: 100_000_000,
            mint_recipient: "0xdef".into(),
            pending_mint_e8s: 0,
            status: ChainVaultStatus::Open,
            opened_at_ns: 42,
            owner_evm: Some("0xfeed".into()),
        },
    );

    // Encode current shape, strip the M2 keys (→ a pre-M2 snapshot), re-encode.
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&s, &mut buf).unwrap();
    let val: ciborium::Value = ciborium::de::from_reader(buf.as_slice()).unwrap();
    let mut pre_m2 = Vec::new();
    ciborium::ser::into_writer(&strip_m2_keys(val), &mut pre_m2).unwrap();

    // The pre-M2 bytes MUST decode into the current struct (proves serde-default).
    let back: MultiChainStateV4 = ciborium::de::from_reader(pre_m2.as_slice()).unwrap();
    assert_eq!(back.chain_supplies.get(&ChainId(71)).copied(), Some(100_000_000));
    let v = back.chain_vaults.get(&1).expect("vault survived");
    assert_eq!(v.debt_e8s, 100_000_000);
    assert_eq!(v.owner_evm, None, "stripped owner_evm must default to None");
    assert!(back.evm_owner_nonces.is_empty(), "stripped evm_owner_nonces must default to empty");
}
