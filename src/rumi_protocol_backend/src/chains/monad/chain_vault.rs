//! Monad vault record. Real impl in Task 3.
#[derive(Clone, Debug)]
pub enum ChainVaultStatus { MintPending, Open, Closing, Closed }

#[derive(Clone, Debug)]
pub struct ChainVaultV1 {
    pub vault_id: u64,
}
