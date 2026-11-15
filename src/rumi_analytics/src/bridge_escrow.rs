//! Bridge escrow accounts and the escrow-adjusted circulating icUSD figure
//! (card BB-58, design `2026-10-30-multichain-icusd-groundwork-design.md`
//! section 2.4).
//!
//! icUSD held in a bridge escrow account backs a bridged token on another
//! chain. Counting the ledger's `icrc1_total_supply` as circulating therefore
//! double counts it. The admin configures the escrow accounts (one per bridge
//! destination on the production ledger, plus the XRPL lane's escrow, each
//! counted once) and the supply refresh publishes `total - sum(balances)`.
//!
//! Everything here is pure (no inter-canister call, no clock) so it can be
//! unit tested natively. The refresh in `timers.rs` does the reads.

use std::collections::BTreeSet;
use std::fmt;

use candid::Principal;
use icrc_ledger_types::icrc1::account::Account;

use crate::storage::SlimState;

/// Upper bound on configured escrow accounts: one per destination network plus
/// the XRPL lane leaves ample room, and each account costs one ledger call per
/// refresh.
pub const MAX_BRIDGE_ESCROW_ACCOUNTS: usize = 32;

/// ICRC-1 identity of an account. A missing subaccount and the all-zero
/// subaccount are the same ledger account, so both map to the same key and a
/// list naming it twice (in either spelling) is a duplicate.
fn account_key(account: &Account) -> (Principal, [u8; 32]) {
    (account.owner, *account.effective_subaccount())
}

/// Validate a replacement list: bounded, no anonymous owner, no duplicates.
pub fn validate_accounts(accounts: &[Account]) -> Result<(), String> {
    if accounts.len() > MAX_BRIDGE_ESCROW_ACCOUNTS {
        return Err(format!(
            "too many bridge escrow accounts: {} (max {})",
            accounts.len(),
            MAX_BRIDGE_ESCROW_ACCOUNTS
        ));
    }
    let mut seen = BTreeSet::new();
    for (index, account) in accounts.iter().enumerate() {
        if account.owner == Principal::anonymous() {
            return Err(format!(
                "bridge escrow account {index}: the anonymous principal cannot own an escrow account"
            ));
        }
        if !seen.insert(account_key(account)) {
            return Err(format!(
                "bridge escrow account {index}: duplicate account (a missing subaccount and the all-zero subaccount are the same ICRC-1 account)"
            ));
        }
    }
    Ok(())
}

/// The configured accounts; empty when none (today's behavior).
pub fn configured_accounts(state: &SlimState) -> Vec<Account> {
    state.bridge_escrow_accounts.clone().unwrap_or_default()
}

/// Replace the configured list. An empty list clears it (stored as `None`).
///
/// Returns `Ok(true)` when the list changed. A change invalidates the cached
/// circulating supply: that figure was computed under the previous list, and
/// in particular the first list must never leave the raw ledger total
/// published. The next refresh repopulates it, and until then `/api/supply`
/// answers "supply not yet cached".
pub fn apply_accounts(state: &mut SlimState, accounts: Vec<Account>) -> Result<bool, String> {
    validate_accounts(&accounts)?;
    let new = if accounts.is_empty() {
        None
    } else {
        Some(accounts)
    };
    if state.bridge_escrow_accounts == new {
        return Ok(false);
    }
    state.bridge_escrow_accounts = new;
    state.circulating_supply_icusd_e8s = None;
    Ok(true)
}

/// Why the escrow-adjusted figure cannot be computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupplyError {
    /// The escrow balances do not fit in a `u128` when added.
    EscrowSumOverflow,
    /// The escrow balances exceed the ledger total: the reads are
    /// inconsistent, and publishing zero (or the raw total) would be a guess.
    EscrowExceedsTotal { total: u128, escrow_sum: u128 },
}

impl fmt::Display for SupplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SupplyError::EscrowSumOverflow => write!(f, "bridge escrow balances overflow u128"),
            SupplyError::EscrowExceedsTotal { total, escrow_sum } => write!(
                f,
                "bridge escrow balances ({escrow_sum}) exceed the icUSD total supply ({total})"
            ),
        }
    }
}

/// `total - sum(escrow_balances)`, checked. With no balances it is `total`.
/// Never saturates: a sum above the total is an error, not zero.
pub fn circulating_after_escrow(
    total: u128,
    escrow_balances: &[u128],
) -> Result<u128, SupplyError> {
    let mut escrow_sum: u128 = 0;
    for balance in escrow_balances {
        escrow_sum = escrow_sum
            .checked_add(*balance)
            .ok_or(SupplyError::EscrowSumOverflow)?;
    }
    total
        .checked_sub(escrow_sum)
        .ok_or(SupplyError::EscrowExceedsTotal { total, escrow_sum })
}

/// What `apply_refresh_result` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// The figure was stored.
    Stored,
    /// The configured accounts changed while the reads were in flight, so the
    /// result belongs to a stale configuration and was dropped. The next
    /// refresh uses the current list.
    DiscardedStaleConfig,
    /// A read or the arithmetic failed: the cached value is kept and the
    /// `icusd_ledger` error counter was incremented.
    Failed,
}

/// Apply the outcome of one refresh. `accounts_used` is the list the refresh
/// read under. A figure is stored only if that list is still the configured
/// one, so a result computed without the escrow subtraction can never land
/// after accounts were configured.
pub fn apply_refresh_result(
    state: &mut SlimState,
    accounts_used: &[Account],
    result: Result<u128, String>,
) -> RefreshOutcome {
    match result {
        Err(_) => {
            state.error_counters.icusd_ledger += 1;
            RefreshOutcome::Failed
        }
        Ok(figure) => {
            if configured_accounts(state) != accounts_used {
                return RefreshOutcome::DiscardedStaleConfig;
            }
            state.circulating_supply_icusd_e8s = Some(figure);
            RefreshOutcome::Stored
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A principal that is never the anonymous one (`[4]`), for any `byte`.
    fn owner(byte: u8) -> Principal {
        Principal::from_slice(&[byte, 0xAA])
    }

    fn account(byte: u8, sub: Option<[u8; 32]>) -> Account {
        Account {
            owner: owner(byte),
            subaccount: sub,
        }
    }

    // --- circulating_after_escrow ---

    #[test]
    fn none_configured_returns_the_total_unchanged() {
        assert_eq!(
            circulating_after_escrow(1_234_567_890, &[]),
            Ok(1_234_567_890)
        );
        assert_eq!(circulating_after_escrow(0, &[]), Ok(0));
    }

    #[test]
    fn normal_subtraction() {
        assert_eq!(
            circulating_after_escrow(100_000_000_000, &[20_000_000_000, 3_000_000_001]),
            Ok(100_000_000_000 - 20_000_000_000 - 3_000_000_001)
        );
        // Zero balances are fine.
        assert_eq!(circulating_after_escrow(500, &[0, 0]), Ok(500));
    }

    #[test]
    fn escrow_equal_to_total_is_a_real_zero() {
        assert_eq!(circulating_after_escrow(700, &[300, 400]), Ok(0));
    }

    #[test]
    fn sum_greater_than_total_is_an_error_not_zero() {
        assert_eq!(
            circulating_after_escrow(700, &[300, 401]),
            Err(SupplyError::EscrowExceedsTotal {
                total: 700,
                escrow_sum: 701
            })
        );
        assert!(circulating_after_escrow(0, &[1]).is_err());
    }

    #[test]
    fn overflowing_sum_is_an_error() {
        assert_eq!(
            circulating_after_escrow(u128::MAX, &[u128::MAX, 1]),
            Err(SupplyError::EscrowSumOverflow)
        );
        // Exactly u128::MAX does not overflow.
        assert_eq!(
            circulating_after_escrow(u128::MAX, &[u128::MAX - 1, 1]),
            Ok(0)
        );
    }

    // --- validate_accounts ---

    #[test]
    fn empty_list_is_valid() {
        assert_eq!(validate_accounts(&[]), Ok(()));
    }

    #[test]
    fn at_most_thirty_two_accounts() {
        let accounts: Vec<Account> = (1..=32u8).map(|b| account(b, None)).collect();
        assert_eq!(validate_accounts(&accounts), Ok(()));

        let mut too_many = accounts;
        too_many.push(account(33, None));
        let err = validate_accounts(&too_many).unwrap_err();
        assert!(err.contains("too many"), "got {err}");
    }

    #[test]
    fn anonymous_owner_is_rejected() {
        let anon = Account {
            owner: Principal::anonymous(),
            subaccount: None,
        };
        let err = validate_accounts(&[account(1, None), anon]).unwrap_err();
        assert!(err.contains("anonymous"), "got {err}");
    }

    #[test]
    fn duplicates_are_rejected_including_the_zero_subaccount_alias() {
        let err = validate_accounts(&[account(1, None), account(1, None)]).unwrap_err();
        assert!(err.contains("duplicate"), "got {err}");

        // A missing subaccount and an all-zero one are the same ICRC-1 account.
        let err = validate_accounts(&[account(1, None), account(1, Some([0; 32]))]).unwrap_err();
        assert!(err.contains("duplicate"), "got {err}");

        // Different subaccounts of one owner, and different owners, are distinct.
        let mut sub = [0u8; 32];
        sub[31] = 1;
        assert_eq!(
            validate_accounts(&[account(1, None), account(1, Some(sub)), account(2, None)]),
            Ok(())
        );
    }

    // --- apply_accounts ---

    #[test]
    fn apply_replaces_the_list_and_empty_clears_it() {
        let mut state = SlimState::default();
        assert!(configured_accounts(&state).is_empty());

        assert_eq!(
            apply_accounts(&mut state, vec![account(1, None), account(2, None)]),
            Ok(true)
        );
        assert_eq!(configured_accounts(&state).len(), 2);

        // Replace, not append.
        assert_eq!(apply_accounts(&mut state, vec![account(3, None)]), Ok(true));
        assert_eq!(configured_accounts(&state), vec![account(3, None)]);

        // Empty restores today's behavior and is stored as None.
        assert_eq!(apply_accounts(&mut state, vec![]), Ok(true));
        assert_eq!(state.bridge_escrow_accounts, None);
        assert_eq!(apply_accounts(&mut state, vec![]), Ok(false));
    }

    #[test]
    fn invalid_list_leaves_state_untouched() {
        let mut state = SlimState {
            circulating_supply_icusd_e8s: Some(42),
            ..SlimState::default()
        };
        apply_accounts(&mut state, vec![account(1, None)]).unwrap();
        state.circulating_supply_icusd_e8s = Some(41);

        let err = apply_accounts(&mut state, vec![account(2, None), account(2, None)]);
        assert!(err.is_err());
        assert_eq!(configured_accounts(&state), vec![account(1, None)]);
        assert_eq!(state.circulating_supply_icusd_e8s, Some(41));
    }

    #[test]
    fn a_changed_list_invalidates_the_cache_and_an_identical_one_does_not() {
        // The raw ledger total cached before any account was configured.
        let mut state = SlimState {
            circulating_supply_icusd_e8s: Some(1_000),
            ..SlimState::default()
        };

        assert_eq!(apply_accounts(&mut state, vec![account(1, None)]), Ok(true));
        assert_eq!(
            state.circulating_supply_icusd_e8s, None,
            "the raw total must not stay published once accounts are configured"
        );

        state.circulating_supply_icusd_e8s = Some(800);
        // Same list again (the zero-subaccount spelling is the same account).
        assert_eq!(
            apply_accounts(&mut state, vec![account(1, Some([0; 32]))]),
            Ok(false)
        );
        assert_eq!(state.circulating_supply_icusd_e8s, Some(800));

        // Clearing the list also drops the figure computed under it.
        assert_eq!(apply_accounts(&mut state, vec![]), Ok(true));
        assert_eq!(state.circulating_supply_icusd_e8s, None);
    }

    // --- apply_refresh_result ---

    #[test]
    fn refresh_success_stores_the_figure() {
        let mut state = SlimState::default();
        assert_eq!(
            apply_refresh_result(&mut state, &[], Ok(777)),
            RefreshOutcome::Stored
        );
        assert_eq!(state.circulating_supply_icusd_e8s, Some(777));
        assert_eq!(state.error_counters.icusd_ledger, 0);
    }

    #[test]
    fn refresh_failure_keeps_the_previous_value_and_counts_the_error() {
        let mut state = SlimState::default();
        apply_accounts(&mut state, vec![account(1, None)]).unwrap();
        let used = configured_accounts(&state);
        assert_eq!(
            apply_refresh_result(&mut state, &used, Ok(600)),
            RefreshOutcome::Stored
        );

        assert_eq!(
            apply_refresh_result(&mut state, &used, Err("ledger unreachable".into())),
            RefreshOutcome::Failed
        );
        assert_eq!(
            state.circulating_supply_icusd_e8s,
            Some(600),
            "a failed read keeps the last good figure"
        );
        assert_eq!(state.error_counters.icusd_ledger, 1);

        // A failure before any figure exists stays None: never the raw total.
        let mut fresh = SlimState::default();
        apply_accounts(&mut fresh, vec![account(1, None)]).unwrap();
        assert_eq!(
            apply_refresh_result(&mut fresh, &used, Err("boom".into())),
            RefreshOutcome::Failed
        );
        assert_eq!(fresh.circulating_supply_icusd_e8s, None);
        assert_eq!(fresh.error_counters.icusd_ledger, 1);
    }

    #[test]
    fn a_result_computed_under_a_stale_list_is_discarded() {
        let mut state = SlimState::default();
        // The refresh started with no accounts and read the raw total ...
        let used: Vec<Account> = vec![];
        // ... then the admin configured an account while the reads were in flight.
        apply_accounts(&mut state, vec![account(1, None)]).unwrap();

        assert_eq!(
            apply_refresh_result(&mut state, &used, Ok(1_000)),
            RefreshOutcome::DiscardedStaleConfig
        );
        assert_eq!(state.circulating_supply_icusd_e8s, None);
        assert_eq!(state.error_counters.icusd_ledger, 0);
    }
}
