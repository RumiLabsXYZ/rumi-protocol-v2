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
 * Debit-authorizing calls (retrieve_doge_with_approval) still need the caller's own
 * identity to satisfy the minter's approval check, so those keep using the wallet actor.
 */

import { Actor, HttpAgent, AnonymousIdentity } from '@dfinity/agent';
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
 * Public/non-debiting minter actor: get_doge_address, update_balance,
 * get_minter_info, estimate_withdrawal_fee, retrieve_doge_status.
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

/** Test hook. Resets the cached anonymous agent between tests. */
export function _resetCkdogeMinterAnonAgent(): void {
  _anonAgent = null;
}
