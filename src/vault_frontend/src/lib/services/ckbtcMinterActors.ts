/** Actor routing for DFINITY's official ckBTC minter. */
import { Actor, AnonymousIdentity, HttpAgent } from '@dfinity/agent';
import { Ed25519KeyIdentity } from '@dfinity/identity';
import type { Principal } from '@dfinity/principal';
import { CONFIG, CANISTER_IDS } from '../config';
import { walletStore } from '../stores/wallet';
import { idlFactory as ckbtcMinterIdl } from '../idls/ckbtc_minter.idl.js';
import { ICRC1_IDL } from '../idls/ledger.idl.js';
import { createOisyActor, getOisySignerAgent } from './oisySigner';
import { isOisyWallet } from './protocol/walletOperations';

let anonAgent: HttpAgent | null = null;
let updateBalanceAgent: HttpAgent | null = null;

async function getAnonAgent(): Promise<HttpAgent> {
  if (!anonAgent) {
    anonAgent = new HttpAgent({ host: CONFIG.host, identity: new AnonymousIdentity() });
    if (CONFIG.isLocal) await anonAgent.fetchRootKey();
  }
  return anonAgent;
}

async function getUpdateBalanceAgent(): Promise<HttpAgent> {
  if (!updateBalanceAgent) {
    updateBalanceAgent = new HttpAgent({ host: CONFIG.host, identity: Ed25519KeyIdentity.generate() });
    if (CONFIG.isLocal) await updateBalanceAgent.fetchRootKey();
  }
  return updateBalanceAgent;
}

/** Anonymous actor for public minter info, address, fee estimates, and status. */
export async function getPublicCkbtcMinterActor(): Promise<any> {
  return Actor.createActor(ckbtcMinterIdl as any, {
    agent: await getAnonAgent(),
    canisterId: CANISTER_IDS.CKBTC_MINTER,
  });
}

/** Anonymous ckBTC ledger actor for read-only balance and fee queries. */
export async function getPublicCkbtcLedgerActor(): Promise<any> {
  return Actor.createActor(ICRC1_IDL as any, {
    agent: await getAnonAgent(),
    canisterId: CANISTER_IDS.CKBTC_LEDGER,
  });
}

/** Wallet actor for calls whose caller identity authorizes spending. */
export async function getWalletCkbtcMinterActor(owner: Principal): Promise<any> {
  if (isOisyWallet()) {
    return createOisyActor(CANISTER_IDS.CKBTC_MINTER, ckbtcMinterIdl, await getOisySignerAgent(owner));
  }
  return walletStore.getActor(CANISTER_IDS.CKBTC_MINTER, ckbtcMinterIdl);
}

/**
 * DFINITY's current ckBTC `update_balance` wrapper rejects anonymous callers;
 * its update implementation resolves the credited account from `args.owner`
 * (falling back to caller only when that option is absent). This call only
 * triggers the minter check/mint path; it cannot spend. Use a transient request
 * identity, while pinning the credited account to the explicit connected owner.
 */
export async function updateBtcBalanceForOwner(
  owner: Principal,
  isStillLive: () => boolean = () => true,
): Promise<any> {
  if (!owner || owner.isAnonymous()) throw new Error('A connected wallet principal is required to check your balance.');
  const agent = await getUpdateBalanceAgent();
  if (!isStillLive()) throw new Error('Deposit check session is no longer live.');
  const actor: any = Actor.createActor(ckbtcMinterIdl as any, {
    agent,
    canisterId: CANISTER_IDS.CKBTC_MINTER,
  });
  return actor.update_balance({ owner: [owner], subaccount: [] });
}

export type CkbtcWithdrawalOutcome =
  | { kind: 'stale' }
  | { kind: 'approval-only'; approveBlockIndex: bigint }
  | { kind: 'approve-error'; message: string }
  | { kind: 'approve-uncertain'; message: string }
  | { kind: 'retrieve-error'; approveBlockIndex: bigint; message: string; retrySafety: 'safe' | 'unknown' }
  | { kind: 'retrieve-uncertain'; approveBlockIndex: bigint; message: string }
  | { kind: 'success'; approveBlockIndex: bigint; withdrawalBlockIndex: bigint };

export interface CkbtcWithdrawalParams {
  owner: Principal;
  ledgerCanisterId: string;
  minterCanisterId: string;
  ledgerIdl: any;
  approveArgs: Record<string, unknown>;
  retrieveArgs: Record<string, unknown>;
  isLive: () => boolean;
}

/** Approve then retrieve on the same signer, with no intervening reads or retries. */
export async function submitCkbtcWithdrawal(params: CkbtcWithdrawalParams): Promise<CkbtcWithdrawalOutcome> {
  const { owner, ledgerCanisterId, minterCanisterId, ledgerIdl, approveArgs, retrieveArgs, isLive } = params;
  let ledger: any;
  let minter: any;

  try {
    if (isOisyWallet()) {
      const signer = await getOisySignerAgent(owner);
      if (!isLive()) return { kind: 'stale' };
      ledger = createOisyActor(ledgerCanisterId, ledgerIdl, signer);
      minter = createOisyActor(minterCanisterId, ckbtcMinterIdl, signer);
    } else {
      ledger = await walletStore.getActor(ledgerCanisterId, ledgerIdl);
      if (!isLive()) return { kind: 'stale' };
    }
  } catch (error) {
    return { kind: 'approve-error', message: readableError(error) };
  }

  let approval: any;
  try {
    approval = await ledger.icrc2_approve(approveArgs);
  } catch (error) {
    return { kind: 'approve-uncertain', message: readableError(error) };
  }
  if (!isLive()) return { kind: 'stale' };
  if (approval && 'Err' in approval) return { kind: 'approve-error', message: JSON.stringify(approval.Err, bigintReplacer) };
  if (!approval || !('Ok' in approval)) return { kind: 'approve-uncertain', message: 'The ledger approval response was incomplete.' };
  const approveBlockIndex = BigInt(approval.Ok);

  if (!isLive()) return { kind: 'approval-only', approveBlockIndex };
  if (!minter) {
    try {
      minter = await walletStore.getActor(minterCanisterId, ckbtcMinterIdl);
    } catch (error) {
      return { kind: 'retrieve-uncertain', approveBlockIndex, message: readableError(error) };
    }
    if (!isLive()) return { kind: 'stale' };
  }

  let retrieval: any;
  try {
    retrieval = await minter.retrieve_btc_with_approval(retrieveArgs);
  } catch (error) {
    return { kind: 'retrieve-uncertain', approveBlockIndex, message: readableError(error) };
  }
  if (retrieval && 'Err' in retrieval) return {
    kind: 'retrieve-error',
    approveBlockIndex,
    message: JSON.stringify(retrieval.Err, bigintReplacer),
    retrySafety: isDefinitePreflightError(retrieval.Err) ? 'safe' : 'unknown',
  };
  if (!retrieval || !('Ok' in retrieval)) return { kind: 'retrieve-uncertain', approveBlockIndex, message: 'The minter response was incomplete.' };
  return { kind: 'success', approveBlockIndex, withdrawalBlockIndex: BigInt(retrieval.Ok.block_index) };
}

function isDefinitePreflightError(error: unknown): boolean {
  if (!error || typeof error !== 'object') return false;
  return ['MalformedAddress', 'AmountTooLow', 'InsufficientFunds', 'InsufficientAllowance']
    .some((tag) => Object.prototype.hasOwnProperty.call(error, tag));
}

function readableError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function bigintReplacer(_key: string, value: unknown) {
  return typeof value === 'bigint' ? value.toString() : value;
}

export function _resetCkbtcMinterAgents(): void {
  anonAgent = null;
  updateBalanceAgent = null;
}
