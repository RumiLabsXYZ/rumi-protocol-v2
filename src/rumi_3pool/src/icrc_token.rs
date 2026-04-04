// ICRC-1 and ICRC-2 token standard implementation for the 3USD LP token.
//
// The LP token balances are tracked internally in ThreePoolState::lp_balances.
// This module exposes them as a proper ICRC-1/ICRC-2 compliant token.
//
// Token: 3USD | Decimals: 8 | Fee: 0
// Subaccounts: balances are tracked by owner principal only — subaccounts are
// accepted on all fields (from, to, spender) but effectively ignored for
// balance lookups. This allows DEX canisters that use per-pool subaccounts
// (e.g. the Rumi AMM) to hold and transfer 3USD without issues.

use candid::{Nat, Principal};
use icrc_ledger_types::icrc::generic_metadata_value::MetadataValue;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::allowance::{Allowance, AllowanceArgs};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};

use crate::state::{mutate_state, read_state};
use crate::types::{LpAllowance, Icrc3Transaction};

// ─── Helpers ───

fn nat_to_u128(n: &Nat) -> Result<u128, ()> {
    use num_traits::cast::ToPrimitive;
    n.0.to_u128().ok_or(())
}

fn effective_allowance(a: &LpAllowance) -> u128 {
    if let Some(exp) = a.expires_at {
        if exp < ic_cdk::api::time() {
            return 0;
        }
    }
    a.amount
}

fn logo_data_uri() -> String {
    let svg = include_str!("../../vault_frontend/static/3pool-logo-v5.svg");
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
    format!("data:image/svg+xml;base64,{}", encoded)
}

// ─── ICRC-1 Queries ───

pub fn icrc1_name() -> String {
    "3USD".to_string()
}

pub fn icrc1_symbol() -> String {
    "3USD".to_string()
}

pub fn icrc1_decimals() -> u8 {
    8
}

pub fn icrc1_fee() -> Nat {
    Nat::from(0u64)
}

pub fn icrc1_total_supply() -> Nat {
    Nat::from(read_state(|s| s.lp_total_supply))
}

pub fn icrc1_minting_account() -> Option<Account> {
    None
}

pub fn icrc1_balance_of(account: Account) -> Nat {
    let p = account.owner;
    Nat::from(read_state(|s| s.lp_balances.get(&p).copied().unwrap_or(0)))
}

pub fn icrc1_metadata() -> Vec<(String, MetadataValue)> {
    vec![
        ("icrc1:name".to_string(), MetadataValue::Text("3USD".to_string())),
        ("icrc1:symbol".to_string(), MetadataValue::Text("3USD".to_string())),
        ("icrc1:decimals".to_string(), MetadataValue::Nat(Nat::from(8u64))),
        ("icrc1:fee".to_string(), MetadataValue::Nat(Nat::from(0u64))),
        ("icrc1:logo".to_string(), MetadataValue::Text(logo_data_uri())),
    ]
}

// ─── ICRC-1 Transfer ───

pub fn icrc1_transfer(caller: Principal, args: TransferArg) -> Result<Nat, TransferError> {
    // Validate fee
    if let Some(ref fee) = args.fee {
        if *fee != Nat::from(0u64) {
            return Err(TransferError::BadFee {
                expected_fee: Nat::from(0u64),
            });
        }
    }

    // Both from_subaccount and to accept any subaccount — balances are keyed
    // by owner principal only, so subaccounts are effectively ignored.
    let to_principal = args.to.owner;

    let amount = nat_to_u128(&args.amount).map_err(|_| TransferError::GenericError {
        error_code: Nat::from(2u64),
        message: "amount overflow".to_string(),
    })?;

    if amount == 0 {
        return Err(TransferError::GenericError {
            error_code: Nat::from(3u64),
            message: "transfer amount must be positive".to_string(),
        });
    }

    if caller == to_principal {
        return Err(TransferError::GenericError {
            error_code: Nat::from(4u64),
            message: "cannot transfer to self".to_string(),
        });
    }

    mutate_state(|s| {
        let from_balance = s.lp_balances.get(&caller).copied().unwrap_or(0);
        if from_balance < amount {
            return Err(TransferError::InsufficientFunds {
                balance: Nat::from(from_balance),
            });
        }

        // Debit
        let from_entry = s.lp_balances.get_mut(&caller).unwrap();
        *from_entry -= amount;
        if *from_entry == 0 {
            s.lp_balances.remove(&caller);
        }

        // Credit
        *s.lp_balances.entry(to_principal).or_insert(0) += amount;

        let id = s.log_block(Icrc3Transaction::Transfer {
            from: caller,
            to: to_principal,
            amount,
            spender: None,
        });
        Ok(Nat::from(id))
    })
}

// ─── ICRC-2 Approve ───

pub fn icrc2_approve(caller: Principal, args: ApproveArgs) -> Result<Nat, ApproveError> {
    // Validate fee
    if let Some(ref fee) = args.fee {
        if *fee != Nat::from(0u64) {
            return Err(ApproveError::BadFee {
                expected_fee: Nat::from(0u64),
            });
        }
    }

    // Subaccounts accepted but ignored — balances keyed by principal only.
    let spender_principal = args.spender.owner;

    let amount = nat_to_u128(&args.amount).map_err(|_| ApproveError::GenericError {
        error_code: Nat::from(2u64),
        message: "amount overflow".to_string(),
    })?;

    // Check expires_at is in the future
    if let Some(expires_at) = args.expires_at {
        let now = ic_cdk::api::time();
        if expires_at < now {
            return Err(ApproveError::Expired { ledger_time: now });
        }
    }

    mutate_state(|s| {
        let key = (caller, spender_principal);

        // CAS: check expected_allowance
        if let Some(ref expected) = args.expected_allowance {
            let current = s
                .allowances()
                .get(&key)
                .map(|a| effective_allowance(a))
                .unwrap_or(0);
            let expected_u128 = nat_to_u128(expected).unwrap_or(u128::MAX);
            if current != expected_u128 {
                return Err(ApproveError::AllowanceChanged {
                    current_allowance: Nat::from(current),
                });
            }
        }

        // Set allowance
        s.allowances_mut().insert(
            key,
            LpAllowance {
                amount,
                expires_at: args.expires_at,
            },
        );

        let id = s.log_block(Icrc3Transaction::Approve {
            from: caller,
            spender: spender_principal,
            amount,
            expires_at: args.expires_at,
        });
        Ok(Nat::from(id))
    })
}

// ─── ICRC-2 Allowance Query ───

pub fn icrc2_allowance(args: AllowanceArgs) -> Allowance {
    let owner = args.account.owner;
    let spender = args.spender.owner;

    read_state(|s| match s.allowances().get(&(owner, spender)) {
        Some(a) => {
            let eff = effective_allowance(a);
            Allowance {
                allowance: Nat::from(eff),
                expires_at: if eff > 0 { a.expires_at } else { None },
            }
        }
        None => Allowance {
            allowance: Nat::from(0u64),
            expires_at: None,
        },
    })
}

// ─── ICRC-2 Transfer From ───

pub fn icrc2_transfer_from(
    caller: Principal,
    args: TransferFromArgs,
) -> Result<Nat, TransferFromError> {
    // Validate fee
    if let Some(ref fee) = args.fee {
        if *fee != Nat::from(0u64) {
            return Err(TransferFromError::BadFee {
                expected_fee: Nat::from(0u64),
            });
        }
    }

    // Subaccounts accepted but ignored — balances keyed by principal only.
    let from_principal = args.from.owner;
    let to_principal = args.to.owner;

    let amount = nat_to_u128(&args.amount).map_err(|_| TransferFromError::GenericError {
        error_code: Nat::from(2u64),
        message: "amount overflow".to_string(),
    })?;

    if amount == 0 {
        return Err(TransferFromError::GenericError {
            error_code: Nat::from(3u64),
            message: "transfer amount must be positive".to_string(),
        });
    }

    mutate_state(|s| {
        // Check and deduct allowance (unless self-transfer)
        if caller != from_principal {
            let key = (from_principal, caller);
            let current_allowance = s
                .allowances()
                .get(&key)
                .map(|a| effective_allowance(a))
                .unwrap_or(0);
            if current_allowance < amount {
                return Err(TransferFromError::InsufficientAllowance {
                    allowance: Nat::from(current_allowance),
                });
            }

            // Deduct allowance
            let allowances = s.allowances_mut();
            let entry = allowances.get_mut(&key).unwrap();
            entry.amount = entry.amount.saturating_sub(amount);
            if entry.amount == 0 {
                allowances.remove(&key);
            }
        }

        // Check balance
        let from_balance = s.lp_balances.get(&from_principal).copied().unwrap_or(0);
        if from_balance < amount {
            return Err(TransferFromError::InsufficientFunds {
                balance: Nat::from(from_balance),
            });
        }

        // Debit
        let from_entry = s.lp_balances.get_mut(&from_principal).unwrap();
        *from_entry -= amount;
        if *from_entry == 0 {
            s.lp_balances.remove(&from_principal);
        }

        // Credit
        *s.lp_balances.entry(to_principal).or_insert(0) += amount;

        let id = s.log_block(Icrc3Transaction::Transfer {
            from: from_principal,
            to: to_principal,
            amount,
            spender: Some(caller),
        });
        Ok(Nat::from(id))
    })
}
