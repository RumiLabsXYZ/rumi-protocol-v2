import { describe, expect, it } from 'vitest';
import { formatBtcSats, isValidBitcoinMainnetAddress, parseBtcSats, txidBytesToHex } from './bitcoinMinterFlow';

describe('bitcoinMinterFlow', () => {
  it('parses BTC amounts as exact satoshis and rejects excess precision or Nat64 overflow', () => {
    expect(parseBtcSats('0.00000001')).toBe(1n);
    expect(parseBtcSats('1.23')).toBe(123_000_000n);
    expect(parseBtcSats('01')).toBeNull();
    expect(parseBtcSats('0.000000001')).toBeNull();
    expect(parseBtcSats('184467440737.09551616')).toBeNull();
  });

  it('formats satoshi amounts without floating point', () => {
    expect(formatBtcSats(123_000_001n)).toBe('1.23000001');
    expect(formatBtcSats(100_000_000n)).toBe('1');
  });

  it('renders the Bitcoin canister txid bytes in explorer display order', () => {
    expect(txidBytesToHex(Uint8Array.from([1, 2, 255]))).toBe('ff0201');
  });

  it('accepts checksummed mainnet Base58 addresses including mixed case', () => {
    expect(isValidBitcoinMainnetAddress('1BoatSLRHtKNngkdXEeobR76b53LETtpyT')).toBe(true);
    expect(isValidBitcoinMainnetAddress('1BoatSLRHtKNngkdXEeobR76b53LETtpyU')).toBe(false);
    expect(isValidBitcoinMainnetAddress('3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy')).toBe(true);
  });

  it('accepts valid lowercase bech32 and taproot addresses and rejects mixed case', () => {
    expect(isValidBitcoinMainnetAddress('bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4')).toBe(true);
    expect(isValidBitcoinMainnetAddress('bc1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3qccfmv3')).toBe(true);
    expect(isValidBitcoinMainnetAddress('bc1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqzk5jj0')).toBe(true);
    expect(isValidBitcoinMainnetAddress('tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx')).toBe(false);
    expect(isValidBitcoinMainnetAddress('bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kemeawh')).toBe(false);
    expect(isValidBitcoinMainnetAddress('bc1qW508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4')).toBe(false);
    expect(isValidBitcoinMainnetAddress('BC1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KV8F3T4')).toBe(true);
  });
});
