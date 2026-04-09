//! BalanceTracker: StableBTreeMap-backed running balance and first-seen
//! tracking for icUSD and 3USD holders.

use candid::Principal;
use ic_stable_structures::storable::{Bound, Storable};
use ic_stable_structures::StableBTreeMap;
use std::borrow::Cow;
use std::cell::RefCell;
use super::{Memory, get_memory};
use super::{MEM_BAL_ICUSD, MEM_BAL_3USD, MEM_FIRSTSEEN_ICUSD, MEM_FIRSTSEEN_3USD};

/// ICRC-1 Account: principal + optional 32-byte subaccount.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub owner: Principal,
    pub subaccount: Option<[u8; 32]>,
}

/// Fixed-size key for StableBTreeMap. Layout: 1 byte principal length,
/// up to 29 bytes principal, 1 byte subaccount flag (0 or 1),
/// 32 bytes subaccount (zeroed if absent). Total: 63 bytes.
const ACCOUNT_KEY_LEN: usize = 63;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AccountKey(pub [u8; ACCOUNT_KEY_LEN]);

impl AccountKey {
    pub fn from_account(acct: &Account) -> Self {
        let mut buf = [0u8; ACCOUNT_KEY_LEN];
        let principal_bytes = acct.owner.as_slice();
        buf[0] = principal_bytes.len() as u8;
        buf[1..1 + principal_bytes.len()].copy_from_slice(principal_bytes);
        match &acct.subaccount {
            Some(sub) => {
                buf[30] = 1;
                buf[31..63].copy_from_slice(sub);
            }
            None => {
                buf[30] = 0;
            }
        }
        Self(buf)
    }

    pub fn to_account(&self) -> Account {
        let plen = self.0[0] as usize;
        let owner = Principal::from_slice(&self.0[1..1 + plen]);
        let subaccount = if self.0[30] == 1 {
            let mut sub = [0u8; 32];
            sub.copy_from_slice(&self.0[31..63]);
            Some(sub)
        } else {
            None
        };
        Account { owner, subaccount }
    }

    pub fn owner(&self) -> Principal {
        let plen = self.0[0] as usize;
        Principal::from_slice(&self.0[1..1 + plen])
    }
}

impl Storable for AccountKey {
    fn to_bytes(&self) -> Cow<[u8]> {
        Cow::Borrowed(&self.0)
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        let mut buf = [0u8; ACCOUNT_KEY_LEN];
        buf.copy_from_slice(&bytes[..ACCOUNT_KEY_LEN]);
        Self(buf)
    }
    const BOUND: Bound = Bound::Bounded {
        max_size: ACCOUNT_KEY_LEN as u32,
        is_fixed_size: true,
    };
}

#[derive(Clone, Debug)]
pub struct BalVal(pub u64);

impl Storable for BalVal {
    fn to_bytes(&self) -> Cow<[u8]> {
        Cow::Owned(self.0.to_le_bytes().to_vec())
    }
    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&bytes[..8]);
        Self(u64::from_le_bytes(arr))
    }
    const BOUND: Bound = Bound::Bounded {
        max_size: 8,
        is_fixed_size: true,
    };
}

thread_local! {
    static BAL_ICUSD_MAP: RefCell<StableBTreeMap<AccountKey, BalVal, Memory>> = RefCell::new(
        StableBTreeMap::init(get_memory(MEM_BAL_ICUSD))
    );
    static BAL_3USD_MAP: RefCell<StableBTreeMap<AccountKey, BalVal, Memory>> = RefCell::new(
        StableBTreeMap::init(get_memory(MEM_BAL_3USD))
    );
    static FIRSTSEEN_ICUSD_MAP: RefCell<StableBTreeMap<AccountKey, BalVal, Memory>> = RefCell::new(
        StableBTreeMap::init(get_memory(MEM_FIRSTSEEN_ICUSD))
    );
    static FIRSTSEEN_3USD_MAP: RefCell<StableBTreeMap<AccountKey, BalVal, Memory>> = RefCell::new(
        StableBTreeMap::init(get_memory(MEM_FIRSTSEEN_3USD))
    );
}

#[derive(Clone, Copy, Debug)]
pub enum Token { IcUsd, ThreeUsd }

fn with_bal<F, R>(token: Token, f: F) -> R
where F: FnOnce(&mut StableBTreeMap<AccountKey, BalVal, Memory>) -> R {
    match token {
        Token::IcUsd => BAL_ICUSD_MAP.with(|m| f(&mut m.borrow_mut())),
        Token::ThreeUsd => BAL_3USD_MAP.with(|m| f(&mut m.borrow_mut())),
    }
}

fn with_firstseen<F, R>(token: Token, f: F) -> R
where F: FnOnce(&mut StableBTreeMap<AccountKey, BalVal, Memory>) -> R {
    match token {
        Token::IcUsd => FIRSTSEEN_ICUSD_MAP.with(|m| f(&mut m.borrow_mut())),
        Token::ThreeUsd => FIRSTSEEN_3USD_MAP.with(|m| f(&mut m.borrow_mut())),
    }
}

fn maybe_set_firstseen(token: Token, key: &AccountKey, timestamp_ns: u64) {
    with_firstseen(token, |map| {
        if map.get(key).is_none() {
            map.insert(key.clone(), BalVal(timestamp_ns));
        }
    });
}

pub fn credit(token: Token, acct: &Account, amount: u64, timestamp_ns: u64) {
    let key = AccountKey::from_account(acct);
    with_bal(token, |map| {
        let current = map.get(&key).map(|v| v.0).unwrap_or(0);
        map.insert(key.clone(), BalVal(current.saturating_add(amount)));
    });
    maybe_set_firstseen(token, &key, timestamp_ns);
}

pub fn debit(token: Token, acct: &Account, amount: u64) {
    let key = AccountKey::from_account(acct);
    with_bal(token, |map| {
        let current = map.get(&key).map(|v| v.0).unwrap_or(0);
        let new_bal = current.saturating_sub(amount);
        if new_bal == 0 {
            map.remove(&key);
        } else {
            map.insert(key, BalVal(new_bal));
        }
    });
}

pub fn apply_transfer(token: Token, from: &Account, to: &Account, amount: u64, fee: u64, timestamp_ns: u64) {
    debit(token, from, amount.saturating_add(fee));
    credit(token, to, amount, timestamp_ns);
}

pub fn apply_mint(token: Token, to: &Account, amount: u64, timestamp_ns: u64) {
    credit(token, to, amount, timestamp_ns);
}

pub fn apply_burn(token: Token, from: &Account, amount: u64) {
    debit(token, from, amount);
}

pub fn get_balance(token: Token, acct: &Account) -> u64 {
    let key = AccountKey::from_account(acct);
    with_bal(token, |map| map.get(&key).map(|v| v.0).unwrap_or(0))
}

pub fn holder_count(token: Token) -> u64 {
    with_bal(token, |map| map.len())
}

pub fn all_balances(token: Token) -> Vec<(Account, u64)> {
    with_bal(token, |map| {
        map.iter().map(|(k, v)| (k.to_account(), v.0)).collect()
    })
}

pub fn get_firstseen(token: Token, acct: &Account) -> Option<u64> {
    let key = AccountKey::from_account(acct);
    with_firstseen(token, |map| map.get(&key).map(|v| v.0))
}

pub fn count_new_holders(token: Token, from_ns: u64, to_ns: u64) -> u32 {
    with_firstseen(token, |map| {
        map.iter()
            .filter(|(_, v)| v.0 >= from_ns && v.0 < to_ns)
            .count() as u32
    })
}

pub fn total_supply_tracked(token: Token) -> u64 {
    with_bal(token, |map| {
        map.iter().map(|(_, v)| v.0).fold(0u64, |a, b| a.saturating_add(b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use candid::Principal;

    #[test]
    fn account_key_roundtrip() {
        let acct = Account {
            owner: Principal::anonymous(),
            subaccount: None,
        };
        let key = AccountKey::from_account(&acct);
        let decoded = key.to_account();
        assert_eq!(decoded.owner, acct.owner);
        assert_eq!(decoded.subaccount, acct.subaccount);
    }

    #[test]
    fn account_key_with_subaccount_roundtrip() {
        let mut sub = [0u8; 32];
        sub[0] = 1;
        sub[31] = 0xFF;
        let acct = Account {
            owner: Principal::anonymous(),
            subaccount: Some(sub),
        };
        let key = AccountKey::from_account(&acct);
        let decoded = key.to_account();
        assert_eq!(decoded.owner, acct.owner);
        assert_eq!(decoded.subaccount, Some(sub));
    }
}
