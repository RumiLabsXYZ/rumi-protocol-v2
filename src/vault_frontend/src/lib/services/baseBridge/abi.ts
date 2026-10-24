/**
 * Hand-rolled ABI helpers for the `BridgedIcUSD` Base contract.
 *
 * No EVM library is used: the surface is five view/write calls, one event and
 * eight custom errors, all of whose selectors are pinned here and verified in
 * `abi.spec.ts` against vectors produced offline with `cast`.
 *
 *   bridgeOut(uint256 amount, bytes icpPrincipal, bytes32 icpSubaccount, bytes32 clientNonce)
 *   computeTransferId(address from, bytes32 clientNonce) view returns (bytes32)
 *   bridgeOutUsed(bytes32 transferId) view returns (bool)
 *   balanceOf(address) view returns (uint256)
 *   decimals() view returns (uint8)
 *
 *   event BridgeOut(bytes32 indexed transferId, address indexed from, uint256 amount,
 *                   uint64 sequence, bytes icpPrincipal, bytes32 icpSubaccount)
 *   event BridgeMint(bytes32 indexed mintId, address indexed to, uint256 amount)
 */

import { U64_MAX } from './amounts';

export const SELECTORS = {
  bridgeOut: '0x803451ee',
  computeTransferId: '0xaee78e6a',
  bridgeOutUsed: '0x3e942d93',
  balanceOf: '0x70a08231',
  decimals: '0x313ce567',
} as const;

export const EVENT_TOPICS = {
  BridgeOut: '0x57c1323d2ddfd2271cfa8cd347d9cf1d9c8fdfe66100459f1f35561d847d3a4b',
  BridgeMint: '0x07a55522f881c2542a34d1528374c51dba3994954e541d288d0cfd4fbc961dcc',
} as const;

export const ERROR_SELECTORS = {
  MintIsPaused: '0x58fc3182',
  BridgeOutIsPaused: '0x97221de2',
  AmountBelowMinimum: '0x30196ade',
  AmountTooLarge: '0x08083b2a',
  InvalidIcpPrincipal: '0x2ab131c7',
  BridgeOutAlreadyUsed: '0xc9dcda1c',
  ZeroAmount: '0x1f2a2005',
  ERC20InsufficientBalance: '0xe450d38c',
} as const;

/** Solidity's built-in `Error(string)` and `Panic(uint256)` selectors. */
const ERROR_STRING_SELECTOR = '0x08c379a0';
const PANIC_SELECTOR = '0x4e487b71';

export const ZERO_BYTES32 = `0x${'00'.repeat(32)}`;
export const MAX_ICP_PRINCIPAL_LEN = 29;

// ---------------------------------------------------------------------------
// Hex helpers
// ---------------------------------------------------------------------------

export function strip0x(hex: string): string {
  return hex.startsWith('0x') || hex.startsWith('0X') ? hex.slice(2) : hex;
}

/** True for `0x`-prefixed hex; with `bytes` set, also requires that exact length. */
export function isHex(value: unknown, bytes?: number): value is string {
  if (typeof value !== 'string' || !/^0x[0-9a-fA-F]*$/.test(value)) return false;
  const digits = value.length - 2;
  if (digits % 2 !== 0) return false;
  return bytes === undefined ? true : digits === bytes * 2;
}

export function isAddress(value: unknown): value is string {
  return isHex(value, 20);
}

export function isBytes32(value: unknown): value is string {
  return isHex(value, 32);
}

export function isZeroAddress(value: string): boolean {
  return /^0x0{40}$/i.test(value);
}

export function sameAddress(a: string | null | undefined, b: string | null | undefined): boolean {
  return !!a && !!b && a.toLowerCase() === b.toLowerCase();
}

export function bytesToHex(bytes: Uint8Array | ArrayLike<number>): string {
  let out = '0x';
  for (let i = 0; i < bytes.length; i++) out += (bytes[i] & 0xff).toString(16).padStart(2, '0');
  return out;
}

export function hexToBytes(hex: string): Uint8Array {
  if (!isHex(hex)) throw new Error('Expected 0x-prefixed hex with an even number of digits.');
  const digits = strip0x(hex);
  const out = new Uint8Array(digits.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(digits.slice(i * 2, i * 2 + 2), 16);
  return out;
}

// ---------------------------------------------------------------------------
// Word encoders
// ---------------------------------------------------------------------------

export function encodeUint256(value: bigint): string {
  if (value < 0n || value >= 1n << 256n) throw new RangeError('uint256 out of range');
  return value.toString(16).padStart(64, '0');
}

export function encodeAddressWord(address: string): string {
  if (!isAddress(address)) throw new Error('Invalid Base address.');
  return strip0x(address).toLowerCase().padStart(64, '0');
}

export function encodeBytes32Word(value: string): string {
  if (!isBytes32(value)) throw new Error('Expected a 32-byte hex value.');
  return strip0x(value).toLowerCase();
}

// ---------------------------------------------------------------------------
// Call encoders
// ---------------------------------------------------------------------------

export interface BridgeOutArgs {
  amountE8s: bigint;
  /** Raw ICP principal bytes (1 to 29 bytes, `Principal.toUint8Array()`). */
  icpPrincipal: Uint8Array;
  /** 32-byte subaccount as 0x hex. All zero means the default subaccount. */
  icpSubaccount: string;
  clientNonce: string;
}

/**
 * calldata = selector ++ amount ++ 0x80 ++ subaccount ++ nonce ++ len ++ principal (right padded).
 * The dynamic `bytes` argument is the third word of the head, so its offset is always 4 * 32 = 0x80.
 */
export function encodeBridgeOut(args: BridgeOutArgs): string {
  if (args.amountE8s <= 0n || args.amountE8s > U64_MAX) throw new RangeError('bridgeOut amount must be 1..u64::MAX e8s.');
  const len = args.icpPrincipal.length;
  if (len < 1 || len > MAX_ICP_PRINCIPAL_LEN) throw new RangeError('ICP principal must be 1 to 29 bytes.');
  const paddedLen = Math.ceil(len / 32) * 32;
  const body = new Uint8Array(paddedLen);
  body.set(args.icpPrincipal, 0);
  return (
    SELECTORS.bridgeOut +
    encodeUint256(args.amountE8s) +
    encodeUint256(0x80n) +
    encodeBytes32Word(args.icpSubaccount) +
    encodeBytes32Word(args.clientNonce) +
    encodeUint256(BigInt(len)) +
    strip0x(bytesToHex(body))
  );
}

export function encodeComputeTransferId(from: string, clientNonce: string): string {
  return SELECTORS.computeTransferId + encodeAddressWord(from) + encodeBytes32Word(clientNonce);
}

export function encodeBridgeOutUsed(transferId: string): string {
  return SELECTORS.bridgeOutUsed + encodeBytes32Word(transferId);
}

export function encodeBalanceOf(account: string): string {
  return SELECTORS.balanceOf + encodeAddressWord(account);
}

export function encodeDecimals(): string {
  return SELECTORS.decimals;
}

// ---------------------------------------------------------------------------
// Decoders
// ---------------------------------------------------------------------------

function wordAt(data: string, index: number): string {
  const digits = strip0x(data);
  const start = index * 64;
  if (digits.length < start + 64) throw new Error('ABI data is too short.');
  return digits.slice(start, start + 64);
}

export function decodeUint256(data: unknown): bigint {
  if (!isHex(data) || strip0x(data as string).length < 64) throw new Error('Expected a uint256 result.');
  return BigInt(`0x${wordAt(data as string, 0)}`);
}

export function decodeBool(data: unknown): boolean {
  const value = decodeUint256(data);
  if (value > 1n) throw new Error('Expected a bool result.');
  return value === 1n;
}

export function decodeBytes32(data: unknown): string {
  if (!isHex(data) || strip0x(data as string).length < 64) throw new Error('Expected a bytes32 result.');
  return `0x${wordAt(data as string, 0)}`;
}

export interface BridgeOutEventData {
  amountE8s: bigint;
  sequence: bigint;
  icpPrincipal: Uint8Array;
  icpSubaccount: string;
}

/** Decode the non-indexed `BridgeOut` data: (amount, sequence, bytes principal, bytes32 subaccount). */
export function decodeBridgeOutData(data: string): BridgeOutEventData {
  if (!isHex(data)) throw new Error('BridgeOut data must be hex.');
  const amountE8s = BigInt(`0x${wordAt(data, 0)}`);
  const sequence = BigInt(`0x${wordAt(data, 1)}`);
  const offset = Number(BigInt(`0x${wordAt(data, 2)}`));
  const icpSubaccount = `0x${wordAt(data, 3)}`;
  if (offset % 32 !== 0) throw new Error('BridgeOut data has an unaligned bytes offset.');
  const lenWord = wordAt(data, offset / 32);
  const len = Number(BigInt(`0x${lenWord}`));
  if (len < 1 || len > MAX_ICP_PRINCIPAL_LEN) throw new Error('BridgeOut principal length is out of range.');
  const digits = strip0x(data);
  const start = (offset / 32 + 1) * 64;
  if (digits.length < start + len * 2) throw new Error('BridgeOut data is truncated.');
  return {
    amountE8s,
    sequence,
    icpPrincipal: hexToBytes(`0x${digits.slice(start, start + len * 2)}`),
    icpSubaccount,
  };
}

// ---------------------------------------------------------------------------
// Revert decoding
// ---------------------------------------------------------------------------

export interface DecodedRevert {
  /** Custom error name, `Error`, or `Panic`. */
  name: string;
  /** Plain-language explanation safe to show to a user. */
  message: string;
  args: bigint[];
}

function fmt8(value: bigint): string {
  const whole = value / 100_000_000n;
  const fraction = (value % 100_000_000n).toString().padStart(8, '0').replace(/0+$/, '');
  return fraction ? `${whole}.${fraction}` : whole.toString();
}

/** Decode revert data from a failed `eth_call`, `eth_estimateGas` or wallet error. */
export function decodeRevert(data: string | null | undefined): DecodedRevert | null {
  if (!isHex(data) || data.length < 10) return null;
  const selector = data.slice(0, 10).toLowerCase();
  const body = `0x${data.slice(10)}`;
  const word = (i: number): bigint => BigInt(`0x${wordAt(body, i)}`);
  try {
    switch (selector) {
      case ERROR_SELECTORS.MintIsPaused:
        return { name: 'MintIsPaused', message: 'Minting on Base is paused.', args: [] };
      case ERROR_SELECTORS.BridgeOutIsPaused:
        return { name: 'BridgeOutIsPaused', message: 'Bridging from Base to ICP is paused right now.', args: [] };
      case ERROR_SELECTORS.AmountBelowMinimum: {
        const [amount, minimum] = [word(0), word(1)];
        return {
          name: 'AmountBelowMinimum',
          message: `The amount (${fmt8(amount)} icUSD) is below the minimum of ${fmt8(minimum)} icUSD.`,
          args: [amount, minimum],
        };
      }
      case ERROR_SELECTORS.AmountTooLarge:
        return { name: 'AmountTooLarge', message: 'The amount is too large for the bridge.', args: [word(0)] };
      case ERROR_SELECTORS.InvalidIcpPrincipal:
        return { name: 'InvalidIcpPrincipal', message: 'The ICP destination principal is not valid for the bridge.', args: [] };
      case ERROR_SELECTORS.BridgeOutAlreadyUsed:
        return {
          name: 'BridgeOutAlreadyUsed',
          message: 'This bridge request was already sent from this account. Check its status instead of sending again.',
          args: [word(0)],
        };
      case ERROR_SELECTORS.ZeroAmount:
        return { name: 'ZeroAmount', message: 'The amount must be greater than zero.', args: [] };
      case ERROR_SELECTORS.ERC20InsufficientBalance: {
        const [, balance, needed] = [word(0), word(1), word(2)];
        return {
          name: 'ERC20InsufficientBalance',
          message: `Not enough icUSD on Base. Balance ${fmt8(balance)} icUSD, needed ${fmt8(needed)} icUSD.`,
          args: [balance, needed],
        };
      }
      case ERROR_STRING_SELECTOR: {
        const offset = Number(word(0));
        const len = Number(BigInt(`0x${wordAt(body, offset / 32)}`));
        const digits = strip0x(body);
        const start = (offset / 32 + 1) * 64;
        const bytes = hexToBytes(`0x${digits.slice(start, start + len * 2)}`);
        const text = new TextDecoder().decode(bytes);
        return { name: 'Error', message: text || 'The transaction would revert.', args: [] };
      }
      case PANIC_SELECTOR:
        return { name: 'Panic', message: 'The contract call failed an internal check.', args: [word(0)] };
      default:
        return null;
    }
  } catch {
    return null;
  }
}

/** Walk the usual wallet error shapes looking for revert data. */
export function extractRevertData(error: unknown): string | null {
  const seen = new Set<unknown>();
  const visit = (value: unknown, depth: number): string | null => {
    if (depth > 5 || value === null || value === undefined || seen.has(value)) return null;
    if (typeof value === 'string') return isHex(value) && value.length >= 10 ? value : null;
    if (typeof value !== 'object') return null;
    seen.add(value);
    const record = value as Record<string, unknown>;
    for (const key of ['data', 'error', 'originalError', 'cause', 'info', 'revert']) {
      const found = visit(record[key], depth + 1);
      if (found) return found;
    }
    return null;
  };
  return visit(error, 0);
}

/** Best user-facing sentence for a failed read, estimate or send. */
export function revertMessage(error: unknown): string | null {
  const decoded = decodeRevert(extractRevertData(error));
  return decoded ? decoded.message : null;
}
