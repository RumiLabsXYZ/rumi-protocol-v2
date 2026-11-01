import { describe, expect, it } from 'vitest';
import {
  ERROR_SELECTORS,
  EVENT_TOPICS,
  SELECTORS,
  ZERO_BYTES32,
  bytesToHex,
  decodeBool,
  decodeBridgeOutData,
  decodeBytes32,
  decodeRevert,
  decodeUint256,
  encodeAddressWord,
  encodeBalanceOf,
  encodeBridgeOut,
  encodeBridgeOutUsed,
  encodeComputeTransferId,
  encodeDecimals,
  encodeUint256,
  extractRevertData,
  hexToBytes,
  isAddress,
  isBytes32,
  isHex,
  isZeroAddress,
  revertMessage,
  sameAddress,
} from './abi';

const PRINCIPAL = '0x00000000000000020101'; // 10 bytes
const SUBACCOUNT = `0x${'0'.repeat(61)}abc`;
const NONCE = `0x${'0'.repeat(62)}ff`;

describe('pinned selectors and topics', () => {
  it('matches the values produced by `cast sig` / `cast keccak`', () => {
    expect(SELECTORS).toEqual({
      bridgeOut: '0x803451ee',
      computeTransferId: '0xaee78e6a',
      bridgeOutUsed: '0x3e942d93',
      balanceOf: '0x70a08231',
      decimals: '0x313ce567',
    });
    expect(EVENT_TOPICS.BridgeOut).toBe('0x57c1323d2ddfd2271cfa8cd347d9cf1d9c8fdfe66100459f1f35561d847d3a4b');
    expect(EVENT_TOPICS.BridgeMint).toBe('0x07a55522f881c2542a34d1528374c51dba3994954e541d288d0cfd4fbc961dcc');
  });

  it('pins every custom error selector', () => {
    expect(ERROR_SELECTORS).toEqual({
      MintIsPaused: '0x58fc3182',
      BridgeOutIsPaused: '0x97221de2',
      AmountBelowMinimum: '0x30196ade',
      AmountTooLarge: '0x08083b2a',
      InvalidIcpPrincipal: '0x2ab131c7',
      BridgeOutAlreadyUsed: '0xc9dcda1c',
      ZeroAmount: '0x1f2a2005',
      ERC20InsufficientBalance: '0xe450d38c',
    });
  });
});

describe('bridgeOut calldata', () => {
  // cast calldata 'bridgeOut(uint256,bytes,bytes32,bytes32)' 2500000000 0x00000000000000020101 \
  //   0x...0abc 0x...00ff
  const EXPECTED =
    '0x803451ee' +
    '000000000000000000000000000000000000000000000000000000009502f900' + // amount 2500000000
    '0000000000000000000000000000000000000000000000000000000000000080' + // offset of the bytes tail
    '0000000000000000000000000000000000000000000000000000000000000abc' + // subaccount
    '00000000000000000000000000000000000000000000000000000000000000ff' + // client nonce
    '000000000000000000000000000000000000000000000000000000000000000a' + // principal length (10)
    '0000000000000002010100000000000000000000000000000000000000000000'; // principal, right padded

  it('equals the offline `cast calldata` vector', () => {
    const data = encodeBridgeOut({
      amountE8s: 2_500_000_000n,
      icpPrincipal: hexToBytes(PRINCIPAL),
      icpSubaccount: SUBACCOUNT,
      clientNonce: NONCE,
    });
    expect(data).toBe(EXPECTED);
  });

  it('is selector ++ amount ++ 0x80 ++ subaccount ++ nonce ++ len ++ padded principal', () => {
    const data = encodeBridgeOut({
      amountE8s: 2_500_000_000n,
      icpPrincipal: hexToBytes(PRINCIPAL),
      icpSubaccount: SUBACCOUNT,
      clientNonce: NONCE,
    });
    const body = data.slice(10);
    const words = body.match(/.{64}/g)!;
    expect(data.slice(0, 10)).toBe(SELECTORS.bridgeOut);
    expect(BigInt(`0x${words[0]}`)).toBe(2_500_000_000n);
    expect(BigInt(`0x${words[1]}`)).toBe(0x80n);
    expect(`0x${words[2]}`).toBe(SUBACCOUNT);
    expect(`0x${words[3]}`).toBe(NONCE);
    expect(BigInt(`0x${words[4]}`)).toBe(10n);
    expect(words[5]).toBe('0000000000000002010100000000000000000000000000000000000000000000');
    expect(words).toHaveLength(6);
  });

  it('pads a 29 byte principal to exactly one tail word', () => {
    const principal = new Uint8Array(29).fill(0xab);
    const data = encodeBridgeOut({ amountE8s: 1n, icpPrincipal: principal, icpSubaccount: ZERO_BYTES32, clientNonce: NONCE });
    const words = data.slice(10).match(/.{64}/g)!;
    expect(words).toHaveLength(6);
    expect(BigInt(`0x${words[4]}`)).toBe(29n);
    expect(words[5]).toBe('ab'.repeat(29) + '000000');
  });

  it('rejects an out of range amount or principal', () => {
    const base = { icpSubaccount: ZERO_BYTES32, clientNonce: NONCE };
    expect(() => encodeBridgeOut({ ...base, amountE8s: 0n, icpPrincipal: new Uint8Array(1) })).toThrow();
    expect(() => encodeBridgeOut({ ...base, amountE8s: 1n << 64n, icpPrincipal: new Uint8Array(1) })).toThrow();
    expect(() => encodeBridgeOut({ ...base, amountE8s: 1n, icpPrincipal: new Uint8Array(0) })).toThrow();
    expect(() => encodeBridgeOut({ ...base, amountE8s: 1n, icpPrincipal: new Uint8Array(30) })).toThrow();
    expect(() => encodeBridgeOut({ ...base, icpSubaccount: '0x12', amountE8s: 1n, icpPrincipal: new Uint8Array(1) })).toThrow();
  });
});

describe('view call encoders', () => {
  const ACCOUNT = '0x1111111111111111111111111111111111111111';

  it('encodes computeTransferId(address,bytes32)', () => {
    expect(encodeComputeTransferId(ACCOUNT, NONCE)).toBe(
      '0xaee78e6a' +
        '0000000000000000000000001111111111111111111111111111111111111111' +
        '00000000000000000000000000000000000000000000000000000000000000ff',
    );
  });

  it('lower-cases mixed-case addresses before encoding', () => {
    expect(encodeAddressWord('0xAbCdEf0123456789aBcDeF0123456789AbCdEf01')).toBe(
      '000000000000000000000000abcdef0123456789abcdef0123456789abcdef01',
    );
  });

  it('encodes bridgeOutUsed(bytes32), balanceOf(address) and decimals()', () => {
    expect(encodeBridgeOutUsed(NONCE)).toBe('0x3e942d9300000000000000000000000000000000000000000000000000000000000000ff');
    expect(encodeBalanceOf(ACCOUNT)).toBe('0x70a082310000000000000000000000001111111111111111111111111111111111111111');
    expect(encodeDecimals()).toBe('0x313ce567');
  });

  it('refuses malformed addresses and words', () => {
    expect(() => encodeBalanceOf('0x1234')).toThrow();
    expect(() => encodeBridgeOutUsed('0x1234')).toThrow();
    expect(() => encodeUint256(-1n)).toThrow();
  });
});

describe('result decoders', () => {
  it('decodes uint256, bool and bytes32 words', () => {
    expect(decodeUint256(`0x${encodeUint256(12_000_000n)}`)).toBe(12_000_000n);
    expect(decodeBool(`0x${encodeUint256(1n)}`)).toBe(true);
    expect(decodeBool(`0x${encodeUint256(0n)}`)).toBe(false);
    expect(() => decodeBool(`0x${encodeUint256(2n)}`)).toThrow();
    expect(decodeBytes32(NONCE)).toBe(NONCE);
    expect(() => decodeUint256('0x')).toThrow();
    expect(() => decodeUint256(undefined)).toThrow();
  });
});

describe('BridgeOut event data', () => {
  // amount 2500000000, sequence 1, principal 0x00000000000000020101, subaccount bytes32(0xabc)
  const EVENT_DATA =
    '0x' +
    '000000000000000000000000000000000000000000000000000000009502f900' +
    '0000000000000000000000000000000000000000000000000000000000000001' +
    '0000000000000000000000000000000000000000000000000000000000000080' +
    '0000000000000000000000000000000000000000000000000000000000000abc' +
    '000000000000000000000000000000000000000000000000000000000000000a' +
    '0000000000000002010100000000000000000000000000000000000000000000';

  it('decodes the pinned vector', () => {
    const decoded = decodeBridgeOutData(EVENT_DATA);
    expect(decoded.amountE8s).toBe(2_500_000_000n);
    expect(decoded.sequence).toBe(1n);
    expect(bytesToHex(decoded.icpPrincipal)).toBe(PRINCIPAL);
    expect(decoded.icpSubaccount).toBe(SUBACCOUNT);
  });

  it('rejects truncated or out-of-range data', () => {
    expect(() => decodeBridgeOutData(EVENT_DATA.slice(0, 200))).toThrow();
    const zeroLen = EVENT_DATA.replace(
      '000000000000000000000000000000000000000000000000000000000000000a',
      '0000000000000000000000000000000000000000000000000000000000000000',
    );
    expect(() => decodeBridgeOutData(zeroLen)).toThrow();
  });
});

describe('revert decoding', () => {
  // Vectors from `cast calldata '<Error>(...)' ...`
  const BELOW_MIN =
    '0x30196ade' +
    '0000000000000000000000000000000000000000000000000000000002faf080' +
    '0000000000000000000000000000000000000000000000000000000005f5e100';
  const INSUFFICIENT =
    '0xe450d38c' +
    '0000000000000000000000001111111111111111111111111111111111111111' +
    '0000000000000000000000000000000000000000000000000000000005f5e100' +
    '000000000000000000000000000000000000000000000000000000000ee6b280';
  const ERROR_BOOM =
    '0x08c379a0' +
    '0000000000000000000000000000000000000000000000000000000000000020' +
    '0000000000000000000000000000000000000000000000000000000000000004' +
    '626f6f6d00000000000000000000000000000000000000000000000000000000';

  it('decodes each custom error with arguments', () => {
    expect(decodeRevert(BELOW_MIN)).toMatchObject({
      name: 'AmountBelowMinimum',
      args: [50_000_000n, 100_000_000n],
    });
    expect(decodeRevert(BELOW_MIN)!.message).toContain('0.5 icUSD');
    expect(decodeRevert(BELOW_MIN)!.message).toContain('minimum of 1 icUSD');
    expect(decodeRevert(INSUFFICIENT)).toMatchObject({ name: 'ERC20InsufficientBalance', args: [100_000_000n, 250_000_000n] });
    expect(decodeRevert(INSUFFICIENT)!.message).toContain('Balance 1 icUSD');
    expect(decodeRevert('0xc9dcda1c' + '00'.repeat(31) + 'ff')).toMatchObject({ name: 'BridgeOutAlreadyUsed' });
    expect(decodeRevert('0x08083b2a' + '00'.repeat(31) + '05')).toMatchObject({ name: 'AmountTooLarge', args: [5n] });
  });

  it('decodes the argument-less errors', () => {
    expect(decodeRevert('0x58fc3182')).toMatchObject({ name: 'MintIsPaused' });
    expect(decodeRevert('0x97221de2')!.message).toMatch(/paused/i);
    expect(decodeRevert('0x2ab131c7')).toMatchObject({ name: 'InvalidIcpPrincipal' });
    expect(decodeRevert('0x1f2a2005')).toMatchObject({ name: 'ZeroAmount' });
  });

  it('decodes Error(string) and returns null for unknown or short data', () => {
    expect(decodeRevert(ERROR_BOOM)).toMatchObject({ name: 'Error', message: 'boom' });
    expect(decodeRevert('0xdeadbeef')).toBeNull();
    expect(decodeRevert('0x')).toBeNull();
    expect(decodeRevert(undefined)).toBeNull();
    expect(decodeRevert('not hex')).toBeNull();
  });

  it('finds revert data in the usual nested wallet error shapes', () => {
    expect(extractRevertData({ code: -32603, data: { code: 3, message: 'execution reverted', data: '0x97221de2' } })).toBe('0x97221de2');
    expect(extractRevertData({ error: { data: '0x97221de2' } })).toBe('0x97221de2');
    expect(extractRevertData({ data: '0x97221de2' })).toBe('0x97221de2');
    expect(extractRevertData({ message: 'nope' })).toBeNull();
    expect(revertMessage({ data: { data: '0x97221de2' } })).toMatch(/paused/i);
    expect(revertMessage(new Error('x'))).toBeNull();
  });

  it('survives a self-referencing error object', () => {
    const loop: Record<string, unknown> = {};
    loop.error = loop;
    expect(extractRevertData(loop)).toBeNull();
  });
});

describe('hex helpers', () => {
  it('validates hex strings and addresses', () => {
    expect(isHex('0x')).toBe(true);
    expect(isHex('0xabc')).toBe(false); // odd length
    expect(isHex('abcd')).toBe(false);
    expect(isAddress('0x1111111111111111111111111111111111111111')).toBe(true);
    expect(isAddress('0x11')).toBe(false);
    expect(isBytes32(NONCE)).toBe(true);
    expect(isZeroAddress('0x0000000000000000000000000000000000000000')).toBe(true);
    expect(isZeroAddress('0x0000000000000000000000000000000000000001')).toBe(false);
    expect(sameAddress('0xAA', '0xaa')).toBe(true);
    expect(sameAddress(null, '0xaa')).toBe(false);
  });

  it('round-trips bytes and hex', () => {
    const bytes = Uint8Array.of(0, 1, 2, 255);
    expect(bytesToHex(bytes)).toBe('0x000102ff');
    expect(Array.from(hexToBytes('0x000102ff'))).toEqual([0, 1, 2, 255]);
    expect(() => hexToBytes('000102ff')).toThrow();
  });
});
