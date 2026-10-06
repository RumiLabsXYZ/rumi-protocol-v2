//! Generate the additive receipt Candid contract from its Rust types.
#![allow(dead_code)]
use candid::{Nat, Principal};
use rumi_3pool::receipts::*;
use rumi_3pool::ClaimProofErrorV1;
#[candid::candid_method(update)]
fn swap_with_receipt_v1(_: SwapRequestV1) -> Result<SwapReceiptV1, SwapReceiptErrorV1> {
    unimplemented!()
}
#[candid::candid_method(query)]
fn get_swap_receipt_v1(_: Vec<u8>) -> Option<SwapReceiptV1> {
    unimplemented!()
}
#[candid::candid_method(update)]
fn set_swap_receipt_client_v1(_: Principal, _: bool) -> Result<(), SwapReceiptErrorV1> {
    unimplemented!()
}
#[candid::candid_method(query)]
fn is_swap_receipt_client_v1(_: Principal) -> bool {
    unimplemented!()
}
#[candid::candid_method(query)]
fn get_next_intent_sequence_v1() -> Option<u64> { unimplemented!() }
#[candid::candid_method(update)]
fn reconcile_swap_leg_v1(_: Vec<u8>, _: u8, _: Nat) -> Result<SwapReceiptV1, SwapReceiptErrorV1> { unimplemented!() }
#[candid::candid_method(update)]
fn advance_swap_absence_scan_v1(_: Vec<u8>, _: u8) -> Result<SwapReceiptV1, SwapReceiptErrorV1> { unimplemented!() }
#[candid::candid_method(update)]
fn add_liquidity_with_receipt_v1(_: Vec<u8>, _: Vec<u128>, _: u128) -> Result<IngressReceiptV1, IngressReceiptErrorV1> { unimplemented!() }
#[candid::candid_method(update)]
fn donate_with_receipt_v1(_: Vec<u8>, _: u8, _: u128) -> Result<IngressReceiptV1, IngressReceiptErrorV1> { unimplemented!() }
#[candid::candid_method(query)]
fn get_ingress_receipt_v1(_: Vec<u8>) -> Option<IngressReceiptV1> { unimplemented!() }
#[candid::candid_method(update)]
fn reconcile_ingress_pull_v1(_: Vec<u8>, _: u8, _: Nat) -> Result<IngressReceiptV1, IngressReceiptErrorV1> { unimplemented!() }
#[candid::candid_method(update)]
fn advance_ingress_absence_scan_v1(_: Vec<u8>, _: u8) -> Result<IngressReceiptV1, IngressReceiptErrorV1> { unimplemented!() }
#[candid::candid_method(update)]
fn reconcile_pending_claim_v1(_: u64, _: Nat) -> Result<(), ClaimProofErrorV1> { unimplemented!() }
candid::export_service!();
fn main() {
    print!("{}", __export_service());
}
