/**
 * Transfer Service for ICP and icUSD tokens
 * 
 * Handles ICRC-1 transfers for Internet Identity users.
 * Uses walletStore.getActor() which properly handles II delegation auth.
 */

import { Principal } from '@dfinity/principal';
import { canisterIDLs } from './pnp';
import { walletStore } from '../stores/wallet';
import { CONFIG } from '../config';
import { ICRC1_IDL } from '../idls/ledger.idl.js';

// Transfer fee in e8s (0.0001 ICP = 10,000 e8s)
export const ICP_TRANSFER_FEE = BigInt(10_000);

// icUSD transfer fee - typically same as ICP but may differ
export const ICUSD_TRANSFER_FEE = BigInt(10_000);

export interface TransferResult {
  success: boolean;
  blockIndex?: bigint;
  error?: string;
}

/**
 * Validates if a string is a valid ICP principal
 */
export function isValidPrincipal(principalStr: string): boolean {
  if (!principalStr || typeof principalStr !== 'string') {
    return false;
  }
  
  try {
    Principal.fromText(principalStr.trim());
    return true;
  } catch {
    return false;
  }
}

/**
 * Converts ICRC-1 transfer error variant to human-readable string
 */
function formatTransferError(err: any): string {
  if (!err) return 'Unknown error';
  
  // Handle ICRC-1 error variants
  if ('InsufficientFunds' in err) {
    const balance = err.InsufficientFunds?.balance;
    return `Insufficient funds. Available balance: ${balance ? Number(balance) / 1e8 : 'unknown'}`;
  }
  if ('BadFee' in err) {
    const expected = err.BadFee?.expected_fee;
    return `Invalid fee. Expected: ${expected ? Number(expected) / 1e8 : 'unknown'}`;
  }
  if ('BadBurn' in err) {
    return 'Invalid burn amount (minimum burn not met)';
  }
  if ('InsufficientAllowance' in err) {
    return 'Insufficient allowance for transfer';
  }
  if ('TooOld' in err) {
    return 'Transaction too old';
  }
  if ('CreatedInFuture' in err) {
    return 'Transaction created in the future';
  }
  if ('Duplicate' in err) {
    return 'Duplicate transaction';
  }
  if ('TemporarilyUnavailable' in err) {
    return 'Ledger temporarily unavailable. Please try again.';
  }
  if ('GenericError' in err) {
    return err.GenericError?.message || 'Generic error occurred';
  }
  
  // Fallback: stringify the error
  return JSON.stringify(err);
}

/**
 * Generic ICRC-1 transfer — works for any token (ICP, icUSD, ckUSDT, ckUSDC, etc.)
 *
 * @param ledgerCanisterId - The canister ID of the token's ICRC-1 ledger
 * @param recipient - The principal ID of the recipient
 * @param amountRaw - Amount in the ledger's smallest unit (e.g. e8s for ICP, e6s for ckUSDT)
 */
export async function transferICRC1(
  ledgerCanisterId: string,
  recipient: string,
  amountRaw: bigint
): Promise<TransferResult> {
  try {
    if (!isValidPrincipal(recipient)) {
      return { success: false, error: 'Invalid recipient principal' };
    }
    if (amountRaw <= 0n) {
      return { success: false, error: 'Amount must be greater than 0' };
    }

    console.log('📤 Creating ICRC-1 ledger actor for:', ledgerCanisterId);
    const actor = await walletStore.getActor(ledgerCanisterId, ICRC1_IDL);
    if (!actor) {
      return { success: false, error: 'Failed to create ledger actor. Please reconnect your wallet.' };
    }

    const transferArgs = {
      to: {
        owner: Principal.fromText(recipient.trim()),
        subaccount: [] as [] | [Uint8Array]
      },
      amount: amountRaw,
      fee: [] as [] | [bigint],
      memo: [] as [] | [Uint8Array],
      from_subaccount: [] as [] | [Uint8Array],
      created_at_time: [] as [] | [bigint]
    };

    console.log('📤 Initiating ICRC-1 transfer:', {
      ledger: ledgerCanisterId,
      to: recipient,
      amountRaw: amountRaw.toString()
    });

    const result = await (actor as any).icrc1_transfer(transferArgs);
    console.log('📤 ICRC-1 transfer result:', result);

    if ('Ok' in result) {
      console.log('✅ Transfer successful, block index:', result.Ok.toString());
      return { success: true, blockIndex: result.Ok };
    } else if ('Err' in result) {
      const errorMsg = formatTransferError(result.Err);
      console.error('❌ Transfer error:', errorMsg);
      return { success: false, error: errorMsg };
    }

    return { success: false, error: 'Unexpected response from ledger' };
  } catch (err) {
    console.error('❌ Transfer exception:', err);
    return { success: false, error: err instanceof Error ? err.message : 'Transfer failed' };
  }
}

/**
 * Query a ledger's ICRC-1 fee (returns raw smallest-unit bigint)
 */
export async function queryICRC1Fee(ledgerCanisterId: string): Promise<bigint> {
  try {
    const { TokenService } = await import('./tokenService');
    const actor = await TokenService.createAnonymousActor(ledgerCanisterId, ICRC1_IDL);
    return await (actor as any).icrc1_fee();
  } catch (err) {
    console.warn('Failed to query fee for', ledgerCanisterId, err);
    return 10_000n; // fallback
  }
}

/** Transfer ICP (convenience wrapper) */
export async function transferICP(recipient: string, amountE8s: bigint): Promise<TransferResult> {
  return transferICRC1(CONFIG.currentIcpLedgerId, recipient, amountE8s);
}

/** Transfer icUSD (convenience wrapper) */
export async function transferICUSD(recipient: string, amountE8s: bigint): Promise<TransferResult> {
  return transferICRC1(CONFIG.currentIcusdLedgerId, recipient, amountE8s);
}

/**
 * Converts a human-readable amount to e8s
 * @param amount - Amount in decimal (e.g., 1.5 for 1.5 ICP)
 * @returns Amount in e8s as bigint
 */
export function toE8s(amount: number): bigint {
  return BigInt(Math.floor(amount * 1e8));
}

/**
 * Converts e8s to human-readable amount
 * @param e8s - Amount in e8s
 * @returns Amount as a number (e.g., 1.5 for 150000000 e8s)
 */
export function fromE8s(e8s: bigint): number {
  return Number(e8s) / 1e8;
}
