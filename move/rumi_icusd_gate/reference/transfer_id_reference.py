#!/usr/bin/env python3
"""Independent reference for the rumi_icusd_gate transfer id.

It shares no code with the Move module and uses only the standard library. The BCS
encoding is written out by hand:
  - vector<u8>: ULEB128 length, then the bytes
  - address:    32 raw bytes
  - u64:        8 little-endian bytes
and the id is sha2-256 over the five fields in order:
  bcs(DOMAIN) ++ bcs(chain_identifier) ++ gate ++ sender ++ bcs(client_nonce)

Run: python3 -I transfer_id_reference.py
The vectors printed here are pinned in tests/rumi_icusd_gate_tests.move.
"""
import hashlib

DOMAIN = b"rumi.icusd.sui.return.v1"


def uleb128(n: int) -> bytes:
    out = bytearray()
    while True:
        byte = n & 0x7F
        n >>= 7
        if n:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def bcs_bytes(v: bytes) -> bytes:
    return uleb128(len(v)) + v


def bcs_address(a: bytes) -> bytes:
    assert len(a) == 32
    return a


def bcs_u64(n: int) -> bytes:
    assert 0 <= n < 2**64
    return n.to_bytes(8, "little")


def preimage(chain: bytes, gate: bytes, sender: bytes, nonce: int) -> bytes:
    return (
        bcs_bytes(DOMAIN)
        + bcs_bytes(chain)
        + bcs_address(gate)
        + bcs_address(sender)
        + bcs_u64(nonce)
    )


def transfer_id(chain: bytes, gate: bytes, sender: bytes, nonce: int) -> str:
    return hashlib.sha256(preimage(chain, gate, sender, nonce)).hexdigest()


CHAIN_A = bytes(range(1, 33))  # 0x01 .. 0x20
CHAIN_B = bytes([0x11] * 32)
GATE_A = bytes([0xAA] * 32)
GATE_B = bytes([0xAB] * 32)
SENDER_A = bytes([0xBB] * 32)
SENDER_B = bytes([0xCC] * 32)
U64_MAX = 2**64 - 1

VECTORS = [
    ("v1 baseline", CHAIN_A, GATE_A, SENDER_A, 1),
    ("v2 nonce zero", CHAIN_A, GATE_A, SENDER_A, 0),
    ("v3 nonce u64 max", CHAIN_A, GATE_A, SENDER_A, U64_MAX),
    ("v4 other sender", CHAIN_A, GATE_A, SENDER_B, 1),
    ("v5 other chain", CHAIN_B, GATE_A, SENDER_A, 1),
    ("v6 other gate", CHAIN_A, GATE_B, SENDER_A, 1),
]

if __name__ == "__main__":
    pre = preimage(CHAIN_A, GATE_A, SENDER_A, 1)
    print("v1 preimage length:", len(pre), "(1+24 domain, 1+32 chain, 32 gate, 32 sender, 8 nonce)")
    print("v1 preimage hex:", pre.hex())
    for name, chain, gate, sender, nonce in VECTORS:
        print(f"{name}: {transfer_id(chain, gate, sender, nonce)}")
