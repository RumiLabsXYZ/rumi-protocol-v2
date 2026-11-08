/**
 * The frozen independent reference for the Sui return: the Gate's `transfer_id` and the exact bytes of the `bridge_out`
 * transaction. NOTHING here is produced by the code under test.
 *
 *  - Transfer ids: the six vectors pinned by the accepted Move tests (`move/rumi_icusd_gate/tests`, 23f75ef1) and by
 *    `ids.rs`, produced by `move/rumi_icusd_gate/reference/transfer_id_reference.py` (sha256 of that script:
 *    b42eb3d61f9c04dec5ee5177ad41af4047ff827df81fd1dd0a544c822314b56f), plus two more produced by
 *    `reference/bridge_out_reference.py` (sha256 a03f395390db03d1eb5eb49cb21c5f17e92081d408bd4ede4badcbc9c94ba6af).
 *  - Call bytes: the BCS `TransactionData` that `reference/bridge_out_reference.py` writes by hand from the Sui transaction
 *    layout (standard library only, no Sui SDK), for four shapes: exact coin, split, merge plus split, merge exact. The
 *    SDK-built transaction must equal them byte for byte.
 * Rerun the scripts with `python3 -I` to regenerate; the outputs are in the BB-54 evidence logs.
 */
import { Transaction } from '@mysten/sui/transactions';
import { describe, expect, it } from 'vitest';
import { Principal } from '@dfinity/principal';
import { base58Decode, base58Encode, fromHex, toHex } from '../solanaBridge/bytes';
import { RETURN_DOMAIN, bcsString, bcsU64, bcsVectorU8, transferId, transferIdBytes } from './ids';
import { SUI_MAINNET, SUI_TESTNET } from './networks';
import { buildReturnTransaction, type ReturnCallPlan } from './transaction';

const fill = (byte: number, n = 32) => new Uint8Array(n).fill(byte);
const addr = (byte: number) => `0x${byte.toString(16).padStart(2, '0').repeat(32)}`;
const CHAIN_A = Uint8Array.from({ length: 32 }, (_, i) => i + 1);

describe('the Gate transfer id (independent vectors)', () => {
  it('matches the six vectors the Move gate and ids.rs pin', () => {
    const gate = fill(0xaa);
    const sender = fill(0xbb);
    expect(transferId(CHAIN_A, gate, sender, 1n)).toBe('d9f230d9c727a3d311b72b92db4a127bf94b9af42643a4ccb4cd7f98b474cea2');
    expect(transferId(CHAIN_A, gate, sender, 0n)).toBe('85f1a74776c47a2abaa38a52a4db09b97c9b206f178ff89172c6e17ba2efda71');
    expect(transferId(CHAIN_A, gate, sender, 18_446_744_073_709_551_615n)).toBe('2f94a6e9243dfa2926c128f2116aa532c15a82864a73b800a2944bdec05b2641');
    expect(transferId(CHAIN_A, gate, fill(0xcc), 1n)).toBe('c394fd448284bbaa385a723bdb5152b4a887370c706f15357d03bdbb122ec325');
    expect(transferId(fill(0x11), gate, sender, 1n)).toBe('b5a581713bfc17dda3a799b9620a4d648ebcff60d4adf974b27e6c985d2176e6');
    expect(transferId(CHAIN_A, fill(0xab), sender, 1n)).toBe('d8b7c2b90ebf5c379e16543f373045e5db63ca4afcf020a1aa483a45657c6c49');
  });

  it('is built from exactly the preimage the reference prints (domain, chain id, gate, sender, nonce)', () => {
    const preimage = Uint8Array.from([...bcsString(RETURN_DOMAIN), ...bcsVectorU8(CHAIN_A), ...fill(0xaa), ...fill(0xbb), ...bcsU64(1n)]);
    expect(toHex(preimage)).toBe('1872756d692e69637573642e7375692e72657475726e2e7631200102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb0100000000000000');
    expect(preimage.length).toBe(130);
    expect(toHex(transferIdBytes(CHAIN_A, fill(0xaa), fill(0xbb), 1n))).toBe('d9f230d9c727a3d311b72b92db4a127bf94b9af42643a4ccb4cd7f98b474cea2');
  });

  it('matches the reference for the testnet identifier and this page\'s sample sender and Gate', () => {
    const chain = fromHex(SUI_TESTNET.chainIdentifier)!;
    expect(transferId(chain, fill(0x6a), fill(0xbb), 1n)).toBe('62335ca1f13e2f316decb1b73d049bac44dde0fed6c4d4fc2efe9247c1da05bb');
    expect(transferId(chain, fill(0x6a), fill(0xbb), 18_446_744_073_709_551_615n)).toBe('7c78e7761a8b5ffa0134e92c8f72b2a70edbfec38d2826ab7a30ba0f079292ba');
  });

  it('changes with every input, and a nonce outside u64 is refused', () => {
    const base = transferId(CHAIN_A, fill(0xaa), fill(0xbb), 5n);
    expect(new Set([base, transferId(CHAIN_A, fill(0xaa), fill(0xbb), 6n), transferId(CHAIN_A, fill(0xab), fill(0xbb), 5n), transferId(CHAIN_A, fill(0xaa), fill(0xbc), 5n), transferId(fill(0x02), fill(0xaa), fill(0xbb), 5n)]).size).toBe(5);
    expect(() => transferId(CHAIN_A, fill(0xaa), fill(0xbb), 18_446_744_073_709_551_616n)).toThrow(RangeError);
    expect(() => transferId(CHAIN_A, fill(0xaa), fill(0xbb), -1n)).toThrow(RangeError);
  });
});

describe('the network identifiers', () => {
  it('are the full genesis digests: base58 decodes to the hex the canister reports', () => {
    expect(toHex(base58Decode(SUI_MAINNET.chainIdentifierBase58)!)).toBe(SUI_MAINNET.chainIdentifier);
    expect(toHex(base58Decode(SUI_TESTNET.chainIdentifierBase58)!)).toBe(SUI_TESTNET.chainIdentifier);
    expect(base58Encode(fromHex(SUI_MAINNET.chainIdentifier)!)).toBe('4btiuiMPvEENsttpZC7CZ53DruC3MAgfznDbASZ7DR6S');
    expect(SUI_MAINNET.chainIdentifier).toHaveLength(64);
    expect(SUI_TESTNET.chainIdentifier).toHaveLength(64);
  });
});

// ---------------------------------------------------------------------------
// The bridge_out transaction bytes
// ---------------------------------------------------------------------------

const PACKAGE = addr(0x70);
const GATE = addr(0x6a);
const SENDER = addr(0xbb);
const GAS = { objectId: addr(0x67), version: '9', digest: base58Encode(fill(0x56)) };
const coin = (idByte: number, version: string, digestByte: number, balance: bigint) => ({ objectId: addr(idByte), version, digest: base58Encode(fill(digestByte)), balance });
const COIN_A = coin(0xc1, '7', 0x57, 250n * 100_000_000n);
const COIN_B = coin(0xc2, '11', 0x58, 100n * 100_000_000n);
const PRINCIPAL = Uint8Array.from({ length: 10 }, (_, i) => i + 1);
const ZERO = new Uint8Array(32);
const SUB = Uint8Array.from({ length: 32 }, (_, i) => 0xa0 + i);

async function bytesOf(plan: Partial<ReturnCallPlan> & Pick<ReturnCallPlan, 'coins' | 'amountE8s' | 'subaccount' | 'nonce'>): Promise<string> {
  const tx: Transaction = buildReturnTransaction({
    packageId: PACKAGE,
    gateId: GATE,
    gateInitialSharedVersion: '42',
    sender: SENDER,
    principalBytes: PRINCIPAL,
    ...plan,
  });
  tx.setGasPrice(1000n);
  tx.setGasBudget(5_000_000n);
  tx.setGasPayment([GAS]);
  tx.setExpiration({ Epoch: 123 });
  return toHex(await tx.build());
}

describe('the bridge_out transaction bytes (independent reference)', () => {
  it('A: a coin of exactly the amount goes in whole, with no merge and no split', async () => {
    expect(await bytesOf({ coins: [COIN_A], amountE8s: 250n * 100_000_000n, subaccount: ZERO, nonce: 1n })).toBe('00000501016a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a2a00000000000000010100c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c10700000000000000205757575757575757575757575757575757575757575757575757575757575757000b0a0102030405060708090a002120000000000000000000000000000000000000000000000000000000000000000000080100000000000000010070707070707070707070707070707070707070707070707070707070707070700f72756d695f69637573645f676174650a6272696467655f6f75740005010000010100010200010300010400bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb0167676767676767676767676767676767676767676767676767676767676767670900000000000000205656565656565656565656565656565656565656565656565656565656565656bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbe803000000000000404b4c0000000000017b00000000000000');
  });

  it('B: a larger coin has exactly the amount split off, and the 32-byte subaccount and a u64-max nonce are carried', async () => {
    expect(await bytesOf({ coins: [{ ...COIN_A, balance: 400n * 100_000_000n }], amountE8s: 250n * 100_000_000n, subaccount: SUB, nonce: 18_446_744_073_709_551_615n })).toBe('00000601016a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a2a00000000000000010100c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c10700000000000000205757575757575757575757575757575757575757575757575757575757575757000800ba1dd205000000000b0a0102030405060708090a002120a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebf0008ffffffffffffffff0202010100010102000070707070707070707070707070707070707070707070707070707070707070700f72756d695f69637573645f676174650a6272696467655f6f757400050100000300000000010300010400010500bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb0167676767676767676767676767676767676767676767676767676767676767670900000000000000205656565656565656565656565656565656565656565656565656565656565656bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbe803000000000000404b4c0000000000017b00000000000000');
  });

  it('C: two coins are merged, then exactly the amount is split off', async () => {
    expect(await bytesOf({ coins: [COIN_B, COIN_A], amountE8s: 300n * 100_000_000n, subaccount: ZERO, nonce: 0n })).toBe('00000701016a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a2a00000000000000010100c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c20b000000000000002058585858585858585858585858585858585858585858585858585858585858580100c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c10700000000000000205757575757575757575757575757575757575757575757575757575757575757000800ac23fc06000000000b0a0102030405060708090a00212000000000000000000000000000000000000000000000000000000000000000000008000000000000000003030101000101020002010100010103000070707070707070707070707070707070707070707070707070707070707070700f72756d695f69637573645f676174650a6272696467655f6f757400050100000301000000010400010500010600bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb0167676767676767676767676767676767676767676767676767676767676767670900000000000000205656565656565656565656565656565656565656565656565656565656565656bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbe803000000000000404b4c0000000000017b00000000000000');
  });

  it('D: two coins that add up to exactly the amount are merged and burned whole', async () => {
    expect(await bytesOf({ coins: [COIN_A, COIN_B], amountE8s: 350n * 100_000_000n, subaccount: ZERO, nonce: 5n })).toBe('00000601016a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a6a2a00000000000000010100c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c107000000000000002057575757575757575757575757575757575757575757575757575757575757570100c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c20b00000000000000205858585858585858585858585858585858585858585858585858585858585858000b0a0102030405060708090a0021200000000000000000000000000000000000000000000000000000000000000000000805000000000000000203010100010102000070707070707070707070707070707070707070707070707070707070707070700f72756d695f69637573645f676174650a6272696467655f6f75740005010000010100010300010400010500bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb0167676767676767676767676767676767676767676767676767676767676767670900000000000000205656565656565656565656565656565656565656565656565656565656565656bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbe803000000000000404b4c0000000000017b00000000000000');
  });

  it('refuses a subaccount that is not exactly 32 bytes and coins that hold less than the amount', () => {
    const plan = { packageId: PACKAGE, gateId: GATE, gateInitialSharedVersion: '42', sender: SENDER, principalBytes: PRINCIPAL, coins: [COIN_A], amountE8s: 1n, nonce: 1n };
    expect(() => buildReturnTransaction({ ...plan, subaccount: new Uint8Array(31) })).toThrow(RangeError);
    expect(() => buildReturnTransaction({ ...plan, subaccount: new Uint8Array(33) })).toThrow(RangeError);
    expect(() => buildReturnTransaction({ ...plan, subaccount: ZERO, amountE8s: COIN_A.balance + 1n })).toThrow(RangeError);
    expect(() => buildReturnTransaction({ ...plan, subaccount: ZERO, coins: [] })).toThrow();
  });

  it('takes the principal as its raw bytes (1 to 29), the way the Gate stores it', () => {
    const text = 'rrkah-fqaaa-aaaaa-aaaaq-cai';
    expect(Principal.fromText(text).toUint8Array()).toEqual(Uint8Array.from([0, 0, 0, 0, 0, 0, 0, 1, 1, 1]));
  });
});
