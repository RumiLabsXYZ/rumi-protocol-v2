import { describe, expect, it } from 'vitest';
import { isCurrentVaultPullStatusRead } from './vaultPullRecoveryViewState';

describe('isCurrentVaultPullStatusRead', () => {
  it('accepts a result only for the current generation and connected owner', () => {
    expect(isCurrentVaultPullStatusRead(3, 3, 'owner-a', 'owner-a')).toBe(true);
    expect(isCurrentVaultPullStatusRead(2, 3, 'owner-a', 'owner-a')).toBe(false);
    expect(isCurrentVaultPullStatusRead(3, 3, 'owner-a', '')).toBe(false);
    expect(isCurrentVaultPullStatusRead(3, 3, 'owner-a', 'owner-b')).toBe(false);
  });
});
