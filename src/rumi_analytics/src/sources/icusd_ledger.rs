//! Typed wrapper around icusd_ledger ICRC-1 queries.

use candid::{Nat, Principal};

pub async fn icrc1_total_supply(ledger: Principal) -> Result<u128, String> {
    let res: Result<(Nat,), _> = ic_cdk::api::call::call(ledger, "icrc1_total_supply", ()).await;
    match res {
        Ok((nat,)) => nat
            .0
            .try_into()
            .map_err(|e| format!("icrc1_total_supply nat -> u128: {}", e)),
        Err((code, msg)) => Err(format!("icrc1_total_supply: {:?} {}", code, msg)),
    }
}
