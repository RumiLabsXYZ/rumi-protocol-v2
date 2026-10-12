//! Generate the additive receipt Candid contract from its Rust types.
#![allow(dead_code)]
use candid::Principal;
use rumi_3pool::receipts::*;
use rumi_3pool::types::{DonationReceiptCapacityV1, DonationReceiptErrorV1};
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
#[candid::candid_method(update)]
fn receive_donation_v2(_: u128, _: u64, _: u8, _: u128) -> Result<(), DonationReceiptErrorV1> {
    unimplemented!()
}
#[candid::candid_method(query)]
fn get_donation_receipt_capacity_v1() -> DonationReceiptCapacityV1 {
    unimplemented!()
}
candid::export_service!();
fn main() {
    print!("{}", __export_service());
}
