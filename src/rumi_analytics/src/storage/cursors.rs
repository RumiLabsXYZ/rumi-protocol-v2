//! Cursor StableCells for event tailing. Each cursor tracks the next event
//! index to fetch from a source canister.

use ic_stable_structures::StableCell;
use std::cell::RefCell;
use super::{Memory, get_memory};
use super::{
    MEM_CURSOR_BACKEND_EVENTS, MEM_CURSOR_3POOL_SWAPS,
    MEM_CURSOR_3POOL_LIQUIDITY, MEM_CURSOR_3POOL_BLOCKS,
    MEM_CURSOR_AMM_SWAPS, MEM_CURSOR_STABILITY_EVENTS, MEM_CURSOR_ICUSD_BLOCKS,
    MEM_CURSOR_AMM_LIQUIDITY,
};

thread_local! {
    static CURSOR_BACKEND_EVENTS: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_BACKEND_EVENTS), 0u64)
            .expect("init cursor backend_events")
    );
    static CURSOR_3POOL_SWAPS: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_3POOL_SWAPS), 0u64)
            .expect("init cursor 3pool_swaps")
    );
    static CURSOR_3POOL_LIQUIDITY: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_3POOL_LIQUIDITY), 0u64)
            .expect("init cursor 3pool_liquidity")
    );
    static CURSOR_3POOL_BLOCKS: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_3POOL_BLOCKS), 0u64)
            .expect("init cursor 3pool_blocks")
    );
    static CURSOR_AMM_SWAPS: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_AMM_SWAPS), 0u64)
            .expect("init cursor amm_swaps")
    );
    static CURSOR_STABILITY_EVENTS: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_STABILITY_EVENTS), 0u64)
            .expect("init cursor stability_events")
    );
    static CURSOR_ICUSD_BLOCKS: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_ICUSD_BLOCKS), 0u64)
            .expect("init cursor icusd_blocks")
    );
    static CURSOR_AMM_LIQUIDITY: RefCell<StableCell<u64, Memory>> = RefCell::new(
        StableCell::init(get_memory(MEM_CURSOR_AMM_LIQUIDITY), 0u64)
            .expect("init cursor amm_liquidity")
    );
}

/// Cursor identifiers matching MemoryIds. Used as keys in SlimState metadata maps.
pub const CURSOR_ID_BACKEND_EVENTS: u8 = 1;
pub const CURSOR_ID_3POOL_SWAPS: u8 = 2;
pub const CURSOR_ID_3POOL_LIQUIDITY: u8 = 3;
pub const CURSOR_ID_3POOL_BLOCKS: u8 = 4;
pub const CURSOR_ID_AMM_SWAPS: u8 = 5;
pub const CURSOR_ID_STABILITY_EVENTS: u8 = 6;
pub const CURSOR_ID_ICUSD_BLOCKS: u8 = 7;
pub const CURSOR_ID_AMM_LIQUIDITY: u8 = 8;

macro_rules! cursor_accessors {
    ($mod_name:ident, $cell:ident) => {
        pub mod $mod_name {
            use super::*;
            pub fn get() -> u64 {
                $cell.with(|c| *c.borrow().get())
            }
            pub fn set(val: u64) {
                $cell.with(|c| c.borrow_mut().set(val).expect(concat!("set cursor ", stringify!($mod_name))));
            }
        }
    };
}

cursor_accessors!(backend_events, CURSOR_BACKEND_EVENTS);
cursor_accessors!(three_pool_swaps, CURSOR_3POOL_SWAPS);
cursor_accessors!(three_pool_liquidity, CURSOR_3POOL_LIQUIDITY);
cursor_accessors!(three_pool_blocks, CURSOR_3POOL_BLOCKS);
cursor_accessors!(amm_swaps, CURSOR_AMM_SWAPS);
cursor_accessors!(stability_events, CURSOR_STABILITY_EVENTS);
cursor_accessors!(icusd_blocks, CURSOR_ICUSD_BLOCKS);
cursor_accessors!(amm_liquidity, CURSOR_AMM_LIQUIDITY);
