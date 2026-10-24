import { describe, expect, it, vi } from 'vitest';
import {
  CHECKSUM_ERROR_MESSAGE,
  CHECKSUM_UNVERIFIABLE_MESSAGE,
  checksumStatus,
  displayAddress,
  passesChecksumRule,
  toChecksumAddress,
} from './eip55';
import { keccak256 as configuredKeccak256 } from './keccak';
import { EIP55_VECTORS, MIXED_CASE_NO_CHECKSUM, ORACLE, flipCase } from './testSupport';

describe('toChecksumAddress', () => {
  it('reproduces every EIP-55 vector from its lowercase and uppercase forms', () => {
    for (const vector of EIP55_VECTORS) {
      expect(toChecksumAddress(vector.toLowerCase(), ORACLE)).toBe(vector);
      expect(toChecksumAddress(`0x${vector.slice(2).toUpperCase()}`, ORACLE)).toBe(vector);
      expect(toChecksumAddress(vector, ORACLE)).toBe(vector);
    }
  });

  it('hashes the lowercase hex text without the 0x prefix', () => {
    const spy = vi.fn(ORACLE);
    toChecksumAddress(EIP55_VECTORS[0], spy);
    expect(spy).toHaveBeenCalledTimes(1);
    const bytes = spy.mock.calls[0][0];
    expect(String.fromCharCode(...Array.from(bytes))).toBe('5aaeb6053f3e94c9b9a09f33669435e7ef1beaed');
  });

  it('returns null for malformed input, with no hash, or when the hash returns the wrong size', () => {
    expect(toChecksumAddress('0x1234', ORACLE)).toBeNull();
    expect(toChecksumAddress(`0x${'g'.repeat(40)}`, ORACLE)).toBeNull();
    expect(toChecksumAddress(EIP55_VECTORS[0], null)).toBeNull();
    expect(toChecksumAddress(EIP55_VECTORS[0], () => new Uint8Array(31))).toBeNull();
  });
});

describe('checksumStatus', () => {
  it('accepts the EIP-55 vectors as valid', () => {
    for (const vector of EIP55_VECTORS) expect(checksumStatus(vector, ORACLE)).toBe('valid');
  });

  it('treats all-lowercase and all-uppercase hex as unchecksummed, without needing a hash', () => {
    for (const vector of EIP55_VECTORS) {
      expect(checksumStatus(vector.toLowerCase(), null)).toBe('unchecksummed');
      expect(checksumStatus(`0x${vector.slice(2).toUpperCase()}`, null)).toBe('unchecksummed');
    }
    expect(checksumStatus('0x1111111111111111111111111111111111111111', null)).toBe('unchecksummed');
    expect(checksumStatus(`0x${'0'.repeat(40)}`, null)).toBe('unchecksummed');
  });

  it('rejects a one-character case flip anywhere in any vector', () => {
    for (const vector of EIP55_VECTORS) {
      let flipped = 0;
      for (let i = 0; i < 40; i++) {
        if (!/[a-fA-F]/.test(vector[i + 2])) continue;
        flipped += 1;
        expect(checksumStatus(flipCase(vector, i), ORACLE), `${vector} flipped at ${i}`).toBe('invalid');
      }
      expect(flipped).toBeGreaterThan(0);
    }
  });

  it('rejects a mixed-case address that was never checksummed at all', () => {
    expect(checksumStatus(MIXED_CASE_NO_CHECKSUM, ORACLE)).toBe('invalid');
  });

  it('cannot verify a mixed-case address without a hash and never calls it valid', () => {
    for (const vector of EIP55_VECTORS) expect(checksumStatus(vector, null)).toBe('unverifiable');
  });

  it('flags malformed input', () => {
    expect(checksumStatus('', ORACLE)).toBe('malformed');
    expect(checksumStatus('0x1234', ORACLE)).toBe('malformed');
    expect(checksumStatus(EIP55_VECTORS[0].slice(2), ORACLE)).toBe('malformed');
    expect(checksumStatus(`0x${'z'.repeat(40)}`, ORACLE)).toBe('malformed');
    expect(checksumStatus(undefined as unknown as string, ORACLE)).toBe('malformed');
  });
});

describe('passesChecksumRule', () => {
  it('passes unchecksummed and valid addresses only', () => {
    expect(passesChecksumRule(EIP55_VECTORS[1], ORACLE)).toBe(true);
    expect(passesChecksumRule(EIP55_VECTORS[1].toLowerCase(), ORACLE)).toBe(true);
    expect(passesChecksumRule(flipCase(EIP55_VECTORS[1], 0), ORACLE)).toBe(false);
    expect(passesChecksumRule(EIP55_VECTORS[1], null)).toBe(false);
    expect(passesChecksumRule('0x12', ORACLE)).toBe(false);
  });
});

describe('displayAddress', () => {
  it('shows the EIP-55 form when a hash is available', () => {
    expect(displayAddress(EIP55_VECTORS[2].toLowerCase(), ORACLE)).toBe(EIP55_VECTORS[2]);
  });

  it('falls back to lowercase without a hash and leaves non-addresses alone', () => {
    expect(displayAddress(EIP55_VECTORS[2], null)).toBe(EIP55_VECTORS[2].toLowerCase());
    expect(displayAddress('not an address', ORACLE)).toBe('not an address');
    expect(displayAddress('', ORACLE)).toBe('');
  });
});

describe('messages', () => {
  it('uses the exact inline wording and no em dashes', () => {
    expect(CHECKSUM_ERROR_MESSAGE).toBe('This address has a checksum error. Check for a typo, or paste it in all lowercase.');
    expect(CHECKSUM_UNVERIFIABLE_MESSAGE).toContain('all lowercase');
    expect(CHECKSUM_ERROR_MESSAGE + CHECKSUM_UNVERIFIABLE_MESSAGE).not.toContain('\u2014');
  });
});

describe('the configured Keccak-256 (keccak.ts)', () => {
  it.skipIf(configuredKeccak256 !== null)('is not wired yet, so mixed-case addresses fail closed', () => {
    for (const vector of EIP55_VECTORS) {
      expect(checksumStatus(vector)).toBe('unverifiable');
      expect(passesChecksumRule(vector)).toBe(false);
      expect(passesChecksumRule(vector.toLowerCase())).toBe(true);
      expect(displayAddress(vector)).toBe(vector.toLowerCase());
    }
  });

  it.skipIf(configuredKeccak256 === null)('is Keccak-256 (not NIST SHA3-256) and satisfies the EIP-55 vectors', () => {
    const hex = (bytes: Uint8Array) => Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
    expect(hex(configuredKeccak256!(new Uint8Array()))).toBe('c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470');
    expect(hex(configuredKeccak256!(new TextEncoder().encode('abc')))).toBe('4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45');
    for (const vector of EIP55_VECTORS) {
      expect(checksumStatus(vector)).toBe('valid');
      expect(displayAddress(vector.toLowerCase())).toBe(vector);
      expect(checksumStatus(flipCase(vector, vector.slice(2).search(/[a-fA-F]/)))).toBe('invalid');
    }
  });
});
