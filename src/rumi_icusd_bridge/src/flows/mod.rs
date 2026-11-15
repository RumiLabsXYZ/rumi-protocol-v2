//! Operation flows that connect ledger and Base effects to the accounting buckets.
//! Buckets change only through `state::apply_transition(s)`.
//!
//! Ledger operations follow one discipline (see `ledger_ops`):
//! 1. The full call tuple is persisted before the first await and reused unchanged.
//! 2. An escrow outflow is counted in `unknown_outflow` before its first send and
//!    released only when the outcome is proven, so coverage is never overstated.
//! 3. Only `Accepted` (Ok or Duplicate) or a complete ICRC-3 search resolves an
//!    operation that ever had an ambiguous attempt.

pub mod deposit;
pub mod ledger_ops;
pub mod release;
