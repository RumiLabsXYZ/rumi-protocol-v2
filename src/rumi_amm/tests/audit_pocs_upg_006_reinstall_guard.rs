//! UPG-006 regression fence: rumi_amm init must include the
//! `stable64_size() == 0` reinstall-guard assertion.
//!
//! Audit report: audit-reports/2026-04-22-28e9896/raw-pass-results/upgrade-safety.json (UPG-006).
//!
//! See `audit_pocs_upg_006_reinstall_guard.rs` in the stability_pool crate for
//! a detailed explanation of why this is success-path-only. Briefly: per IC
//! interface spec, mode=Reinstall wipes stable memory before init runs, so the
//! assertion's trap path is unreachable in PocketIC. The assertion is defense
//! in depth against future IC behavior changes; this test verifies the canister
//! still installs cleanly with the assertion in place.

use candid::{encode_one, Principal};
use pocket_ic::PocketIcBuilder;
use rumi_amm::types::AmmInitArgs;

fn rumi_amm_wasm() -> Vec<u8> {
    include_bytes!("../../../target/wasm32-unknown-unknown/release/rumi_amm.wasm").to_vec()
}

#[test]
fn upg_006_init_succeeds_with_empty_stable_memory() {
    let pic = PocketIcBuilder::new().with_application_subnet().build();

    let canister_id = pic.create_canister();
    pic.add_cycles(canister_id, 2_000_000_000_000);

    let admin = Principal::self_authenticating(&[5, 6, 7, 8]);
    let init_args = encode_one(AmmInitArgs { admin }).unwrap();

    pic.install_canister(canister_id, rumi_amm_wasm(), init_args, None);
}
