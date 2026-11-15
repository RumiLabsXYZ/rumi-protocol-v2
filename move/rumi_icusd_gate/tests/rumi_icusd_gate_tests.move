#[test_only]
module rumi_icusd_gate::rumi_icusd_gate_tests;

use rumi_icusd_gate::rumi_icusd_gate::{
    Self as gate,
    Gate,
    MintCap,
    AdminCap,
    Minted,
    BridgeOut,
    LimitsSet,
    RUMI_ICUSD_GATE
};
use std::unit_test::assert_eq;
use sui::coin::{Self, Coin, TreasuryCap};
use sui::coin_registry::{Self, Currency, MetadataCap};
use sui::event;
use sui::test_scenario::{Self as ts, Scenario};

const PUBLISHER: address = @0xA11CE;
const ALICE: address = @0xA1;
const BOB: address = @0xB0B;
const E8: u64 = 100_000_000;

/// 0x01 .. 0x20, the same chain identifier the Python reference uses for its first vectors.
const CHAIN_ID: vector<u8> = x"0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";

// ===== Helpers =====

fun chain_id(): vector<u8> { CHAIN_ID }

fun id32(n: u8): vector<u8> { vector::tabulate!(32, |_| n) }

/// A 29-byte principal (the longest valid length).
fun principal29(): vector<u8> { vector::tabulate!(29, |i| (i as u8) + 1) }

/// A non-default, non-trivial 32-byte subaccount.
fun subaccount(): vector<u8> { vector::tabulate!(32, |i| (i as u8) + 100) }

fun start(): Scenario {
    let mut s = ts::begin(PUBLISHER);
    gate::init_for_testing(CHAIN_ID, s.ctx());
    s.next_tx(PUBLISHER);
    s
}

fun set_limits(s: &mut Scenario, supply_cap: u64, min_bridge_out: u64) {
    s.next_tx(PUBLISHER);
    let mut g = s.take_shared<Gate>();
    let admin = s.take_from_sender<AdminCap>();
    gate::set_limits(&mut g, &admin, supply_cap, min_bridge_out);
    ts::return_shared(g);
    s.return_to_sender(admin);
}

fun mint(s: &mut Scenario, mint_id: vector<u8>, to: address, amount: u64) {
    s.next_tx(PUBLISHER);
    let mut g = s.take_shared<Gate>();
    let cap = s.take_from_sender<MintCap>();
    gate::mint(&mut g, &cap, mint_id, to, amount, s.ctx());
    ts::return_shared(g);
    s.return_to_sender(cap);
}

/// `who` returns the most recent coin they received.
fun bridge_out(
    s: &mut Scenario,
    who: address,
    icp_principal: vector<u8>,
    icp_subaccount: vector<u8>,
    nonce: u64,
) {
    s.next_tx(who);
    let mut g = s.take_shared<Gate>();
    let coin = s.take_from_sender<Coin<RUMI_ICUSD_GATE>>();
    gate::bridge_out(&mut g, coin, icp_principal, icp_subaccount, nonce, s.ctx());
    ts::return_shared(g);
}

fun force_version(s: &mut Scenario, version: u64) {
    s.next_tx(PUBLISHER);
    let mut g = s.take_shared<Gate>();
    gate::set_version_for_testing(&mut g, version);
    ts::return_shared(g);
}

fun migrate(s: &mut Scenario) {
    s.next_tx(PUBLISHER);
    let mut g = s.take_shared<Gate>();
    let admin = s.take_from_sender<AdminCap>();
    gate::migrate(&mut g, &admin);
    ts::return_shared(g);
    s.return_to_sender(admin);
}

fun check_accounting(g: &Gate) {
    assert_eq!(gate::minted_total(g) - gate::burned_total(g), (gate::total_supply(g) as u128));
}

/// Two gates of the same package, published by different addresses. Returns the gate ids.
fun start_two_gates(): (Scenario, ID, ID) {
    let mut s = ts::begin(PUBLISHER);
    gate::init_for_testing(CHAIN_ID, s.ctx());
    s.next_tx(BOB);
    let gate_a = ts::most_recent_id_shared<Gate>().destroy_some();
    gate::init_for_testing(CHAIN_ID, s.ctx());
    s.next_tx(PUBLISHER);
    let gate_b = ts::most_recent_id_shared<Gate>().destroy_some();
    (s, gate_a, gate_b)
}

// ===== Publish and the initial zero cap =====

#[test]
fun init_creates_a_disabled_shared_gate_and_delivers_the_caps() {
    let mut s = start();

    // Capabilities went to the publisher; the treasury cap is wrapped inside the gate, so
    // no account holds it and nothing shared exposes it.
    assert!(ts::has_most_recent_for_address<MintCap>(PUBLISHER));
    assert!(ts::has_most_recent_for_address<AdminCap>(PUBLISHER));
    assert!(ts::has_most_recent_for_address<MetadataCap<RUMI_ICUSD_GATE>>(PUBLISHER));
    assert!(!ts::has_most_recent_for_address<TreasuryCap<RUMI_ICUSD_GATE>>(PUBLISHER));
    assert!(!ts::has_most_recent_shared<TreasuryCap<RUMI_ICUSD_GATE>>());
    assert!(ts::has_most_recent_shared<Gate>());

    let g = s.take_shared<Gate>();
    assert_eq!(gate::version(&g), gate::version_constant());
    assert_eq!(gate::chain_identifier(&g), CHAIN_ID);
    // The on-chain DISABLED state: nothing can be minted until an admin raises the cap.
    assert_eq!(gate::supply_cap(&g), 0);
    assert_eq!(gate::min_bridge_out(&g), 1);
    assert_eq!(gate::minted_total(&g), 0);
    assert_eq!(gate::burned_total(&g), 0);
    assert_eq!(gate::mint_count(&g), 0);
    assert_eq!(gate::bridge_out_count(&g), 0);
    assert_eq!(gate::total_supply(&g), 0);
    ts::return_shared(g);

    // The currency was created through the registry as an unregulated 8-decimal coin and
    // sent to the registry address for `finalize_registration`.
    assert!(ts::has_most_recent_for_address<Currency<RUMI_ICUSD_GATE>>(@0xc));
    let currency = s.take_from_address<Currency<RUMI_ICUSD_GATE>>(@0xc);
    assert_eq!(coin_registry::decimals(&currency), 8);
    assert_eq!(coin_registry::symbol(&currency), b"icUSD".to_string());
    assert_eq!(coin_registry::name(&currency), b"Rumi icUSD (escrow-backed)".to_string());
    assert!(!coin_registry::is_regulated(&currency));
    assert!(!coin_registry::is_supply_fixed(&currency));
    assert!(!coin_registry::is_supply_burn_only(&currency));
    assert!(coin_registry::treasury_cap_id(&currency).is_some());
    ts::return_to_address(@0xc, currency);
    s.end();
}

#[test, expected_failure(abort_code = gate::EBadChainIdentifier)]
fun the_shipped_unset_chain_identifier_cannot_be_published() {
    let mut s = ts::begin(PUBLISHER);
    gate::init_shipped_for_testing(s.ctx());
    s.end();
}

#[test, expected_failure(abort_code = gate::EBadChainIdentifier)]
fun init_refuses_a_chain_identifier_that_is_not_32_bytes() {
    let mut s = ts::begin(PUBLISHER);
    gate::init_for_testing(vector::tabulate!(31, |_| 7u8), s.ctx());
    s.end();
}

#[test, expected_failure(abort_code = gate::EBadChainIdentifier)]
fun init_refuses_an_all_zero_chain_identifier() {
    let mut s = ts::begin(PUBLISHER);
    gate::init_for_testing(vector::tabulate!(32, |_| 0u8), s.ctx());
    s.end();
}

#[test, expected_failure(abort_code = gate::ESupplyCapExceeded)]
fun a_fresh_gate_refuses_every_mint_because_the_cap_is_zero() {
    let mut s = start();
    mint(&mut s, id32(1), ALICE, 1);
    s.end();
}

// ===== Authorized mint and consumed ids =====

#[test]
fun an_authorized_mint_delivers_the_coin_records_the_id_and_emits_the_event() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 1);

    s.next_tx(PUBLISHER);
    let mut g = s.take_shared<Gate>();
    let cap = s.take_from_sender<MintCap>();
    gate::mint(&mut g, &cap, id32(7), ALICE, 400 * E8, s.ctx());

    assert_eq!(
        event::events_by_type<Minted>(),
        vector[gate::new_minted_event(id32(7), ALICE, 400 * E8, 1, ((400 * E8) as u128), 400 * E8)],
    );
    assert!(gate::is_mint_consumed(&g, id32(7)));
    assert!(!gate::is_mint_consumed(&g, id32(8)));
    assert_eq!(gate::mint_record(&g, id32(7)), gate::new_mint_record(ALICE, 400 * E8, 1));
    assert_eq!(gate::mint_count(&g), 1);
    assert_eq!(gate::minted_total(&g), ((400 * E8) as u128));
    assert_eq!(gate::total_supply(&g), 400 * E8);
    check_accounting(&g);
    ts::return_shared(g);
    s.return_to_sender(cap);

    s.next_tx(ALICE);
    let minted = s.take_from_sender<Coin<RUMI_ICUSD_GATE>>();
    assert_eq!(minted.value(), 400 * E8);
    s.return_to_sender(minted);
    s.end();
}

#[test, expected_failure(abort_code = gate::EMintConsumed)]
fun a_consumed_mint_id_cannot_be_replayed_even_with_other_arguments() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 1);
    mint(&mut s, id32(7), ALICE, 10 * E8);
    mint(&mut s, id32(7), BOB, 20 * E8);
    s.end();
}

// ===== Amount, recipient and cap boundaries =====

#[test]
fun a_mint_may_reach_the_cap_exactly() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    mint(&mut s, id32(1), ALICE, 60 * E8);
    mint(&mut s, id32(2), BOB, 40 * E8);
    s.next_tx(PUBLISHER);
    let g = s.take_shared<Gate>();
    assert_eq!(gate::total_supply(&g), gate::supply_cap(&g));
    ts::return_shared(g);
    s.end();
}

#[test, expected_failure(abort_code = gate::ESupplyCapExceeded)]
fun one_unit_above_the_cap_is_refused() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    mint(&mut s, id32(1), ALICE, 60 * E8);
    mint(&mut s, id32(2), BOB, 40 * E8 + 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::ESupplyCapExceeded)]
fun a_single_mint_above_the_cap_is_refused() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    mint(&mut s, id32(1), ALICE, 100 * E8 + 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::ESupplyCapExceeded)]
fun lowering_the_cap_below_the_supply_blocks_every_further_mint() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    mint(&mut s, id32(1), ALICE, 100 * E8);
    set_limits(&mut s, 50 * E8, 1);
    mint(&mut s, id32(2), ALICE, 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::EZeroAmount)]
fun a_zero_amount_mint_is_refused() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    mint(&mut s, id32(1), ALICE, 0);
    s.end();
}

#[test, expected_failure(abort_code = gate::EBadRecipient)]
fun minting_to_the_zero_address_is_refused() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    mint(&mut s, id32(1), @0x0, 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::EBadRecipient)]
fun minting_to_the_gate_itself_is_refused() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    s.next_tx(PUBLISHER);
    let mut g = s.take_shared<Gate>();
    let cap = s.take_from_sender<MintCap>();
    let own_address = gate::gate_address(&g);
    gate::mint(&mut g, &cap, id32(1), own_address, 1, s.ctx());
    ts::return_shared(g);
    s.return_to_sender(cap);
    s.end();
}

#[test, expected_failure(abort_code = gate::EBadMintId)]
fun a_31_byte_mint_id_is_refused() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    mint(&mut s, vector::tabulate!(31, |_| 1u8), ALICE, 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::EBadMintId)]
fun a_33_byte_mint_id_is_refused() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    mint(&mut s, vector::tabulate!(33, |_| 1u8), ALICE, 1);
    s.end();
}

#[test]
fun set_limits_accepts_the_maximum_cap_and_emits_the_event() {
    let mut s = start();
    s.next_tx(PUBLISHER);
    let mut g = s.take_shared<Gate>();
    let admin = s.take_from_sender<AdminCap>();
    gate::set_limits(&mut g, &admin, gate::max_supply_cap(), 5 * E8);
    assert_eq!(
        event::events_by_type<LimitsSet>(),
        vector[gate::new_limits_set_event(gate::max_supply_cap(), 5 * E8)],
    );
    assert_eq!(gate::supply_cap(&g), 18_446_744_073_709_551_614);
    assert_eq!(gate::min_bridge_out(&g), 5 * E8);
    ts::return_shared(g);
    s.return_to_sender(admin);
    s.end();
}

#[test, expected_failure(abort_code = gate::ESupplyCapTooHigh)]
fun set_limits_above_the_maximum_cap_is_refused() {
    let mut s = start();
    set_limits(&mut s, gate::max_supply_cap() + 1, 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::EBadMinimum)]
fun set_limits_with_a_zero_minimum_return_is_refused() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 0);
    s.end();
}

// ===== Exact destination binding =====

#[test]
fun bridge_out_burns_the_coin_and_saves_the_exact_destination() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 1);
    mint(&mut s, id32(1), ALICE, 250 * E8);

    s.next_tx(ALICE);
    let mut g = s.take_shared<Gate>();
    let coin = s.take_from_sender<Coin<RUMI_ICUSD_GATE>>();
    let expected_id = gate::compute_transfer_id(&chain_id(), gate::gate_address(&g), ALICE, 42);
    gate::bridge_out(&mut g, coin, principal29(), subaccount(), 42, s.ctx());

    // The event, the durable record and the id all carry the exact principal and subaccount.
    assert_eq!(
        event::events_by_type<BridgeOut>(),
        vector[
            gate::new_bridge_out_event(
                expected_id,
                ALICE,
                250 * E8,
                1,
                principal29(),
                subaccount(),
                42,
                ((250 * E8) as u128),
                0,
            ),
        ],
    );
    assert!(gate::is_return_used(&g, expected_id));
    assert_eq!(
        gate::return_record(&g, expected_id),
        gate::new_return_record(ALICE, 250 * E8, 1, principal29(), subaccount(), 42),
    );
    assert_eq!(gate::bridge_out_count(&g), 1);
    assert_eq!(gate::burned_total(&g), ((250 * E8) as u128));
    assert_eq!(gate::total_supply(&g), 0);
    check_accounting(&g);
    ts::return_shared(g);

    // The coin is gone.
    s.next_tx(ALICE);
    assert!(!ts::has_most_recent_for_address<Coin<RUMI_ICUSD_GATE>>(ALICE));
    s.end();
}

#[test]
fun bridge_out_accepts_the_boundary_principals_and_the_default_subaccount() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 1);
    // One byte that is not the anonymous principal, the 29-byte maximum, and the all-zero
    // (default) subaccount.
    mint(&mut s, id32(1), ALICE, 10 * E8);
    bridge_out(&mut s, ALICE, vector[0x05], vector::tabulate!(32, |_| 0u8), 1);
    mint(&mut s, id32(2), ALICE, 10 * E8);
    bridge_out(&mut s, ALICE, principal29(), subaccount(), 2);
    mint(&mut s, id32(3), ALICE, 10 * E8);
    bridge_out(&mut s, ALICE, vector[0x04, 0x04], subaccount(), 3);

    s.next_tx(PUBLISHER);
    let g = s.take_shared<Gate>();
    assert_eq!(gate::bridge_out_count(&g), 3);
    ts::return_shared(g);
    s.end();
}

#[test, expected_failure(abort_code = gate::EBadPrincipal)]
fun an_empty_principal_is_refused() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 1);
    mint(&mut s, id32(1), ALICE, 10 * E8);
    bridge_out(&mut s, ALICE, vector[], subaccount(), 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::EBadPrincipal)]
fun a_30_byte_principal_is_refused() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 1);
    mint(&mut s, id32(1), ALICE, 10 * E8);
    bridge_out(&mut s, ALICE, vector::tabulate!(30, |_| 9u8), subaccount(), 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::EBadPrincipal)]
fun the_anonymous_principal_is_refused() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 1);
    mint(&mut s, id32(1), ALICE, 10 * E8);
    bridge_out(&mut s, ALICE, vector[0x04], subaccount(), 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::EBadSubaccount)]
fun a_31_byte_subaccount_is_refused() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 1);
    mint(&mut s, id32(1), ALICE, 10 * E8);
    bridge_out(&mut s, ALICE, principal29(), vector::tabulate!(31, |_| 1u8), 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::EBadSubaccount)]
fun a_33_byte_subaccount_is_refused() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 1);
    mint(&mut s, id32(1), ALICE, 10 * E8);
    bridge_out(&mut s, ALICE, principal29(), vector::tabulate!(33, |_| 1u8), 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::EBelowMinimum)]
fun a_return_below_the_minimum_is_refused() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 10 * E8);
    mint(&mut s, id32(1), ALICE, 10 * E8 - 1);
    bridge_out(&mut s, ALICE, principal29(), subaccount(), 1);
    s.end();
}

#[test]
fun a_return_of_exactly_the_minimum_is_accepted() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 10 * E8);
    mint(&mut s, id32(1), ALICE, 10 * E8);
    bridge_out(&mut s, ALICE, principal29(), subaccount(), 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::EBelowMinimum)]
fun a_zero_value_coin_is_refused() {
    let mut s = start();
    s.next_tx(ALICE);
    let mut g = s.take_shared<Gate>();
    let zero = coin::zero<RUMI_ICUSD_GATE>(s.ctx());
    gate::bridge_out(&mut g, zero, principal29(), subaccount(), 1, s.ctx());
    ts::return_shared(g);
    s.end();
}

// ===== Duplicate and per-sender nonces =====

#[test, expected_failure(abort_code = gate::EReturnUsed)]
fun the_same_sender_and_nonce_cannot_return_twice() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 1);
    mint(&mut s, id32(1), ALICE, 10 * E8);
    bridge_out(&mut s, ALICE, principal29(), subaccount(), 5);
    mint(&mut s, id32(2), ALICE, 10 * E8);
    bridge_out(&mut s, ALICE, principal29(), subaccount(), 5);
    s.end();
}

#[test]
fun another_senders_same_nonce_gets_its_own_distinct_return() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 1);
    mint(&mut s, id32(1), ALICE, 10 * E8);
    mint(&mut s, id32(2), BOB, 20 * E8);
    bridge_out(&mut s, ALICE, principal29(), subaccount(), 5);
    bridge_out(&mut s, BOB, vector[0x05, 0x06], subaccount(), 5);

    s.next_tx(PUBLISHER);
    let g = s.take_shared<Gate>();
    let gate_address = gate::gate_address(&g);
    let alice_id = gate::compute_transfer_id(&chain_id(), gate_address, ALICE, 5);
    let bob_id = gate::compute_transfer_id(&chain_id(), gate_address, BOB, 5);
    assert!(alice_id != bob_id);
    assert_eq!(
        gate::return_record(&g, alice_id),
        gate::new_return_record(ALICE, 10 * E8, 1, principal29(), subaccount(), 5),
    );
    assert_eq!(
        gate::return_record(&g, bob_id),
        gate::new_return_record(BOB, 20 * E8, 2, vector[0x05, 0x06], subaccount(), 5),
    );
    assert_eq!(gate::bridge_out_count(&g), 2);
    ts::return_shared(g);
    s.end();
}

// ===== Transfer id against the separate reference =====

// The expected values below come from reference/transfer_id_reference.py, which encodes
// the BCS by hand and shares no code with the Move module.
const GATE_A: address = @0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;
const GATE_B: address = @0xabababababababababababababababababababababababababababababababab;
const SENDER_A: address = @0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb;
const SENDER_B: address = @0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc;

#[test]
fun transfer_ids_match_the_independent_reference_vectors() {
    let chain_b = vector::tabulate!(32, |_| 0x11u8);
    assert_eq!(
        gate::compute_transfer_id(&chain_id(), GATE_A, SENDER_A, 1),
        x"d9f230d9c727a3d311b72b92db4a127bf94b9af42643a4ccb4cd7f98b474cea2",
    );
    assert_eq!(
        gate::compute_transfer_id(&chain_id(), GATE_A, SENDER_A, 0),
        x"85f1a74776c47a2abaa38a52a4db09b97c9b206f178ff89172c6e17ba2efda71",
    );
    assert_eq!(
        gate::compute_transfer_id(&chain_id(), GATE_A, SENDER_A, 18_446_744_073_709_551_615),
        x"2f94a6e9243dfa2926c128f2116aa532c15a82864a73b800a2944bdec05b2641",
    );
    assert_eq!(
        gate::compute_transfer_id(&chain_id(), GATE_A, SENDER_B, 1),
        x"c394fd448284bbaa385a723bdb5152b4a887370c706f15357d03bdbb122ec325",
    );
    assert_eq!(
        gate::compute_transfer_id(&chain_b, GATE_A, SENDER_A, 1),
        x"b5a581713bfc17dda3a799b9620a4d648ebcff60d4adf974b27e6c985d2176e6",
    );
    assert_eq!(
        gate::compute_transfer_id(&chain_id(), GATE_B, SENDER_A, 1),
        x"d8b7c2b90ebf5c379e16543f373045e5db63ca4afcf020a1aa483a45657c6c49",
    );
}

// ===== Sequences and the supply identity =====

#[test]
fun sequences_are_contiguous_and_minted_minus_burned_always_equals_supply() {
    let mut s = start();
    set_limits(&mut s, 1_000 * E8, 1);

    mint(&mut s, id32(1), ALICE, 100 * E8);
    mint(&mut s, id32(2), BOB, 200 * E8);
    mint(&mut s, id32(3), ALICE, 300 * E8);

    // Most recent coin first: ALICE returns 300, BOB 200, ALICE 100.
    bridge_out(&mut s, ALICE, principal29(), subaccount(), 1);
    bridge_out(&mut s, BOB, vector[0x05], subaccount(), 1);
    bridge_out(&mut s, ALICE, principal29(), subaccount(), 2);
    mint(&mut s, id32(4), BOB, 50 * E8);

    s.next_tx(PUBLISHER);
    let g = s.take_shared<Gate>();
    let gate_address = gate::gate_address(&g);
    assert_eq!(gate::mint_count(&g), 4);
    assert_eq!(gate::bridge_out_count(&g), 3);
    assert_eq!(gate::minted_total(&g), ((650 * E8) as u128));
    assert_eq!(gate::burned_total(&g), ((600 * E8) as u128));
    assert_eq!(gate::total_supply(&g), 50 * E8);
    check_accounting(&g);

    // Mint sequences 1..4 and return sequences 1..3, in order.
    assert_eq!(gate::mint_record(&g, id32(1)), gate::new_mint_record(ALICE, 100 * E8, 1));
    assert_eq!(gate::mint_record(&g, id32(2)), gate::new_mint_record(BOB, 200 * E8, 2));
    assert_eq!(gate::mint_record(&g, id32(3)), gate::new_mint_record(ALICE, 300 * E8, 3));
    assert_eq!(gate::mint_record(&g, id32(4)), gate::new_mint_record(BOB, 50 * E8, 4));
    let first = gate::compute_transfer_id(&chain_id(), gate_address, ALICE, 1);
    let second = gate::compute_transfer_id(&chain_id(), gate_address, BOB, 1);
    let third = gate::compute_transfer_id(&chain_id(), gate_address, ALICE, 2);
    assert_eq!(
        gate::return_record(&g, first),
        gate::new_return_record(ALICE, 300 * E8, 1, principal29(), subaccount(), 1),
    );
    assert_eq!(
        gate::return_record(&g, second),
        gate::new_return_record(BOB, 200 * E8, 2, vector[0x05], subaccount(), 1),
    );
    assert_eq!(
        gate::return_record(&g, third),
        gate::new_return_record(ALICE, 100 * E8, 3, principal29(), subaccount(), 2),
    );
    ts::return_shared(g);
    s.end();
}

// ===== Returns while minting is disabled =====

#[test]
fun returns_stay_available_while_minting_is_disabled() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    mint(&mut s, id32(1), ALICE, 100 * E8);
    // Minting is switched off (cap 0), and the cap is far below the supply.
    set_limits(&mut s, 0, 1);

    bridge_out(&mut s, ALICE, principal29(), subaccount(), 1);

    s.next_tx(PUBLISHER);
    let g = s.take_shared<Gate>();
    assert_eq!(gate::supply_cap(&g), 0);
    assert_eq!(gate::bridge_out_count(&g), 1);
    assert_eq!(gate::total_supply(&g), 0);
    check_accounting(&g);
    ts::return_shared(g);
    s.end();
}

#[test, expected_failure(abort_code = gate::ESupplyCapExceeded)]
fun minting_stays_disabled_after_the_cap_is_set_to_zero() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    mint(&mut s, id32(1), ALICE, 10 * E8);
    set_limits(&mut s, 0, 1);
    mint(&mut s, id32(2), ALICE, 1);
    s.end();
}

// ===== Version guard and admin migration =====

#[test, expected_failure(abort_code = gate::EWrongVersion)]
fun a_stale_gate_blocks_mint() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    force_version(&mut s, 0);
    mint(&mut s, id32(1), ALICE, 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::EWrongVersion)]
fun a_stale_gate_blocks_bridge_out() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    mint(&mut s, id32(1), ALICE, 10 * E8);
    force_version(&mut s, 0);
    bridge_out(&mut s, ALICE, principal29(), subaccount(), 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::EWrongVersion)]
fun a_stale_gate_blocks_set_limits() {
    let mut s = start();
    force_version(&mut s, 0);
    set_limits(&mut s, 100 * E8, 1);
    s.end();
}

#[test]
fun migrate_by_the_admin_cap_unblocks_a_stale_gate() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    force_version(&mut s, 0);
    migrate(&mut s);

    s.next_tx(PUBLISHER);
    let g = s.take_shared<Gate>();
    assert_eq!(gate::version(&g), gate::version_constant());
    ts::return_shared(g);

    // The gate works again, and its state was untouched by the migration.
    mint(&mut s, id32(1), ALICE, 10 * E8);
    bridge_out(&mut s, ALICE, principal29(), subaccount(), 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::ENotOlderVersion)]
fun migrate_aborts_when_the_gate_is_already_current() {
    let mut s = start();
    migrate(&mut s);
    s.end();
}

#[test, expected_failure(abort_code = gate::EWrongVersion)]
fun a_gate_newer_than_this_code_blocks_calls() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);
    force_version(&mut s, gate::version_constant() + 1);
    mint(&mut s, id32(1), ALICE, 1);
    s.end();
}

#[test, expected_failure(abort_code = gate::ENotOlderVersion)]
fun migrate_can_never_lower_the_version() {
    let mut s = start();
    force_version(&mut s, gate::version_constant() + 1);
    migrate(&mut s);
    s.end();
}

// ===== Capability authority binding =====

#[test, expected_failure(abort_code = gate::EWrongGate)]
fun a_mint_cap_from_another_gate_is_refused() {
    let (mut s, _gate_a, gate_b) = start_two_gates();
    // PUBLISHER owns gate A's caps and uses them on gate B.
    s.next_tx(PUBLISHER);
    let mut g = s.take_shared_by_id<Gate>(gate_b);
    let cap = s.take_from_sender<MintCap>();
    gate::mint(&mut g, &cap, id32(1), ALICE, 1, s.ctx());
    ts::return_shared(g);
    s.return_to_sender(cap);
    s.end();
}

#[test, expected_failure(abort_code = gate::EWrongGate)]
fun an_admin_cap_from_another_gate_cannot_set_limits() {
    let (mut s, _gate_a, gate_b) = start_two_gates();
    s.next_tx(PUBLISHER);
    let mut g = s.take_shared_by_id<Gate>(gate_b);
    let admin = s.take_from_sender<AdminCap>();
    gate::set_limits(&mut g, &admin, 1_000 * E8, 1);
    ts::return_shared(g);
    s.return_to_sender(admin);
    s.end();
}

#[test, expected_failure(abort_code = gate::EWrongGate)]
fun an_admin_cap_from_another_gate_cannot_migrate() {
    let (mut s, _gate_a, gate_b) = start_two_gates();
    // Gate B is stale, so only the cap binding can be what stops the call.
    s.next_tx(BOB);
    let mut g_b = s.take_shared_by_id<Gate>(gate_b);
    gate::set_version_for_testing(&mut g_b, 0);
    ts::return_shared(g_b);
    s.next_tx(PUBLISHER);
    let mut g = s.take_shared_by_id<Gate>(gate_b);
    let admin = s.take_from_sender<AdminCap>();
    gate::migrate(&mut g, &admin);
    ts::return_shared(g);
    s.return_to_sender(admin);
    s.end();
}

#[test]
fun each_gates_own_caps_work_and_the_gates_do_not_share_state() {
    let (mut s, gate_a, gate_b) = start_two_gates();

    // BOB published gate B, so BOB holds gate B's caps.
    s.next_tx(BOB);
    let mut g_b = s.take_shared_by_id<Gate>(gate_b);
    let admin_b = s.take_from_sender<AdminCap>();
    let cap_b = s.take_from_sender<MintCap>();
    gate::set_limits(&mut g_b, &admin_b, 10 * E8, 1);
    gate::mint(&mut g_b, &cap_b, id32(1), ALICE, 10 * E8, s.ctx());
    assert_eq!(gate::mint_count(&g_b), 1);
    ts::return_shared(g_b);
    s.return_to_sender(admin_b);
    s.return_to_sender(cap_b);

    // Gate A saw none of it.
    s.next_tx(PUBLISHER);
    let g_a = s.take_shared_by_id<Gate>(gate_a);
    assert_eq!(gate::mint_count(&g_a), 0);
    assert_eq!(gate::supply_cap(&g_a), 0);
    assert!(!gate::is_mint_consumed(&g_a, id32(1)));
    ts::return_shared(g_a);
    s.end();
}

#[test]
fun a_mint_cap_moves_by_transfer_and_the_new_holder_controls_the_gate() {
    let mut s = start();
    set_limits(&mut s, 100 * E8, 1);

    s.next_tx(PUBLISHER);
    let cap = s.take_from_sender<MintCap>();
    transfer::public_transfer(cap, BOB);

    // The previous holder no longer has it; the new holder mints.
    s.next_tx(PUBLISHER);
    assert!(!ts::has_most_recent_for_address<MintCap>(PUBLISHER));
    s.next_tx(BOB);
    let mut g = s.take_shared<Gate>();
    let cap = s.take_from_sender<MintCap>();
    gate::mint(&mut g, &cap, id32(1), ALICE, 5 * E8, s.ctx());
    assert_eq!(gate::mint_count(&g), 1);
    ts::return_shared(g);
    s.return_to_sender(cap);
    s.end();
}
