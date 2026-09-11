import { describe, it, expect, vi, beforeEach } from 'vitest';

// ──────────────────────────────────────────────────────────────
// The regression this guards: the official ckDOGE minter doesn't implement
// icrc21_canister_call_consent_message, so Oisy's ICRC-21 signer rejects
// consent for every call routed through walletStore.getActor — including
// read-only methods like get_doge_address. Public/non-debiting methods must
// go through a plain anonymous HttpAgent actor instead; only the
// debit-authorizing retrieve_doge_with_approval call may use the wallet actor.
// ──────────────────────────────────────────────────────────────

const mocks = vi.hoisted(() => ({
  createActor: vi.fn((_idl: any, _options: { agent: any; canisterId: string }) => ({
    __kind: 'public-actor',
  })),
  fetchRootKey: vi.fn().mockResolvedValue(undefined),
  walletGetActor: vi.fn(async () => ({ __kind: 'wallet-actor' })),
}));

vi.mock('@dfinity/agent', async () => {
  const actual = await vi.importActual<typeof import('@dfinity/agent')>('@dfinity/agent');
  return {
    ...actual,
    Actor: {
      ...actual.Actor,
      createActor: mocks.createActor,
    },
    HttpAgent: vi.fn(() => ({
      fetchRootKey: mocks.fetchRootKey,
    })),
    AnonymousIdentity: vi.fn(() => ({})),
  };
});

vi.mock('../config', () => ({
  CONFIG: { host: 'https://icp0.io', isLocal: false },
  CANISTER_IDS: { CKDOGE_MINTER: 'eqltq-xqaaa-aaaar-qb3vq-cai' },
}));

vi.mock('../stores/wallet', () => ({
  walletStore: { getActor: mocks.walletGetActor },
}));

vi.mock('../idls/ckdoge_minter.idl.js', () => ({
  idlFactory: { name: 'ckdoge_minter' },
}));

import {
  getPublicMinterActor,
  getWalletMinterActor,
  _resetCkdogeMinterAnonAgent,
} from './ckdogeMinterActors';

describe('ckdogeMinterActors', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    _resetCkdogeMinterAnonAgent();
  });

  describe('getPublicMinterActor', () => {
    it('builds an actor from an anonymous HttpAgent, not the wallet actor', async () => {
      const actor = await getPublicMinterActor();

      expect(actor).toEqual({ __kind: 'public-actor' });
      expect(mocks.createActor).toHaveBeenCalledTimes(1);
      expect(mocks.walletGetActor).not.toHaveBeenCalled();
    });

    it('passes the ckDOGE minter canister id and an agent to Actor.createActor', async () => {
      await getPublicMinterActor();

      const [, options] = mocks.createActor.mock.calls[0];
      expect(options.canisterId).toBe('eqltq-xqaaa-aaaar-qb3vq-cai');
      expect(options.agent).toBeDefined();
    });

    it('reuses the same anonymous agent across repeated calls', async () => {
      await getPublicMinterActor();
      await getPublicMinterActor();

      const { HttpAgent } = await import('@dfinity/agent');
      expect(HttpAgent).toHaveBeenCalledTimes(1);
    });

    it('does not fetch the root key against the mainnet host', async () => {
      await getPublicMinterActor();
      expect(mocks.fetchRootKey).not.toHaveBeenCalled();
    });
  });

  describe('getPublicMinterActor on local replica', () => {
    it('fetches the root key when CONFIG.isLocal is true', async () => {
      vi.resetModules();
      vi.doMock('../config', () => ({
        CONFIG: { host: 'http://localhost:4943', isLocal: true },
        CANISTER_IDS: { CKDOGE_MINTER: 'eqltq-xqaaa-aaaar-qb3vq-cai' },
      }));
      const mod = await import('./ckdogeMinterActors');
      await mod.getPublicMinterActor();
      expect(mocks.fetchRootKey).toHaveBeenCalledTimes(1);
    });
  });

  describe('getWalletMinterActor', () => {
    it('routes through walletStore.getActor for debit-authorizing calls', async () => {
      const actor = await getWalletMinterActor();

      expect(actor).toEqual({ __kind: 'wallet-actor' });
      expect(mocks.walletGetActor).toHaveBeenCalledTimes(1);
      expect(mocks.walletGetActor).toHaveBeenCalledWith(
        'eqltq-xqaaa-aaaar-qb3vq-cai',
        { name: 'ckdoge_minter' },
      );
      expect(mocks.createActor).not.toHaveBeenCalled();
    });
  });
});
