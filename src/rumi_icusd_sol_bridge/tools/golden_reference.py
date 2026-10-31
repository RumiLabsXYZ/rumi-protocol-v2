#!/usr/bin/env python3
"""Independent reference for the BB-47 golden vectors (messages and ids).

Written from the documented formats only (chain-solana.md sections 2.1 to 2.4 and 5, the
Solana legacy message wire layout and the SDK account-ordering rule), with no Solana library
and no code shared with the Rust crate: `hashlib` for sha256, a pure-Python base58, and a
pure-Python ed25519 point decompression check for the PDA off-curve rule.

    python3 tools/golden_reference.py            print every vector
    python3 tools/golden_reference.py --check    compare against the constants pinned in
                                                 src/tx/golden.rs and src/ids.rs (exit 1 on any mismatch)

It also reproduces the SPL associated-token-account known answer that the backend pins, which
is the external anchor for the PDA and base58 code here.
"""
import base64
import hashlib
import re
import struct
import sys
import zlib
from pathlib import Path

ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58decode(s):
    n = 0
    for ch in s:
        n = n * 58 + ALPHABET.index(ch)
    body = n.to_bytes((n.bit_length() + 7) // 8, "big") if n else b""
    return b"\x00" * (len(s) - len(s.lstrip("1"))) + body


def b58encode(b):
    n = int.from_bytes(b, "big")
    out = ""
    while n:
        n, r = divmod(n, 58)
        out = ALPHABET[r] + out
    return "1" * (len(b) - len(b.lstrip(b"\x00"))) + out


TOKEN = b58decode("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA")
TOKEN22 = b58decode("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")
ATA_PROGRAM = b58decode("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL")
MEMO_V3 = b58decode("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr")
COMPUTE_BUDGET = b58decode("ComputeBudget111111111111111111111111111111")
SYSTEM = b"\x00" * 32
RECENT_BLOCKHASHES = b58decode("SysvarRecentB1ockHashes11111111111111111111")
RENT_SYSVAR = b58decode("SysvarRent111111111111111111111111111111111")
assert all(len(k) == 32 for k in (TOKEN, TOKEN22, ATA_PROGRAM, MEMO_V3, COMPUTE_BUDGET, RECENT_BLOCKHASHES, RENT_SYSVAR))

P = 2**255 - 19
D = (-121665 * pow(121666, P - 2, P)) % P


def on_curve(b):
    """Whether the 32 bytes decompress to an ed25519 point (x^2 = (y^2-1)/(d*y^2+1) has a root)."""
    y = (int.from_bytes(b, "little") & ((1 << 255) - 1)) % P
    u = (y * y - 1) % P
    v = (D * y * y + 1) % P
    x2 = u * pow(v, P - 2, P) % P
    return x2 == 0 or pow(x2, (P - 1) // 2, P) == 1


def ata(wallet, token_program, mint):
    seeds = wallet + token_program + mint
    for bump in range(255, -1, -1):
        h = hashlib.sha256(seeds + bytes([bump]) + ATA_PROGRAM + b"ProgramDerivedAddress").digest()
        if not on_curve(h):
            return h
    raise RuntimeError("no pda")


def cu16(n):
    out = bytearray()
    while True:
        byte = n & 0x7F
        n >>= 7
        out.append(byte | (0x80 if n else 0))
        if not n:
            return bytes(out)


def compile_legacy(ixs, payer, blockhash):
    """SDK order: payer, then writable signers, read-only signers, writable others, read-only
    others, each group ascending by key bytes."""
    meta = {}
    for prog, accts, _ in ixs:
        meta.setdefault(prog, [False, False])
        for key, s, w in accts:
            m = meta.setdefault(key, [False, False])
            m[0] |= s
            m[1] |= w
    meta.pop(payer, None)
    ws = [payer] + sorted(k for k, (s, w) in meta.items() if s and w)
    rs = sorted(k for k, (s, w) in meta.items() if s and not w)
    wn = sorted(k for k, (s, w) in meta.items() if not s and w)
    rn = sorted(k for k, (s, w) in meta.items() if not s and not w)
    keys = ws + rs + wn + rn
    out = bytearray([len(ws) + len(rs), len(rs), len(rn)])
    out += cu16(len(keys)) + b"".join(keys) + blockhash + cu16(len(ixs))
    for prog, accts, data in ixs:
        out.append(keys.index(prog))
        out += cu16(len(accts)) + bytes(keys.index(k) for k, _, _ in accts)
        out += cu16(len(data)) + data
    return bytes(out)


def advance_nonce(nonce, authority):
    return (SYSTEM, [(nonce, False, True), (RECENT_BLOCKHASHES, False, False), (authority, True, False)], struct.pack("<I", 4))


def create_idempotent(funder, wallet, mint, tp):
    return (ATA_PROGRAM, [(funder, True, True), (ata(wallet, tp, mint), False, True), (wallet, False, False), (mint, False, False), (SYSTEM, False, False), (tp, False, False)], b"\x01")


def create_with_seed(base, seed, owner):
    assert len(seed.encode()) <= 32
    return hashlib.sha256(base + seed.encode() + owner).digest()


def create_account_with_seed(frm, to, base, seed, lamports, space, owner):
    """System tag 3 (bincode): u32 tag, base 32, seed as u64 length + bytes, lamports, space, owner."""
    data = struct.pack("<I", 3) + base + struct.pack("<Q", len(seed.encode())) + seed.encode() + struct.pack("<QQ", lamports, space) + owner
    accts = [(frm, True, True), (to, False, True)]
    if base != frm:
        accts.append((base, True, False))
    return (SYSTEM, accts, data)


def initialize_nonce(nonce, authority):
    return (SYSTEM, [(nonce, False, True), (RECENT_BLOCKHASHES, False, False), (RENT_SYSVAR, False, False)], struct.pack("<I", 6) + authority)


def create_account(frm, to, lamports, space, owner):
    return (SYSTEM, [(frm, True, True), (to, True, True)], struct.pack("<I", 0) + struct.pack("<QQ", lamports, space) + owner)


def initialize_mint2(tp, mint, decimals, authority, freeze=None):
    data = bytes([20, decimals]) + authority + (b"\x00" if freeze is None else b"\x01" + freeze)
    return (tp, [(mint, False, True)], data)


def checked(tag, amount):
    return bytes([tag]) + struct.pack("<Q", amount) + bytes([8])


def mint_to_checked(tp, mint, dest, authority, amount):
    return (tp, [(mint, False, True), (dest, False, True), (authority, True, False)], checked(14, amount))


def burn_checked(tp, account, mint, owner, amount):
    return (tp, [(account, False, True), (mint, False, True), (owner, True, False)], checked(15, amount))


def transfer_checked(tp, source, mint, dest, authority, amount):
    return (tp, [(source, False, True), (mint, False, False), (dest, False, True), (authority, True, False)], checked(12, amount))


def memo(text):
    return (MEMO_V3, [], text.encode("ascii"))


def cb_limit(units):
    return (COMPUTE_BUDGET, [], bytes([2]) + struct.pack("<I", units))


def cb_price(price):
    return (COMPUTE_BUDGET, [], bytes([3]) + struct.pack("<Q", price))


def principal_text(raw):
    crc = zlib.crc32(raw).to_bytes(4, "big")
    s = base64.b32encode(crc + raw).decode().lower().rstrip("=")
    return "-".join(s[i:i + 5] for i in range(0, len(s), 5))


assert principal_text(b"") == "aaaaa-aa" and principal_text(b"\x04") == "2vxsx-fae"

# ---- ids: sha256(domain || (u32_be(len) || field)*), integers 8-byte big-endian ----
def idhash(domain, fields):
    return hashlib.sha256(domain.encode() + b"".join(struct.pack(">I", len(f)) + f for f in fields)).digest()


def u64be(n):
    return struct.pack(">Q", n)


FEE_PAYER, MINT_AUTH, RETURN_OWNER, MINT = b"\x11" * 32, b"\x22" * 32, b"\x33" * 32, b"\x44" * 32
NONCE_ACCT, NONCE_VALUE, RECIPIENT, SENDER = b"\x55" * 32, b"\x66" * 32, b"\x77" * 32, b"\x88" * 32
SENDER_SOURCE, HOLDER, HOLDER_SOURCE, RETURN_BLOCKHASH = b"\x99" * 32, b"\xa0" * 32, b"\xa1" * 32, b"\x77" * 32
AMOUNT = 123_456_789


def id_vectors():
    g, salt, sig, sig2 = b"\x01" * 32, b"\x05" * 32, b"\x9a" * 64, b"\x9b" * 64
    ret = idhash("rumi.icusd.sol.return.v1", [g, sig])
    ret2 = idhash("rumi.icusd.sol.return.v1", [g, sig2])
    return {
        "MINT_ID": idhash("rumi.icusd.sol.mint.v1", [g, MINT, salt, u64be(7), RECIPIENT, u64be(100)]),
        "RETURN_ID": ret,
        "REMINT_ID": idhash("rumi.icusd.sol.remint.v1", [g, MINT, salt, ret]),
        "BURN_ID_ONE": idhash("rumi.icusd.sol.burn.v1", [g, MINT, salt, u64be(1), ret]),
        "RETURN_ID_TWO": ret2,
        "BURN_ID_TWO": idhash("rumi.icusd.sol.burn.v1", [g, MINT, salt, u64be(2), ret, ret2]),
        "BURN_ID_TWO_REVERSED": idhash("rumi.icusd.sol.burn.v1", [g, MINT, salt, u64be(2), ret2, ret]),
        "REFUND_ID": idhash("rumi.icusd.sol.refund.v1", [g, MINT, salt, sig]),
        "ESCROW_SUBACCOUNT": hashlib.sha256(b"rumi-icusd-sol-bridge:escrow:v1").digest(),
        "INSTANCE_SALT": hashlib.sha256(b"rumi-icusd-sol-bridge:instance-salt:v1" + b"\xbb" * 10 + u64be(1_700_000_000_000_000_000)).digest(),
    }


def message_vectors():
    out = {}
    user_memo = "rumi1|r|%s|-|42" % principal_text(b"\x07" * 10)
    for suffix, tp in (("CLASSIC", TOKEN), ("T22", TOKEN22)):
        recipient_ata, return_ata, sender_ata = ata(RECIPIENT, tp, MINT), ata(RETURN_OWNER, tp, MINT), ata(SENDER, tp, MINT)
        mint_ix = [advance_nonce(NONCE_ACCT, FEE_PAYER), create_idempotent(FEE_PAYER, RECIPIENT, MINT, tp), mint_to_checked(tp, MINT, recipient_ata, MINT_AUTH, AMOUNT), memo("rumi1|m|" + (b"\xa1" * 32).hex())]
        burn_ix = [advance_nonce(NONCE_ACCT, FEE_PAYER), burn_checked(tp, return_ata, MINT, RETURN_OWNER, AMOUNT), memo("rumi1|b|" + (b"\xb2" * 32).hex())]
        refund_existing = [advance_nonce(NONCE_ACCT, FEE_PAYER), memo("rumi1|f|" + (b"\xc3" * 32).hex()), transfer_checked(tp, return_ata, MINT, SENDER_SOURCE, RETURN_OWNER, AMOUNT)]
        refund_create = [advance_nonce(NONCE_ACCT, FEE_PAYER), create_idempotent(FEE_PAYER, SENDER, MINT, tp), memo("rumi1|f|" + (b"\xc3" * 32).hex()), transfer_checked(tp, return_ata, MINT, sender_ata, RETURN_OWNER, AMOUNT)]
        ret = [transfer_checked(tp, HOLDER_SOURCE, MINT, return_ata, HOLDER, AMOUNT), memo(user_memo)]
        ret_cb = [cb_limit(200000), cb_price(1000)] + ret
        for name, ixs, payer, bh in (
            ("MINT", mint_ix, FEE_PAYER, NONCE_VALUE),
            ("BURN", burn_ix, FEE_PAYER, NONCE_VALUE),
            ("REFUND_EXISTING", refund_existing, FEE_PAYER, NONCE_VALUE),
            ("REFUND_CREATE_ATA", refund_create, FEE_PAYER, NONCE_VALUE),
            ("RETURN", ret, HOLDER, RETURN_BLOCKHASH),
            ("RETURN_COMPUTE_BUDGET", ret_cb, HOLDER, RETURN_BLOCKHASH),
        ):
            out["%s_%s" % (name, suffix)] = compile_legacy(ixs, payer, bh)
        out["ATA_RECIPIENT_" + suffix] = recipient_ata
        out["ATA_RETURN_" + suffix] = return_ata
        out["ATA_SENDER_" + suffix] = sender_ata
    # Mint creation and the return ATA (49a), per token program.
    for suffix, tp in (("CLASSIC", TOKEN), ("T22", TOKEN22)):
        out["MINT_CREATE_" + suffix] = compile_legacy(
            [create_account(FEE_PAYER, MINT, 1_461_600, 82, tp), initialize_mint2(tp, MINT, 8, MINT_AUTH)], FEE_PAYER, RETURN_BLOCKHASH
        )
        out["RETURN_ATA_CREATE_" + suffix] = compile_legacy([create_idempotent(FEE_PAYER, RETURN_OWNER, MINT, tp)], FEE_PAYER, RETURN_BLOCKHASH)
    # Nonce pool: advance-only (cancel) and the create + initialize bootstrap message.
    out["ADVANCE_ONLY"] = compile_legacy([advance_nonce(NONCE_ACCT, FEE_PAYER)], FEE_PAYER, NONCE_VALUE)
    seed = "rumi-nonce-m-0"
    address = create_with_seed(FEE_PAYER, seed, SYSTEM)
    out["NONCE_POOL_ADDRESS"] = address
    out["CREATE_NONCE_ACCOUNT"] = compile_legacy(
        [create_account_with_seed(FEE_PAYER, address, FEE_PAYER, seed, 1_447_680, 80, SYSTEM), initialize_nonce(address, FEE_PAYER)],
        FEE_PAYER,
        RETURN_BLOCKHASH,
    )
    return out


def spl_known_answer():
    got = b58encode(ata(b58decode("1117mWrzzrZr312ebPDHu8tbfMwFNvCvMbr6WepCNG"), TOKEN, b58decode("8N6gdBxJaZUG9cBnSSaHDsx7vMeQ4VR1LmCmk9SCu38s")))
    assert got == "3kP4RCoX8u1PhyiUkuNpFfbUmFF32ZC9uscL7H84Xs2u", got
    return got


def parse_rust_consts(path):
    """`const NAME: &str = "hex";` and `const NAME: &str = concat!("..", "..");` -> {NAME: hex}."""
    text = Path(path).read_text()
    out = {}
    for m in re.finditer(r"const (\w+): &str = (concat!\((.*?)\)|\"([0-9a-f]+)\");", text, re.S):
        name = m.group(1)
        body = m.group(3) if m.group(3) is not None else '"%s"' % m.group(4)
        out[name] = "".join(re.findall(r'"([0-9a-f]+)"', body))
    return out


def main():
    root = Path(__file__).resolve().parent.parent
    print("spl_known_answer_ok", spl_known_answer())
    ids, msgs = id_vectors(), message_vectors()
    if "--check" not in sys.argv:
        for k, v in {**ids, **msgs}.items():
            print(k, v.hex())
        return 0
    bad = 0
    golden = parse_rust_consts(root / "src/tx/golden.rs")
    idsrc = parse_rust_consts(root / "src/ids.rs")
    for name, v in msgs.items():
        if name.startswith("ATA_") or name == "NONCE_POOL_ADDRESS":
            # Pinned in golden.rs as `h32("...")` literals.
            continue
        ok = golden.get(name) == v.hex()
        bad += not ok
        print("OK  " if ok else "FAIL", "golden.rs", name)
    rust = open(root / "src/tx/golden.rs").read()
    for name, v in msgs.items():
        if name.startswith("ATA_") or name == "NONCE_POOL_ADDRESS":
            ok = v.hex() in rust
            bad += not ok
            print("OK  " if ok else "FAIL", "golden.rs", name)
    for name, v in ids.items():
        ok = idsrc.get(name) == v.hex()
        bad += not ok
        print("OK  " if ok else "FAIL", "ids.rs   ", name)
    print("mismatches:", bad)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
