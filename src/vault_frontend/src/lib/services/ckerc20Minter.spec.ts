import { describe, expect, it } from 'vitest';
import { validateEthereumAddress } from './ckerc20Minter';

describe('validateEthereumAddress', () => {
  it('accepts EIP-55 checksums and legacy uniform-case addresses', () => {
    expect(validateEthereumAddress('0x52908400098527886E0F7030069857D2E4169EE7')).toBe(true);
    expect(validateEthereumAddress('0xde709f2102306220921060314715629080e2fb77')).toBe(true);
    expect(validateEthereumAddress('0xDE709F2102306220921060314715629080E2FB77')).toBe(true);
  });

  it('rejects invalid mixed-case checksums and the zero address', () => {
    expect(validateEthereumAddress('0x52908400098527886e0F7030069857D2E4169EE7')).toBe(false);
    expect(validateEthereumAddress(`0x${'0'.repeat(40)}`)).toBe(false);
  });

  it('rejects malformed address strings', () => {
    expect(validateEthereumAddress('0x1234')).toBe(false);
    expect(validateEthereumAddress('not-an-address')).toBe(false);
  });
});
