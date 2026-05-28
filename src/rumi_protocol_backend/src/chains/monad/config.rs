//! Monad config defaults. Real impl in Task 2.
use super::super::config::ChainId;

pub const MONAD_CHAIN_ID: ChainId = ChainId(10143);

#[derive(Clone, Debug, Default)]
pub struct MonadContracts {
    pub icusd: Option<String>,
}

pub static CONTRACTS: MonadContracts = MonadContracts { icusd: None };

pub fn monad_default_config() {}
