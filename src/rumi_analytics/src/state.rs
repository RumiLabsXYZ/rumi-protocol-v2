//! Heap-side mirror of `SlimState` and the bridge to/from `storage`.
//!
//! This file mirrors `src/rumi_3pool/src/state.rs`. Read that file first if
//! you haven't seen the pattern.
//!
//! The heap mirror lets hot-path code read SlimState without going through the
//! StableCell on every access. `pre_upgrade` flushes the heap mirror back to
//! the cell; `post_upgrade` reloads it.

use std::cell::RefCell;

use crate::storage::SlimState;

thread_local! {
    static STATE: RefCell<SlimState> = RefCell::new(SlimState::default());
}

pub fn read_state<F, R>(f: F) -> R
where
    F: FnOnce(&SlimState) -> R,
{
    STATE.with(|s| f(&s.borrow()))
}

pub fn mutate_state<F, R>(f: F) -> R
where
    F: FnOnce(&mut SlimState) -> R,
{
    STATE.with(|s| f(&mut s.borrow_mut()))
}

pub fn replace_state(s: SlimState) {
    STATE.with(|cell| *cell.borrow_mut() = s);
}

/// Load the heap mirror from the SlimState StableCell. Called from
/// `post_upgrade` and from `init` (where it loads the default).
pub fn hydrate_from_slim() {
    replace_state(crate::storage::get_slim());
}

/// Flush the heap mirror back to the SlimState StableCell. Called from
/// `pre_upgrade`.
pub fn snapshot_slim_to_cell() {
    let s = read_state(|s| s.clone());
    crate::storage::set_slim(s);
}
