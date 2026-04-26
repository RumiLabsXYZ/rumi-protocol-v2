//! UPG-006 regression fence: stability_pool init must include the
//! `stable64_size() == 0` reinstall-guard assertion.
//!
//! Audit report: audit-reports/2026-04-22-28e9896/raw-pass-results/upgrade-safety.json (UPG-006).
//!
//! ## Why this test is success-path-only
//!
//! Per the IC interface specification, `mode=Reinstall` UNINSTALLS the canister
//! (clearing stable memory) before installing the new module and calling init.
//! `mode=Install` requires the canister to have no module installed already.
//! Both paths therefore present init with empty stable memory, so the assertion
//! cannot fire under any normal install_code flow.
//!
//! Attempts to manufacture the trap path in PocketIC fail at every step:
//!  - `set_stable_memory` requires the canister to have a wasm module already
//!    installed (returns `CanisterIsEmpty` otherwise), so it cannot pre-populate
//!    stable memory before the first install.
//!  - After install, `set_stable_memory` works, but the only ways to call init
//!    again are reinstall (which wipes) or uninstall+install (which also wipes).
//!  - Upgrade mode runs post_upgrade, not init, and `skip_pre_upgrade` does not
//!    change that.
//!
//! The assertion is therefore defense in depth: it documents intent in code and
//! catches future IC behavior changes, hand-crafted install_code calls that do
//! not zero stable memory first, or environments that do not honor the wipe.
//! `cargo build` validates the assertion compiles; this test validates the
//! canister still installs cleanly after the assertion was added (regression
//! fence against the assertion accidentally being always-failing or moved into
//! a wrong code path).

use candid::{encode_one, Principal};
use pocket_ic::PocketIcBuilder;
use stability_pool::types::*;

fn stability_pool_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/stability_pool.wasm").to_vec()
}

#[test]
fn upg_006_init_succeeds_with_empty_stable_memory() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();

    let canister_id = pic.create_canister();
    pic.add_cycles(canister_id, 2_000_000_000_000);

    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);
    let fake_protocol = Principal::self_authenticating(&[9, 10, 11, 12]);
    let init_args = encode_one(StabilityPoolInitArgs {
        protocol_canister_id: fake_protocol,
        authorized_admins: vec![admin],
    })
    .unwrap();

    pic.install_canister(canister_id, stability_pool_wasm(), init_args, None);
}
