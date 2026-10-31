#!/usr/bin/env python3
"""Independent golden-vector reference for the Sui bridge foundation (BB-52).

Standard library only, and it shares no code with the Rust crate. Everything the canister
computes is written out by hand here from the pinned official sources (see the crate's
README section in lib.rs for the citations):

  * BCS: ULEB128 lengths, little-endian integers, 32-byte addresses, length-prefixed digests.
  * TransactionData::V1 for the mint programmable transaction (sui-types transaction.rs at
    mainnet-v1.82.3: TransactionData, TransactionDataV1, TransactionKind, ProgrammableTransaction,
    CallArg, ObjectArg, SharedObjectMutability, Command, ProgrammableMoveCall, Argument, GasData,
    TransactionExpiration).
  * The signing digest: BLAKE2b-256 over [0,0,0] ++ BCS(TransactionData) (intent framing,
    sui-sdk-types hash.rs `Transaction::signing_digest`; sui-types crypto.rs `new_secure`).
  * The typed transaction digest: BLAKE2b-256 over b"TransactionData::" ++ BCS(TransactionData)
    (sui-sdk-types hash.rs `Transaction::digest`; sui-types crypto.rs `Signable`/`default_hash`).
  * Ed25519 (RFC 8032, section 6 reference algorithm, written out below with its own test vector)
    and the Sui wire signature flag(0x00) ++ signature(64) ++ public key(32), base64.
  * Sui addresses: BLAKE2b-256(0x00 ++ ed25519 public key).

Run: python3 -I sui_golden_reference.py
The printed values are pinned in src/vectors.rs tests of the crate.
"""
import base64
import hashlib

# ---------------------------------------------------------------------------
# Ed25519, RFC 8032 section 6 (reference implementation, big-integer arithmetic)
# ---------------------------------------------------------------------------
P = 2**255 - 19
Q = 2**252 + 27742317777372353535851937790883648493


def sha512(s: bytes) -> bytes:
    return hashlib.sha512(s).digest()


def inv(x: int) -> int:
    return pow(x, P - 2, P)


D = -121665 * inv(121666) % P
SQRT_M1 = pow(2, (P - 1) // 4, P)


def point_add(a, b):
    A = (a[1] - a[0]) * (b[1] - b[0]) % P
    B = (a[1] + a[0]) * (b[1] + b[0]) % P
    C = 2 * a[3] * b[3] * D % P
    DD = 2 * a[2] * b[2] % P
    E, F, G, H = B - A, DD - C, DD + C, B + A
    return (E * F % P, G * H % P, F * G % P, E * H % P)


def point_mul(s, pt):
    r = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            r = point_add(r, pt)
        pt = point_add(pt, pt)
        s >>= 1
    return r


def point_equal(a, b):
    if (a[0] * b[2] - b[0] * a[2]) % P != 0:
        return False
    if (a[1] * b[2] - b[1] * a[2]) % P != 0:
        return False
    return True


def recover_x(y, sign):
    if y >= P:
        return None
    x2 = (y * y - 1) * inv(D * y * y + 1) % P
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (P + 3) // 8, P)
    if (x * x - x2) % P != 0:
        x = x * SQRT_M1 % P
    if (x * x - x2) % P != 0:
        return None
    if (x & 1) != sign:
        x = P - x
    return x


G_Y = 4 * inv(5) % P
G_X = recover_x(G_Y, 0)
G = (G_X, G_Y, 1, G_X * G_Y % P)


def point_compress(pt):
    zinv = inv(pt[2])
    x = pt[0] * zinv % P
    y = pt[1] * zinv % P
    return int.to_bytes(y | ((x & 1) << 255), 32, "little")


def secret_expand(secret: bytes):
    assert len(secret) == 32
    h = sha512(secret)
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return a, h[32:]


def public_key(secret: bytes) -> bytes:
    a, _ = secret_expand(secret)
    return point_compress(point_mul(a, G))


def sign(secret: bytes, msg: bytes) -> bytes:
    a, prefix = secret_expand(secret)
    A = point_compress(point_mul(a, G))
    r = int.from_bytes(sha512(prefix + msg), "little") % Q
    R = point_mul(r, G)
    Rs = point_compress(R)
    h = int.from_bytes(sha512(Rs + A + msg), "little") % Q
    s = (r + h * a) % Q
    return Rs + int.to_bytes(s, 32, "little")


def self_test_rfc8032():
    # RFC 8032 section 7.1, TEST 1 (empty message).
    secret = bytes.fromhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60")
    pub = bytes.fromhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
    sig = bytes.fromhex(
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155"
        "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
    )
    assert public_key(secret) == pub
    assert sign(secret, b"") == sig


self_test_rfc8032()

# ---------------------------------------------------------------------------
# Hashes and encodings
# ---------------------------------------------------------------------------
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def blake2b256(b: bytes) -> bytes:
    return hashlib.blake2b(b, digest_size=32).digest()


def b58encode(b: bytes) -> str:
    n = int.from_bytes(b, "big")
    out = ""
    while n > 0:
        n, r = divmod(n, 58)
        out = B58[r] + out
    pad = len(b) - len(b.lstrip(b"\x00"))
    return "1" * pad + out


def sui_address(pub: bytes) -> bytes:
    return blake2b256(b"\x00" + pub)


# ---------------------------------------------------------------------------
# BCS
# ---------------------------------------------------------------------------
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


def u64(n: int) -> bytes:
    return n.to_bytes(8, "little")


def u16(n: int) -> bytes:
    return n.to_bytes(2, "little")


def bcs_vec_u8(b: bytes) -> bytes:
    return uleb(len(b)) + b


def bcs_string(s: str) -> bytes:
    return bcs_vec_u8(s.encode())


def object_ref(oid: bytes, version: int, digest: bytes) -> bytes:
    assert len(oid) == 32 and len(digest) == 32
    # ObjectDigest is serialized as bytes: a 0x20 length prefix, then 32 bytes.
    return oid + u64(version) + uleb(32) + digest


# CallArg: Pure = 0, Object = 1, FundsWithdrawal = 2.
# ObjectArg: ImmOrOwnedObject = 0, SharedObject = 1, Receiving = 2.
# SharedObjectMutability: Immutable = 0, Mutable = 1, NonExclusiveWrite = 2.
def call_arg_pure(raw: bytes) -> bytes:
    return uleb(0) + bcs_vec_u8(raw)


def call_arg_owned(ref: bytes) -> bytes:
    return uleb(1) + uleb(0) + ref


def call_arg_shared(oid: bytes, initial_version: int, mutability: int) -> bytes:
    return uleb(1) + uleb(1) + oid + u64(initial_version) + uleb(mutability)


def argument_input(i: int) -> bytes:
    return uleb(1) + u16(i)  # Argument: GasCoin 0, Input 1, Result 2, NestedResult 3


def move_call(package: bytes, module: str, function: str, type_args: int, args: list) -> bytes:
    assert type_args == 0
    return (
        uleb(0)  # Command::MoveCall
        + package
        + bcs_string(module)
        + bcs_string(function)
        + uleb(0)  # no type arguments
        + uleb(len(args))
        + b"".join(argument_input(i) for i in args)
    )


def transaction_data_v1(inputs: list, commands: list, sender: bytes, payment: list, gas_owner: bytes,
                        price: int, budget: int, expiration: bytes) -> bytes:
    kind = uleb(0) + uleb(len(inputs)) + b"".join(inputs) + uleb(len(commands)) + b"".join(commands)
    gas = uleb(len(payment)) + b"".join(payment) + gas_owner + u64(price) + u64(budget)
    return uleb(0) + kind + sender + gas + expiration  # TransactionData::V1 = 0


def expiration_epoch(epoch: int) -> bytes:
    return uleb(1) + u64(epoch)  # None = 0, Epoch = 1


def signing_digest(tx_bcs: bytes) -> bytes:
    return blake2b256(bytes([0, 0, 0]) + tx_bcs)  # IntentScope::TransactionData, V0, AppId::Sui


def transaction_digest(tx_bcs: bytes) -> bytes:
    return blake2b256(b"TransactionData::" + tx_bcs)


def wire_signature(sig: bytes, pub: bytes) -> bytes:
    assert len(sig) == 64 and len(pub) == 32
    return b"\x00" + sig + pub  # Ed25519 flag 0x00


# ---------------------------------------------------------------------------
# The fixed sample the Rust tests pin
# ---------------------------------------------------------------------------
SEED_TREASURY = bytes([0x11] * 32)
SEED_GAS = bytes([0x22] * 32)
SEED_CUSTODY = bytes([0x33] * 32)

CHAIN_A = bytes(range(1, 33))  # 0x01 .. 0x20 (the Move test chain identifier)
# Sui testnet genesis checkpoint digest (base58 69WiPg3DAQiwdxfncX6wYQ2siKwAe6L9BZthQea3JNMD, sui-types digests.rs)
TESTNET_CHAIN = bytes.fromhex("4c78adacf2a2f5ad80f27ed7d54aa69d3a78f1ca67fdef9ecf5754f5b8bb77b0")
PACKAGE = bytes([0x70] * 32)
GATE_ID = bytes([0x6A] * 32)
GATE_INITIAL_SHARED_VERSION = 42
MINT_CAP_ID = bytes([0x6D] * 32)
MINT_CAP_VERSION = 77
MINT_CAP_DIGEST = bytes([0x55] * 32)
GAS_COIN_ID = bytes([0x67] * 32)
GAS_COIN_VERSION = 9
GAS_COIN_DIGEST = bytes([0x56] * 32)
GAS_PRICE = 1000
GAS_BUDGET = 5_000_000
EPOCH = 123
MINT_ID = bytes(range(0x80, 0xA0))
RECIPIENT = bytes([0x8B] * 32)
AMOUNT = 250 * 10**8


def mint_transaction():
    pub_t, pub_g = public_key(SEED_TREASURY), public_key(SEED_GAS)
    sender, gas_owner = sui_address(pub_t), sui_address(pub_g)
    inputs = [
        call_arg_shared(GATE_ID, GATE_INITIAL_SHARED_VERSION, 1),
        call_arg_owned(object_ref(MINT_CAP_ID, MINT_CAP_VERSION, MINT_CAP_DIGEST)),
        call_arg_pure(bcs_vec_u8(MINT_ID)),  # Move `vector<u8>` argument
        call_arg_pure(RECIPIENT),  # Move `address` argument
        call_arg_pure(u64(AMOUNT)),  # Move `u64` argument
    ]
    commands = [move_call(PACKAGE, "rumi_icusd_gate", "mint", 0, [0, 1, 2, 3, 4])]
    tx = transaction_data_v1(
        inputs, commands, sender,
        [object_ref(GAS_COIN_ID, GAS_COIN_VERSION, GAS_COIN_DIGEST)], gas_owner,
        GAS_PRICE, GAS_BUDGET, expiration_epoch(EPOCH),
    )
    return tx, sender, gas_owner, pub_t, pub_g


# ---------------------------------------------------------------------------
# Gate object BCS (Move struct order of move/rumi_icusd_gate at 23f75ef1)
# ---------------------------------------------------------------------------
def gate_bcs(version=1, supply=400 * 10**8, chain=TESTNET_CHAIN, supply_cap=1000 * 10**8, min_out=1,
             minted=650 * 10**8, burned=250 * 10**8, mint_count=4, out_count=3) -> bytes:
    uid = lambda b: bytes([b]) * 32
    return (
        uid(0x6A)  # id: UID
        + u64(version)
        + uid(0xDD) + u64(supply)  # treasury: TreasuryCap { id: UID, total_supply: Supply { value } }
        + bcs_vec_u8(chain)
        + u64(supply_cap) + u64(min_out)
        + minted.to_bytes(16, "little") + burned.to_bytes(16, "little")
        + u64(mint_count) + u64(out_count)
        + uid(0xE1) + u64(mint_count)  # mints: Table { id, size }
        + uid(0xE2) + u64(out_count)  # returns: Table { id, size }
    )


def mint_record_bcs(recipient: bytes, amount: int, sequence: int) -> bytes:
    return recipient + u64(amount) + u64(sequence)


def return_record_bcs(sender: bytes, amount: int, sequence: int, principal: bytes, subaccount: bytes,
                      nonce: int) -> bytes:
    return sender + u64(amount) + u64(sequence) + bcs_vec_u8(principal) + bcs_vec_u8(subaccount) + u64(nonce)


# ---------------------------------------------------------------------------
# Canister-side ids (domain, then u32-big-endian length-prefixed fields)
# ---------------------------------------------------------------------------
def lp(field: bytes) -> bytes:
    return len(field).to_bytes(4, "big") + field


def mint_id(chain: bytes, package: bytes, gate: bytes, salt: bytes, block: int, recipient: bytes,
            amount: int) -> bytes:
    pre = b"rumi.icusd.sui.mint.v1"
    for f in (chain, package, gate, salt, u64(block)[::-1], recipient, u64(amount)[::-1]):
        pre += lp(f)
    return hashlib.sha256(pre).digest()


def instance_salt(canister: bytes, install_ns: int) -> bytes:
    return hashlib.sha256(b"rumi-icusd-sui-bridge:instance-salt:v1" + canister + install_ns.to_bytes(8, "big")).digest()


def transfer_id(chain: bytes, gate: bytes, sender: bytes, nonce: int) -> bytes:
    pre = bcs_vec_u8(b"rumi.icusd.sui.return.v1") + bcs_vec_u8(chain) + gate + sender + u64(nonce)
    return hashlib.sha256(pre).digest()


def h(b: bytes) -> str:
    return b.hex()


if __name__ == "__main__":
    tx, sender, gas_owner, pub_t, pub_g = mint_transaction()
    sd, td = signing_digest(tx), transaction_digest(tx)
    sig_t, sig_g = sign(SEED_TREASURY, sd), sign(SEED_GAS, sd)
    print("pub_treasury        ", h(pub_t))
    print("pub_gas             ", h(pub_g))
    print("pub_custody         ", h(public_key(SEED_CUSTODY)))
    print("addr_treasury       ", h(sender))
    print("addr_gas            ", h(gas_owner))
    print("addr_custody        ", h(sui_address(public_key(SEED_CUSTODY))))
    print("tx_len              ", len(tx))
    print("tx_bcs              ", h(tx))
    print("signing_digest      ", h(sd))
    print("transaction_digest  ", h(td))
    print("transaction_digest58", b58encode(td))
    print("sig_treasury        ", h(sig_t))
    print("sig_gas             ", h(sig_g))
    print("wire_treasury_b64   ", base64.b64encode(wire_signature(sig_t, pub_t)).decode())
    print("wire_gas_b64        ", base64.b64encode(wire_signature(sig_g, pub_g)).decode())
    print("gate_bcs            ", h(gate_bcs()))
    print("gate_bcs_b64        ", base64.b64encode(gate_bcs()).decode())
    print("gate_bcs_version2   ", h(gate_bcs(version=2)))
    print("mint_record_bcs     ", h(mint_record_bcs(RECIPIENT, AMOUNT, 4)))
    principal = bytes(range(1, 30))
    subaccount = bytes(range(100, 132))
    print("return_record_bcs   ", h(return_record_bcs(bytes([0xBB] * 32), AMOUNT, 1, principal, subaccount, 42)))
    print("dynfield_name_b64   ", base64.b64encode(bcs_vec_u8(MINT_ID)).decode())
    salt = instance_salt(bytes([0xBB] * 10), 1_700_000_000_000_000_000)
    print("instance_salt       ", h(salt))
    print("mint_id             ", h(mint_id(CHAIN_A, PACKAGE, GATE_ID, salt, 12345, RECIPIENT, AMOUNT)))
    for name, args in (
        ("tid_v1", (CHAIN_A, bytes([0xAA] * 32), bytes([0xBB] * 32), 1)),
        ("tid_v3", (CHAIN_A, bytes([0xAA] * 32), bytes([0xBB] * 32), 2**64 - 1)),
    ):
        print(name, h(transfer_id(*args)))
