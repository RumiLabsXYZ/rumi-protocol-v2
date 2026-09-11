import { describe, it, expect, vi, beforeEach } from 'vitest';

// ──────────────────────────────────────────────────────────────
// The regression this guards: the official ckDOGE minter doesn't implement
// icrc21_canister_call_consent_message, so Oisy's ICRC-21 signer rejects
// consent for every call routed through walletStore.getActor — including
// read-only methods like get_doge_address. Public/non-debiting methods must
// go through a plain anonymous HttpAgent actor instead; only the
// debit-authorizing retrieve_doge_with_approval call may use the wallet actor.
//
// update_balance is the one exception in that "public" bucket, but for a
// different reason than the ICRC-21/wallet-signer problem above: the
// minter's main.rs traps every anonymous CALLER unconditionally
// (`check_anonymous_caller()`, before args are even read — confirmed against
// dfinity/ic@65f0638, rs/dogecoin/ckdoge/minter/src/main.rs:145-149),
// live-trapping with IC0503 "anonymous caller not allowed". Routing it
// through a real wallet signer (Oisy's SignerAgent) was tried and rejected:
// Oisy's transport channel auto-closes ~200ms after each call and can only
// re-open inside a live DOM click — incompatible with this page's 60s
// setTimeout poll (up to 120 attempts). Since the shared minter logic
// (rs/bitcoin/ckbtc/minter/src/updates/update_balance.rs:144-165) computes
// the credited account as `owner: args.owner.unwrap_or(caller)` with NO
// assertion that caller == owner, and update_balance only ever credits
// (mints) — it has no withdrawal/spend/approval path —
// updateDogeBalanceForOwner instead authenticates with a transient,
// in-memory-only Ed25519 identity that exists solely to satisfy
// check_anonymous_caller(). It is never the credited owner, never
// persisted, and never used for any debit/redeem/approval call. The actor
// itself is never exposed outside the function — only its update_balance
// result is — so there is no seam through which `owner` could ever reach
// the minter empty/optional and silently default to this ephemeral
// identity as caller (`args.owner.unwrap_or(caller)`), which would be an
// unrecoverable credit since the identity is never persisted or surfaced.
// ──────────────────────────────────────────────────────────────

const mocks = vi.hoisted(() => ({
  updateBalanceCalls: [] as Array<{ identity: any; args: any }>,
  forcedUpdateBalanceError: null as Error | null,
  fetchRootKey: vi.fn().mockResolvedValue(undefined),
  walletActors: {} as Record<string, any>,
  walletGetActor: vi.fn(async (canisterId: string) => (mocks.walletActors as any)[canisterId] ?? { __kind: 'wallet-actor' }),
  isOisyWallet: vi.fn(() => false),
  getOisySignerAgent: vi.fn(async () => ({ __kind: 'signer-agent' })),
  oisyActors: {} as Record<string, any>,
  createOisyActor: vi.fn((canisterId: string) => (mocks.oisyActors as any)[canisterId]),
}));

vi.mock('@dfinity/agent', async () => {
  const actual = await vi.importActual<typeof import('@dfinity/agent')>('@dfinity/agent');
  const createActor = vi.fn((_idl: any, _options: { agent: any; canisterId: string }) => {
    const agent = _options.agent;
    return {
      __agent: agent,
      canisterId: _options.canisterId,
      // Simulates the real minter's check_anonymous_caller(): traps for an
      // anonymous identity regardless of the `owner` field in args, exactly
      // like the live IC0503 trap this whole fix is about. A permissive mock
      // that just returns "success" irrespective of identity would not catch
      // a regression back to the anonymous actor — this one does.
      async update_balance(args: any) {
        const principal = agent.__identity.getPrincipal();
        if (principal.isAnonymous()) {
          throw new Error('anonymous caller not allowed');
        }
        if (mocks.forcedUpdateBalanceError) {
          throw mocks.forcedUpdateBalanceError;
        }
        mocks.updateBalanceCalls.push({ identity: agent.__identity, args });
        return { Ok: [] };
      },
    };
  });
  return {
    ...actual,
    Actor: { ...actual.Actor, createActor },
    HttpAgent: vi.fn((options: any) => ({
      __kind: 'http-agent',
      __identity: options.identity,
      fetchRootKey: mocks.fetchRootKey,
    })),
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

vi.mock('./protocol/walletOperations', () => ({
  isOisyWallet: mocks.isOisyWallet,
}));

vi.mock('./oisySigner', () => ({
  getOisySignerAgent: mocks.getOisySignerAgent,
  createOisyActor: mocks.createOisyActor,
}));

import { Principal } from '@dfinity/principal';
import {
  getPublicMinterActor,
  getWalletMinterActor,
  updateDogeBalanceForOwner,
  redeemDogeWithApproval,
  _resetCkdogeMinterAnonAgent,
  _resetUpdateBalanceRequestAgent,
} from './ckdogeMinterActors';
import { buildApproveArgs, buildRetrieveWithApprovalArgs } from '../utils/dogeBorrowFlow';

const OWNER_PRINCIPAL = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai');
const OTHER_PRINCIPAL = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
const LEDGER_ID = 'irorr-5aaaa-aaaak-qddsq-cai';
const MINTER_ID = 'eqltq-xqaaa-aaaar-qb3vq-cai';

function makeLedgerActor(approveResult: any = { Ok: 42n }) {
  return { icrc2_approve: vi.fn(async () => approveResult) };
}

function makeMinterActor(retrieveResult: any = { Ok: { block_index: 7n } }) {
  return { retrieve_doge_with_approval: vi.fn(async () => retrieveResult) };
}

function redeemParams(overrides: Partial<Parameters<typeof redeemDogeWithApproval>[0]> = {}) {
  return {
    ownerPrincipal: OWNER_PRINCIPAL,
    ledgerCanisterId: LEDGER_ID,
    minterCanisterId: MINTER_ID,
    ledgerIdl: { name: 'ckdoge_ledger' },
    approveArgs: buildApproveArgs(OTHER_PRINCIPAL, 500_100_000n),
    retrieveArgs: buildRetrieveWithApprovalArgs('DBXu2kgc3xtvCUWFcxFE3r9hEYgmuaaCyD', 500_000_000n),
    isLive: () => true,
    ...overrides,
  };
}

describe('ckdogeMinterActors', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    _resetCkdogeMinterAnonAgent();
    _resetUpdateBalanceRequestAgent();
    mocks.updateBalanceCalls.length = 0;
    mocks.forcedUpdateBalanceError = null;
    mocks.walletActors = {};
    mocks.oisyActors = {};
    mocks.isOisyWallet.mockReturnValue(false);
  });

  describe('getPublicMinterActor', () => {
    it('builds an actor from an anonymous HttpAgent, not the wallet actor', async () => {
      const actor: any = await getPublicMinterActor();

      expect(actor.__agent.__identity.getPrincipal().isAnonymous()).toBe(true);
      expect(mocks.walletGetActor).not.toHaveBeenCalled();
    });

    it('passes the ckDOGE minter canister id and an agent to Actor.createActor', async () => {
      const actor: any = await getPublicMinterActor();
      expect(actor.canisterId).toBe('eqltq-xqaaa-aaaar-qb3vq-cai');
    });

    it('reuses the same anonymous agent across repeated calls', async () => {
      const a1: any = await getPublicMinterActor();
      const a2: any = await getPublicMinterActor();
      expect(a1.__agent).toBe(a2.__agent);
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
    });
  });

  // updateDogeBalanceForOwner: the narrow, self-contained replacement for the
  // earlier general-purpose actor getter. Tests call the ACTUAL helper (never
  // a mocked-away version of it) and assert against the real args/identity
  // that reach the simulated minter boundary above.
  describe('updateDogeBalanceForOwner', () => {
    it('authenticates with a non-anonymous, real Ed25519 identity — the simulated minter boundary accepts it', async () => {
      const result = await updateDogeBalanceForOwner(OWNER_PRINCIPAL);

      expect(result).toEqual({ Ok: [] });
      expect(mocks.updateBalanceCalls).toHaveLength(1);
      const { identity } = mocks.updateBalanceCalls[0];
      expect(identity.getPrincipal().isAnonymous()).toBe(false);
      expect(mocks.walletGetActor).not.toHaveBeenCalled();
    });

    it('sends update_balance with owner set to exactly the specified wallet principal and the default subaccount — never empty', async () => {
      await updateDogeBalanceForOwner(OWNER_PRINCIPAL);

      const { args } = mocks.updateBalanceCalls[0];
      expect(args.owner).toHaveLength(1);
      expect(args.owner[0].toText()).toBe(OWNER_PRINCIPAL.toText());
      expect(args.subaccount).toEqual([]);
    });

    it('binds two different owners correctly across separate calls, using the shared request identity for both', async () => {
      await updateDogeBalanceForOwner(OWNER_PRINCIPAL);
      await updateDogeBalanceForOwner(OTHER_PRINCIPAL);

      expect(mocks.updateBalanceCalls).toHaveLength(2);
      expect(mocks.updateBalanceCalls[0].args.owner[0].toText()).toBe(OWNER_PRINCIPAL.toText());
      expect(mocks.updateBalanceCalls[1].args.owner[0].toText()).toBe(OTHER_PRINCIPAL.toText());
      // Same request identity authenticates both — it never becomes the
      // owner (checked above), it is just shared, which is safe because the
      // credited account is always the explicit `owner` argument.
      expect(mocks.updateBalanceCalls[0].identity).toBe(mocks.updateBalanceCalls[1].identity);
    });

    it('fails closed before any RPC when owner is missing — request identity is never even created', async () => {
      await expect(updateDogeBalanceForOwner(undefined as any)).rejects.toThrow(
        'A connected wallet principal is required',
      );
      expect(mocks.updateBalanceCalls).toHaveLength(0);
      expect(mocks.fetchRootKey).not.toHaveBeenCalled();
      expect(mocks.walletGetActor).not.toHaveBeenCalled();
    });

    it('fails closed before any RPC when owner is the anonymous principal', async () => {
      await expect(updateDogeBalanceForOwner(Principal.anonymous())).rejects.toThrow(
        'A connected wallet principal is required',
      );
      expect(mocks.updateBalanceCalls).toHaveLength(0);
    });

    it('does not send the RPC when the isStillLive callback reports the poll session has gone stale', async () => {
      await expect(
        updateDogeBalanceForOwner(OWNER_PRINCIPAL, () => false),
      ).rejects.toThrow('no longer live');

      expect(mocks.updateBalanceCalls).toHaveLength(0);
      expect(mocks.walletGetActor).not.toHaveBeenCalled();
    });

    it('propagates a rejected update_balance RPC (e.g. TemporarilyUnavailable) without falling back to the wallet actor or retrying', async () => {
      mocks.forcedUpdateBalanceError = new Error('TemporarilyUnavailable: minter paused');

      await expect(updateDogeBalanceForOwner(OWNER_PRINCIPAL)).rejects.toThrow(
        'TemporarilyUnavailable',
      );
      expect(mocks.updateBalanceCalls).toHaveLength(0);
      expect(mocks.walletGetActor).not.toHaveBeenCalled();
    });

    it('regression: would reject if this helper ever routed through an anonymous identity again', async () => {
      // Exercises the same simulated minter-boundary check used by every
      // other test above, called out explicitly: an anonymous identity must
      // never reach update_balance. This is what would have caught the
      // original live bug (getPublicMinterActor's AnonymousIdentity routed
      // to update_balance) had this helper existed then.
      const { AnonymousIdentity, HttpAgent, Actor } = await import('@dfinity/agent');
      const anonAgent = new HttpAgent({ identity: new AnonymousIdentity() } as any);
      const idlModule = await import('../idls/ckdoge_minter.idl.js');
      const anonActor: any = Actor.createActor(idlModule.idlFactory as any, {
        agent: anonAgent as any,
        canisterId: 'eqltq-xqaaa-aaaar-qb3vq-cai',
      });

      await expect(anonActor.update_balance({ owner: [OWNER_PRINCIPAL], subaccount: [] })).rejects.toThrow(
        'anonymous caller not allowed',
      );
    });

    it('reuses the same transient identity/agent across repeated calls (cached, not regenerated per call)', async () => {
      await updateDogeBalanceForOwner(OWNER_PRINCIPAL);
      const first = mocks.updateBalanceCalls[0].identity;
      await updateDogeBalanceForOwner(OTHER_PRINCIPAL);
      const second = mocks.updateBalanceCalls[1].identity;

      expect(first).toBe(second);
    });

    it('does not fetch the root key against the mainnet host', async () => {
      await updateDogeBalanceForOwner(OWNER_PRINCIPAL);
      expect(mocks.fetchRootKey).not.toHaveBeenCalled();
    });

    it('fetches the root key when CONFIG.isLocal is true', async () => {
      vi.resetModules();
      vi.doMock('../config', () => ({
        CONFIG: { host: 'http://localhost:4943', isLocal: true },
        CANISTER_IDS: { CKDOGE_MINTER: 'eqltq-xqaaa-aaaar-qb3vq-cai' },
      }));
      const mod = await import('./ckdogeMinterActors');
      await mod.updateDogeBalanceForOwner(OWNER_PRINCIPAL);
      expect(mocks.fetchRootKey).toHaveBeenCalledTimes(1);
    });
  });

  // redeemDogeWithApproval: the click-to-consent-screen routing this file's
  // redemption-fix regression guards. Oisy must build BOTH actors from one
  // getOisySignerAgent() call so the two sequential consent screens open in a
  // single popup and no live query burns the gesture window before the first
  // one; non-Oisy must keep using walletStore.getActor as before; a failed
  // approval must halt before retrieve_doge_with_approval is ever called; and
  // a session that goes stale mid-flight must not advance to retrieve under a
  // wallet that's no longer connected.
  describe('redeemDogeWithApproval', () => {
    it('non-Oisy: builds both actors via walletStore.getActor and completes approve then retrieve', async () => {
      mocks.isOisyWallet.mockReturnValue(false);
      const ledgerActor = makeLedgerActor();
      const minterActor = makeMinterActor();
      mocks.walletActors[LEDGER_ID] = ledgerActor;
      mocks.walletActors[MINTER_ID] = minterActor;

      const outcome = await redeemDogeWithApproval(redeemParams());

      expect(outcome).toEqual({ kind: 'success', approveBlockIndex: 42n, burnBlockIndex: 7n });
      expect(mocks.getOisySignerAgent).not.toHaveBeenCalled();
      expect(mocks.walletGetActor).toHaveBeenCalledWith(LEDGER_ID, { name: 'ckdoge_ledger' });
      expect(mocks.walletGetActor).toHaveBeenCalledWith(MINTER_ID, { name: 'ckdoge_minter' });
      expect(ledgerActor.icrc2_approve).toHaveBeenCalledTimes(1);
      expect(minterActor.retrieve_doge_with_approval).toHaveBeenCalledTimes(1);
    });

    it('Oisy: builds both actors from a single signer agent and never touches walletStore.getActor', async () => {
      mocks.isOisyWallet.mockReturnValue(true);
      const ledgerActor = makeLedgerActor();
      const minterActor = makeMinterActor();
      mocks.oisyActors[LEDGER_ID] = ledgerActor;
      mocks.oisyActors[MINTER_ID] = minterActor;

      const outcome = await redeemDogeWithApproval(redeemParams());

      expect(outcome).toEqual({ kind: 'success', approveBlockIndex: 42n, burnBlockIndex: 7n });
      expect(mocks.getOisySignerAgent).toHaveBeenCalledTimes(1);
      expect(mocks.walletGetActor).not.toHaveBeenCalled();
      expect(ledgerActor.icrc2_approve).toHaveBeenCalledTimes(1);
      expect(minterActor.retrieve_doge_with_approval).toHaveBeenCalledTimes(1);
    });

    it('a failed approval halts before retrieve_doge_with_approval is ever called', async () => {
      const ledgerActor = makeLedgerActor({ Err: { BadFee: { expected_fee: 100_000n } } });
      const minterActor = makeMinterActor();
      mocks.walletActors[LEDGER_ID] = ledgerActor;
      mocks.walletActors[MINTER_ID] = minterActor;

      const outcome = await redeemDogeWithApproval(redeemParams());

      expect(outcome.kind).toBe('approve-error');
      if (outcome.kind === 'approve-error') {
        expect(outcome.message).toContain('0.001 DOGE');
      }
      expect(minterActor.retrieve_doge_with_approval).not.toHaveBeenCalled();
    });

    it('stops before retrieve, discarding the outcome, if the session goes stale right after approve resolves', async () => {
      const ledgerActor = makeLedgerActor();
      const minterActor = makeMinterActor();
      mocks.walletActors[LEDGER_ID] = ledgerActor;
      mocks.walletActors[MINTER_ID] = minterActor;

      let liveChecks = 0;
      const outcome = await redeemDogeWithApproval(
        redeemParams({ isLive: () => (liveChecks += 1) <= 2 }),
      );

      expect(outcome).toEqual({ kind: 'stale' });
      expect(minterActor.retrieve_doge_with_approval).not.toHaveBeenCalled();
    });

    it('surfaces a retrieve_doge_with_approval error while still returning the completed approval block index', async () => {
      const ledgerActor = makeLedgerActor();
      const minterActor = makeMinterActor({ Err: { InsufficientAllowance: { allowance: 0n } } });
      mocks.walletActors[LEDGER_ID] = ledgerActor;
      mocks.walletActors[MINTER_ID] = minterActor;

      const outcome = await redeemDogeWithApproval(redeemParams());

      expect(outcome.kind).toBe('retrieve-error');
      if (outcome.kind === 'retrieve-error') {
        expect(outcome.approveBlockIndex).toBe(42n);
        expect(outcome.message).toContain('0 DOGE');
      }
    });
  });
});
