//! Self-check semantics: when sum(chain_supplies) != total_debt, the
//! self-check flips the halt flag and Mode and stops mutating supplies.

use super::config::ChainId;
use super::multi_chain_state::MultiChainStateV1;
use super::supply::check_invariant;

#[test]
fn check_invariant_passes_when_sum_equals_total_debt() {
    let mut s = MultiChainStateV1::default();
    s.chain_supplies.insert(ChainId(1), 100);
    s.chain_supplies.insert(ChainId(2), 200);
    assert!(check_invariant(&s, 300).is_ok());
}

#[test]
fn check_invariant_fails_on_drift() {
    let mut s = MultiChainStateV1::default();
    s.chain_supplies.insert(ChainId(1), 100);
    s.chain_supplies.insert(ChainId(2), 200);
    let err = check_invariant(&s, 299).expect_err("drift must be caught");
    assert!(matches!(err, super::supply::SupplyInvariantError::Divergence { .. }));
}

#[test]
fn empty_state_passes_when_total_debt_is_zero() {
    let s = MultiChainStateV1::default();
    assert!(check_invariant(&s, 0).is_ok());
}
