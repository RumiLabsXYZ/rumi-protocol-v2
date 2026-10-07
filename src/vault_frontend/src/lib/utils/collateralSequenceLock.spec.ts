import { describe, expect, it } from 'vitest';
import { collateralSequenceLockName } from './collateralSequenceLock';
import { bitcoinBorrowActionLockName } from './bitcoinBorrowWizard';
import { dogeBorrowActionLockName } from './dogeBorrowWizard';
import { repaymentActionLockName } from './repaymentActionLock';

describe('collateral sequence lock namespace', () => {
  it('shares one owner/network/ledger lock across the root, BTC, DOGE, and existing-vault callers', () => {
    const owner = 'owner-principal';
    const network = 'mainnet';
    const ledger = 'shared-collateral-ledger';
    const shared = collateralSequenceLockName(owner, network, ledger);

    expect(bitcoinBorrowActionLockName(owner, network, ledger)).toBe(shared);
    expect(dogeBorrowActionLockName(owner, network, ledger)).toBe(shared);
    expect(collateralSequenceLockName(owner, network, 'other-ledger')).not.toBe(shared);
    expect(collateralSequenceLockName('other-owner', network, ledger)).not.toBe(shared);
    expect(collateralSequenceLockName(owner, 'local', ledger)).not.toBe(shared);
  });
});

describe('repayment request lock namespace', () => {
  it('serializes owner-global repayment request IDs across vaults and repay modes', () => {
    expect(repaymentActionLockName('owner', 'mainnet')).toBe(repaymentActionLockName('owner', 'mainnet'));
    expect(repaymentActionLockName('owner', 'mainnet')).not.toBe(repaymentActionLockName('other-owner', 'mainnet'));
    expect(repaymentActionLockName('owner', 'mainnet')).not.toBe(repaymentActionLockName('owner', 'local'));
  });
});
