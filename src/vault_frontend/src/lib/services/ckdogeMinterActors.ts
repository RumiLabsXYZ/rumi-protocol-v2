/**
 * ckDOGE minter actor routing.
 *
 * The official ckDOGE minter exposes icrc21_canister_call_consent_message, but it
 * returns UnsupportedCanisterCall for public, non-debiting methods like
 * get_doge_address — so Oisy's ICRC-21 signer can't get consent and rejects those
 * calls when routed through the wallet actor (walletStore.getActor). Those public
 * methods must go through a plain anonymous HttpAgent actor instead, passing the
 * connected principal explicitly as `owner`.
 *
 * update_balance is the one exception in that "public" bucket, but for a different
 * reason than a wallet-signer routing problem: the minter's main.rs traps every
 * anonymous CALLER unconditionally (`check_anonymous_caller()`, checked before args
 * are even read: dfinity/ic@65f0638, rs/dogecoin/ckdoge/minter/src/main.rs:145-149).
 * Two things it does NOT require, confirmed against the same pinned commit's shared
 * `rs/bitcoin/ckbtc/minter/src/updates/update_balance.rs:144-165` that ckDOGE calls
 * into: (a) caller does not need to equal the credited `owner` — the account credited
 * is `Account{ owner: args.owner.unwrap_or(caller), subaccount: args.subaccount }`,
 * fully determined by the explicit args, not by who's calling; (b) update_balance only
 * ever credits (mints) into that account — it has no withdrawal/spend/approval path,
 * so a caller with no relationship to the account cannot move funds by calling it.
 * This is why routing it through a real wallet signer (Oisy's popup-based SignerAgent,
 * which auto-closes its channel ~200ms after each call — see signer.js's
 * `autoCloseTransportChannel`/`closeTransportChannelAfter: 200` — and can only
 * re-open inside a live DOM click, never from this page's 60s `setTimeout` poll) is
 * both unnecessary and, for a background poll, non-functional. updateDogeBalanceForOwner
 * instead uses a transient, in-memory-only Ed25519 identity whose sole purpose is
 * satisfying `check_anonymous_caller()`; it is never the credited owner, is never
 * persisted, is never used for anything debit/redeem/approval-related, and carries
 * no assets or allowances of its own. `retrieve_doge_with_approval` remains on the
 * connected wallet's own identity below — this reasoning does not extend to it.
 *
 * Debit-authorizing calls (retrieve_doge_with_approval) still need the caller's own
 * identity to satisfy the minter's approval check, so those keep using the wallet actor.
 */

import { Actor, HttpAgent, AnonymousIdentity } from '@dfinity/agent';
import { Ed25519KeyIdentity } from '@dfinity/identity';
import type { Principal } from '@dfinity/principal';
import { CONFIG, CANISTER_IDS } from '../config';
import { walletStore } from '../stores/wallet';
import { idlFactory as ckdogeMinterIdl } from '../idls/ckdoge_minter.idl.js';

// Shared across all connected principals: safe only because every account-bound call
// (e.g. get_doge_address) passes the connected principal explicitly as `owner` rather
// than relying on the agent's own identity.
let _anonAgent: HttpAgent | null = null;

async function getAnonAgent(): Promise<HttpAgent> {
  if (!_anonAgent) {
    _anonAgent = new HttpAgent({
      host: CONFIG.host,
      identity: new AnonymousIdentity(),
    });
    if (CONFIG.isLocal) {
      await _anonAgent.fetchRootKey();
    }
  }
  return _anonAgent;
}

/**
 * Public, anonymous-safe minter actor: get_doge_address, get_minter_info,
 * estimate_withdrawal_fee, retrieve_doge_status. NOT update_balance — the
 * minter traps every anonymous caller for that one, see updateDogeBalanceForOwner.
 * Bypasses the wallet signer entirely.
 */
export async function getPublicMinterActor(): Promise<any> {
  const agent = await getAnonAgent();
  return Actor.createActor(ckdogeMinterIdl as any, {
    agent,
    canisterId: CANISTER_IDS.CKDOGE_MINTER,
  });
}

/**
 * Debit-authorizing minter actor: retrieve_doge_with_approval only.
 * Must go through the connected wallet so the minter sees the caller's own identity.
 */
export async function getWalletMinterActor(): Promise<any> {
  return walletStore.getActor(CANISTER_IDS.CKDOGE_MINTER, ckdogeMinterIdl);
}

// Transient request identity for update_balance ONLY. Generated once in memory,
// never written to storage, never exposed outside this module, and never the
// credited `owner` (that's always the caller-supplied principal — see
// updateDogeBalanceForOwner). Regenerates on page reload; carries no assets,
// allowances, or permissions, and is never routed to any debit/redeem/approval
// call. Its only job is to be *a* non-anonymous principal so
// check_anonymous_caller() passes; see the module docstring for the upstream
// source citations backing why this is safe for this one method.
let _updateBalanceRequestAgent: HttpAgent | null = null;

async function getUpdateBalanceRequestAgent(): Promise<HttpAgent> {
  if (!_updateBalanceRequestAgent) {
    _updateBalanceRequestAgent = new HttpAgent({
      host: CONFIG.host,
      identity: Ed25519KeyIdentity.generate(),
    });
    if (CONFIG.isLocal) {
      await _updateBalanceRequestAgent.fetchRootKey();
    }
  }
  return _updateBalanceRequestAgent;
}

/**
 * Calls update_balance crediting exactly `owner` with the default subaccount
 * — `{ owner: [owner], subaccount: [] }`, always, never an optional/empty
 * owner. The actor is built and used entirely inside this function and never
 * returned, so there is no seam through which this method (or any other) can
 * be invoked with a different or missing owner: if `owner` were ever allowed
 * to reach the minter empty, `args.owner.unwrap_or(caller)` would silently
 * credit the ephemeral request identity instead — unrecoverable, since that
 * identity is never persisted or exposed. Fails closed before any agent/RPC
 * work if `owner` is missing or the anonymous principal.
 *
 * `isStillLive`, if provided, is re-checked once request-identity setup
 * resolves and before the RPC actually dispatches — closing the same
 * stale-session race window the caller (the doge page's poll loop) already
 * guards before and after this call, in case identity setup ever becomes
 * genuinely async (e.g. the local-replica `fetchRootKey` path).
 */
export async function updateDogeBalanceForOwner(
  owner: Principal,
  isStillLive: () => boolean = () => true,
): Promise<any> {
  if (!owner || owner.isAnonymous()) {
    throw new Error('A connected wallet principal is required to check your balance.');
  }
  const agent = await getUpdateBalanceRequestAgent();
  if (!isStillLive()) {
    throw new Error('Poll session is no longer live.');
  }
  const actor: any = Actor.createActor(ckdogeMinterIdl as any, {
    agent,
    canisterId: CANISTER_IDS.CKDOGE_MINTER,
  });
  return actor.update_balance({ owner: [owner], subaccount: [] });
}

/** Test hook. Resets the cached anonymous agent between tests. */
export function _resetCkdogeMinterAnonAgent(): void {
  _anonAgent = null;
}

/** Test hook. Resets the cached update_balance request agent between tests. */
export function _resetUpdateBalanceRequestAgent(): void {
  _updateBalanceRequestAgent = null;
}
