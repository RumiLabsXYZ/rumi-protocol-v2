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
