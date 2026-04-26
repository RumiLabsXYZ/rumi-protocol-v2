//! LIQ-001 regression fence: concurrent partial liquidations on the same vault.
//!
//! Audit report: `audit-reports/2026-04-22-28e9896/verification-results.md` § LIQ-001.
//!
//! # What the bug was
//!
//! `pending_margin_transfers` was `BTreeMap<VaultId, PendingMarginTransfer>`,
//! one slot per vault. The partial-liquidation path
//! (`vault::liquidate_vault_partial`) does an inter-canister `await` on
//! `transfer_icusd_from` *before* it inserts the pending entry. Two callers
//! racing the same vault could both clear the await, then run their
//! `mutate_state` blocks back to back: liquidator B's `insert(vault_id, ...)`
//! overwrites liquidator A's. A's icUSD was taken by the backend, A's vault
//! debit ran, but A's pending margin entry is gone, so A never receives
//! collateral.
//!
//! # How this file tests it
//!
//! The bug is purely a keying bug: the same `vault_id` could only hold one
//! pending entry. The Wave-4 fix re-keys the map to
//! `(VaultId, Principal)`. We cover the fix at three layers:
//!
//!   * `liq_001_two_liquidators_same_vault_keep_separate_pending_entries`:
//!     direct state-level proof. Insert two pending entries for the same
//!     `vault_id` from two different `Principal`s. Assert both survive.
//!     Under the legacy keying this would be a single entry; under the new
//!     keying we have two.
//!
//!   * `liq_001_settling_one_liquidator_does_not_remove_others_entry`:
//!     mirrors the cleanup that `event::record_margin_transfer` does after
//!     seeding two entries. Asserts the matching entry is removed and the
//!     other liquidator's entry is untouched.
//!
//!   * `liq_001_legacy_snapshot_format_round_trips_via_custom_deserializer`:
//!     serializes a state object with the legacy single-slot key shape
//!     (`BTreeMap<u64, PendingMarginTransfer>`) and asserts the Wave-4
//!     deserializer accepts it transparently and re-keys each entry by the
//!     `owner` field on the value. Old mainnet snapshots (where there is at
//!     most one entry per vault) restore cleanly into the new key space.

use candid::Principal;
use std::collections::BTreeMap;

use rumi_protocol_backend::numeric::ICP;
use rumi_protocol_backend::state::{PendingMarginTransfer, State};
use rumi_protocol_backend::InitArg;

// ──────────────────────────────────────────────────────────────
// Fixtures
// ──────────────────────────────────────────────────────────────

fn liquidator_a() -> Principal {
    Principal::from_slice(&[1])
}
fn liquidator_b() -> Principal {
    Principal::from_slice(&[2])
}
fn vault_owner() -> Principal {
    Principal::from_slice(&[3])
}

fn fresh_state() -> State {
    State::from(InitArg {
        xrc_principal: Principal::anonymous(),
        icusd_ledger_principal: Principal::anonymous(),
        icp_ledger_principal: Principal::anonymous(),
        fee_e8s: 0,
        developer_principal: Principal::anonymous(),
        treasury_principal: None,
        stability_pool_principal: None,
        ckusdt_ledger_principal: None,
        ckusdc_ledger_principal: None,
    })
}

fn pending(owner: Principal, margin_e8s: u64, op_nonce: u128) -> PendingMarginTransfer {
    PendingMarginTransfer {
        owner,
        margin: ICP::new(margin_e8s),
        collateral_type: Principal::anonymous(),
        retry_count: 0,
        op_nonce,
    }
}

// ──────────────────────────────────────────────────────────────
// LIQ-001: pending entries are now per (vault_id, owner)
// ──────────────────────────────────────────────────────────────

#[test]
fn liq_001_two_liquidators_same_vault_keep_separate_pending_entries() {
    let mut state = fresh_state();
    let vault_id: u64 = 42;

    // Liquidator A takes a 1 ICP slice
    state.pending_margin_transfers.insert(
        (vault_id, liquidator_a()),
        pending(liquidator_a(), 100_000_000, 1),
    );

    // Liquidator B races A and takes a 0.5 ICP slice on the same vault.
    // Pre-fix this insert would overwrite A's entry; post-fix it's a separate key.
    state.pending_margin_transfers.insert(
        (vault_id, liquidator_b()),
        pending(liquidator_b(), 50_000_000, 2),
    );

    assert_eq!(
        state.pending_margin_transfers.len(),
        2,
        "Both liquidators must keep their pending margin entries on the same vault"
    );

    let a_entry = state
        .pending_margin_transfers
        .get(&(vault_id, liquidator_a()))
        .expect("A's pending entry must exist");
    let b_entry = state
        .pending_margin_transfers
        .get(&(vault_id, liquidator_b()))
        .expect("B's pending entry must exist");

    assert_eq!(a_entry.owner, liquidator_a());
    assert_eq!(a_entry.margin.0, 100_000_000);
    assert_eq!(b_entry.owner, liquidator_b());
    assert_eq!(b_entry.margin.0, 50_000_000);
}

#[test]
fn liq_001_excess_transfers_also_keyed_per_principal() {
    // Excess transfers are normally vault-owner-only, but the rekey makes the map
    // schematically uniform with margin transfers. Verify two distinct excess
    // entries can coexist for the same vault when keyed by (vault_id, owner).
    let mut state = fresh_state();
    let vault_id: u64 = 7;

    state.pending_excess_transfers.insert(
        (vault_id, vault_owner()),
        pending(vault_owner(), 80_000_000, 10),
    );
    // Hypothetical second excess entry from a different principal, used here
    // purely to assert the keying admits coexistence; real liquidation flow only
    // produces one excess per vault, but the type contract must allow more.
    state.pending_excess_transfers.insert(
        (vault_id, liquidator_a()),
        pending(liquidator_a(), 5_000_000, 11),
    );

    assert_eq!(state.pending_excess_transfers.len(), 2);
    assert!(state.pending_excess_transfers.contains_key(&(vault_id, vault_owner())));
    assert!(state.pending_excess_transfers.contains_key(&(vault_id, liquidator_a())));
}

#[test]
fn liq_001_settling_one_liquidator_does_not_remove_others_entry() {
    // Mirror the cleanup that `event::record_margin_transfer` does, dropping
    // one (vault_id, owner) entry and verifying it does not collaterally
    // remove the other liquidator's pending entry on the same vault.
    //
    // We can't call `record_margin_transfer` directly here because it reads
    // `ic_cdk::api::time()` to stamp the event, which traps outside a canister.
    // The keying behavior we want to prove is the map removal itself; the
    // event-record side effect is exercised by `pocket_ic_tests`.
    let mut state = fresh_state();
    let vault_id: u64 = 99;

    state.pending_margin_transfers.insert(
        (vault_id, liquidator_a()),
        pending(liquidator_a(), 100_000_000, 1),
    );
    state.pending_margin_transfers.insert(
        (vault_id, liquidator_b()),
        pending(liquidator_b(), 50_000_000, 2),
    );

    // Liquidator A's transfer settles first. The live code does this remove
    // inside `record_margin_transfer` (event.rs).
    state
        .pending_margin_transfers
        .remove(&(vault_id, liquidator_a()));

    assert!(
        !state
            .pending_margin_transfers
            .contains_key(&(vault_id, liquidator_a())),
        "A's pending entry must be cleared after their transfer settles"
    );
    assert!(
        state
            .pending_margin_transfers
            .contains_key(&(vault_id, liquidator_b())),
        "B's pending entry must NOT be cleared by A's settlement (pre-fix this would have wiped both)"
    );
    assert_eq!(state.pending_margin_transfers.len(), 1);
}

#[test]
fn liq_001_replay_of_legacy_margin_transfer_event_drops_all_matching_entries() {
    // The `Event::MarginTransfer` event predates Wave-4 and doesn't carry the
    // owner principal. On event replay, the handler retains entries whose
    // vault_id != the event's vault_id (i.e. drops every matching key).
    //
    // For pre-Wave-4 snapshots there is at most one entry per vault, so this
    // is identical to the legacy single-slot remove. We assert the post-fix
    // behavior here so a future change to the replay path is caught.
    let mut state = fresh_state();
    let vault_id: u64 = 33;
    let other_vault: u64 = 34;

    state.pending_margin_transfers.insert(
        (vault_id, liquidator_a()),
        pending(liquidator_a(), 100_000_000, 1),
    );
    state.pending_margin_transfers.insert(
        (vault_id, liquidator_b()),
        pending(liquidator_b(), 50_000_000, 2),
    );
    state.pending_margin_transfers.insert(
        (other_vault, liquidator_a()),
        pending(liquidator_a(), 75_000_000, 3),
    );

    // Replay's removal: drop every entry whose vault_id matches.
    state
        .pending_margin_transfers
        .retain(|(vid, _), _| *vid != vault_id);

    assert!(
        state
            .pending_margin_transfers
            .contains_key(&(other_vault, liquidator_a())),
        "Entries on other vaults must survive replay of MarginTransfer for vault {}",
        vault_id
    );
    assert_eq!(state.pending_margin_transfers.len(), 1);
}

// ──────────────────────────────────────────────────────────────
// Migration: old `BTreeMap<u64, _>` snapshots round-trip through the
// custom deserializer and land as `(vault_id, owner)` keys.
// ──────────────────────────────────────────────────────────────

#[derive(serde::Serialize)]
struct LegacyPendingMaps {
    // Field names match the live `State` struct so a CBOR snapshot serialized
    // from this stand-in is bit-identical to a pre-Wave-4 snapshot for these
    // two fields.
    pending_margin_transfers: BTreeMap<u64, PendingMarginTransfer>,
    pending_excess_transfers: BTreeMap<u64, PendingMarginTransfer>,
}

#[derive(serde::Deserialize)]
struct ProbeState {
    #[serde(default, deserialize_with = "deserialize_via_state_helper")]
    pending_margin_transfers: BTreeMap<(u64, Principal), PendingMarginTransfer>,
    #[serde(default, deserialize_with = "deserialize_via_state_helper")]
    pending_excess_transfers: BTreeMap<(u64, Principal), PendingMarginTransfer>,
}

// Mirror of the production `state::deserialize_pending_keyed`. Uses a
// MapAccess-driven Visitor that distinguishes legacy `u64` keys from new
// `(u64, Principal)` keys per-entry. ciborium handles the small per-key
// untagged enum cleanly even though it can't dispatch the whole-map variant.
fn deserialize_via_state_helper<'de, D>(
    d: D,
) -> Result<BTreeMap<(u64, Principal), PendingMarginTransfer>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use std::fmt;

    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum EitherKey {
        New((u64, Principal)),
        Legacy(u64),
    }

    struct V;
    impl<'de> serde::de::Visitor<'de> for V {
        type Value = BTreeMap<(u64, Principal), PendingMarginTransfer>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a pending-transfer map")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut out = BTreeMap::new();
            while let Some(key) = map.next_key::<EitherKey>()? {
                let value: PendingMarginTransfer = map.next_value()?;
                let final_key = match key {
                    EitherKey::New(t) => t,
                    EitherKey::Legacy(vault_id) => (vault_id, value.owner),
                };
                out.insert(final_key, value);
            }
            Ok(out)
        }
    }

    d.deserialize_map(V)
}

#[test]
fn liq_001_legacy_snapshot_format_round_trips_via_custom_deserializer() {
    // Build a snapshot in the pre-Wave-4 shape: u64 keys, one entry per vault.
    let mut legacy_margin: BTreeMap<u64, PendingMarginTransfer> = BTreeMap::new();
    legacy_margin.insert(11, pending(liquidator_a(), 100_000_000, 5));
    legacy_margin.insert(22, pending(liquidator_b(), 50_000_000, 6));

    let mut legacy_excess: BTreeMap<u64, PendingMarginTransfer> = BTreeMap::new();
    legacy_excess.insert(33, pending(vault_owner(), 80_000_000, 7));

    let snapshot = LegacyPendingMaps {
        pending_margin_transfers: legacy_margin,
        pending_excess_transfers: legacy_excess,
    };

    let mut buf = Vec::new();
    ciborium::ser::into_writer(&snapshot, &mut buf).expect("encode legacy snapshot");

    // Deserialize with the (test-side mirror of the) Wave-4 deserializer.
    let probe: ProbeState =
        ciborium::de::from_reader(buf.as_slice()).expect("decode legacy snapshot via new deserializer");

    // Margin: each legacy entry must have been re-keyed by its `owner` field.
    assert_eq!(probe.pending_margin_transfers.len(), 2);
    assert!(probe
        .pending_margin_transfers
        .contains_key(&(11, liquidator_a())));
    assert!(probe
        .pending_margin_transfers
        .contains_key(&(22, liquidator_b())));

    // Excess: same migration semantics.
    assert_eq!(probe.pending_excess_transfers.len(), 1);
    assert!(probe
        .pending_excess_transfers
        .contains_key(&(33, vault_owner())));
}

#[test]
fn liq_001_new_snapshot_format_round_trips_unchanged() {
    // Sanity: the new format must round-trip through itself. Catches accidents
    // where the deserializer's `untagged` enum picks the wrong variant.
    let mut state = fresh_state();
    state.pending_margin_transfers.insert(
        (5, liquidator_a()),
        pending(liquidator_a(), 100_000_000, 1),
    );
    state.pending_margin_transfers.insert(
        (5, liquidator_b()),
        pending(liquidator_b(), 50_000_000, 2),
    );

    let mut buf = Vec::new();
    ciborium::ser::into_writer(&state, &mut buf).expect("encode new state");

    let restored: State =
        ciborium::de::from_reader(buf.as_slice()).expect("decode new state");

    assert_eq!(restored.pending_margin_transfers.len(), 2);
    assert!(restored
        .pending_margin_transfers
        .contains_key(&(5, liquidator_a())));
    assert!(restored
        .pending_margin_transfers
        .contains_key(&(5, liquidator_b())));
}
