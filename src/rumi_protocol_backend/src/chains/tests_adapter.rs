//! Adapter-trait shape tests. No production impl; tests use a stub.

use super::adapter::{
    ChainAdapter, ChainAdapterError, DepositRecord, FinalitySnapshot,
    MintInstruction, SignedBurn, SignedMint, SignedWithdrawal, WithdrawalRequest,
};
use super::config::ChainId;
use async_trait::async_trait;
use candid::Principal;

struct StubAdapter {
    chain_id: ChainId,
}

#[async_trait(?Send)]
impl ChainAdapter for StubAdapter {
    fn chain_id(&self) -> ChainId {
        self.chain_id
    }

    async fn verify_deposit(&self, _tx_hash: &str) -> Result<DepositRecord, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    async fn sign_withdrawal(&self, _req: WithdrawalRequest) -> Result<SignedWithdrawal, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    async fn sign_mint(&self, _instr: MintInstruction) -> Result<SignedMint, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    async fn sign_burn(&self, _amount_e8s: u128, _burner: Principal) -> Result<SignedBurn, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    async fn fetch_finality(&self) -> Result<FinalitySnapshot, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }

    async fn observe_event(&self, _from_block: u64) -> Result<Vec<DepositRecord>, ChainAdapterError> {
        Err(ChainAdapterError::NotImplemented)
    }
}

#[test]
fn adapter_can_be_held_as_trait_object() {
    let a: Box<dyn ChainAdapter> = Box::new(StubAdapter { chain_id: ChainId(7) });
    assert_eq!(a.chain_id(), ChainId(7));
}

#[test]
fn adapter_error_serializes_via_candid() {
    use candid::{Decode, Encode};
    let err = ChainAdapterError::NotImplemented;
    let bytes = Encode!(&err).expect("encode");
    let round_trip: ChainAdapterError = Decode!(&bytes, ChainAdapterError).expect("decode");
    assert!(matches!(round_trip, ChainAdapterError::NotImplemented));
}
