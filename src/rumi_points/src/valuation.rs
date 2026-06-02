//! Asset valuation (spec Section 7 "Asset valuation", implementation plan Phase 5).
//!
//! PHASE 5 SCOPE (skeleton only here).
//!   - ckUSDC, ckUSDT, icUSD, 3USD: valued at $1.00. No peg-aware cap (dropped
//!     from v1). Face value only.
//!   - ICP: protocol XRC oracle at epoch time (the same source the backend uses).
//!   - LP positions: derived from underlying pool reserves at the epoch boundary,
//!     cached once per snapshot to avoid repeated cross-canister queries.
//!
//! The 3USD verification rule (spec Section 5) also lives at this layer in
//! Phase 4: `active_deposit_value = min(recorded_deposit, verified_3usd)` where
//! `verified_3usd = wallet + stability_pool + amm_lp_3usd_share`, summed across
//! the three venues a holder can keep 3USD in.

#![allow(dead_code)] // Phase 4/5 surface.

use crate::types::AssetType;
use icrc_ledger_types::icrc1::account::Account;

/// USD value (in whole-dollar fixed-point, scaling TBD in Phase 5) of `amount`
/// units of `asset`. Stables resolve to face value; ICP requires the oracle.
pub fn value_usd(_asset: AssetType, _amount: u128) -> u128 {
    unimplemented!("Phase 5: $1 stables, XRC for ICP");
}

/// Sum of a principal's verified 3USD across wallet + stability pool + AMM LP
/// share (spec Section 5), used to cap 3pool deposit accrual. Phase 4.
pub async fn verified_3usd(_account: Account) -> u128 {
    unimplemented!("Phase 4: wallet + SP + AMM LP 3USD verification");
}
