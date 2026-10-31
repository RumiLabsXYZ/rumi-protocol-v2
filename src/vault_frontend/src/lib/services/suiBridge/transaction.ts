/**
 * The Sui to ICP return transaction: what the holder signs, how it is built, and how a wallet's answer is checked.
 *
 * The accepted Move entry is `rumi_icusd_gate::bridge_out(gate: &mut Gate, coin: Coin<RUMI_ICUSD_GATE>,
 * icp_principal: vector<u8>, icp_subaccount: vector<u8>, client_nonce: u64, ctx)` (move/rumi_icusd_gate at 23f75ef1). It
 * burns the WHOLE coin it is given, so the transaction hands it a coin of exactly the reviewed amount: the SDK builds the
 * exact-amount `mergeCoins` and `splitCoins` here, with fully resolved object references, so the wallet only adds the
 * sender's gas. The principal is its raw bytes (1 to 29), the subaccount is EXACTLY 32 bytes (all zero by default) and the
 * nonce is a u64.
 *
 * `validateSignedReturn` checks what a wallet returns against the reviewed attempt BEFORE anything is saved or
 * broadcast: the sender, the Move call target, the Gate (shared object, version and mutability), the coin objects and the
 * exact amount, the principal, the subaccount and the nonce, plus that the signature verifies for that sender. Any
 * command beyond the expected merge, split and call is refused. It returns the typed transaction digest derived from the
 * bytes: the attempt's identity on chain (never the payout id; that is the Gate's `transfer_id`, see `ids.ts`).
 *
 * Amounts are bigint e8s (icUSD has 8 decimals); gas is SUI in MIST (9 decimals) and is never mixed with them.
 */

import { Transaction, TransactionDataBuilder } from '@mysten/sui/transactions';
import { bcs } from '@mysten/sui/bcs';
import { fromBase64 } from '@mysten/sui/utils';
import { verifyTransactionSignature } from '@mysten/sui/verify';
import { bytesEqual, type Bytes } from '../solanaBridge/bytes';
import { bcsU64, bcsVectorU8 } from './ids';
import type { SuiCoin } from './rpc';

export const GATE_MODULE = 'rumi_icusd_gate';
export const BRIDGE_OUT = 'bridge_out';
/** A transaction that merges more coins than this is refused: it would be large and slow. Merge some coins first. */
export const MAX_RETURN_COINS = 50;
export const SUBACCOUNT_BYTES = 32;

export interface ReturnCallPlan {
  packageId: string;
  gateId: string;
  /** The Gate's initial shared version, as the canister observed it. */
  gateInitialSharedVersion: string;
  /** The Sui address that sends, signs and pays gas. */
  sender: string;
  /** The coins the amount is taken from. `coins[0]` is the one the others merge into. */
  coins: readonly SuiCoin[];
  amountE8s: bigint;
  /** The ICP principal's raw bytes. */
  principalBytes: Bytes;
  /** Exactly 32 bytes; all zero for the default subaccount. */
  subaccount: Bytes;
  nonce: bigint;
}

export type CoinSelection = { ok: true; coins: SuiCoin[]; totalE8s: bigint } | { ok: false; message: string };

/**
 * Pick the coins an exact amount is taken from: a coin of exactly that amount, else the smallest single coin that covers it,
 * else the largest coins first. Deterministic for the same holdings.
 */
export function selectCoins(holdings: readonly SuiCoin[], amountE8s: bigint): CoinSelection {
  if (amountE8s <= 0n) return { ok: false, message: 'Enter an amount to return.' };
  const sorted = [...holdings].filter((c) => c.balance > 0n).sort((a, b) => (a.balance === b.balance ? a.objectId.localeCompare(b.objectId) : a.balance > b.balance ? -1 : 1));
  const total = sorted.reduce((sum, c) => sum + c.balance, 0n);
  if (total < amountE8s) return { ok: false, message: 'Your Sui address does not hold that much icUSD.' };
  const exact = sorted.find((c) => c.balance === amountE8s);
  if (exact) return { ok: true, coins: [exact], totalE8s: exact.balance };
  const covering = [...sorted].reverse().find((c) => c.balance > amountE8s);
  if (covering) return { ok: true, coins: [covering], totalE8s: covering.balance };
  const picked: SuiCoin[] = [];
  let sum = 0n;
  for (const coin of sorted) {
    picked.push(coin);
    sum += coin.balance;
    if (sum >= amountE8s) break;
  }
  if (picked.length > MAX_RETURN_COINS) return { ok: false, message: `Returning this amount would merge more than ${MAX_RETURN_COINS} coin objects. Merge some of your icUSD coins first.` };
  return { ok: true, coins: picked, totalE8s: sum };
}

/** The Move call with every object referenced exactly. The wallet adds only gas. Inputs are created in a fixed order. */
export function buildReturnTransaction(plan: ReturnCallPlan): Transaction {
  if (plan.coins.length === 0) throw new Error('A return needs at least one coin.');
  if (plan.subaccount.length !== SUBACCOUNT_BYTES) throw new RangeError('The subaccount must be exactly 32 bytes.');
  const tx = new Transaction();
  const gate = tx.sharedObjectRef({ objectId: plan.gateId, initialSharedVersion: plan.gateInitialSharedVersion, mutable: true });
  const refs = plan.coins.map((coin) => tx.objectRef({ objectId: coin.objectId, version: coin.version, digest: coin.digest }));
  const total = plan.coins.reduce((sum, c) => sum + c.balance, 0n);
  if (total < plan.amountE8s) throw new RangeError('The coins hold less than the amount.');
  if (refs.length > 1) tx.mergeCoins(refs[0], refs.slice(1));
  let coin: ReturnType<typeof tx.splitCoins>[number] | (typeof refs)[number] = refs[0];
  if (total > plan.amountE8s) [coin] = tx.splitCoins(refs[0], [tx.pure.u64(plan.amountE8s)]);
  tx.moveCall({
    target: `${plan.packageId}::${GATE_MODULE}::${BRIDGE_OUT}`,
    arguments: [gate, coin, tx.pure.vector('u8', Array.from(plan.principalBytes)), tx.pure.vector('u8', Array.from(plan.subaccount)), tx.pure.u64(plan.nonce)],
  });
  tx.setSender(plan.sender);
  return tx;
}

// ---------------------------------------------------------------------------
// Validating what a wallet returned
// ---------------------------------------------------------------------------

export interface ExpectedReturn {
  sender: string;
  packageId: string;
  gateId: string;
  gateInitialSharedVersion: string;
  coinIds: readonly string[];
  coinTotalE8s: bigint;
  amountE8s: bigint;
  principalBytes: Bytes;
  subaccount: Bytes;
  nonce: bigint;
}

export type ReturnCheckReason =
  | 'unparseable'
  | 'not_a_programmable_transaction'
  | 'sender_changed'
  | 'sponsored'
  | 'commands_changed'
  | 'gate_changed'
  | 'coins_changed'
  | 'amount_changed'
  | 'destination_changed'
  | 'bad_signature';

export type ReturnCheck = { ok: true; digest: string } | { ok: false; reason: ReturnCheckReason; message: string };

const MESSAGES: Record<ReturnCheckReason, string> = {
  unparseable: 'The wallet returned bytes that are not a Sui transaction.',
  not_a_programmable_transaction: 'The wallet returned a transaction that is not the reviewed return.',
  sender_changed: 'The wallet signed for a different Sui address than the one reviewed.',
  sponsored: 'The wallet returned a sponsored transaction. The return must pay its own gas.',
  commands_changed: 'The wallet changed the steps of the transaction.',
  gate_changed: 'The wallet changed the Gate or the package the transaction calls.',
  coins_changed: 'The wallet changed which icUSD coins the transaction spends.',
  amount_changed: 'The wallet changed the amount the transaction burns.',
  destination_changed: 'The wallet changed the Internet Computer destination, subaccount or nonce.',
  bad_signature: 'The wallet\'s signature does not verify for the reviewed Sui address.',
};

const no = (reason: ReturnCheckReason): ReturnCheck => ({ ok: false, reason, message: MESSAGES[reason] });

type Parsed = ReturnType<typeof bcs.TransactionData.parse>;
type Argument = { $kind: string; Input?: number; NestedResult?: [number, number]; Result?: number };

function pureBytes(parsed: Parsed, argument: Argument | undefined): Bytes | null {
  const kind = parsed.V1.kind;
  if (!argument || argument.$kind !== 'Input' || argument.Input === undefined || kind.$kind !== 'ProgrammableTransaction') return null;
  const input = kind.ProgrammableTransaction.inputs[argument.Input];
  return input && input.$kind === 'Pure' ? fromBase64(input.Pure.bytes) : null;
}

function ownedObjectId(parsed: Parsed, argument: Argument | undefined): string | null {
  const kind = parsed.V1.kind;
  if (!argument || argument.$kind !== 'Input' || argument.Input === undefined || kind.$kind !== 'ProgrammableTransaction') return null;
  const input = kind.ProgrammableTransaction.inputs[argument.Input];
  return input && input.$kind === 'Object' && input.Object.$kind === 'ImmOrOwnedObject' ? input.Object.ImmOrOwnedObject.objectId : null;
}

/**
 * Check a wallet's signed transaction against the reviewed attempt. Pure and offline: it reads no network.
 * `signature` is the wallet's base64 signature over `bytes`.
 */
export async function validateSignedReturn(bytes: Bytes, signature: string, expected: ExpectedReturn): Promise<ReturnCheck> {
  let parsed: Parsed;
  try {
    parsed = bcs.TransactionData.parse(bytes);
  } catch {
    return no('unparseable');
  }
  const data = parsed.V1;
  const kind = data?.kind;
  if (!data || !kind || kind.$kind !== 'ProgrammableTransaction') return no('not_a_programmable_transaction');
  if (data.sender !== expected.sender) return no('sender_changed');
  if (data.gasData.owner !== expected.sender) return no('sponsored');

  const { inputs, commands } = kind.ProgrammableTransaction;
  const merges = expected.coinIds.length > 1;
  const splits = expected.coinTotalE8s > expected.amountE8s;
  const shape = [...(merges ? ['MergeCoins'] : []), ...(splits ? ['SplitCoins'] : []), 'MoveCall'];
  if (commands.length !== shape.length || commands.some((c, i) => c.$kind !== shape[i])) return no('commands_changed');

  let index = 0;
  const primary = expected.coinIds[0];
  if (merges) {
    const merge = commands[index++] as Extract<(typeof commands)[number], { $kind: 'MergeCoins' }>;
    const destination = ownedObjectId(parsed, merge.MergeCoins.destination as Argument);
    const sources = merge.MergeCoins.sources.map((s) => ownedObjectId(parsed, s as Argument));
    if (destination !== primary || sources.length !== expected.coinIds.length - 1 || sources.some((id, i) => id !== expected.coinIds[i + 1])) return no('coins_changed');
  }
  let spent: Argument = { $kind: 'Input' };
  if (splits) {
    const split = commands[index] as Extract<(typeof commands)[number], { $kind: 'SplitCoins' }>;
    if (ownedObjectId(parsed, split.SplitCoins.coin as Argument) !== primary) return no('coins_changed');
    const amounts = split.SplitCoins.amounts.map((a) => pureBytes(parsed, a as Argument));
    if (amounts.length !== 1 || amounts[0] === null || !bytesEqual(amounts[0], bcsU64(expected.amountE8s))) return no('amount_changed');
    spent = { $kind: 'NestedResult', NestedResult: [index, 0] };
    index += 1;
  }
  const call = (commands[index] as Extract<(typeof commands)[number], { $kind: 'MoveCall' }>).MoveCall;
  if (call.package !== expected.packageId || call.module !== GATE_MODULE || call.function !== BRIDGE_OUT || call.typeArguments.length !== 0 || call.arguments.length !== 5) {
    return no('gate_changed');
  }
  const gateArg = call.arguments[0] as Argument;
  const gateInput = gateArg.$kind === 'Input' && gateArg.Input !== undefined ? inputs[gateArg.Input] : undefined;
  if (
    !gateInput ||
    gateInput.$kind !== 'Object' ||
    gateInput.Object.$kind !== 'SharedObject' ||
    gateInput.Object.SharedObject.objectId !== expected.gateId ||
    gateInput.Object.SharedObject.initialSharedVersion !== expected.gateInitialSharedVersion ||
    gateInput.Object.SharedObject.mutable !== true
  ) {
    return no('gate_changed');
  }
  const coinArg = call.arguments[1] as Argument;
  if (splits) {
    if (coinArg.$kind !== 'NestedResult' || coinArg.NestedResult?.[0] !== spent.NestedResult?.[0] || coinArg.NestedResult?.[1] !== 0) return no('coins_changed');
  } else if (ownedObjectId(parsed, coinArg) !== primary) {
    return no('coins_changed');
  }
  const principal = pureBytes(parsed, call.arguments[2] as Argument);
  const subaccount = pureBytes(parsed, call.arguments[3] as Argument);
  const nonce = pureBytes(parsed, call.arguments[4] as Argument);
  if (
    !principal || !bytesEqual(principal, bcsVectorU8(expected.principalBytes)) ||
    !subaccount || !bytesEqual(subaccount, bcsVectorU8(expected.subaccount)) ||
    !nonce || !bytesEqual(nonce, bcsU64(expected.nonce))
  ) {
    return no('destination_changed');
  }

  try {
    await verifyTransactionSignature(bytes, signature, { address: expected.sender });
  } catch {
    return no('bad_signature');
  }
  return { ok: true, digest: TransactionDataBuilder.getDigestFromBytes(bytes) };
}

/** The typed transaction digest (base58) of bytes the page already holds. */
export function digestOf(bytes: Bytes): string {
  return TransactionDataBuilder.getDigestFromBytes(bytes);
}
