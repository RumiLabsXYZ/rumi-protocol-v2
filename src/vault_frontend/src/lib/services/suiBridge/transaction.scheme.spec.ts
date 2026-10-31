import { Principal } from '@dfinity/principal';
import { fromBase64 } from '@mysten/sui/utils';
import { describe, expect, it, vi } from 'vitest';

// The SDK needs a Sui client to verify a zkLogin signature. This spec stands in for exactly that SDK error; no wallet or node is involved.
vi.mock('@mysten/sui/verify', () => ({
  verifyTransactionSignature: async () => {
    throw new Error('A Sui Client (with the core API) is required to verify zkLogin signatures');
  },
}));

import { KEYS, signLikeAWallet } from './testSupport';
import { buildReturnTransaction, validateSignedReturn } from './transaction';

describe('a signature scheme this page cannot check offline', () => {
  it('is reported as unsupported, not as a bad signature, and is still never accepted', async () => {
    const plan = {
      packageId: KEYS.packageId,
      gateId: KEYS.gateId,
      gateInitialSharedVersion: '42',
      sender: KEYS.holder,
      coins: [{ objectId: KEYS.coinA, version: '7', digest: '4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi', balance: 5n }],
      amountE8s: 5n,
      principalBytes: Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai').toUint8Array(),
      subaccount: new Uint8Array(32),
      nonce: 1n,
    };
    const signed = await signLikeAWallet(buildReturnTransaction(plan));
    const check = await validateSignedReturn(fromBase64(signed.bytes), signed.signature, { ...plan, coinIds: [KEYS.coinA], coinTotalE8s: 5n });
    expect(check).toMatchObject({ ok: false, reason: 'unsupported_signature' });
    expect((check as { message: string }).message).toContain('NOT found invalid');
    expect((check as { message: string }).message).not.toContain('does not verify');
  });
});
