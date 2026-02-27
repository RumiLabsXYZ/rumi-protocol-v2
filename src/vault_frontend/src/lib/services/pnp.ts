import { Principal } from "@dfinity/principal";
import { CONFIG, CANISTER_IDS, LOCAL_CANISTER_IDS, vault_frontend } from '../config';
import { idlFactory as rumi_backendIDL } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import { idlFactory as icp_ledgerIDL } from '$declarations/icp_ledger/icp_ledger.did.js';
import { idlFactory as icusd_ledgerIDL } from '$declarations/icusd_ledger/icusd_ledger.did.js';
import { idlFactory as stabilityPoolIDL } from '$declarations/rumi_stability_pool/rumi_stability_pool.did.js';
import { createPNP, type PNP, ConfigBuilder, BaseSignerAdapter } from '@windoge98/plug-n-play';

// Define types for supported canisters
export type CanisterType =
  | "rumi_backend"
  | "icp_ledger"
  | "icusd_ledger"
  | "stability_pool";

// Collect all canister IDLs in one place
export const canisterIDLs = {
  rumi_backend: rumi_backendIDL,
  icp_ledger: icp_ledgerIDL,
  icusd_ledger: icusd_ledgerIDL,
  stability_pool: stabilityPoolIDL,
};

let globalPnp: PNP | null = null;

export const REQUIRED_CANISTERS = {
  protocol: CONFIG.currentCanisterId,
  icpLedger: CONFIG.currentIcpLedgerId,
  icusdLedger: CONFIG.currentIcusdLedgerId,
  stabilityPool: CANISTER_IDS.STABILITY_POOL
};

// All delegation targets for comprehensive permissions
const getAllDelegationTargets = (): string[] => {
  return [
    CONFIG.currentCanisterId,      // Protocol canister
    CONFIG.currentIcpLedgerId,     // ICP Ledger  
    CONFIG.currentIcusdLedgerId,   // icUSD Ledger
    CANISTER_IDS.STABILITY_POOL    // Stability Pool canister
  ].filter(Boolean); // Filter out any undefined values
};

export async function silentPlugReconnect(): Promise<{owner: Principal} | null> {
  try {
    if (!window.ic?.plug) {
      return null;
    }

    const isConnected = await window.ic.plug.isConnected();
    if (!isConnected) {
      return null;
    }

    // Session exists - try to get the principal from existing agent
    if (window.ic.plug.agent) {
      try {
        const principal = await window.ic.plug.agent.getPrincipal();
        return { owner: principal };
      } catch (err) {
        console.warn('Failed to get principal from existing Plug agent:', err);
      }
    }

    // If agent doesn't exist or getPrincipal failed, try createAgent silently
    // This reconnects using stored credentials without showing a popup
    try {
      const targets = getAllDelegationTargets();
      await (window.ic.plug as any).createAgent({
        whitelist: targets,
        host: CONFIG.isLocal ? 'http://localhost:4943' : 'https://icp0.io'
      });

      const principal = await window.ic.plug.agent.getPrincipal();
      return { owner: principal };
    } catch (err) {
      // Silent reconnect failed - user will need to reconnect manually
      return null;
    }
  } catch (err) {
    console.error('Silent Plug reconnect failed:', err);
    return null;
  }
}

// COMPREHENSIVE PERMISSION SYSTEM - Request ALL permissions at once during wallet connection
// Custom connect function that ensures comprehensive permissions for Plug
export async function connectWithComprehensivePermissions(walletId: string): Promise<any> {
  try {
    console.log('🔐 Connecting with comprehensive permissions for:', walletId);
    
    // For Plug wallet, manually handle the comprehensive permission request
    if (walletId === 'plug' && window.ic?.plug) {
      console.log('🔧 Setting up comprehensive Plug permissions...');
      
      // Request ALL canister permissions upfront for Plug
      const comprehensivePermissions = {
        whitelist: getAllDelegationTargets(),
        host: CONFIG.isLocal ? 'http://localhost:4943' : 'https://icp0.io',
        timeout: 60000
      };
      
      console.log('🔐 Requesting comprehensive Plug permissions:', comprehensivePermissions);
      
      // Use Plug's requestConnect with comprehensive whitelist
      const plugConnected = await window.ic.plug.requestConnect(comprehensivePermissions);
      if (!plugConnected) {
        throw new Error('Plug wallet connection denied');
      }
      
      // Get the account info from Plug directly
      const principal = await window.ic.plug.agent.getPrincipal();
      console.log('✅ Plug connected with comprehensive permissions:', principal.toText());
      
      return { owner: principal };
    }
    
    // For Oisy and other wallets, use standard PNP connection with beta API
    // The beta properly supports ICRC-25/ICRC-21 standards
    console.log('🔧 Using PNP beta connection for:', walletId);
    const result = await globalPnp?.connect(walletId);
    console.log('✅ PNP connection result:', result);
    
    // Normalize the owner principal - OISY and other wallets may return different formats
    // The owner might be: a string, a Principal object, or an object with toString()/toText()
    if (result?.owner) {
      let normalizedPrincipal: Principal;

      // Cast to any for duck-typing since PNP may return various Principal-like types
      const ownerRaw: any = result.owner;

      if (typeof ownerRaw === 'string') {
        // Owner is already a string, convert to Principal
        normalizedPrincipal = Principal.fromText(ownerRaw);
      } else if (ownerRaw instanceof Principal) {
        // Already a proper Principal object
        normalizedPrincipal = ownerRaw;
      } else if (typeof ownerRaw?.toText === 'function') {
        // Has toText method (standard Principal interface)
        normalizedPrincipal = Principal.fromText(ownerRaw.toText());
      } else if (typeof ownerRaw?.toString === 'function') {
        // Fallback to toString
        normalizedPrincipal = Principal.fromText(ownerRaw.toString());
      } else {
        throw new Error('Unable to extract principal from wallet connection result');
      }
      
      console.log('✅ Normalized principal:', normalizedPrincipal.toText());
      return { ...result, owner: normalizedPrincipal };
    }
    
    return result;
  } catch (err) {
    console.error('❌ Failed to connect with comprehensive permissions:', err);
    throw err;
  }
}

export async function requestAllPermissionsUpfront(walletId?: string): Promise<boolean> {
  // This function is kept for backward compatibility but is no longer used
  // Comprehensive permissions are now handled in connectWithComprehensivePermissions
  console.log('✅ Permission handling moved to connectWithComprehensivePermissions');
  return true;
}

export function initializePNP(): PNP {
  try {
    if (globalPnp) {
      return globalPnp;
    }

    const delegationTargets = getAllDelegationTargets();
    const network = CONFIG.isLocal ? 'local' : 'ic';

    console.log('🔧 Initializing PNP beta with configuration...');
    console.log('🔍 Network:', network);
    console.log('🔍 Delegation targets:', delegationTargets);

    // Create PNP with beta API configuration using ConfigBuilder
    globalPnp = createPNP(
      ConfigBuilder.create()
        .withEnvironment(network, CONFIG.isLocal ? { replica: 4943, frontend: 5173 } : undefined)
        .withDelegation({
          timeout: BigInt(86400000000000), // 24 hours in nanoseconds
          targets: delegationTargets
        })
        .withSecurity(CONFIG.isLocal, true) // fetchRootKey for local, verify signatures
        .withIcAdapters({
          // Enable all IC wallets including Oisy
          ii: { enabled: true },
          plug: { 
            enabled: true,
            whitelist: delegationTargets // Plug-specific whitelist
          },
          oisy: { enabled: true }, // Oisy with ICRC-25/ICRC-21 support
          nfid: { enabled: true },
          stoic: { enabled: true }
        })
        .build()
    );

    console.log('✅ PNP beta initialized with comprehensive permissions');
    console.log('✅ Enabled wallets:', globalPnp.getEnabledWallets().map(w => w.id));
    return globalPnp;
  } catch (error) {
    console.error("Error initializing PNP:", error);
    throw error;
  }
}

export function getPnpInstance(): PNP {
  if (!globalPnp) {
    return initializePNP();
  }
  return globalPnp;
}

// Enhanced PNP wrapper that uses Plug directly when connected
export const pnp = {
  // Spread the PNP instance methods
  get config() { return getPnpInstance().config; },
  get adapter() { return getPnpInstance().adapter; },
  get provider() { return getPnpInstance().provider; },
  get account() { return getPnpInstance().account; },
  get status() { return getPnpInstance().status; },
  
  // Override connect to use our comprehensive method
  connect: connectWithComprehensivePermissions,
  
  // Disconnect method
  async disconnect(): Promise<void> {
    // Disconnect from Plug if connected
    if (window.ic?.plug) {
      try {
        await window.ic.plug.disconnect();
      } catch (e) {
        console.warn('Plug disconnect warning:', e);
      }
    }
    // Also disconnect from PNP
    await globalPnp?.disconnect();
  },
  
  // Check if authenticated (sync check)
  isAuthenticated(): boolean {
    // Check if Plug wallet exists and has an agent (indicates connected state)
    if (window.ic?.plug?.agent) {
      return true;
    }
    // Fall back to PNP's isAuthenticated
    if (globalPnp) {
      return globalPnp.isAuthenticated();
    }
    return false;
  },

  // Legacy isConnected method for backward compatibility
  isConnected(): boolean {
    return this.isAuthenticated();
  },

  // Async version for when we need to verify with Plug directly
  async isConnectedAsync(): Promise<boolean> {
    if (window.ic?.plug) {
      return await window.ic.plug.isConnected();
    }
    if (globalPnp) {
      return globalPnp.isAuthenticated();
    }
    return false;
  },
  
  // Override getActor to use Plug directly when possible and beta API format
  async getActor(canisterId: string, idl: any) {
    try {
      // For Plug wallet, use the direct Plug API to avoid permission prompts
      if (window.ic?.plug && await window.ic.plug.isConnected()) {
        console.log('🔧 Using Plug direct API for actor:', canisterId);
        return await window.ic.plug.createActor({
          canisterId,
          interfaceFactory: idl
        });
      }
      
      // For other wallets (including Oisy), use PNP beta API with options object
      console.log('🔧 Using PNP beta getActor for:', canisterId);
      return globalPnp?.getActor({ canisterId, idl });
    } catch (err) {
      console.error('Error getting actor for canister', canisterId, err);
      throw err;
    }
  },

  // New method: Get ICRC actor for token operations (beta feature)
  getIcrcActor(canisterId: string, options?: { anon?: boolean; requiresSigning?: boolean }) {
    return globalPnp?.getIcrcActor(canisterId, options);
  },

  // Get enabled wallets list
  getEnabledWallets() {
    return globalPnp?.getEnabledWallets() || [];
  },

  // Open channel for Safari compatibility (beta feature)
  async openChannel(): Promise<void> {
    await globalPnp?.openChannel();
  },

  /**
   * Get the SignerAgent from the current PNP adapter (Oisy/NFID).
   * Returns null for non-signer wallets (Plug, II) or when not connected.
   * Used for ICRC-112 batching: approve + action in a single signer popup.
   *
   * The SignerAgent exposes batch()/execute()/clear() for ICRC-112 batched calls.
   * Sequential batching: batch() → queue call A → batch() → queue call B → execute()
   * sends a single signer popup with ordered sequences.
   */
  getSignerAgent(): any {
    try {
      const provider = globalPnp?.provider;
      if (provider && 'getSignerAgent' in provider) {
        return (provider as any).getSignerAgent();
      }
      return null;
    } catch {
      return null;
    }
  }
};
