/**
 * The return transaction and the memo, against the backend's INDEPENDENT reference.
 *
 * The hex constants below come from `src/rumi_icusd_sol_bridge/tools/golden_reference.py` (the pure-Python
 * reference written from the documented formats, sharing no code with the Rust crate or with this file) and
 * are the same bytes the Rust crate pins in `tx/golden.rs` (`RETURN_*`). The reference inputs:
 * holder 0xa0*32, source token account 0xa1*32, mint 0x44*32, return owner 0x33*32 (fee payer 0x11*32, mint
 * authority 0x22*32), blockhash 0x77*32, amount 123456789, memo for principal 0x07*10, default subaccount, nonce 42.
 */
import { createHash } from 'node:crypto';
import { Principal } from '@dfinity/principal';
import { describe, expect, it } from 'vitest';
import { base58Decode, base58Encode, fromHex, isOnCurve, pubkeyFromBase58, sha256, toHex, type Bytes } from './bytes';
import {
  ATA_PROGRAM_B58,
  COMPUTE_BUDGET_PROGRAM_B58,
  MEMO_PROGRAM_V3_B58,
  RESERVED_KEYS_B58,
  SYSTEM_PROGRAM_B58,
  TOKEN_2022_PROGRAM_B58,
  TOKEN_PROGRAM_B58,
  deriveAta,
  tokenProgramBytes,
} from './programs';
import { MemoError, checkNonce, checkSubaccountField, encodeReturnMemo, parseReturnMemo } from './memo';
import { buildReturnMessage, checkSignedTransaction, compactU16, messageOfWire, unsignedTransactionBytes } from './transaction';
import { returnId } from './ids';

const RETURN_CLASSIC =
  '01000306a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a056a28726ac4ed418cf2020ea6c60baf1fae7acfeda8698a7a1332eafce708f8aa1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa8404105448d06ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf5857eff00a944444444444444444444444444444444444444444444444444444444444444447777777777777777777777777777777777777777777777777777777777777777020404020501000a0c15cd5b07000000000803002872756d69317c727c6a746736722d69696861342d64716f62792d68613464712d6f62797c2d7c3432';
const RETURN_T22 =
  '01000306a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a09ab6c0bed9e85c4aad77dce93322d976ae6bbac60e6798581a1cc5a3bb2f5b35a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa8404105448d06ddf6e1ee758fde18425dbce46ccddab61afc4d83b90d27febdf928d8a18bfc44444444444444444444444444444444444444444444444444444444444444447777777777777777777777777777777777777777777777777777777777777777020404020501000a0c15cd5b07000000000803002872756d69317c727c6a746736722d69696861342d64716f62792d68613464712d6f62797c2d7c3432';
const RETURN_COMPUTE_BUDGET_CLASSIC =
  '01000407a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a056a28726ac4ed418cf2020ea6c60baf1fae7acfeda8698a7a1332eafce708f8aa1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a10306466fe5211732ffecadba72c39be7bc8ce5bbc5f7126b2c439b3a40000000054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa8404105448d06ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf5857eff00a9444444444444444444444444444444444444444444444444444444444444444477777777777777777777777777777777777777777777777777777777777777770403000502400d030003000903e8030000000000000504020601000a0c15cd5b07000000000804002872756d69317c727c6a746736722d69696861342d64716f62792d68613464712d6f62797c2d7c3432';
const RETURN_COMPUTE_BUDGET_T22 =
  '01000407a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a09ab6c0bed9e85c4aad77dce93322d976ae6bbac60e6798581a1cc5a3bb2f5b35a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a10306466fe5211732ffecadba72c39be7bc8ce5bbc5f7126b2c439b3a40000000054a535a992921064d24e87160da387c7c35b5ddbc92bb81e41fa8404105448d06ddf6e1ee758fde18425dbce46ccddab61afc4d83b90d27febdf928d8a18bfc444444444444444444444444444444444444444444444444444444444444444477777777777777777777777777777777777777777777777777777777777777770403000502400d030003000903e8030000000000000504020601000a0c15cd5b07000000000804002872756d69317c727c6a746736722d69696861342d64716f62792d68613464712d6f62797c2d7c3432';
const ATA_RETURN_CLASSIC = '56a28726ac4ed418cf2020ea6c60baf1fae7acfeda8698a7a1332eafce708f8a';
const ATA_RETURN_T22 = '9ab6c0bed9e85c4aad77dce93322d976ae6bbac60e6798581a1cc5a3bb2f5b35';
const RETURN_ID_VECTOR = 'f4ae3a40e9e15a2b5f32de2c5a5a7fa1ed58001b83711dcac38e8450be4e138d';
const MEMO_VECTOR = 'rumi1|r|jtg6r-iiha4-dqoby-ha4dq-oby|-|42';

const fill = (byte: number, n = 32): Bytes => new Uint8Array(n).fill(byte);
const HOLDER = fill(0xa0);
const SOURCE = fill(0xa1);
const MINT = fill(0x44);
const RETURN_OWNER = fill(0x33);
const ROLE_KEYS = [fill(0x11), fill(0x22)];
const BLOCKHASH = fill(0x77);
const AMOUNT = 123_456_789n;
const BRIDGE_TEXT = Principal.fromUint8Array(fill(0xbb, 10)).toText();

function build(tokenProgram: 'classic' | 'token2022', computeBudget?: { unitLimit?: number; unitPrice?: bigint }) {
  return buildReturnMessage({
    tokenProgram,
    mint: MINT,
    returnOwner: RETURN_OWNER,
    roleKeys: ROLE_KEYS,
    holder: HOLDER,
    sourceTokenAccount: SOURCE,
    amount: AMOUNT,
    memo: MEMO_VECTOR,
    recentBlockhash: BLOCKHASH,
    computeBudget,
  });
}

describe('base58, hex and SHA-256', () => {
  it('decodes the program ids to the bytes the backend pins', () => {
    const classic = pubkeyFromBase58(TOKEN_PROGRAM_B58)!;
    expect(toHex(classic)).toBe('06ddf6e1d765a193d9cbe146ceeb79ac1cb485ed5f5b37913a8cf5857eff00a9');
    expect(toHex(pubkeyFromBase58(TOKEN_2022_PROGRAM_B58)!)).toBe('06ddf6e1ee758fde18425dbce46ccddab61afc4d83b90d27febdf928d8a18bfc');
    expect(toHex(pubkeyFromBase58(ATA_PROGRAM_B58)!)).toBe('8c97258f4e2489f1bb3d1029148e0d830b5a1399daff1084048e7bd8dbe9f859');
    expect(pubkeyFromBase58(SYSTEM_PROGRAM_B58)).toEqual(new Uint8Array(32));
    for (const text of [MEMO_PROGRAM_V3_B58, COMPUTE_BUDGET_PROGRAM_B58, ...RESERVED_KEYS_B58]) expect(pubkeyFromBase58(text)).not.toBeNull();
  });

  it('round-trips keys, including leading zero bytes, and refuses text outside the alphabet or of the wrong size', () => {
    for (const key of [fill(0x11), new Uint8Array(32), Uint8Array.from([0, 0, ...fill(7, 30)]), fill(0xff)]) {
      expect(pubkeyFromBase58(base58Encode(key))).toEqual(key);
    }
    expect(base58Decode('0OIl')).toBeNull();
    expect(pubkeyFromBase58('abc')).toBeNull();
    expect(pubkeyFromBase58('')).toBeNull();
    expect(pubkeyFromBase58('z'.repeat(45))).toBeNull();
  });

  it('computes SHA-256 like node for many lengths', () => {
    expect(toHex(sha256(new Uint8Array()))).toBe('e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855');
    expect(toHex(sha256(new TextEncoder().encode('abc')))).toBe('ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
    for (const n of [1, 55, 56, 57, 63, 64, 65, 119, 120, 1000]) {
      const data = Uint8Array.from({ length: n }, (_, i) => (i * 31 + 7) & 0xff);
      expect(toHex(sha256(data))).toBe(createHash('sha256').update(data).digest('hex'));
    }
  });
});

describe('associated token account derivation', () => {
  it('matches the SPL known answer and the backend reference for both token programs', () => {
    const wallet = pubkeyFromBase58('1117mWrzzrZr312ebPDHu8tbfMwFNvCvMbr6WepCNG');
    // The known-answer wallet in the reference is 32 bytes once left-padded with zero bytes.
    const padded = Uint8Array.from([...new Uint8Array(32 - (base58Decode('1117mWrzzrZr312ebPDHu8tbfMwFNvCvMbr6WepCNG')?.length ?? 0)), ...(base58Decode('1117mWrzzrZr312ebPDHu8tbfMwFNvCvMbr6WepCNG') ?? [])]);
    expect(wallet === null || wallet.length === 32).toBe(true);
    const known = deriveAta(padded, tokenProgramBytes('classic'), pubkeyFromBase58('8N6gdBxJaZUG9cBnSSaHDsx7vMeQ4VR1LmCmk9SCu38s')!);
    expect(base58Encode(known)).toBe('3kP4RCoX8u1PhyiUkuNpFfbUmFF32ZC9uscL7H84Xs2u');
    expect(toHex(deriveAta(RETURN_OWNER, tokenProgramBytes('classic'), MINT))).toBe(ATA_RETURN_CLASSIC);
    expect(toHex(deriveAta(RETURN_OWNER, tokenProgramBytes('token2022'), MINT))).toBe(ATA_RETURN_T22);
  });

  it('puts the derived address off the curve and a private-key public key on it', () => {
    expect(isOnCurve(deriveAta(RETURN_OWNER, tokenProgramBytes('token2022'), MINT))).toBe(false);
    // The ed25519 base point is on the curve.
    expect(isOnCurve(fromHex('5866666666666666666666666666666666666666666666666666666666666666')!)).toBe(true);
  });
});

describe('the return message', () => {
  it('is byte-identical to the backend reference for both token programs, with and without ComputeBudget', () => {
    expect(toHex(build('classic').message)).toBe(RETURN_CLASSIC);
    expect(toHex(build('token2022').message)).toBe(RETURN_T22);
    expect(toHex(build('classic', { unitLimit: 200_000, unitPrice: 1000n }).message)).toBe(RETURN_COMPUTE_BUDGET_CLASSIC);
    expect(toHex(build('token2022', { unitLimit: 200_000, unitPrice: 1000n }).message)).toBe(RETURN_COMPUTE_BUDGET_T22);
    expect(toHex(build('classic').returnAta)).toBe(ATA_RETURN_CLASSIC);
  });

  it('refuses a zero amount, a holder that is a bridge key and an oversize packet', () => {
    const base = { tokenProgram: 'token2022' as const, mint: MINT, returnOwner: RETURN_OWNER, roleKeys: ROLE_KEYS, holder: HOLDER, sourceTokenAccount: SOURCE, amount: 1n, memo: MEMO_VECTOR, recentBlockhash: BLOCKHASH };
    expect(() => buildReturnMessage({ ...base, amount: 0n })).toThrow(/at least 1/);
    expect(() => buildReturnMessage({ ...base, holder: RETURN_OWNER })).toThrow(/role keys/);
    expect(() => buildReturnMessage({ ...base, holder: ROLE_KEYS[0] })).toThrow(/role keys/);
    expect(() => buildReturnMessage({ ...base, memo: 'x'.repeat(1200) })).toThrow(/one packet/);
  });

  it('wraps the message for the wallet and recognizes only our own message signed', () => {
    const { message } = build('token2022');
    const unsigned = unsignedTransactionBytes(message);
    expect(unsigned.length).toBe(1 + 64 + message.length);
    expect([unsigned[0], ...unsigned.slice(1, 65)].every((b, i) => (i === 0 ? b === 1 : b === 0))).toBe(true);
    expect(messageOfWire(unsigned)).toEqual(message);
    const signed = Uint8Array.from([1, ...fill(9, 64), ...message]);
    expect(checkSignedTransaction(signed, message)).toEqual({ ok: true, signature: fill(9, 64) });
    expect(checkSignedTransaction(unsigned, message)).toEqual({ ok: false, reason: 'unsigned' });
    expect(checkSignedTransaction(Uint8Array.from([2, ...fill(9, 128), ...message]), message)).toEqual({ ok: false, reason: 'signature_count' });
    const changed = Uint8Array.from([1, ...fill(9, 64), ...message, 0]);
    expect(checkSignedTransaction(changed, message)).toEqual({ ok: false, reason: 'message_changed' });
    expect(checkSignedTransaction(new Uint8Array(10), message)).toEqual({ ok: false, reason: 'malformed' });
    expect(compactU16(0)).toEqual(Uint8Array.of(0));
    expect(compactU16(300)).toEqual(Uint8Array.of(0xac, 0x02));
  });
});

describe('the return id', () => {
  it('matches the backend vector for the genesis 0x01*32 and the signature 0x9a*64', () => {
    expect(returnId(fill(1), fill(0x9a, 64))).toBe(RETURN_ID_VECTOR);
    expect(() => returnId(fill(1, 31), fill(0x9a, 64))).toThrow();
  });
});

describe('the return memo (memo.rs parity)', () => {
  const user = Principal.fromUint8Array(fill(0x07, 10)).toText();

  it('writes the reference memo and the canonical forms', () => {
    expect(user).toBe('jtg6r-iiha4-dqoby-ha4dq-oby');
    expect(encodeReturnMemo({ principal: user, subaccount: null, nonce: 42n }, BRIDGE_TEXT)).toBe(MEMO_VECTOR);
    expect(encodeReturnMemo({ principal: user, subaccount: '', nonce: '0' }, BRIDGE_TEXT)).toBe(`rumi1|r|${user}|-|0`);
    // The default subaccount, spelled as all zeros, is written as `-`.
    expect(encodeReturnMemo({ principal: user, subaccount: '0'.repeat(64), nonce: 1n }, BRIDGE_TEXT)).toBe(`rumi1|r|${user}|-|1`);
    const sub = `${'00'.repeat(31)}01`;
    expect(encodeReturnMemo({ principal: user, subaccount: sub, nonce: 18_446_744_073_709_551_615n }, BRIDGE_TEXT)).toBe(
      `rumi1|r|${user}|${sub}|18446744073709551615`,
    );
  });

  it('fits the longest legal memo in 160 bytes and refuses what the backend refuses', () => {
    const longest = Principal.fromUint8Array(fill(0xee, 29)).toText();
    const memo = encodeReturnMemo({ principal: longest, subaccount: 'ab'.repeat(32), nonce: 18_446_744_073_709_551_615n }, BRIDGE_TEXT);
    expect(new TextEncoder().encode(memo).length).toBeLessThanOrEqual(160);
    const refuse = (input: { principal: string; subaccount: string | null; nonce: string | bigint }, code: string) => {
      try {
        encodeReturnMemo(input, BRIDGE_TEXT);
        throw new Error(`accepted ${JSON.stringify(input)}`);
      } catch (error) {
        expect(error).toBeInstanceOf(MemoError);
        expect((error as MemoError).code).toBe(code);
      }
    };
    refuse({ principal: '', subaccount: null, nonce: 1n }, 'empty_principal');
    refuse({ principal: 'not-a-principal', subaccount: null, nonce: 1n }, 'principal_syntax');
    refuse({ principal: user.toUpperCase(), subaccount: null, nonce: 1n }, 'principal_syntax');
    refuse({ principal: user.replace(/-/g, ''), subaccount: null, nonce: 1n }, 'principal_syntax');
    refuse({ principal: '2vxsx-fae', subaccount: null, nonce: 1n }, 'principal_anonymous');
    refuse({ principal: 'aaaaa-aa', subaccount: null, nonce: 1n }, 'principal_management');
    refuse({ principal: BRIDGE_TEXT, subaccount: null, nonce: 1n }, 'principal_is_bridge');
    refuse({ principal: user, subaccount: 'AB'.repeat(32), nonce: 1n }, 'subaccount_syntax');
    refuse({ principal: user, subaccount: 'ab'.repeat(31), nonce: 1n }, 'subaccount_syntax');
    refuse({ principal: user, subaccount: 'zz'.repeat(32), nonce: 1n }, 'subaccount_syntax');
    refuse({ principal: user, subaccount: null, nonce: '07' }, 'nonce_leading_zero');
    refuse({ principal: user, subaccount: null, nonce: '' }, 'nonce_syntax');
    refuse({ principal: user, subaccount: null, nonce: '-1' }, 'nonce_syntax');
    refuse({ principal: user, subaccount: null, nonce: '0x1' }, 'nonce_syntax');
    refuse({ principal: user, subaccount: null, nonce: '18446744073709551616' }, 'nonce_overflow');
    expect(checkNonce('0')).toBe('0');
    expect(checkSubaccountField(null)).toBe('-');
  });

  it('parses its own output back and refuses the table of bad memos', () => {
    const memo = `rumi1|r|${user}|${'ab'.repeat(32)}|7`;
    expect(parseReturnMemo(memo, BRIDGE_TEXT)).toEqual({ principal: user, subaccount: 'ab'.repeat(32), nonce: 7n });
    expect(parseReturnMemo(MEMO_VECTOR, BRIDGE_TEXT)).toEqual({ principal: user, subaccount: null, nonce: 42n });
    for (const bad of [
      `${MEMO_VECTOR} `,
      `${MEMO_VECTOR}|x`,
      `rumi2|r|${user}|-|1`,
      `rumi1|m|${user}|-|1`,
      `rumi1|r|${user}|${'0'.repeat(64)}|1`,
      `rumi1|r|${user}|-`,
      `rumi1|r|${user}|-|${'0'.repeat(200)}`,
    ]) {
      expect(() => parseReturnMemo(bad, BRIDGE_TEXT), bad).toThrow();
    }
  });
});
