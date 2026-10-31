/// Rumi icUSD on Sui: an escrow-backed bridge token behind one shared `Gate`.
///
/// The Rumi bridge canister mints here against icUSD it holds in escrow on the Internet
/// Computer, and a holder returns icUSD by burning it through `bridge_out`, which records
/// the ICP account that must receive the escrowed icUSD. Nothing else can mint or burn:
/// the `TreasuryCap` is wrapped inside the gate and no function releases it.
///
/// Authorities (see README.md for the full disclosure):
/// - `MintCap`: the only way to call `mint`. Bounded by `supply_cap`, which starts at 0.
/// - `AdminCap`: `set_limits` and `migrate`. It can raise `supply_cap`.
/// - `UpgradeCap` (created by the Sui publish, not by this module): whoever holds it can
///   upgrade the package under the policy it carries. A compatible upgrade can change any
///   function body, so the checks below bind the code that is published today, not a
///   future version. The version guard stops STALE package versions, not a new one.
/// - `MetadataCap`: edits name, description and icon only.
/// - No `DenyCap` exists and `make_regulated` is never called.
///
/// Returns are never gated: `bridge_out` has no pause flag and does not read
/// `supply_cap`, so holders can always return while the version guard is satisfied.
module rumi_icusd_gate::rumi_icusd_gate;

use std::bcs;
use std::hash;
use sui::coin::{Self, Coin, TreasuryCap};
use sui::coin_registry;
use sui::event;
use sui::table::{Self, Table};

// ===== Errors =====

/// `gate.version` differs from this package's `VERSION`.
const EWrongVersion: u64 = 0;
/// The 32-byte mint id was already consumed.
const EMintConsumed: u64 = 1;
/// The mint id is not exactly 32 bytes.
const EBadMintId: u64 = 2;
/// The mint amount is zero.
const EZeroAmount: u64 = 3;
/// The recipient is `@0x0` or the gate's own address.
const EBadRecipient: u64 = 4;
/// The mint would take the supply above `supply_cap`.
const ESupplyCapExceeded: u64 = 5;
/// The returned amount is below `min_bridge_out`.
const EBelowMinimum: u64 = 6;
/// The ICP principal is empty, longer than 29 bytes, or the anonymous principal `[0x04]`.
const EBadPrincipal: u64 = 7;
/// The ICP subaccount is not exactly 32 bytes.
const EBadSubaccount: u64 = 8;
/// A return with this (sender, nonce) already exists on this gate.
const EReturnUsed: u64 = 9;
/// `supply_cap` is above `MAX_SUPPLY_CAP`.
const ESupplyCapTooHigh: u64 = 10;
/// `min_bridge_out` is zero.
const EBadMinimum: u64 = 11;
/// `migrate` was called but `gate.version` is not older than `VERSION`.
const ENotOlderVersion: u64 = 12;
/// A capability that belongs to another gate was used.
const EWrongGate: u64 = 13;
/// The chain identifier is not 32 bytes or is the unset all-zero value.
const EBadChainIdentifier: u64 = 14;

// ===== Constants =====

/// The code version. `Gate.version` must equal it for every state-changing call.
const VERSION: u64 = 1;

/// 8 decimals, so one base unit equals one ICP icUSD e8s.
const DECIMALS: u8 = 8;

/// The largest settable cap. One below `U64_MAX`, the conservative reading of the
/// framework's supply bound (`balance.move` allows `U64_MAX`; the `coin::mint_balance`
/// doc comment says it aborts at `U64_MAX`).
const MAX_SUPPLY_CAP: u64 = 18_446_744_073_709_551_614;

/// The minimum return a fresh gate accepts. Raise it with `set_limits`.
const INITIAL_MIN_BRIDGE_OUT: u64 = 1;

const CHAIN_IDENTIFIER_LEN: u64 = 32;
const MINT_ID_LEN: u64 = 32;
const SUBACCOUNT_LEN: u64 = 32;
const PRINCIPAL_MAX_LEN: u64 = 29;

/// Domain separation for `transfer_id`.
const DOMAIN_RETURN: vector<u8> = b"rumi.icusd.sui.return.v1";

/// The 32-byte genesis checkpoint digest of the network this build is published to.
/// UNSET (all zero) in this source on purpose: `init` aborts on it, so a build that was
/// not bound to a network cannot be published. Set it per network before publish (see
/// README.md for the two sourced values and how they were derived).
const CHAIN_IDENTIFIER: vector<u8> =
    x"0000000000000000000000000000000000000000000000000000000000000000";

// ===== Types =====

/// One-time witness; also the coin type.
public struct RUMI_ICUSD_GATE has drop {}

/// The single shared gate. It owns the treasury capability and all accounting.
public struct Gate has key {
    id: UID,
    version: u64,
    treasury: TreasuryCap<RUMI_ICUSD_GATE>,
    chain_identifier: vector<u8>,
    supply_cap: u64,
    min_bridge_out: u64,
    /// Cumulative minted amount. Never decreases.
    minted_total: u128,
    /// Cumulative burned amount. Never decreases.
    burned_total: u128,
    mint_count: u64,
    bridge_out_count: u64,
    /// Consumed mint ids. Entries are never removed.
    mints: Table<vector<u8>, MintRecord>,
    /// Used transfer ids. Entries are never removed.
    returns: Table<vector<u8>, ReturnRecord>,
}

/// The only way to call `mint`.
public struct MintCap has key, store {
    id: UID,
    gate_id: ID,
}

/// Needed for `set_limits` and `migrate`.
public struct AdminCap has key, store {
    id: UID,
    gate_id: ID,
}

/// What a consumed mint id proves: the recipient and amount of mint number `sequence`.
public struct MintRecord has copy, drop, store {
    recipient: address,
    amount: u64,
    sequence: u64,
}

/// What a used transfer id proves, with the destination saved on chain (events expire
/// from indexers, table entries do not).
public struct ReturnRecord has copy, drop, store {
    sender: address,
    amount: u64,
    sequence: u64,
    icp_principal: vector<u8>,
    icp_subaccount: vector<u8>,
    client_nonce: u64,
}

// ===== Events =====

public struct Minted has copy, drop {
    mint_id: vector<u8>,
    recipient: address,
    amount: u64,
    sequence: u64,
    minted_total: u128,
    supply: u64,
}

public struct BridgeOut has copy, drop {
    transfer_id: vector<u8>,
    sender: address,
    amount: u64,
    sequence: u64,
    icp_principal: vector<u8>,
    icp_subaccount: vector<u8>,
    client_nonce: u64,
    burned_total: u128,
    supply: u64,
}

public struct LimitsSet has copy, drop {
    supply_cap: u64,
    min_bridge_out: u64,
}

// ===== Publish =====

fun init(otw: RUMI_ICUSD_GATE, ctx: &mut TxContext) {
    init_gate(otw, CHAIN_IDENTIFIER, ctx);
}

#[allow(lint(self_transfer))]
/// Creates the currency, wraps the treasury cap into the shared gate and hands the
/// capabilities to the publisher. `supply_cap` starts at 0, so no mint can succeed until
/// an `AdminCap` holder raises it: that is the on-chain DISABLED state.
fun init_gate(otw: RUMI_ICUSD_GATE, chain_identifier: vector<u8>, ctx: &mut TxContext) {
    assert!(chain_identifier.length() == CHAIN_IDENTIFIER_LEN, EBadChainIdentifier);
    assert!(!chain_identifier.all!(|b| *b == 0), EBadChainIdentifier);

    let (initializer, treasury) = coin_registry::new_currency_with_otw(
        otw,
        DECIMALS,
        b"icUSD".to_string(),
        b"Rumi icUSD (escrow-backed)".to_string(),
        b"Escrow-backed Rumi icUSD on Sui. Every unit is backed one for one by icUSD held in escrow on the Internet Computer. Not the native ICP icUSD ledger token.".to_string(),
        b"".to_string(),
        ctx,
    );
    // Unregulated, unknown supply: the initializer is finalized as is and `make_regulated`
    // is never called. The currency is registered with the coin registry afterwards by a
    // separate `finalize_registration` transaction.
    let metadata_cap = initializer.finalize(ctx);

    let gate = Gate {
        id: object::new(ctx),
        version: VERSION,
        treasury,
        chain_identifier,
        supply_cap: 0,
        min_bridge_out: INITIAL_MIN_BRIDGE_OUT,
        minted_total: 0,
        burned_total: 0,
        mint_count: 0,
        bridge_out_count: 0,
        mints: table::new(ctx),
        returns: table::new(ctx),
    };
    let gate_id = object::id(&gate);
    let publisher = ctx.sender();
    transfer::public_transfer(MintCap { id: object::new(ctx), gate_id }, publisher);
    transfer::public_transfer(AdminCap { id: object::new(ctx), gate_id }, publisher);
    transfer::public_transfer(metadata_cap, publisher);
    transfer::share_object(gate);
}

// ===== ICP to Sui =====

/// Mint `amount` to `recipient` against the 32-byte `mint_id`, which can be used once.
public fun mint(
    gate: &mut Gate,
    cap: &MintCap,
    mint_id: vector<u8>,
    recipient: address,
    amount: u64,
    ctx: &mut TxContext,
) {
    assert_version(gate);
    assert!(cap.gate_id == object::id(gate), EWrongGate);
    assert!(mint_id.length() == MINT_ID_LEN, EBadMintId);
    assert!(!gate.mints.contains(mint_id), EMintConsumed);
    assert!(amount > 0, EZeroAmount);
    assert!(recipient != @0x0 && recipient != gate.id.to_address(), EBadRecipient);
    let supply = coin::total_supply(&gate.treasury);
    assert!((supply as u128) + (amount as u128) <= (gate.supply_cap as u128), ESupplyCapExceeded);

    gate.mint_count = gate.mint_count + 1;
    gate.minted_total = gate.minted_total + (amount as u128);
    let sequence = gate.mint_count;
    gate.mints.add(mint_id, MintRecord { recipient, amount, sequence });

    let minted = coin::mint(&mut gate.treasury, amount, ctx);
    transfer::public_transfer(minted, recipient);

    event::emit(Minted {
        mint_id,
        recipient,
        amount,
        sequence,
        minted_total: gate.minted_total,
        supply: coin::total_supply(&gate.treasury),
    });
}

// ===== Sui to ICP =====

/// Burn `coin` and record that the same amount of escrowed icUSD is owed to the ICP
/// account (`icp_principal`, `icp_subaccount`). `client_nonce` makes the transfer id
/// unique per sender. There is no pause check and no cap check: holders can always
/// return while the version guard is satisfied.
public fun bridge_out(
    gate: &mut Gate,
    coin: Coin<RUMI_ICUSD_GATE>,
    icp_principal: vector<u8>,
    icp_subaccount: vector<u8>,
    client_nonce: u64,
    ctx: &mut TxContext,
) {
    assert_version(gate);
    let amount = coin.value();
    // `min_bridge_out >= 1` always holds, so this also rejects a zero coin.
    assert!(amount >= gate.min_bridge_out, EBelowMinimum);
    let principal_len = icp_principal.length();
    assert!(principal_len >= 1 && principal_len <= PRINCIPAL_MAX_LEN, EBadPrincipal);
    assert!(!(principal_len == 1 && icp_principal[0] == 0x04), EBadPrincipal);
    assert!(icp_subaccount.length() == SUBACCOUNT_LEN, EBadSubaccount);

    let sender = ctx.sender();
    let transfer_id = compute_transfer_id(
        &gate.chain_identifier,
        gate.id.to_address(),
        sender,
        client_nonce,
    );
    assert!(!gate.returns.contains(transfer_id), EReturnUsed);

    coin::burn(&mut gate.treasury, coin);
    gate.bridge_out_count = gate.bridge_out_count + 1;
    gate.burned_total = gate.burned_total + (amount as u128);
    let sequence = gate.bridge_out_count;
    gate.returns.add(
        transfer_id,
        ReturnRecord { sender, amount, sequence, icp_principal, icp_subaccount, client_nonce },
    );

    event::emit(BridgeOut {
        transfer_id,
        sender,
        amount,
        sequence,
        icp_principal,
        icp_subaccount,
        client_nonce,
        burned_total: gate.burned_total,
        supply: coin::total_supply(&gate.treasury),
    });
}

/// The id `bridge_out` assigns to (`sender`, `client_nonce`) on this chain and gate:
/// sha2-256 over the BCS of the domain, the chain identifier, the gate address, the
/// sender and the nonce. BCS of a `vector<u8>` is length-prefixed, an address is its 32
/// raw bytes and a `u64` is 8 little-endian bytes.
public fun compute_transfer_id(
    chain_identifier: &vector<u8>,
    gate: address,
    sender: address,
    client_nonce: u64,
): vector<u8> {
    let domain = DOMAIN_RETURN;
    let mut bytes = bcs::to_bytes(&domain);
    bytes.append(bcs::to_bytes(chain_identifier));
    bytes.append(bcs::to_bytes(&gate));
    bytes.append(bcs::to_bytes(&sender));
    bytes.append(bcs::to_bytes(&client_nonce));
    hash::sha2_256(bytes)
}

// ===== Administration =====

/// Set the mint cap and the minimum return. `supply_cap = 0` stops minting; it never
/// stops returns. Lowering the cap below the current supply is allowed and simply blocks
/// every further mint.
public fun set_limits(
    gate: &mut Gate,
    admin: &AdminCap,
    supply_cap: u64,
    min_bridge_out: u64,
) {
    assert_version(gate);
    assert!(admin.gate_id == object::id(gate), EWrongGate);
    assert!(supply_cap <= MAX_SUPPLY_CAP, ESupplyCapTooHigh);
    assert!(min_bridge_out >= 1, EBadMinimum);
    gate.supply_cap = supply_cap;
    gate.min_bridge_out = min_bridge_out;
    event::emit(LimitsSet { supply_cap, min_bridge_out });
}

/// Move a gate that an older package version created up to this package's `VERSION`.
/// Run it as its own transaction after each upgrade. It does not check the version
/// guard (every other call is blocked until it runs) and it cannot lower the version.
public fun migrate(gate: &mut Gate, admin: &AdminCap) {
    assert!(admin.gate_id == object::id(gate), EWrongGate);
    assert!(gate.version < VERSION, ENotOlderVersion);
    gate.version = VERSION;
}

// ===== Internal =====

fun assert_version(gate: &Gate) {
    assert!(gate.version == VERSION, EWrongVersion);
}

// ===== Test support =====

#[test_only]
public fun init_for_testing(chain_identifier: vector<u8>, ctx: &mut TxContext) {
    init_gate(RUMI_ICUSD_GATE {}, chain_identifier, ctx);
}

/// Runs the real `init`, with the shipped (unset) chain identifier.
#[test_only]
public fun init_shipped_for_testing(ctx: &mut TxContext) {
    init(RUMI_ICUSD_GATE {}, ctx);
}

#[test_only]
public fun version_constant(): u64 { VERSION }

#[test_only]
public fun max_supply_cap(): u64 { MAX_SUPPLY_CAP }

#[test_only]
public fun set_version_for_testing(gate: &mut Gate, version: u64) {
    gate.version = version;
}

#[test_only]
public fun version(gate: &Gate): u64 { gate.version }

#[test_only]
public fun gate_address(gate: &Gate): address { gate.id.to_address() }

#[test_only]
public fun chain_identifier(gate: &Gate): vector<u8> { gate.chain_identifier }

#[test_only]
public fun supply_cap(gate: &Gate): u64 { gate.supply_cap }

#[test_only]
public fun min_bridge_out(gate: &Gate): u64 { gate.min_bridge_out }

#[test_only]
public fun minted_total(gate: &Gate): u128 { gate.minted_total }

#[test_only]
public fun burned_total(gate: &Gate): u128 { gate.burned_total }

#[test_only]
public fun mint_count(gate: &Gate): u64 { gate.mint_count }

#[test_only]
public fun bridge_out_count(gate: &Gate): u64 { gate.bridge_out_count }

#[test_only]
public fun total_supply(gate: &Gate): u64 { coin::total_supply(&gate.treasury) }

#[test_only]
public fun is_mint_consumed(gate: &Gate, mint_id: vector<u8>): bool { gate.mints.contains(mint_id) }

#[test_only]
public fun is_return_used(gate: &Gate, transfer_id: vector<u8>): bool {
    gate.returns.contains(transfer_id)
}

#[test_only]
public fun mint_record(gate: &Gate, mint_id: vector<u8>): MintRecord { *gate.mints.borrow(mint_id) }

#[test_only]
public fun return_record(gate: &Gate, transfer_id: vector<u8>): ReturnRecord {
    *gate.returns.borrow(transfer_id)
}

#[test_only]
public fun new_mint_record(recipient: address, amount: u64, sequence: u64): MintRecord {
    MintRecord { recipient, amount, sequence }
}

#[test_only]
public fun new_return_record(
    sender: address,
    amount: u64,
    sequence: u64,
    icp_principal: vector<u8>,
    icp_subaccount: vector<u8>,
    client_nonce: u64,
): ReturnRecord {
    ReturnRecord { sender, amount, sequence, icp_principal, icp_subaccount, client_nonce }
}

#[test_only]
public fun new_minted_event(
    mint_id: vector<u8>,
    recipient: address,
    amount: u64,
    sequence: u64,
    minted_total: u128,
    supply: u64,
): Minted {
    Minted { mint_id, recipient, amount, sequence, minted_total, supply }
}

#[test_only]
public fun new_bridge_out_event(
    transfer_id: vector<u8>,
    sender: address,
    amount: u64,
    sequence: u64,
    icp_principal: vector<u8>,
    icp_subaccount: vector<u8>,
    client_nonce: u64,
    burned_total: u128,
    supply: u64,
): BridgeOut {
    BridgeOut {
        transfer_id,
        sender,
        amount,
        sequence,
        icp_principal,
        icp_subaccount,
        client_nonce,
        burned_total,
        supply,
    }
}

#[test_only]
public fun new_limits_set_event(supply_cap: u64, min_bridge_out: u64): LimitsSet {
    LimitsSet { supply_cap, min_bridge_out }
}
