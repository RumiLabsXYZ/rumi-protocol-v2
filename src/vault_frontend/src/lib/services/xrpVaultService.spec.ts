import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { readCachedPendingDeposits, writeCachedPendingDeposits, XrpVaultService } from './xrpVaultService';
import { Principal } from '@dfinity/principal';
import * as oisy from './oisySigner';
import { ApiClient } from './protocol/apiClient';
import { walletStore } from '../stores/wallet';
import { currentWalletType, WALLET_TYPES } from './auth';

const OWNER = 'aaaaa-aa';
const CACHE_KEY = `rumi_xrp_pending_deposits:${OWNER}`;

describe('XRP pending-deposit cache', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
  });

  // Regression: reserveBaseDrops is a nat64 (bigint). JSON.stringify threw
  // "Do not know how to serialize a BigInt" AFTER open_xrp_vault had already
  // landed on-chain, so the UI reported "Could not prepare XRP address" and
  // never showed the custody address.
  it('round-trips bigint drops through localStorage', () => {
    writeCachedPendingDeposits(
      [{ vaultId: 7, custodyAddress: 'rCustody', openedAtMs: 1_700_000_000_000, reserveBaseDrops: 1_000_000n }],
      OWNER
    );

    expect(JSON.parse(localStorage.getItem(CACHE_KEY)!)[0].reserveBaseDrops).toBe('1000000');
    expect(readCachedPendingDeposits(OWNER)).toEqual([
      { vaultId: 7, custodyAddress: 'rCustody', openedAtMs: 1_700_000_000_000, reserveBaseDrops: 1_000_000n },
    ]);
  });

  it('revives legacy number drops and defaults unusable values to zero', () => {
    localStorage.setItem(
      CACHE_KEY,
      JSON.stringify([
        { vaultId: 1, custodyAddress: 'rLegacy', openedAtMs: 1, updatedAtMs: 1, reserveBaseDrops: 1_000_000 },
        { vaultId: 2, custodyAddress: 'rMissing', openedAtMs: 2, updatedAtMs: 2 },
      ])
    );

    expect(readCachedPendingDeposits(OWNER).map((p) => p.reserveBaseDrops)).toEqual([1_000_000n, 0n]);
  });

  it('never throws when localStorage rejects the write', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('QuotaExceededError');
    });

    expect(() =>
      writeCachedPendingDeposits(
        [{ vaultId: 9, custodyAddress: 'rCustody', openedAtMs: 1, reserveBaseDrops: 1_000_000n }],
        OWNER
      )
    ).not.toThrow();
  });
});

describe('readMyPendingDeposits (tri-state)', () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });

  it('a failed read is observable (not an empty list)', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    vi.spyOn(walletStore, 'getActor').mockResolvedValue({ get_my_xrp_pending_deposits: async () => { throw new Error('replica down'); } } as any);
    expect(await XrpVaultService.readMyPendingDeposits()).toEqual({ ok: false, error: 'replica down' });
    // the legacy reader keeps its behavior (empty list on failure)
    expect(await XrpVaultService.getMyPendingDeposits()).toEqual([]);
  });

  it('returns backend deposits unfiltered by user-hidden ids', async () => {
    vi.spyOn(walletStore, 'getActor').mockResolvedValue({
      get_my_xrp_pending_deposits: async () => [[7n, { custody_address: 'rC', opened_at_ns: 5_000_000n, reserve_base_drops: 1_000_000n }]],
    } as any);
    const r = await XrpVaultService.readMyPendingDeposits();
    expect(r).toEqual({ ok: true, source: 'backend', deposits: [{ vaultId: 7, custodyAddress: 'rC', openedAtMs: 5, reserveBaseDrops: 1_000_000n }] });
  });
});

describe('readMyPendingDeposits for a gesture-gated signer (Oisy)', () => {
  beforeEach(() => { localStorage.clear(); vi.restoreAllMocks(); });
  afterEach(() => { currentWalletType.set(null); });

  it('an EMPTY cache is "unknown", never a clean empty answer', async () => {
    currentWalletType.set(WALLET_TYPES.OISY);
    const actorSpy = vi.spyOn(walletStore, 'getActor');
    const r = await XrpVaultService.readMyPendingDeposits();
    expect(r).toEqual({ ok: true, deposits: [], source: 'cache', unknown: true });
    expect(actorSpy).not.toHaveBeenCalled(); // no signer popup
  });

  it('a non-empty cache is returned as a cache hit (not flagged unknown)', async () => {
    currentWalletType.set(WALLET_TYPES.OISY);
    vi.spyOn(walletStore, 'subscribe').mockImplementation(((fn: (s: unknown) => void) => {
      fn({ principal: { toText: () => OWNER } });
      return () => {};
    }) as any);
    writeCachedPendingDeposits(
      [{ vaultId: 4, custodyAddress: 'rC', openedAtMs: 1, reserveBaseDrops: 1_000_000n }],
      OWNER
    );
    const r = await XrpVaultService.readMyPendingDeposits();
    expect(r.ok && r.source).toBe('cache');
    expect(r.ok && r.unknown).toBeUndefined();
  });
});

describe('borrower XRP claims', () => {
  const owner = Principal.fromText(OWNER);
  const other = Principal.anonymous();
  let current = true;
  const ctx = { expectedPrincipalText: OWNER, assertCurrent: () => current };
  const actor = {
    get_my_xrp_claims: vi.fn(),
    settle_xrp_claim: vi.fn(),
    settle_xrp_claim_with_tag: vi.fn(),
  };

  beforeEach(() => {
    current = true;
    vi.restoreAllMocks();
    vi.resetAllMocks();
    vi.spyOn(walletStore, 'subscribe').mockImplementation(((fn: (state: unknown) => void) => {
      fn({ principal: owner });
      return () => {};
    }) as any);
    vi.spyOn(walletStore, 'getActor').mockResolvedValue(actor);
    vi.spyOn(ApiClient, 'executeSequentialOperation').mockImplementation((operation) => operation());
    currentWalletType.set(WALLET_TYPES.INTERNET_IDENTITY);
  });

  afterEach(() => {
    currentWalletType.set(null);
    vi.restoreAllMocks();
  });

  it('lists only claims owned by the connected borrower against their own custody address', async () => {
    const claim = {
      claimant: owner, custody_owner: owner, custody_nonce: 208n, drops: 275174474n,
      created_at_ns: 123n, settlement: [], quarantine_reason: [],
    };
    actor.get_my_xrp_claims.mockResolvedValue([
      [20n, claim],
      [21n, { ...claim, custody_owner: other }],
      [22n, { ...claim, claimant: other }],
    ]);
    const result = await XrpVaultService.readMyClaims(ctx);
    expect(result).toHaveLength(1);
    expect(result[0]).toMatchObject({ claimId: '20', drops: 275174474n, vaultId: 208, inFlight: false });
    actor.get_my_xrp_claims.mockRejectedValue(new Error('wallet unavailable'));
    await expect(XrpVaultService.readMyClaims(ctx)).rejects.toThrow('wallet unavailable');
  });

  it.each([undefined, 0, 4294967295])('opens the Oisy channel from the click and settles using destination tag %s', async tag => {
    currentWalletType.set(WALLET_TYPES.OISY);
    const open = vi.spyOn(oisy, 'openOisySignerChannel').mockResolvedValue();
    vi.spyOn(oisy, 'getOisySignerAgent').mockResolvedValue({});
    vi.spyOn(oisy, 'createOisyActor').mockReturnValue(actor);
    actor.get_my_xrp_claims.mockResolvedValue([]);
    const lookup = XrpVaultService.readMyClaims(ctx);
    expect(open).toHaveBeenCalledWith(owner);
    await expect(lookup).resolves.toEqual([]);
    const method = tag === undefined ? actor.settle_xrp_claim : actor.settle_xrp_claim_with_tag;
    method.mockResolvedValue({ Ok: 'hash' });
    const result = XrpVaultService.settleXrpClaim('20', 'rRecipient', tag, ctx);
    expect(open).toHaveBeenCalledTimes(2);
    await expect(result).resolves.toMatchObject({ success: true, data: { txHash: 'hash' } });
    expect(method.mock.calls).toEqual([tag === undefined ? [20n, 'rRecipient'] : [20n, 'rRecipient', tag]]);
    expect(walletStore.getActor).not.toHaveBeenCalled();
  });

  it('prevents queued settlement when the connected wallet changes before submission', async () => {
    vi.mocked(ApiClient.executeSequentialOperation).mockImplementation(async (operation) => {
      current = false;
      return operation();
    });
    const result = await XrpVaultService.settleXrpClaim('20', 'rRecipient', undefined, ctx);
    expect(result.success).toBe(false);
    expect(actor.settle_xrp_claim).not.toHaveBeenCalled();
  });

  it('discards a claim lookup if the wallet changes before the response', async () => {
    actor.get_my_xrp_claims.mockImplementation(async () => { current = false; return []; });
    await expect(XrpVaultService.readMyClaims(ctx)).rejects.toThrow('Wallet session changed');
  });
});
