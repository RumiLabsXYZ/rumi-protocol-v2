import { writable, get } from 'svelte/store';
import { browser } from '$app/environment';
import { pnp, connectWithComprehensivePermissions, getPnpInstance, silentPlugReconnect } from './pnp';
import { TokenService } from './tokenService';
import { CONFIG, CANISTER_IDS, LOCAL_CANISTER_IDS } from '../config';
import { canisterIDLs } from './pnp';
import { permissionManager } from './PermissionManager';
import { Principal } from '@dfinity/principal';
import { AuthClient } from '@dfinity/auth-client';
import { HttpAgent } from '@dfinity/agent';

// Storage keys for persistence
const STORAGE_KEYS = {
  LAST_WALLET: "rumi_last_wallet",
  AUTO_CONNECT_ATTEMPTED: "rumi_auto_connect_attempted",
  WAS_CONNECTED: "rumi_was_connected"
} as const;

// Wallet types
export const WALLET_TYPES = {
  PLUG: 'plug',
  INTERNET_IDENTITY: 'internet-identity',
  OISY: 'oisy'
} as const;

export type WalletType = typeof WALLET_TYPES[keyof typeof WALLET_TYPES];

// Create initial stores
export const selectedWalletId = writable<string | null>(null);
export const connectionError = writable<string | null>(null);
export const currentWalletType = writable<WalletType | null>(null);

// Type definition for auth state
interface AuthState {
  isConnected: boolean;
  account: {
    owner: Principal;
    balance: bigint;
    [key: string]: any;
  } | null;
  isInitialized: boolean;
  walletType: WalletType | null;
}

function createAuthStore() {
  const store = writable<AuthState>({
    isConnected: false,
    account: null,
    isInitialized: false,
    walletType: null
  });

  const { subscribe, set } = store;

  // Internet Identity auth client
  let authClient: AuthClient | null = null;
  let agent: HttpAgent | null = null;

  // Storage helper with type safety
  const storage = {
    get: (key: keyof typeof STORAGE_KEYS): string | null => 
      browser ? localStorage.getItem(STORAGE_KEYS[key]) : null,
    set: (key: keyof typeof STORAGE_KEYS, value: string): void => {
      if (browser) localStorage.setItem(STORAGE_KEYS[key], value);
    },
    clear: (): void => {
      if (browser) {
        Object.values(STORAGE_KEYS).forEach(k => localStorage.removeItem(k));
        permissionManager.clearCache();
      }
    }
  };

  // Fetch wallet balance with better error handling
  const refreshWalletBalance = async (principal: Principal): Promise<bigint> => {
    try {
      const icpLedgerId = CONFIG.isLocal ? LOCAL_CANISTER_IDS.ICP_LEDGER : CANISTER_IDS.ICP_LEDGER;
      const balance = await TokenService.getTokenBalance(icpLedgerId, principal);
      console.log('Auth balance refresh:', balance.toString());
      return balance;
    } catch (error) {
      console.error('Auth balance refresh failed:', error);
      throw error;
    }
  };

  return {
    subscribe,
    pnp,
    refreshBalance: refreshWalletBalance,

    // Initialize Internet Identity auth client
    async initAuthClient(): Promise<AuthClient> {
      if (!authClient) {
        authClient = await AuthClient.create({
          idleOptions: {
            disableIdle: true
          }
        });
      }
      return authClient;
    },

    async initialize(): Promise<void> {
      if (!browser) return;

      // Clear the auto-connect flag on each page load to allow fresh reconnect attempts
      sessionStorage.removeItem(STORAGE_KEYS.AUTO_CONNECT_ATTEMPTED);

      const lastWallet = storage.get("LAST_WALLET");
      const wasConnected = storage.get("WAS_CONNECTED");

      if (!lastWallet || !wasConnected) return;

      console.log('🔄 Attempting to restore session for:', lastWallet);

      try {
        if (lastWallet === WALLET_TYPES.INTERNET_IDENTITY) {
          // For II, check if already authenticated
          const client = await this.initAuthClient();
          const isAuthenticated = await client.isAuthenticated();
          
          if (isAuthenticated) {
            console.log('✅ II session still valid, restoring...');
            const identity = client.getIdentity();
            const principal = identity.getPrincipal();
            
            // Create agent for Internet Identity
            agent = new HttpAgent({
              identity: identity as any,
              host: CONFIG.isLocal ? 'http://localhost:4943' : 'https://ic0.app'
            });
            
            if (CONFIG.isLocal) {
              await agent.fetchRootKey();
            }

            const principalText = principal.toString();
            const convertedPrincipal = Principal.fromText(principalText);
            const balance = await refreshWalletBalance(convertedPrincipal);

            set({
              isConnected: true,
              account: { owner: convertedPrincipal, balance },
              isInitialized: true,
              walletType: WALLET_TYPES.INTERNET_IDENTITY
            });

            selectedWalletId.set(lastWallet);
            currentWalletType.set(WALLET_TYPES.INTERNET_IDENTITY);
            console.log('🎉 II session restored successfully');
          } else {
            console.log('⚠️ II session expired, clearing storage');
            storage.clear();
          }
        } else if (lastWallet === WALLET_TYPES.PLUG) {
          // For Plug, use silent reconnect function
          console.log('🔄 Attempting Plug session restore...');

          // Wait for Plug wallet extension to be ready (it may load after page script)
          const waitForPlug = async (maxAttempts = 20, interval = 100): Promise<boolean> => {
            for (let i = 0; i < maxAttempts; i++) {
              if (window.ic?.plug) {
                console.log(`✅ Plug wallet detected after ${i * interval}ms`);
                return true;
              }
              await new Promise(resolve => setTimeout(resolve, interval));
            }
            return false;
          };

          const plugAvailable = await waitForPlug();

          if (!plugAvailable) {
            console.log('⚠️ Plug wallet not available after waiting');
            storage.clear();
            return;
          }

          // Use the silentPlugReconnect function for clean session restore
          const result = await silentPlugReconnect();

          if (result?.owner) {
            console.log('✅ Plug session restored:', result.owner.toText());

            let balance = BigInt(0);
            try {
              balance = await refreshWalletBalance(result.owner);
            } catch (e) {
              console.warn('Balance fetch failed during restore:', e);
            }

            set({
              isConnected: true,
              account: { owner: result.owner, balance },
              isInitialized: true,
              walletType: WALLET_TYPES.PLUG
            });

            selectedWalletId.set(lastWallet);
            currentWalletType.set(WALLET_TYPES.PLUG);
            console.log('🎉 Plug session restored successfully');
            return;
          }

          // If silent reconnect fails, don't trigger popup - just clear and let user reconnect manually
          console.log('⚠️ Could not restore Plug session silently, user must reconnect');
          storage.clear();
        } else if (lastWallet === WALLET_TYPES.OISY || lastWallet.toLowerCase().includes('oisy')) {
          // For Oisy and other PNP wallets, try to restore session via PNP library
          console.log('🔄 Attempting Oisy/PNP session restore...');
          
          try {
            // Check if PNP has an active connection
            const pnpInstance = getPnpInstance();
            
            if (pnpInstance && typeof pnpInstance.isConnected === 'function' && await pnpInstance.isConnected()) {
              console.log('✅ PNP session still active, checking principal...');
              
              // Try to get the principal from PNP account
              const account = pnpInstance.account;
              
              if (account?.owner) {
                const convertedPrincipal = Principal.fromText(account.owner.toString());
                const balance = await refreshWalletBalance(convertedPrincipal);
                
                set({
                  isConnected: true,
                  account: { owner: convertedPrincipal, balance },
                  isInitialized: true,
                  walletType: WALLET_TYPES.OISY
                });
                
                selectedWalletId.set(lastWallet);
                currentWalletType.set(WALLET_TYPES.OISY);
                console.log('🎉 Oisy session restored successfully');
                return;
              }
            }
            
            // If PNP session not active, clear storage
            console.log('⚠️ Oisy session not found, user must reconnect');
            storage.clear();
          } catch (oisyError) {
            console.log('⚠️ Oisy session restore failed:', oisyError);
            storage.clear();
          }
        } else {
          // Unknown wallet type, clear storage
          console.log('⚠️ Unknown wallet type:', lastWallet);
          storage.clear();
        }
      } catch (error) {
        console.warn("Session restore failed:", error);
        storage.clear();
        connectionError.set(error instanceof Error ? error.message : String(error));
      } finally {
        sessionStorage.setItem(STORAGE_KEYS.AUTO_CONNECT_ATTEMPTED, "true");
      }
    },

    async connect(walletId: string): Promise<{owner: Principal} | null> {
      try {
        connectionError.set(null);
        
        if (walletId === WALLET_TYPES.INTERNET_IDENTITY) {
          return await this.connectInternetIdentity();
        } else if (walletId === WALLET_TYPES.PLUG) {
          return await this.connectPlug(walletId);
        } else {
          // For Oisy and other PNP wallets
          return await this.connectPNPWallet(walletId);
        }
      } catch (error) {
        this.handleConnectionError(error);
        throw error;
      }
    },

    async connectPNPWallet(walletId: string): Promise<{owner: Principal} | null> {
      // Use standard PNP connection for Oisy and other wallets
      console.log('🔗 Connecting PNP wallet:', walletId);
      
      const result = await connectWithComprehensivePermissions(walletId);
      
      if (!result?.owner) {
        throw new Error("Invalid connection result");
      }

      // Get initial balance after connection
      const balance = await refreshWalletBalance(result.owner);
      console.log('Initial balance:', balance.toString());

      // Determine wallet type for storage
      const walletType = walletId.toLowerCase().includes('oisy') ? WALLET_TYPES.OISY : walletId;

      set({ 
        isConnected: true, 
        account: {
          ...result,
          balance
        }, 
        isInitialized: true,
        walletType: walletType as WalletType
      });

      // Update storage
      selectedWalletId.set(walletId);
      currentWalletType.set(walletType as WalletType);
      storage.set("LAST_WALLET", walletId);
      storage.set("WAS_CONNECTED", "true");

      console.log(`🎉 ${walletId} wallet connected successfully`);
      return result;
    },

    async connectPlug(walletId: string): Promise<{owner: Principal} | null> {
      // Use custom connect function that ensures comprehensive permissions
      console.log('🔗 Connecting Plug wallet with comprehensive permissions...');
      
      const result = await connectWithComprehensivePermissions(walletId);
      
      if (!result?.owner) {
        throw new Error("Invalid connection result");
      }

      // Get initial balance after connection
      const balance = await refreshWalletBalance(result.owner);
      console.log('Initial balance:', balance.toString());

      set({ 
        isConnected: true, 
        account: {
          ...result,
          balance
        }, 
        isInitialized: true,
        walletType: WALLET_TYPES.PLUG
      });

      // Update storage
      selectedWalletId.set(walletId);
      currentWalletType.set(WALLET_TYPES.PLUG);
      storage.set("LAST_WALLET", walletId);
      storage.set("WAS_CONNECTED", "true");

      console.log('🎉 Plug wallet connected with comprehensive permissions');
      return result;
    },

    async connectInternetIdentity(): Promise<{owner: Principal} | null> {
      console.log('🔗 Connecting Internet Identity...');
      
      const client = await this.initAuthClient();
      
      return new Promise((resolve, reject) => {
        client.login({
          identityProvider: CONFIG.isLocal 
            ? `http://localhost:4943/?canisterId=${LOCAL_CANISTER_IDS.INTERNET_IDENTITY}` 
            : "https://id.ai",
          onSuccess: async () => {
            try {
              const identity = client.getIdentity();
              const principal = identity.getPrincipal();
              
              // Create agent for Internet Identity
              agent = new HttpAgent({
                // Cast to any to avoid type incompatibility when multiple copies of @dfinity packages exist
                identity: identity as any,
                host: CONFIG.isLocal ? 'http://localhost:4943' : 'https://ic0.app'
              });
              
              if (CONFIG.isLocal) {
                await agent.fetchRootKey();
              }

              // Convert principal to the correct type by recreating it
              const principalText = principal.toString();
              const convertedPrincipal = Principal.fromText(principalText);

              // Get initial balance
              const balance = await refreshWalletBalance(convertedPrincipal);
              console.log('II Initial balance:', balance.toString());

              const result = { owner: convertedPrincipal };

              set({
                isConnected: true,
                account: {
                  ...result,
                  balance
                },
                isInitialized: true,
                walletType: WALLET_TYPES.INTERNET_IDENTITY
              });

              // Update storage
              selectedWalletId.set(WALLET_TYPES.INTERNET_IDENTITY);
              currentWalletType.set(WALLET_TYPES.INTERNET_IDENTITY);
              storage.set("LAST_WALLET", WALLET_TYPES.INTERNET_IDENTITY);
              storage.set("WAS_CONNECTED", "true");

              console.log('🎉 Internet Identity connected successfully');
              resolve(result);
            } catch (error) {
              console.error('Internet Identity connection error:', error);
              reject(error);
            }
          },
          onError: (error) => {
            console.error('Internet Identity login error:', error);
            reject(new Error('Internet Identity login failed'));
          }
        });
      });
    },

    async disconnect(): Promise<void> {
      const state = get(store);
      
      if (state.walletType === WALLET_TYPES.INTERNET_IDENTITY && authClient) {
        await authClient.logout();
      } else if (state.walletType === WALLET_TYPES.PLUG) {
        // Clear permission cache on disconnect
        permissionManager.clearCache();
        // Disconnect Plug wallet if available
        if (window.ic?.plug) {
          try {
            await window.ic.plug.disconnect();
          } catch (error) {
            console.warn('Plug disconnect failed, but continuing cleanup:', error);
          }
        }
      }
      
      set({ 
        isConnected: false, 
        account: null, 
        isInitialized: true,
        walletType: null
      });
      selectedWalletId.set(null);
      currentWalletType.set(null);
      connectionError.set(null);
      storage.clear();
    },

    handleConnectionError(error: any): void {
      console.error("Connection error:", error);
      set({ 
        isConnected: false, 
        account: null, 
        isInitialized: true,
        walletType: null
      });
      connectionError.set(error instanceof Error ? error.message : String(error));
      selectedWalletId.set(null);
      currentWalletType.set(null);
      // Clear permissions on error
      permissionManager.clearCache();
    },

    async getActor<T>(canisterId: string, idl: any): Promise<T> {
      const state = get(store);
      
      if (!state.isConnected) {
        throw new Error('Wallet not connected');
      }

      if (state.walletType === WALLET_TYPES.INTERNET_IDENTITY) {
        if (!agent) {
          throw new Error('Internet Identity agent not initialized');
        }
        
        // Create actor using Internet Identity agent
        const { Actor } = await import('@dfinity/agent');
        return Actor.createActor(idl, {
          agent,
          canisterId
        }) as T;
      } else if (state.walletType === WALLET_TYPES.OISY || state.walletType === WALLET_TYPES.PLUG) {
        // Use PNP for both Oisy and Plug wallets
        // Trust the state.isConnected flag which was set during connection
        // The pnp.isConnectedAsync() check doesn't work reliably for all PNP wallets
        
        return pnp.getActor(canisterId, idl) as unknown as T;
      } else {
        // Fallback for any other PNP-based wallets
        // Trust state.isConnected since it was validated during connection
        return pnp.getActor(canisterId, idl) as unknown as T;
      }
    },

    async isWalletConnected(): Promise<boolean> {
      const state = get(store);
      
      if (state.walletType === WALLET_TYPES.INTERNET_IDENTITY) {
        return authClient ? await authClient.isAuthenticated() : false;
      } else if (state.walletType === WALLET_TYPES.OISY) {
        // For Oisy, trust the state.isConnected flag
        return state.isConnected;
      } else {
        // For Plug and other PNP wallets
        return await pnp.isConnectedAsync();
      }
    },
    
    // Check if wallet is authenticated and has permissions
    async isAuthenticated(): Promise<boolean> {
      const isConnected = await this.isWalletConnected();
      if (!isConnected) return false;
      
      const state = get(store);
      if (state.walletType === WALLET_TYPES.INTERNET_IDENTITY || state.walletType === WALLET_TYPES.OISY) {
        return true; // Internet Identity and Oisy are authenticated if connected
      } else {
        return permissionManager.hasPermissions();
      }
    },
    
    // Ensure both connection and permissions
    async ensureAuthenticated(): Promise<boolean> {
      const isConnected = await this.isWalletConnected();
      if (!isConnected) return false;
      
      const state = get(store);
      if (state.walletType === WALLET_TYPES.INTERNET_IDENTITY || state.walletType === WALLET_TYPES.OISY) {
        return true; // Internet Identity and Oisy don't need additional permissions
      } else {
        return await permissionManager.ensurePermissions();
      }
    },
    
    // Get the current principal
    getPrincipal(): Principal | null {
      const state = get(store);
      return state.account?.owner || null;
    }
  };
}

// Create singleton instance
export const auth = createAuthStore();

// Helper function with more descriptive error message
export function requireWalletConnection(): void {
  const isConnected = get(auth).isConnected;
  if (!isConnected) {
    throw new Error("Wallet connection required. Please connect your wallet first.");
  }
}
