use super::adapter::MonadAdapter;
use super::evm_rpc::{BURN_EVENT_TOPIC0, MINT_EVENT_TOPIC0};
use crate::chains::adapter::ChainAdapter;
use crate::chains::config::ChainId;
use sha3::{Digest, Keccak256};

#[test]
fn adapter_reports_monad_chain_id() {
    let a = MonadAdapter::new(ChainId(10143));
    assert_eq!(a.chain_id(), ChainId(10143));
}

#[test]
fn adapter_is_trait_object_safe() {
    let a: Box<dyn ChainAdapter> = Box::new(MonadAdapter::new(ChainId(10143)));
    assert_eq!(a.chain_id(), ChainId(10143));
}

#[test]
fn burn_topic0_matches_canonical_signature() {
    let expected = format!("0x{}", hex::encode(Keccak256::digest(b"Burn(uint256,address,uint256)")));
    assert_eq!(BURN_EVENT_TOPIC0.to_lowercase(), expected);
}

#[test]
fn mint_topic0_matches_canonical_signature() {
    let expected = format!("0x{}", hex::encode(Keccak256::digest(b"Mint(uint256,address,uint256)")));
    assert_eq!(MINT_EVENT_TOPIC0.to_lowercase(), expected);
}
