import { Actor, HttpAgent, type ActorSubclass } from '@dfinity/agent';
import { Principal } from '@dfinity/principal';
import { CONFIG, CANISTER_IDS } from '../config';
import { walletStore } from '../stores/wallet';
import { idlFactory } from '../idls/cksol_minter.idl.js';
import type { _SERVICE as CksolMinter, MinterInfo } from '../idls/cksol_minter.idl';
import { ICRC1_IDL } from '../idls/ledger.idl.js';
import { idlFactory as backendIdl } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import { get } from 'svelte/store';
import { auth, walletSessionGeneration } from './auth';
import { describeCksolError, NAT64_MAX, validateSolanaAddress } from '../utils/cksolMinterFlow';

export const CKSOL_MINTER = CANISTER_IDS.CKSOL_MINTER;
export const CKSOL_LEDGER = CANISTER_IDS.CKSOL_LEDGER;
export function isCksolOisyWallet(): boolean { return get(auth).walletType === 'oisy'; }

function pinCksolSession(owner: Principal, isLive: () => boolean): () => boolean {
  const generation = get(walletSessionGeneration), walletType = get(auth).walletType;
  return () => {
    const wallet = get(walletStore), active = get(auth);
    return isLive() && get(walletSessionGeneration) === generation && !wallet.loading
      && wallet.isConnected && wallet.principal?.toText() === owner.toText()
      && active.isConnected && active.account?.owner?.toText() === owner.toText()
      && active.walletType === walletType;
  };
}

async function verifyCksolActorOwner(actor: any, owner: Principal): Promise<void> {
  const agent = Actor.agentOf(actor);
  if (!agent || (await agent.getPrincipal()).toText() !== owner.toText()) {
    throw new Error('The wallet signer principal differs from the connected ICP account. Reconnect before continuing.');
  }
}

let minter: ActorSubclass<CksolMinter>;
let ledger: any;
let backend: any;
function mainnetAgent() {
  if (CONFIG.isLocal) throw new Error('ckSOL is available on ICP mainnet only.');
  return new HttpAgent({ host: CONFIG.host });
}
export function getCksolMinter(): ActorSubclass<CksolMinter> {
  return minter ??= Actor.createActor<CksolMinter>(idlFactory, { agent: mainnetAgent(), canisterId: CKSOL_MINTER });
}
export function getCksolLedger(): any {
  return ledger ??= Actor.createActor(ICRC1_IDL as any, { agent: mainnetAgent(), canisterId: CKSOL_LEDGER });
}
export function getCksolRelay(): any {
  return backend ??= Actor.createActor(backendIdl, { agent: mainnetAgent(), canisterId: CONFIG.currentCanisterId });
}
export async function getSolDepositAddress(owner: Principal): Promise<string> {
  if (!owner || owner.isAnonymous()) throw new Error('Connect an ICP wallet first.');
  return getCksolMinter().get_deposit_address({ owner: [owner], subaccount: [] });
}
export async function clearSolDeposit(isLive: () => boolean): Promise<void> {
  if (CONFIG.isLocal) throw new Error('ckSOL is available on ICP mainnet only.');
  const owner = get(walletStore).principal;
  if (!owner) throw new Error('Connect an ICP wallet first.');
  const current = pinCksolSession(owner, isLive);
  if (!current()) throw new Error('Wallet session changed.');
  const actor: any = await walletStore.getActor(CONFIG.currentCanisterId, backendIdl);
  if (!current()) throw new Error('Wallet session changed.');
  await verifyCksolActorOwner(actor, owner);
  if (!current()) throw new Error('Wallet session changed.');
  const result = await actor.clear_cksol_deposit();
  if ('Err' in result) throw new Error(result.Err);
}

export interface SolWithdrawalQuote {
  amount: bigint; address: string; owner: string; ledgerFee: bigint;
  minterFee: bigint; minimum: bigint; allowance: bigint; balance: bigint; quotedAt: number;
}
export async function quoteSolWithdrawal(owner: Principal, amount: bigint, address: string): Promise<SolWithdrawalQuote> {
  if (!owner || owner.isAnonymous()) throw new Error('Connect an ICP wallet first.');
  if (amount <= 0n || amount > NAT64_MAX) throw new Error('Invalid lamport amount.');
  if (!validateSolanaAddress(address)) throw new Error('Enter a valid Solana mainnet address.');
  const account = { owner, subaccount: [] };
  const spender = { owner: Principal.fromText(CKSOL_MINTER), subaccount: [] };
  const [info, fee, balance, allowance] = await Promise.all([
    getCksolMinter().get_minter_info(), getCksolLedger().icrc1_fee(),
    getCksolLedger().icrc1_balance_of(account), getCksolLedger().icrc2_allowance({ account, spender }),
  ]);
  const quote = { amount, address, owner: owner.toText(), ledgerFee: BigInt(fee), minterFee: info.withdrawal_fee,
    minimum: info.minimum_withdrawal_amount, allowance: BigInt(allowance.allowance), balance: BigInt(balance), quotedAt: Date.now() };
  if (amount < quote.minimum || amount <= quote.minterFee) throw new Error('Amount is below the minter withdrawal minimum or fee.');
  if (amount + 2n * quote.ledgerFee > quote.balance) throw new Error('Not enough ckSOL for the withdrawal and ledger fees.');
  return quote;
}

export type WithdrawalPhase = 'Approving' | 'Approved' | 'Withdrawing' | 'Submitted' | 'Uncertain' | 'Rejected';
export interface SolWithdrawalIntent {
  id: string; owner: string; amount: string; address: string; phase: WithdrawalPhase;
  createdAt: number; approveBlock?: string; burnBlock?: string; message?: string;
}
export interface IntentStore { getItem(key: string): string | null; setItem(key: string, value: string): void; removeItem(key: string): void }
export function solWithdrawalKey(owner: string): string {
  return `rumi:cksol:withdrawal:mainnet:${CKSOL_LEDGER}:${CKSOL_MINTER}:${owner}`;
}
export function loadSolWithdrawal(storage: IntentStore, owner: string): SolWithdrawalIntent | null {
  const raw = storage.getItem(solWithdrawalKey(owner));
  if (!raw) return null;
  const data = JSON.parse(raw);
  if (typeof data.id !== 'string' || !/^[0-9a-f-]{36}$/.test(data.id) || data.owner !== owner || !/^[0-9]+$/.test(data.amount) || BigInt(data.amount) <= 0n || BigInt(data.amount) > NAT64_MAX
      || !validateSolanaAddress(data.address) || !['Approving', 'Approved', 'Withdrawing', 'Submitted', 'Uncertain', 'Rejected'].includes(data.phase)
      || !Number.isFinite(data.createdAt) || (data.burnBlock !== undefined && !/^[0-9]+$/.test(data.burnBlock))) {
    throw new Error('Saved ckSOL withdrawal is invalid. Preserve it for support before starting another withdrawal.');
  }
  return data;
}

/** Durable intent precedes approval and burn dispatch. Any lost burn response remains blocked. */
export async function submitSolWithdrawal(quote: SolWithdrawalQuote, owner: Principal, storage: IntentStore, isLive: () => boolean): Promise<SolWithdrawalIntent> {
  if (CONFIG.isLocal) throw new Error('ckSOL is available on ICP mainnet only.');
  if (isCksolOisyWallet()) throw new Error('The official ckSOL minter does not support OISY consent for withdrawals. Use Internet Identity or Plug. No approval was sent.');
  const current = pinCksolSession(owner, isLive);
  if (owner.isAnonymous() || quote.owner !== owner.toText() || !current()) throw new Error('Wallet session changed.');
  if (Date.now() - quote.quotedAt > 60_000) throw new Error('Refresh the withdrawal quote.');
  const key = solWithdrawalKey(quote.owner);
  if (loadSolWithdrawal(storage, quote.owner)) throw new Error('Resolve the saved withdrawal before starting another.');
  const fresh = await quoteSolWithdrawal(owner, quote.amount, quote.address);
  if (!current()) throw new Error('Wallet session changed.');
  for (const field of ['ledgerFee', 'minterFee', 'minimum', 'allowance'] as const) {
    if (fresh[field] !== quote[field]) throw new Error('Withdrawal terms changed. Refresh the quote before approving.');
  }
  const intent: SolWithdrawalIntent = { id: crypto.randomUUID(), owner: quote.owner, amount: quote.amount.toString(), address: quote.address, phase: 'Approving', createdAt: Date.now() };
  const save = () => storage.setItem(key, JSON.stringify(intent));
  const ledgerActor: any = await walletStore.getActor(CKSOL_LEDGER, ICRC1_IDL);
  if (!current()) throw new Error('Wallet session changed.');
  await verifyCksolActorOwner(ledgerActor, owner);
  if (!current()) throw new Error('Wallet session changed.');
  const minterActor: any = await walletStore.getActor(CKSOL_MINTER, idlFactory);
  if (!current()) throw new Error('Wallet session changed.');
  await verifyCksolActorOwner(minterActor, owner);
  if (!current()) throw new Error('Wallet session changed.');
  // No asynchronous gap between durable reservation and first mutation.
  save();
  const approve = await ledgerActor.icrc2_approve({
    spender: { owner: Principal.fromText(CKSOL_MINTER), subaccount: [] },
    amount: quote.amount + quote.ledgerFee, expected_allowance: [quote.allowance],
    fee: [quote.ledgerFee], from_subaccount: [], memo: [],
    expires_at: [BigInt(Date.now() + 10 * 60_000) * 1_000_000n], created_at_time: [BigInt(Date.now()) * 1_000_000n],
  });
  if ('Err' in approve) { intent.phase = 'Rejected'; intent.message = `Approval rejected: ${describeCksolError(approve.Err)}`; save(); return intent; }
  intent.approveBlock = BigInt(approve.Ok).toString(); intent.phase = 'Approved'; save();
  if (!current()) return intent;
  await verifyCksolActorOwner(minterActor, owner);
  if (!current()) return intent;
  intent.phase = 'Withdrawing'; save();
  let result: any;
  try {
    result = await minterActor.withdraw_sol({ address: quote.address, amount: quote.amount, from_subaccount: [] });
  } catch (error) {
    intent.phase = 'Uncertain'; intent.message = `Withdrawal response unavailable. Do not repeat it. ${String(error)}`;
    try { save(); } catch { intent.message += ' Saving also failed; preserve this page and request details.'; }
    return intent;
  }
  if ('Err' in result) {
    const safe = ['MalformedAddress', 'InvalidDestination', 'ValueTooSmall', 'InsufficientFunds', 'InsufficientAllowance'].some((tag) => tag in result.Err);
    intent.phase = safe ? 'Rejected' : 'Uncertain';
    intent.message = `${safe ? 'Withdrawal rejected' : 'Withdrawal outcome is uncertain; do not repeat it'}: ${describeCksolError(result.Err)}`;
  }
  else { intent.phase = 'Submitted'; intent.burnBlock = BigInt(result.Ok.block_index).toString(); }
  // Keep a confirmed burn receipt in memory even if persistence fails after the burn.
  try { save(); } catch { intent.message = `Saving the receipt failed. Preserve burn block ${intent.burnBlock ?? 'unknown'} and this page before leaving.`; }
  return intent;
}
export function clearSafeSolIntent(storage: IntentStore, owner: string): void {
  const intent = loadSolWithdrawal(storage, owner);
  if (intent && !['Approving', 'Approved', 'Rejected'].includes(intent.phase)) throw new Error('This withdrawal may have burned ckSOL. Keep it until finality is verified.');
  storage.removeItem(solWithdrawalKey(owner));
}
export type { MinterInfo };

/** Call under the same Web Lock as submission; stale tabs may never erase a newer journal. */
export function acknowledgeSolWithdrawal(storage: IntentStore, displayed: SolWithdrawalIntent, finalizedReceipt?: { intentId: string; burnBlock: string }): void {
  const current = loadSolWithdrawal(storage, displayed.owner);
  if (!current) return;
  if (current.id !== displayed.id || current.phase !== displayed.phase || current.burnBlock !== displayed.burnBlock) {
    throw new Error('The saved withdrawal changed in another session. Refresh it before acknowledging.');
  }
  if (finalizedReceipt && current.phase === 'Submitted' && current.burnBlock
      && finalizedReceipt.intentId === current.id && finalizedReceipt.burnBlock === current.burnBlock) {
    storage.removeItem(solWithdrawalKey(displayed.owner));
  } else {
    clearSafeSolIntent(storage, displayed.owner);
  }
}
