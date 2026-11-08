//! The seam between the ledger saga and the icUSD ledger (49b).
//!
//! `ledger.rs` is a verbatim copy of the Base instance's client, classifier and ICRC-3 search. Its
//! calls are free functions bound to the inter-canister API, which a host test cannot answer. The
//! saga (`flows::ledger_ops`, `flows::deposit`, `flows::release`) therefore talks to the ledger
//! only through [`IcpLedger`]. [`CanisterLedger`] is the real implementation and delegates to
//! `ledger.rs` unchanged; the tests use `testkit::MockLedger`, which replays the ledger's
//! deduplication window and block log, so the persist-before-send, same-tuple and absence-proof
//! rules are exercised end to end rather than only through their pure decision functions.

use crate::ledger::{self, CallOutcome, SearchOutcome};
use crate::state::LedgerOp;
use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

#[allow(async_fn_in_trait)]
pub trait IcpLedger {
    /// Send one attempt of the persisted tuple, unchanged (`icrc2_transfer_from` for a pull,
    /// `icrc1_transfer` otherwise).
    async fn execute(&self, op: &LedgerOp) -> CallOutcome;
    /// Bounded ICRC-3 search for the exact tuple, from `from_block`, at most `max_blocks`.
    async fn search(&self, op: &LedgerOp, from_block: u64, max_blocks: u64, now_ns: u64) -> SearchOutcome;
    async fn fee(&self) -> Result<u64, String>;
    async fn balance_of(&self, account: Account) -> Result<u128, String>;
    /// `(allowance, expires_at)` of `spender` over `account`.
    async fn allowance(&self, account: Account, spender: Account) -> Result<(u128, Option<u64>), String>;
    /// The ledger's block count (a lower bound on the tip for a search that starts later).
    async fn log_length(&self) -> Result<u64, String>;
    /// `icrc1_minting_account`: a transfer to it is a burn.
    async fn minting_account(&self) -> Result<Option<Account>, String>;
}

/// The real ledger, addressed by the principal stored at install.
#[derive(Clone, Copy, Debug)]
pub struct CanisterLedger {
    pub ledger: Principal,
}

impl CanisterLedger {
    pub fn from_config() -> CanisterLedger {
        CanisterLedger { ledger: crate::state::read_core(|c| c.config.icusd_ledger) }
    }
}

impl IcpLedger for CanisterLedger {
    async fn execute(&self, op: &LedgerOp) -> CallOutcome {
        ledger::execute(self.ledger, op).await
    }

    async fn search(&self, op: &LedgerOp, from_block: u64, max_blocks: u64, now_ns: u64) -> SearchOutcome {
        ledger::search(self.ledger, op, from_block, max_blocks, now_ns).await
    }

    async fn fee(&self) -> Result<u64, String> {
        ledger::fee(self.ledger).await
    }

    async fn balance_of(&self, account: Account) -> Result<u128, String> {
        ledger::balance_of(self.ledger, account).await
    }

    async fn allowance(&self, account: Account, spender: Account) -> Result<(u128, Option<u64>), String> {
        ledger::allowance(self.ledger, account, spender).await
    }

    async fn log_length(&self) -> Result<u64, String> {
        ledger::log_length(self.ledger).await
    }

    #[cfg(target_arch = "wasm32")]
    async fn minting_account(&self) -> Result<Option<Account>, String> {
        let (acc,): (Option<Account>,) =
            ic_cdk::call(self.ledger, "icrc1_minting_account", ()).await.map_err(|(c, m)| format!("{c:?}: {m}"))?;
        Ok(acc)
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn minting_account(&self) -> Result<Option<Account>, String> {
        Err("unavailable on host".into())
    }
}
