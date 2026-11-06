//! Execution environment shim. On wasm32 these are the IC system APIs; on the host
//! (unit tests) they are thread-local values tests can set.

use candid::Principal;

#[cfg(target_arch = "wasm32")]
pub fn now_ns() -> u64 {
    ic_cdk::api::time()
}

#[cfg(target_arch = "wasm32")]
pub fn caller() -> Principal {
    ic_cdk::caller()
}

#[cfg(target_arch = "wasm32")]
pub fn canister_id() -> Principal {
    ic_cdk::id()
}

#[cfg(target_arch = "wasm32")]
pub fn is_controller(p: &Principal) -> bool {
    ic_cdk::api::is_controller(p)
}

#[cfg(not(target_arch = "wasm32"))]
mod host {
    use candid::Principal;
    use std::cell::{Cell, RefCell};

    thread_local! {
        pub static NOW: Cell<u64> = const { Cell::new(1_700_000_000_000_000_000) };
        pub static CALLER: RefCell<Principal> = const { RefCell::new(Principal::anonymous()) };
        pub static CANISTER: RefCell<Principal> = const { RefCell::new(Principal::from_slice(&[0xbb; 10])) };
        pub static CONTROLLERS: RefCell<Vec<Principal>> = const { RefCell::new(Vec::new()) };
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn now_ns() -> u64 {
    host::NOW.with(|n| n.get())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn caller() -> Principal {
    host::CALLER.with(|c| *c.borrow())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn canister_id() -> Principal {
    host::CANISTER.with(|c| *c.borrow())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn is_controller(p: &Principal) -> bool {
    host::CONTROLLERS.with(|c| c.borrow().contains(p))
}

/// Host-only test controls.
#[cfg(not(target_arch = "wasm32"))]
pub mod test_env {
    use super::host;
    use candid::Principal;

    pub fn set_now_ns(t: u64) {
        host::NOW.with(|n| n.set(t));
    }
    pub fn advance_secs(s: u64) {
        host::NOW.with(|n| n.set(n.get() + s * 1_000_000_000));
    }
    pub fn set_caller(p: Principal) {
        host::CALLER.with(|c| *c.borrow_mut() = p);
    }
    pub fn set_controllers(ps: Vec<Principal>) {
        host::CONTROLLERS.with(|c| *c.borrow_mut() = ps);
    }
}
