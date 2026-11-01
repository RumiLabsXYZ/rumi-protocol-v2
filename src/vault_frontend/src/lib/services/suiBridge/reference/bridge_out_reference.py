"""Independent reference for the Sui `bridge_out` return transaction (BB-54).

Standard library only. It writes the BCS of a `TransactionData::V1` by hand from the Sui transaction layout
(`TransactionData`, `TransactionKind::ProgrammableTransaction`, `CallArg`, `ObjectArg`, `Command`, `Argument`,
`GasData`, `TransactionExpiration`), and the Gate's transfer id from `compute_transfer_id` in
move/rumi_icusd_gate/sources/rumi_icusd_gate.move at 23f75ef1. It shares no code with the TypeScript under test and
does not import the Sui SDK. `reference.spec.ts` pins the values it prints.

Rerun:  python3 -I bridge_out_reference.py

Layout notes (BCS: integers little-endian, lengths ULEB128, enums by variant index):
  TransactionData      = V1(0) { kind, sender: [u8;32], gas_data, expiration }
  TransactionKind      = ProgrammableTransaction(0) { inputs: vec<CallArg>, commands: vec<Command> }
  CallArg              = Pure(0) vec<u8> | Object(1) ObjectArg
  ObjectArg            = ImmOrOwnedObject(0) (id32, version u64, digest vec<u8>) | SharedObject(1) (id32, initial_shared_version u64, mutable bool)
  Command              = MoveCall(0) | SplitCoins(2) | MergeCoins(3)
  ProgrammableMoveCall = package32, module string, function string, type_arguments vec<TypeTag>, arguments vec<Argument>
  Argument             = GasCoin(0) | Input(1) u16 | Result(2) u16 | NestedResult(3) (u16, u16)
  GasData              = payment vec<ObjectRef>, owner [u8;32], price u64, budget u64
  TransactionExpiration= None(0) | Epoch(1) u64
"""

import hashlib

DOMAIN_RETURN = b"rumi.icusd.sui.return.v1"


def uleb(n: int) -> bytes:
    out = bytearray()
    while True:
        byte = n & 0x7F
        n >>= 7
        if n:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def u16(n: int) -> bytes:
    return n.to_bytes(2, "little")


def u64(n: int) -> bytes:
    return n.to_bytes(8, "little")


def vec_u8(b: bytes) -> bytes:
    return uleb(len(b)) + b


def string(s: str) -> bytes:
    return vec_u8(s.encode())


def transfer_id(chain: bytes, gate: bytes, sender: bytes, nonce: int) -> bytes:
    return hashlib.sha256(string(DOMAIN_RETURN.decode()) + vec_u8(chain) + gate + sender + u64(nonce)).digest()


def object_ref(oid: bytes, version: int, digest: bytes) -> bytes:
    return oid + u64(version) + vec_u8(digest)


def pure(raw: bytes) -> bytes:
    return b"\x00" + vec_u8(raw)


def owned(oid: bytes, version: int, digest: bytes) -> bytes:
    return b"\x01" + b"\x00" + object_ref(oid, version, digest)


def shared(oid: bytes, initial_version: int, mutable: bool) -> bytes:
    return b"\x01" + b"\x01" + oid + u64(initial_version) + (b"\x01" if mutable else b"\x00")


def arg_input(i: int) -> bytes:
    return b"\x01" + u16(i)


def arg_nested(cmd: int, idx: int) -> bytes:
    return b"\x03" + u16(cmd) + u16(idx)


def vec(items: list) -> bytes:
    return uleb(len(items)) + b"".join(items)


def move_call(package: bytes, module: str, function: str, args: list) -> bytes:
    return b"\x00" + package + string(module) + string(function) + uleb(0) + vec(args)


def split_coins(coin: bytes, amounts: list) -> bytes:
    return b"\x02" + coin + vec(amounts)


def merge_coins(dest: bytes, sources: list) -> bytes:
    return b"\x03" + dest + vec(sources)


def transaction_data(inputs, commands, sender, payment, price, budget, epoch) -> bytes:
    kind = b"\x00" + vec(inputs) + vec(commands)
    gas = vec([object_ref(*p) for p in payment]) + sender + u64(price) + u64(budget)
    return b"\x00" + kind + sender + gas + b"\x01" + u64(epoch)


def bridge_out(package, gate, gate_version, sender, coins, amount, principal, subaccount, nonce, gas_payment, price, budget, epoch):
    """coins: list of (id, version, digest, balance). Inputs are created in the order gate, coins, [amount], principal, subaccount, nonce."""
    inputs = [shared(gate, gate_version, True)]
    for c in coins:
        inputs.append(owned(c[0], c[1], c[2]))
    total = sum(c[3] for c in coins)
    commands = []
    primary = arg_input(1)
    if len(coins) > 1:
        commands.append(merge_coins(primary, [arg_input(1 + i) for i in range(1, len(coins))]))
    coin_arg = primary
    if total > amount:
        amount_index = len(inputs)
        inputs.append(pure(u64(amount)))
        commands.append(split_coins(primary, [arg_input(amount_index)]))
        coin_arg = arg_nested(len(commands) - 1, 0)
    p = len(inputs)
    inputs += [pure(vec_u8(principal)), pure(vec_u8(subaccount)), pure(u64(nonce))]
    commands.append(move_call(package, "rumi_icusd_gate", "bridge_out", [arg_input(0), coin_arg, arg_input(p), arg_input(p + 1), arg_input(p + 2)]))
    return transaction_data(inputs, commands, sender, [gas_payment], price, budget, epoch)


if __name__ == "__main__":
    PACKAGE = bytes([0x70]) * 32
    GATE = bytes([0x6A]) * 32
    SENDER = bytes([0xBB]) * 32
    GAS = (bytes([0x67]) * 32, 9, bytes([0x56]) * 32)
    COIN_A = (bytes([0xC1]) * 32, 7, bytes([0x57]) * 32, 250 * 10**8)
    COIN_B = (bytes([0xC2]) * 32, 11, bytes([0x58]) * 32, 100 * 10**8)
    PRINCIPAL = bytes(range(1, 11))  # a 10-byte principal
    ZERO = bytes(32)
    SUB = bytes(range(0xA0, 0xC0))
    AMOUNT = 250 * 10**8
    TESTNET = bytes.fromhex("4c78adacf2a2f5ad80f27ed7d54aa69d3a78f1ca67fdef9ecf5754f5b8bb77b0")

    def show(name, data):
        print(f"{name} bcs: {data.hex()}")

    # A: one coin of exactly the amount: no merge, no split.
    show("A exact", bridge_out(PACKAGE, GATE, 42, SENDER, [COIN_A], AMOUNT, PRINCIPAL, ZERO, 1, GAS, 1000, 5_000_000, 123))
    # B: one larger coin: split the amount off.
    show("B split", bridge_out(PACKAGE, GATE, 42, SENDER, [(COIN_A[0], 7, COIN_A[2], 400 * 10**8)], AMOUNT, PRINCIPAL, SUB, 18446744073709551615, GAS, 1000, 5_000_000, 123))
    # C: two coins merged, then the amount split off.
    show("C merge+split", bridge_out(PACKAGE, GATE, 42, SENDER, [COIN_B, COIN_A], 300 * 10**8, PRINCIPAL, ZERO, 0, GAS, 1000, 5_000_000, 123))
    # D: two coins merged to exactly the amount: merge, no split.
    show("D merge exact", bridge_out(PACKAGE, GATE, 42, SENDER, [COIN_A, COIN_B], 350 * 10**8, PRINCIPAL, ZERO, 5, GAS, 1000, 5_000_000, 123))
    # The transfer id for the same sender/gate on the testnet identifier (and on the sequence 1..32 test identifier).
    print("transfer_id testnet gate=6a.. sender=bb.. nonce=1:", transfer_id(TESTNET, GATE, SENDER, 1).hex())
    print("transfer_id testnet gate=6a.. sender=bb.. nonce=u64max:", transfer_id(TESTNET, GATE, SENDER, 18446744073709551615).hex())
