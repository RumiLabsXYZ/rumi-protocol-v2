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
 * Transfer ICP to a recipient
 * 
 * @param recipient - The principal ID of the recipient
 * @param amountE8s - Amount to transfer in e8s (1 ICP = 100,000,000 e8s)
 * @returns TransferResult with success status and block index or error
 */
export async function transferICP(
  recipient: string,
  amountE8s: bigint
): Promise<TransferResult> {
  try {
    // Validate recipient
    if (!isValidPrincipal(recipient)) {
      return { success: false, error: 'Invalid recipient principal' };
    }

    // Validate amount
    if (amountE8s <= BigInt(0)) {
      return { success: false, error: 'Amount must be greater than 0' };
    }

    // Get the ICP ledger actor
    const icpLedgerId = CONFIG.currentIcpLedgerId;
    console.log('📤 Creating ICP ledger actor for:', icpLedgerId);
    
    const actor = await walletStore.getActor(icpLedgerId, canisterIDLs.icp_ledger);
    
    if (!actor) {
      return { success: false, error: 'Failed to create ICP ledger actor. Please reconnect your wallet.' };
    }

    // Prepare transfer arguments (ICRC-1 standard)
    const transferArgs = {
      to: {
        owner: Principal.fromText(recipient.trim()),
        subaccount: [] as [] | [Uint8Array]
      },
      amount: amountE8s,
      fee: [] as [] | [bigint], // Use default fee
      memo: [] as [] | [Uint8Array],
      from_subaccount: [] as [] | [Uint8Array],
      created_at_time: [] as [] | [bigint]
    };

    console.log('📤 Initiating ICP transfer:', {
      to: recipient,
      amount: Number(amountE8s) / 1e8,
      amountE8s: amountE8s.toString()
    });

    // Execute transfer
    const result = await (actor as any).icrc1_transfer(transferArgs);
    
    console.log('📤 ICP transfer result:', result);

    // Handle result variant
    if ('Ok' in result) {
      console.log('✅ ICP transfer successful, block index:', result.Ok.toString());
      return { 
        success: true, 
        blockIndex: result.Ok 
      };
    } else if ('Err' in result) {
      const errorMsg = formatTransferError(result.Err);
      console.error('❌ ICP transfer error:', errorMsg);
      return { 
        success: false, 
        error: errorMsg 
      };
    }

    return { success: false, error: 'Unexpected response from ledger' };

  } catch (err) {
    console.error('❌ ICP transfer exception:', err);
    const errorMsg = err instanceof Error ? err.message : 'Transfer failed';
    return { success: false, error: errorMsg };
  }
}

/**
 * Transfer icUSD to a recipient
 * 
 * @param recipient - The principal ID of the recipient
 * @param amountE8s - Amount to transfer in e8s (1 icUSD = 100,000,000 e8s)
 * @returns TransferResult with success status and block index or error
 */
export async function transferICUSD(
  recipient: string,
  amountE8s: bigint
): Promise<TransferResult> {
  try {
    // Validate recipient
    if (!isValidPrincipal(recipient)) {
      return { success: false, error: 'Invalid recipient principal' };
    }

    // Validate amount
    if (amountE8s <= BigInt(0)) {
      return { success: false, error: 'Amount must be greater than 0' };
    }

    // Get the icUSD ledger actor
    const icusdLedgerId = CONFIG.currentIcusdLedgerId;
    console.log('📤 Creating icUSD ledger actor for:', icusdLedgerId);
    
    const actor = await walletStore.getActor(icusdLedgerId, canisterIDLs.icusd_ledger);
    
    if (!actor) {
      return { success: false, error: 'Failed to create icUSD ledger actor. Please reconnect your wallet.' };
    }

    // Prepare transfer arguments (ICRC-1 standard)
    const transferArgs = {
      to: {
        owner: Principal.fromText(recipient.trim()),
        subaccount: [] as [] | [Uint8Array]
      },
      amount: amountE8s,
      fee: [] as [] | [bigint], // Use default fee
      memo: [] as [] | [Uint8Array],
      from_subaccount: [] as [] | [Uint8Array],
      created_at_time: [] as [] | [bigint]
    };

    console.log('📤 Initiating icUSD transfer:', {
      to: recipient,
      amount: Number(amountE8s) / 1e8,
      amountE8s: amountE8s.toString()
    });

    // Execute transfer
    const result = await (actor as any).icrc1_transfer(transferArgs);
    
    console.log('📤 icUSD transfer result:', result);

    // Handle result variant
    if ('Ok' in result) {
      console.log('✅ icUSD transfer successful, block index:', result.Ok.toString());
      return { 
        success: true, 
        blockIndex: result.Ok 
      };
    } else if ('Err' in result) {
      const errorMsg = formatTransferError(result.Err);
      console.error('❌ icUSD transfer error:', errorMsg);
      return { 
        success: false, 
        error: errorMsg 
      };
    }

    return { success: false, error: 'Unexpected response from ledger' };

  } catch (err) {
    console.error('❌ icUSD transfer exception:', err);
    const errorMsg = err instanceof Error ? err.message : 'Transfer failed';
    return { success: false, error: errorMsg };
  }
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
