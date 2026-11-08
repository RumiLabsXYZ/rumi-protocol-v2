/**
 * The holder's return transaction, byte for byte what the backend's reference builder produces
 * (`tx/builders.rs::build_return_message`): a LEGACY message with
 *
 *   [ComputeBudget (0 to 2, optional)] , TransferChecked(source -> canonical return ATA, authority = holder,
 *   exact token program and mint, 8 decimals) , Memo `rumi1|r|...` (no account metas)
 *
 * The holder is the fee payer and the only signer, and the message uses an ORDINARY recent blockhash:
 * a holder transaction never uses the canister's durable-nonce lane. Account order follows the SDK rule the
 * backend pins (payer first, then writable signers, read-only signers, writable others, read-only others,
 * each group ascending by key bytes), so the same inputs give the same bytes.
 *
 * The specs compare this builder with the vectors of the backend's independent Python reference.
 */

import { concat, u32le, u64le, type Bytes, bytesEqual } from './bytes';
import { COMPUTE_BUDGET_PROGRAM, MEMO_PROGRAM_V3, deriveAta, tokenProgramBytes, type TokenProgramKind } from './programs';

export const TOKEN_DECIMALS = 8;
export const TAG_TRANSFER_CHECKED = 12;
export const MAX_TRANSACTION_BYTES = 1232;

export interface AccountMeta {
  key: Bytes;
  signer: boolean;
  writable: boolean;
}

export interface Instruction {
  program: Bytes;
  accounts: AccountMeta[];
  data: Bytes;
}

export function transferCheckedInstruction(
  tokenProgram: Bytes,
  source: Bytes,
  mint: Bytes,
  destination: Bytes,
  authority: Bytes,
  amount: bigint,
): Instruction {
  return {
    program: tokenProgram,
    accounts: [
      { key: source, signer: false, writable: true },
      { key: mint, signer: false, writable: false },
      { key: destination, signer: false, writable: true },
      { key: authority, signer: true, writable: false },
    ],
    data: concat(Uint8Array.of(TAG_TRANSFER_CHECKED), u64le(amount), Uint8Array.of(TOKEN_DECIMALS)),
  };
}

export function memoInstruction(memoProgram: Bytes, text: string): Instruction {
  return { program: memoProgram, accounts: [], data: new TextEncoder().encode(text) };
}

/** ComputeBudget SetComputeUnitLimit (tag 2, u32) and SetComputeUnitPrice (tag 3, u64): zero account metas. */
export function setComputeUnitLimit(units: number): Instruction {
  return { program: COMPUTE_BUDGET_PROGRAM, accounts: [], data: concat(Uint8Array.of(2), u32le(units)) };
}
export function setComputeUnitPrice(microLamports: bigint): Instruction {
  return { program: COMPUTE_BUDGET_PROGRAM, accounts: [], data: concat(Uint8Array.of(3), u64le(microLamports)) };
}

export function compactU16(value: number): Bytes {
  if (!Number.isInteger(value) || value < 0 || value > 0xffff) throw new RangeError('compact-u16 out of range');
  const out: number[] = [];
  let n = value;
  for (;;) {
    const byte = n & 0x7f;
    n >>= 7;
    if (n === 0) {
      out.push(byte);
      return Uint8Array.from(out);
    }
    out.push(byte | 0x80);
  }
}

function compareKeys(a: Bytes, b: Bytes): number {
  for (let i = 0; i < 32; i++) if (a[i] !== b[i]) return a[i] - b[i];
  return 0;
}

/** A legacy message compiled the way the Solana SDK (and the backend) does it. */
export function compileLegacyMessage(instructions: readonly Instruction[], payer: Bytes, recentBlockhash: Bytes): Bytes {
  if (payer.length !== 32 || recentBlockhash.length !== 32) throw new RangeError('keys and blockhashes are 32 bytes');
  const metas = new Map<string, { key: Bytes; signer: boolean; writable: boolean }>();
  const touch = (key: Bytes, signer: boolean, writable: boolean) => {
    const id = Array.from(key).join(',');
    const m = metas.get(id) ?? { key, signer: false, writable: false };
    m.signer ||= signer;
    m.writable ||= writable;
    metas.set(id, m);
  };
  for (const ix of instructions) {
    touch(ix.program, false, false);
    for (const a of ix.accounts) touch(a.key, a.signer, a.writable);
  }
  const payerId = Array.from(payer).join(',');
  metas.delete(payerId);
  const others = [...metas.values()].sort((x, y) => compareKeys(x.key, y.key));
  const writableSigners = [payer, ...others.filter((m) => m.signer && m.writable).map((m) => m.key)];
  const readonlySigners = others.filter((m) => m.signer && !m.writable).map((m) => m.key);
  const writableOthers = others.filter((m) => !m.signer && m.writable).map((m) => m.key);
  const readonlyOthers = others.filter((m) => !m.signer && !m.writable).map((m) => m.key);
  const keys = [...writableSigners, ...readonlySigners, ...writableOthers, ...readonlyOthers];
  if (keys.length > 256) throw new RangeError('too many accounts');
  const indexOf = (key: Bytes) => keys.findIndex((k) => bytesEqual(k, key));
  const parts: Bytes[] = [
    Uint8Array.of(writableSigners.length + readonlySigners.length, readonlySigners.length, readonlyOthers.length),
    compactU16(keys.length),
    ...keys,
    recentBlockhash,
    compactU16(instructions.length),
  ];
  for (const ix of instructions) {
    parts.push(
      Uint8Array.of(indexOf(ix.program)),
      compactU16(ix.accounts.length),
      Uint8Array.from(ix.accounts.map((a) => indexOf(a.key))),
      compactU16(ix.data.length),
      ix.data,
    );
  }
  return concat(...parts);
}

export interface ReturnTransactionInput {
  tokenProgram: TokenProgramKind;
  mint: Bytes;
  /** The bridge's return-owner key (the owner of the canonical return ATA). */
  returnOwner: Bytes;
  /** The bridge's other role keys: a holder may be none of them. */
  roleKeys: readonly Bytes[];
  memoProgram?: Bytes;
  holder: Bytes;
  sourceTokenAccount: Bytes;
  amount: bigint;
  memo: string;
  recentBlockhash: Bytes;
  /** Optional wallet-style ComputeBudget instructions placed first (at most two are valid). */
  computeBudget?: { unitLimit?: number; unitPrice?: bigint };
}

export interface BuiltReturnTransaction {
  /** The message bytes the holder signs. */
  message: Bytes;
  /** The canonical return ATA the tokens go to. */
  returnAta: Bytes;
}

/** `build_return_message`. Throws on a zero amount, a holder that is a bridge role key or an oversize packet. */
export function buildReturnMessage(input: ReturnTransactionInput): BuiltReturnTransaction {
  if (input.amount <= 0n) throw new RangeError('the amount must be at least 1');
  if ([input.returnOwner, ...input.roleKeys].some((k) => bytesEqual(k, input.holder))) {
    throw new RangeError("a holder cannot be one of the bridge's role keys");
  }
  const tokenProgram = tokenProgramBytes(input.tokenProgram);
  const returnAta = deriveAta(input.returnOwner, tokenProgram, input.mint);
  const instructions: Instruction[] = [];
  if (input.computeBudget?.unitLimit !== undefined) instructions.push(setComputeUnitLimit(input.computeBudget.unitLimit));
  if (input.computeBudget?.unitPrice !== undefined) instructions.push(setComputeUnitPrice(input.computeBudget.unitPrice));
  instructions.push(
    transferCheckedInstruction(tokenProgram, input.sourceTokenAccount, input.mint, returnAta, input.holder, input.amount),
    memoInstruction(input.memoProgram ?? MEMO_PROGRAM_V3, input.memo),
  );
  const message = compileLegacyMessage(instructions, input.holder, input.recentBlockhash);
  if (1 + 64 + message.length > MAX_TRANSACTION_BYTES) throw new RangeError('the transaction does not fit in one packet');
  return { message, returnAta };
}

/** The transaction a wallet is asked to sign: one empty signature slot, then the message. */
export function unsignedTransactionBytes(message: Bytes): Bytes {
  return concat(Uint8Array.of(1), new Uint8Array(64), message);
}

export type SignedCheck =
  | { ok: true; signature: Bytes }
  | { ok: false; reason: 'malformed' | 'signature_count' | 'unsigned' | 'message_changed' };

/**
 * Whether bytes a wallet gave back are OUR transaction, signed: one signature that is not all zero,
 * followed by exactly the message that was prepared. A wallet that changed anything (an added instruction,
 * another blockhash) produces a transaction the bridge would not recognize as a return, so it is never sent.
 */
export function checkSignedTransaction(signed: Bytes, message: Bytes): SignedCheck {
  if (signed.length < 1 + 64) return { ok: false, reason: 'malformed' };
  if (signed[0] !== 1) return { ok: false, reason: 'signature_count' };
  const signature = signed.slice(1, 65);
  if (signature.every((b) => b === 0)) return { ok: false, reason: 'unsigned' };
  return bytesEqual(signed.slice(65), message) ? { ok: true, signature } : { ok: false, reason: 'message_changed' };
}

/** The message bytes of a wire transaction (`[count][signatures][message]`), or null. */
export function messageOfWire(wire: Bytes): Bytes | null {
  if (wire.length < 1) return null;
  let count = 0;
  let shift = 0;
  let at = 0;
  for (;;) {
    if (at >= wire.length || shift > 14) return null;
    const byte = wire[at++];
    count |= (byte & 0x7f) << shift;
    if ((byte & 0x80) === 0) break;
    shift += 7;
  }
  const start = at + count * 64;
  return start <= wire.length ? wire.slice(start) : null;
}
