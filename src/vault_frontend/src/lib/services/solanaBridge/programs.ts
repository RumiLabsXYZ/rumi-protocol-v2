/**
 * Protocol constants and associated-token-account derivation for the Solana bridge branch.
 * The ids are the ones the backend pins (`tx/mod.rs`); the specs check them against its raw bytes.
 */

import { base58Decode, concat, isOnCurve, pubkeyFromBase58, sha256, type Bytes } from './bytes';

export type TokenProgramKind = 'classic' | 'token2022';

function key(text: string): Bytes {
  const bytes = base58Decode(text);
  if (bytes === null || bytes.length !== 32) throw new Error(`bad built-in program id ${text}`);
  return bytes;
}

export const SYSTEM_PROGRAM_B58 = '11111111111111111111111111111111';
export const TOKEN_PROGRAM_B58 = 'TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA';
export const TOKEN_2022_PROGRAM_B58 = 'TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb';
export const ATA_PROGRAM_B58 = 'ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL';
export const MEMO_PROGRAM_V3_B58 = 'MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr';
export const COMPUTE_BUDGET_PROGRAM_B58 = 'ComputeBudget111111111111111111111111111111';
export const RECENT_BLOCKHASHES_SYSVAR_B58 = 'SysvarRecentB1ockHashes11111111111111111111';

export const SYSTEM_PROGRAM = key(SYSTEM_PROGRAM_B58);
export const ATA_PROGRAM = key(ATA_PROGRAM_B58);
export const MEMO_PROGRAM_V3 = key(MEMO_PROGRAM_V3_B58);
export const COMPUTE_BUDGET_PROGRAM = key(COMPUTE_BUDGET_PROGRAM_B58);

export const TOKEN_PROGRAM_IDS: Record<TokenProgramKind, string> = {
  classic: TOKEN_PROGRAM_B58,
  token2022: TOKEN_2022_PROGRAM_B58,
};

export function tokenProgramBytes(kind: TokenProgramKind): Bytes {
  return key(TOKEN_PROGRAM_IDS[kind]);
}

/** The keys no wallet or role may be (`is_reserved_key` in the backend). */
export const RESERVED_KEYS_B58: readonly string[] = [
  SYSTEM_PROGRAM_B58,
  TOKEN_PROGRAM_B58,
  TOKEN_2022_PROGRAM_B58,
  ATA_PROGRAM_B58,
  COMPUTE_BUDGET_PROGRAM_B58,
  RECENT_BLOCKHASHES_SYSVAR_B58,
];

/**
 * The associated token account of `wallet` for `mint` under `tokenProgram`: the program derived address
 * of `[wallet, token program, mint]` under the ATA program (highest bump that is off the curve).
 */
export function deriveAta(wallet: Bytes, tokenProgram: Bytes, mint: Bytes): Bytes {
  const seeds = concat(wallet, tokenProgram, mint);
  const tail = new TextEncoder().encode('ProgramDerivedAddress');
  for (let bump = 255; bump >= 0; bump--) {
    const candidate = sha256(concat(seeds, Uint8Array.of(bump), ATA_PROGRAM, tail));
    if (!isOnCurve(candidate)) return candidate;
  }
  throw new Error('no program derived address exists for these seeds');
}

export function isValidPubkey(text: string): boolean {
  return pubkeyFromBase58(text) !== null;
}
