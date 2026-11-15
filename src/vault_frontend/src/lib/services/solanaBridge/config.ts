/**
 * The Solana bridge instance's live configuration: the native counterpart of the EVM `BridgeConfig`.
 *
 * A Solana instance is NOT an EVM config with different field names: it has a mint, a canonical return
 * account, a token program and a genesis hash instead of a contract and a chain id. This file reads the
 * canister's own answers (`get_bridge_info`, `get_bootstrap_status`) and turns them into one of two
 * results, like `resolveBridgeConfig` does for Base: a usable config, or a plain-language reason the
 * destination is not offered. Only the canister's answer can enable a Solana destination; nothing here
 * enables anything by itself.
 *
 * Checks (a failure makes the destination unavailable, and no wallet is ever asked):
 *  - the canister is initialized, its genesis hash is a cluster this app knows, its memo program is the
 *    Memo v3 program;
 *  - the mint account is bound (`MintState::Bound`) and every key the page needs is present and a 32-byte key;
 *  - the canonical return account the canister reports is the associated token account of
 *    (return owner, token program, mint) as derived HERE, so a wrong account is never shown as the place
 *    to send tokens.
 */

import type { BootstrapStatus, BridgeInfo } from '$declarations/rumi_icusd_sol_bridge/rumi_icusd_sol_bridge.did';
import { bytesEqual, pubkeyFromBase58, base58Encode } from './bytes';
import { solanaNetworkByGenesis, type SolanaNetworkInfo } from './networks';
import { MEMO_PROGRAM_V3_B58, TOKEN_PROGRAM_IDS, deriveAta, tokenProgramBytes, type TokenProgramKind } from './programs';

export interface SolanaBridgeConfig {
  family: 'solana';
  /** The bridge canister instance for this cluster. */
  instanceCanisterId: string;
  network: SolanaNetworkInfo;
  tokenProgram: TokenProgramKind;
  /** Base58 id of the token program (the exact program the mint and the return account belong to). */
  tokenProgramId: string;
  memoProgram: string;
  mint: string;
  returnOwner: string;
  /** The canonical return ATA: the only place a return may be sent. */
  returnAccount: string;
  /** The bridge's other role keys (a holder may be none of them; a recipient neither). */
  feePayer: string;
  mintAuthority: string;
  icusdLedger: string;
  minDepositE8s: bigint;
  /** Also the largest single return the bridge credits. */
  maxDepositE8s: bigint;
  depositFeeE8s: bigint;
  releaseFeeE8s: bigint;
  depositsPaused: boolean;
  /** Mints to Solana paused: a deposit would wait. */
  mintsPaused: boolean;
  releasesPaused: boolean;
  halted: string | null;
}

export type SolanaConfigResult =
  | { ok: true; config: SolanaBridgeConfig }
  | { ok: false; message: string; network: SolanaNetworkInfo | null };

function tokenProgramOf(info: BridgeInfo): TokenProgramKind {
  return 'Token2022' in info.token_program ? 'token2022' : 'classic';
}

function key(label: string, value: string | undefined): string | { problem: string } {
  if (value === undefined || pubkeyFromBase58(value) === null) return { problem: `${label} is missing or is not a Solana key` };
  return value;
}

/** Turn the instance's answers into a config or a reason. Pure. */
export function resolveSolanaConfig(info: BridgeInfo, bootstrap: BootstrapStatus, instanceCanisterId: string): SolanaConfigResult {
  const network = solanaNetworkByGenesis(info.genesis_hash);
  const fail = (message: string): SolanaConfigResult => ({ ok: false, message, network });
  if (!info.initialized) return fail('The Solana bridge is not set up yet.');
  if (network === null) return fail('The Solana bridge reports a network this app does not know, so it is not offered.');
  if (info.memo_program !== MEMO_PROGRAM_V3_B58) return fail('The Solana bridge uses a memo program this app does not recognize.');
  if (!('Bound' in bootstrap.mint_state)) return fail('The Solana bridge has no icUSD mint on Solana yet.');

  const fields = {
    mint: key('The mint', bootstrap.mint_account[0]),
    returnOwner: key('The return owner', bootstrap.return_owner[0]),
    returnAccount: key('The return account', bootstrap.return_account[0]),
    feePayer: key('The fee payer', bootstrap.fee_payer[0]),
    mintAuthority: key('The mint authority', bootstrap.mint_authority[0]),
  };
  for (const value of Object.values(fields)) if (typeof value !== 'string') return fail(`${value.problem}, so the bridge is not offered.`);
  const { mint, returnOwner, returnAccount, feePayer, mintAuthority } = fields as Record<keyof typeof fields, string>;

  const tokenProgram = tokenProgramOf(info);
  const derived = deriveAta(pubkeyFromBase58(returnOwner)!, tokenProgramBytes(tokenProgram), pubkeyFromBase58(mint)!);
  if (!bytesEqual(derived, pubkeyFromBase58(returnAccount)!)) {
    return fail(`The return account the bridge reports is not the associated token account this app derives (${base58Encode(derived)}), so it is not offered.`);
  }
  const limits = info.limits;
  return {
    ok: true,
    config: {
      family: 'solana',
      instanceCanisterId,
      network,
      tokenProgram,
      tokenProgramId: TOKEN_PROGRAM_IDS[tokenProgram],
      memoProgram: info.memo_program,
      mint,
      returnOwner,
      returnAccount,
      feePayer,
      mintAuthority,
      icusdLedger: info.icusd_ledger.toText(),
      minDepositE8s: limits.min_deposit_e8s,
      maxDepositE8s: limits.max_deposit_e8s,
      depositFeeE8s: info.fees.deposit_fee_e8s,
      releaseFeeE8s: info.fees.release_fee_e8s,
      depositsPaused: info.pauses.deposits,
      mintsPaused: info.pauses.mints,
      releasesPaused: info.pauses.releases,
      halted: info.halted[0] ?? null,
    },
  };
}
