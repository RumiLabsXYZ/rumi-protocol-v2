import { Principal } from '@dfinity/principal';
import { Actor, HttpAgent } from "@dfinity/agent";
import { get } from 'svelte/store';
import { walletStore } from '../../stores/wallet';
import { CONFIG } from '../../config';
import { permissionManager } from '../PermissionManager';
import type { UserBalances } from '../types';

// Import types from declarations
import type {
  _SERVICE,
  Vault as CanisterVault,
  ProtocolStatus as CanisterProtocolStatus,
  LiquidityStatus as CanisterLiquidityStatus,
  Fees,
  SuccessWithFee,
  ProtocolError,
  OpenVaultSuccess,
} from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did';

import type { _SERVICE as IcpLedgerService } from '$declarations/icp_ledger/icp_ledger.did';
import type { _SERVICE as IcusdLedgerService } from '$declarations/icusd_ledger/icusd_ledger.did';

export const E8S = 100_000_000;

/**
 * Helper to check if an error is a stale actor/read state error
 */
function isStaleActorError(err: any): boolean {
  if (err instanceof Error) {
    const msg = err.message.toLowerCase();
    return msg.includes('invalid read state request') ||
           msg.includes('response could not be found') ||
           msg.includes('actor') && msg.includes('stale');
  }
  return false;
}

/**
 * Streamlined wallet operations with automatic permission handling
 */
export class walletOperations {
  /**
   * Reset wallet signer state after errors
   */
  static async resetWalletSignerState(): Promise<void> {
    try {
      const walletState = get(walletStore);
      if (walletState.isConnected) {
        console.log('Resetting wallet signer state');
        await walletStore.refreshWallet();
      }
    } catch (err) {
      console.error('Failed to reset wallet signer state:', err);
    }
  }

  /**
   * Approve ICP transfer - now streamlined with automatic permission handling
   */
  static async approveIcpTransfer(amount: bigint, spenderCanisterId: string): Promise<{success: boolean, error?: string}> {
    try {
      // FIXED: Remove permission check that causes "Permission request was denied" errors
      // The wallet will handle permissions automatically when the transaction is attempted
      
      console.log(`Approving ${amount.toString()} e8s ICP for ${spenderCanisterId}`);
      
      // Get the ICP ledger actor
      const icpActor = await walletStore.getActor(CONFIG.currentIcpLedgerId, CONFIG.icp_ledgerIDL) as unknown as IcpLedgerService;
      
      // Request approval
      const approvalResult = await icpActor.icrc2_approve({
        amount,
        spender: { 
          owner: Principal.fromText(spenderCanisterId),
          subaccount: [] 
        },
        expires_at: [], 
        expected_allowance: [], 
        memo: [],
        fee: [],
        from_subaccount: [],
        created_at_time: []
      });
      
      if ('Ok' in approvalResult) {
        console.log('ICP approval successful');
        return { success: true };
      } else {
        return { 
          success: false, 
          error: `ICP approval failed: ${String(approvalResult.Err && typeof approvalResult.Err === 'object' ? Object.keys(approvalResult.Err)[0] : approvalResult.Err)}` 
        };
      }
    } catch (error) {
      console.error('ICP approval failed:', error);
      return { 
        success: false, 
        error: error instanceof Error ? error.message : 'Failed to approve ICP transfer' 
      };
    }
  }

  /**
   * Check ICP allowance with retry on stale actor
   */
  static async checkIcpAllowance(spenderCanisterId: string): Promise<bigint> {
    const maxRetries = 2;
    
    for (let attempt = 0; attempt <= maxRetries; attempt++) {
      try {
        const walletState = get(walletStore);
        if (!walletState.principal) {
          return BigInt(0);
        }
        
        const icpActor = await walletStore.getActor(CONFIG.currentIcpLedgerId, CONFIG.icp_ledgerIDL) as IcpLedgerService;
        const result = await icpActor.icrc2_allowance({
          account: { 
            owner: walletState.principal, 
            subaccount: [] 
          },
          spender: { 
            owner: Principal.fromText(spenderCanisterId), 
            subaccount: [] 
          }
        });
        
        return result.allowance;
      } catch (err) {
        console.error(`Failed to check ICP allowance (attempt ${attempt + 1}/${maxRetries + 1}):`, err);
        
        // If this is a stale actor error and we have retries left, refresh wallet and retry
        if (isStaleActorError(err) && attempt < maxRetries) {
          console.log('Detected stale actor error, refreshing wallet and retrying...');
          try {
            await walletStore.refreshWallet();
            await new Promise(resolve => setTimeout(resolve, 500)); // Brief delay after refresh
          } catch (refreshErr) {
            console.warn('Wallet refresh failed:', refreshErr);
          }
          continue; // Retry
        }
        
        return BigInt(0);
      }
    }
    
    return BigInt(0);
  }

  /**
   * Check if user has sufficient ICP balance
   */
  static async checkSufficientBalance(amount: number): Promise<boolean> {
    try {
      const walletState = get(walletStore);
      
      if (!walletState.isConnected || !walletState.principal) {
        return false;
      }
      
      const balance = walletState.tokenBalances?.ICP?.raw 
        ? Number(walletState.tokenBalances.ICP.raw) / E8S 
        : 0;
      
      return balance >= amount;
    } catch (err) {
      console.error('Error checking balance:', err);
      return false;
    }
  }

  /**
   * Check icUSD allowance with retry on stale actor
   */
  static async checkIcusdAllowance(spenderCanisterId: string): Promise<bigint> {
    const maxRetries = 2;
    
    for (let attempt = 0; attempt <= maxRetries; attempt++) {
      try {
        const walletState = get(walletStore);
        if (!walletState.principal) {
          return BigInt(0);
        }
        
        // On retry, refresh wallet first
        if (attempt > 0) {
          console.log(`Retrying icUSD allowance check (attempt ${attempt + 1}/${maxRetries + 1})...`);
          try {
            await walletStore.refreshWallet();
            await new Promise(resolve => setTimeout(resolve, 500)); // Brief delay after refresh
          } catch (refreshErr) {
            console.warn('Wallet refresh failed during retry:', refreshErr);
          }
        }
        
        const icusdActor = await walletStore.getActor(CONFIG.currentIcusdLedgerId, CONFIG.icusd_ledgerIDL) as IcusdLedgerService;
        const result = await icusdActor.icrc2_allowance({
          account: { 
            owner: walletState.principal, 
            subaccount: [] 
          },
          spender: { 
            owner: Principal.fromText(spenderCanisterId), 
            subaccount: [] 
          }
        });
        
        return result.allowance;
      } catch (err) {
        console.error(`Failed to check icUSD allowance (attempt ${attempt + 1}/${maxRetries + 1}):`, err);
        
        // If this is a stale actor error and we have retries left, continue to retry
        if (isStaleActorError(err) && attempt < maxRetries) {
          console.log('Detected stale actor error for icUSD, will refresh and retry...');
          continue; // Retry (refresh happens at start of next iteration)
        }
        
        // On last attempt or non-stale error, return 0
        return BigInt(0);
      }
    }
    
    return BigInt(0);
  }
  
  /**
   * Approve icUSD transfer - now streamlined with retry on stale actor
   */
  static async approveIcusdTransfer(amount: bigint, spenderCanisterId: string): Promise<{success: boolean, error?: string}> {
    const maxRetries = 2;
    
    for (let attempt = 0; attempt <= maxRetries; attempt++) {
      try {
        // On retry, refresh wallet first
        if (attempt > 0) {
          console.log(`Retrying icUSD approval (attempt ${attempt + 1}/${maxRetries + 1})...`);
          try {
            await walletStore.refreshWallet();
            await new Promise(resolve => setTimeout(resolve, 500));
          } catch (refreshErr) {
            console.warn('Wallet refresh failed during retry:', refreshErr);
          }
        }

        console.log(`Approving ${amount.toString()} e8s icUSD for ${spenderCanisterId}`);
        
        const icusdActor = await walletStore.getActor(CONFIG.currentIcusdLedgerId, CONFIG.icusd_ledgerIDL) as IcusdLedgerService;
        
        const approvalResult = await icusdActor.icrc2_approve({
          amount,
          spender: { 
            owner: Principal.fromText(spenderCanisterId),
            subaccount: [] 
          },
          expires_at: [], 
          expected_allowance: [],
          memo: [],
          fee: [],
          from_subaccount: [],
          created_at_time: []
        });
        
        if ('Ok' in approvalResult) {
          console.log('icUSD approval successful');
          return { success: true };
        } else {
          return { 
            success: false, 
            error: `icUSD approval failed: ${String(approvalResult.Err && typeof approvalResult.Err === 'object' ? Object.keys(approvalResult.Err)[0] : approvalResult.Err)}` 
          };
        }
      } catch (error) {
        console.error(`icUSD approval failed (attempt ${attempt + 1}/${maxRetries + 1}):`, error);
        
        // If this is a stale actor error and we have retries left, continue to retry
        if (isStaleActorError(error) && attempt < maxRetries) {
          console.log('Detected stale actor error during icUSD approval, will refresh and retry...');
          continue;
        }
        
        return { 
          success: false, 
          error: error instanceof Error ? error.message : 'Failed to approve icUSD transfer' 
        };
      }
    }
    
    return { 
      success: false, 
      error: 'Failed to approve icUSD transfer after multiple attempts' 
    };
  }

  /**
   * Approve ckUSDT or ckUSDC transfer for stable token repayments.
   * Uses the same ICRC-2 interface as icUSD since all are ICRC-2 compliant ledgers.
   */
  static async approveStableTransfer(
    amount: bigint,
    spenderCanisterId: string,
    tokenType: 'CKUSDT' | 'CKUSDC'
  ): Promise<{success: boolean, error?: string}> {
    const maxRetries = 2;
    const ledgerId = CONFIG.getStableLedgerId(tokenType);

    for (let attempt = 0; attempt <= maxRetries; attempt++) {
      try {
        if (attempt > 0) {
          console.log(`Retrying ${tokenType} approval (attempt ${attempt + 1}/${maxRetries + 1})...`);
          try {
            await walletStore.refreshWallet();
            await new Promise(resolve => setTimeout(resolve, 500));
          } catch (refreshErr) {
            console.warn('Wallet refresh failed during retry:', refreshErr);
          }
        }

        console.log(`Approving ${amount.toString()} e6s ${tokenType} for ${spenderCanisterId}`);

        // Use icUSD ledger IDL — all ICRC-2 ledgers share the same interface
        const stableActor = await walletStore.getActor(ledgerId, CONFIG.icusd_ledgerIDL) as IcusdLedgerService;

        const approvalResult = await stableActor.icrc2_approve({
          amount,
          spender: {
            owner: Principal.fromText(spenderCanisterId),
            subaccount: []
          },
          expires_at: [],
          expected_allowance: [],
          memo: [],
          fee: [],
          from_subaccount: [],
          created_at_time: []
        });

        if ('Ok' in approvalResult) {
          console.log(`${tokenType} approval successful`);
          return { success: true };
        } else {
          return {
            success: false,
            error: `${tokenType} approval failed: ${String(approvalResult.Err && typeof approvalResult.Err === 'object' ? Object.keys(approvalResult.Err)[0] : approvalResult.Err)}`
          };
        }
      } catch (error) {
        console.error(`${tokenType} approval failed (attempt ${attempt + 1}/${maxRetries + 1}):`, error);

        if (isStaleActorError(error) && attempt < maxRetries) {
          console.log(`Detected stale actor error during ${tokenType} approval, will refresh and retry...`);
          continue;
        }

        return {
          success: false,
          error: error instanceof Error ? error.message : `Failed to approve ${tokenType} transfer`
        };
      }
    }

    return {
      success: false,
      error: `Failed to approve ${tokenType} transfer after multiple attempts`
    };
  }

  /**
   * Check stable token allowance (ckUSDT or ckUSDC)
   */
  static async checkStableAllowance(
    spenderCanisterId: string,
    tokenType: 'CKUSDT' | 'CKUSDC'
  ): Promise<bigint> {
    const maxRetries = 2;
    const ledgerId = CONFIG.getStableLedgerId(tokenType);

    for (let attempt = 0; attempt <= maxRetries; attempt++) {
      try {
        if (attempt > 0) {
          await walletStore.refreshWallet();
          await new Promise(resolve => setTimeout(resolve, 500));
        }

        const stableActor = await walletStore.getActor(ledgerId, CONFIG.icusd_ledgerIDL) as IcusdLedgerService;

        const walletState = get(walletStore);
        if (!walletState.principal) {
          return BigInt(0);
        }

        const allowance = await stableActor.icrc2_allowance({
          account: {
            owner: walletState.principal,
            subaccount: []
          },
          spender: {
            owner: Principal.fromText(spenderCanisterId),
            subaccount: []
          }
        });

        return allowance.allowance;
      } catch (error) {
        console.error(`${tokenType} allowance check failed (attempt ${attempt + 1}):`, error);

        if (isStaleActorError(error) && attempt < maxRetries) {
          continue;
        }

        return BigInt(0);
      }
    }

    return BigInt(0);
  }

  // ── Generic collateral approve/allowance (multi-collateral) ──────────

  /**
   * Approve a transfer of any ICRC-2 collateral token.
   * If the ledger is ICP, delegates to approveIcpTransfer.
   * Otherwise uses the generic ICRC-2 pattern.
   */
  static async approveCollateralTransfer(
    amount: bigint,
    spenderCanisterId: string,
    ledgerCanisterId: string
  ): Promise<{success: boolean, error?: string}> {
    // Delegate to ICP-specific method if this is the ICP ledger
    if (ledgerCanisterId === CONFIG.currentIcpLedgerId) {
      return walletOperations.approveIcpTransfer(amount, spenderCanisterId);
    }

    const maxRetries = 2;
    for (let attempt = 0; attempt <= maxRetries; attempt++) {
      try {
        if (attempt > 0) {
          console.log(`Retrying collateral approval (attempt ${attempt + 1}/${maxRetries + 1})...`);
          try {
            await walletStore.refreshWallet();
            await new Promise(resolve => setTimeout(resolve, 500));
          } catch (refreshErr) {
            console.warn('Wallet refresh failed during retry:', refreshErr);
          }
        }

        console.log(`Approving ${amount.toString()} for spender ${spenderCanisterId} on ledger ${ledgerCanisterId}`);

        // Use icUSD IDL — all ICRC-2 ledgers share the same interface
        const ledgerActor = await walletStore.getActor(ledgerCanisterId, CONFIG.icusd_ledgerIDL) as IcusdLedgerService;

        const approvalResult = await ledgerActor.icrc2_approve({
          amount,
          spender: {
            owner: Principal.fromText(spenderCanisterId),
            subaccount: []
          },
          expires_at: [],
          expected_allowance: [],
          memo: [],
          fee: [],
          from_subaccount: [],
          created_at_time: []
        });

        if ('Ok' in approvalResult) {
          console.log('Collateral approval successful');
          return { success: true };
        } else {
          return {
            success: false,
            error: `Collateral approval failed: ${String(approvalResult.Err && typeof approvalResult.Err === 'object' ? Object.keys(approvalResult.Err)[0] : approvalResult.Err)}`
          };
        }
      } catch (error) {
        console.error(`Collateral approval failed (attempt ${attempt + 1}/${maxRetries + 1}):`, error);

        if (isStaleActorError(error) && attempt < maxRetries) {
          continue;
        }

        return {
          success: false,
          error: error instanceof Error ? error.message : 'Failed to approve collateral transfer'
        };
      }
    }

    return {
      success: false,
      error: 'Failed to approve collateral transfer after multiple attempts'
    };
  }

  /**
   * Check the ICRC-2 allowance for any collateral token.
   * If the ledger is ICP, delegates to checkIcpAllowance.
   */
  static async checkCollateralAllowance(
    spenderCanisterId: string,
    ledgerCanisterId: string
  ): Promise<bigint> {
    // Delegate to ICP-specific method if this is the ICP ledger
    if (ledgerCanisterId === CONFIG.currentIcpLedgerId) {
      return walletOperations.checkIcpAllowance(spenderCanisterId);
    }

    const maxRetries = 2;
    for (let attempt = 0; attempt <= maxRetries; attempt++) {
      try {
        if (attempt > 0) {
          await walletStore.refreshWallet();
          await new Promise(resolve => setTimeout(resolve, 500));
        }

        const walletState = get(walletStore);
        if (!walletState.principal) {
          return BigInt(0);
        }

        const ledgerActor = await walletStore.getActor(ledgerCanisterId, CONFIG.icusd_ledgerIDL) as IcusdLedgerService;

        const result = await ledgerActor.icrc2_allowance({
          account: {
            owner: walletState.principal,
            subaccount: []
          },
          spender: {
            owner: Principal.fromText(spenderCanisterId),
            subaccount: []
          }
        });

        return result.allowance;
      } catch (err) {
        console.error(`Collateral allowance check failed (attempt ${attempt + 1}):`, err);

        if (isStaleActorError(err) && attempt < maxRetries) {
          continue;
        }

        return BigInt(0);
      }
    }

    return BigInt(0);
  }

  /**
   * Get current icUSD balance with retry on stale actor
   */
  static async getIcusdBalance(): Promise<number> {
    const maxRetries = 2;
    
    for (let attempt = 0; attempt <= maxRetries; attempt++) {
      try {
        const walletState = get(walletStore);
        
        if (!walletState.isConnected || !walletState.principal) {
          return 0;
        }
        
        if (walletState.tokenBalances?.ICUSD?.raw) {
          return Number(walletState.tokenBalances.ICUSD.raw) / E8S;
        }
        
        // On retry, refresh wallet first
        if (attempt > 0) {
          console.log(`Retrying icUSD balance check (attempt ${attempt + 1}/${maxRetries + 1})...`);
          try {
            await walletStore.refreshWallet();
            await new Promise(resolve => setTimeout(resolve, 500));
          } catch (refreshErr) {
            console.warn('Wallet refresh failed during retry:', refreshErr);
          }
        }
        
        const icusdActor = await walletStore.getActor(CONFIG.currentIcusdLedgerId, CONFIG.icusd_ledgerIDL) as IcusdLedgerService;
        const balance = await icusdActor.icrc1_balance_of({
          owner: walletState.principal,
          subaccount: []
        });
        
        return Number(balance) / E8S;
      } catch (err) {
        console.error(`Error getting icUSD balance (attempt ${attempt + 1}/${maxRetries + 1}):`, err);
        
        // If this is a stale actor error and we have retries left, continue to retry
        if (isStaleActorError(err) && attempt < maxRetries) {
          continue;
        }
        
        return 0;
      }
    }
    
    return 0;
  }

  /**
   * Get both ICP and icUSD balances with retry on stale actor
   */
  static async getUserBalances(): Promise<UserBalances> {
    const maxRetries = 2;
    
    for (let attempt = 0; attempt <= maxRetries; attempt++) {
      try {
        const walletState = get(walletStore);
        
        if (!walletState.isConnected || !walletState.principal) {
          return { icp: 0, icusd: 0 };
        }
        
        let icpBalance = walletState.tokenBalances?.ICP?.raw 
          ? Number(walletState.tokenBalances.ICP.raw) / E8S 
          : 0;
          
        let icusdBalance = walletState.tokenBalances?.ICUSD?.raw
          ? Number(walletState.tokenBalances.ICUSD.raw) / E8S
          : 0;
        
        // On retry, refresh wallet first
        if (attempt > 0) {
          console.log(`Retrying balance fetch (attempt ${attempt + 1}/${maxRetries + 1})...`);
          try {
            await walletStore.refreshWallet();
            await new Promise(resolve => setTimeout(resolve, 500));
          } catch (refreshErr) {
            console.warn('Wallet refresh failed during retry:', refreshErr);
          }
        }
        
        // Fetch from ledger if not available in tokenBalances
        if (icpBalance === 0) {
          try {
            const icpActor = await walletStore.getActor(CONFIG.currentIcpLedgerId, CONFIG.icp_ledgerIDL) as IcpLedgerService;
            const balance = await icpActor.icrc1_balance_of({
              owner: walletState.principal,
              subaccount: []
            });
            icpBalance = Number(balance) / E8S;
          } catch (err) {
            console.warn('Failed to fetch ICP balance:', err);
            if (isStaleActorError(err) && attempt < maxRetries) {
              throw err; // Trigger retry
            }
          }
        }
        
        if (icusdBalance === 0) {
          try {
            const icusdActor = await walletStore.getActor(CONFIG.currentIcusdLedgerId, CONFIG.icusd_ledgerIDL) as IcusdLedgerService;
            const balance = await icusdActor.icrc1_balance_of({
              owner: walletState.principal,
              subaccount: []
            });
            icusdBalance = Number(balance) / E8S;
          } catch (err) {
            console.warn('Failed to fetch icUSD balance:', err);
            if (isStaleActorError(err) && attempt < maxRetries) {
              throw err; // Trigger retry
            }
          }
        }
        
        return {
          icp: icpBalance,
          icusd: icusdBalance
        };
      } catch (err) {
        console.error(`Error getting user balances (attempt ${attempt + 1}/${maxRetries + 1}):`, err);
        
        // If this is a stale actor error and we have retries left, continue to retry
        if (isStaleActorError(err) && attempt < maxRetries) {
          continue;
        }
        
        return { icp: 0, icusd: 0 };
      }
    }
    
    return { icp: 0, icusd: 0 };
  }
}
