import { writable, derived, get } from 'svelte/store';
import type { Principal } from '@dfinity/principal';
import type { ProtocolStatusDTO, VaultDTO } from '../services/types';
import { registerWalletSessionTransitionListener } from '../services/walletSessionTransitions';

/**
 * CENTRALIZED DATA STORE
 * This single store manages ALL application data to eliminate redundant backend calls
 * NO component should call backend directly - everything goes through this store
 */

/** Per-collateral aggregate debt + vault count, in human units, keyed by collateral principal text. */
export interface CollateralTotalsEntry {
  totalDebt: number;   // human icUSD borrowed against this collateral, aggregate across all its vaults
  vaultCount: number;
}

interface AppDataState {
  // Protocol data
  protocolStatus: ProtocolStatusDTO | null;
  protocolStatusLoading: boolean;
  protocolStatusError: string | null;
  protocolStatusLastFetch: number;

  // Per-collateral debt aggregates (for debt-ceiling headroom in the borrow panel)
  collateralTotals: Record<string, CollateralTotalsEntry>;
  collateralTotalsLoading: boolean;
  collateralTotalsLastFetch: number;

  // User vaults data  
  userVaults: VaultDTO[];
  userVaultsLoading: boolean;
  userVaultsError: string | null;
  userVaultsLastFetch: number;

  // Wallet data
  walletConnected: boolean;
  walletPrincipal: Principal | null;
  icpBalance: bigint | null;
  icusdBalance: bigint | null;
  balancesLoading: boolean;
  balancesError: string | null;
  balancesLastFetch: number;

  // Request tracking to prevent duplicates
  activeRequests: Set<string>;
}

const INITIAL_STATE: AppDataState = {
  protocolStatus: null,
  protocolStatusLoading: false,
  protocolStatusError: null,
  protocolStatusLastFetch: 0,

  collateralTotals: {},
  collateralTotalsLoading: false,
  collateralTotalsLastFetch: 0,

  userVaults: [],
  userVaultsLoading: false,
  userVaultsError: null,
  userVaultsLastFetch: 0,

  walletConnected: false,
  walletPrincipal: null,
  icpBalance: null,
  icusdBalance: null,
  balancesLoading: false,
  balancesError: null,
  balancesLastFetch: 0,

  activeRequests: new Set(),
};

// Cache duration in milliseconds
const CACHE_DURATION = 5000; // 5 seconds

function createAppDataStore() {
  const { subscribe, set, update } = writable<AppDataState>(INITIAL_STATE);
  let walletSessionScope = 'session:initial';
  let walletSessionEpoch = 0;
  let walletSessionTransitioning = false;
  const activeBalancePromises = new Map<string, Promise<{ icpBalance: bigint; icusdBalance: bigint }>>();
  const activeUserVaultPromises = new Map<string, Promise<VaultDTO[]>>();

  function isCurrentUserRequest(
    principal: Principal,
    scope: string,
    epoch: number,
    context?: { assertCurrent: () => boolean },
  ): boolean {
    const state = get(appDataStore);
    return !walletSessionTransitioning && walletSessionScope === scope && walletSessionEpoch === epoch &&
      state.walletConnected && state.walletPrincipal?.toText() === principal.toText() &&
      (!context || context.assertCurrent());
  }

  return {
    subscribe,

    /**
     * PROTOCOL STATUS - Single source of truth
     */
    async fetchProtocolStatus(forceRefresh = false): Promise<ProtocolStatusDTO | null> {
      return new Promise((resolve, reject) => {
        const requestKey = 'protocol_status'; // Move requestKey outside update callback
        
        update(state => {
          const now = Date.now();
          
          // If request already active, wait for it
          if (state.activeRequests.has(requestKey)) {
            console.log('🔄 Protocol status request already in progress...');
            // Wait for the active request to complete
            const checkCompletion = () => {
              const currentState = get(appDataStore);
              if (!currentState.activeRequests.has(requestKey)) {
                resolve(currentState.protocolStatus);
              } else {
                setTimeout(checkCompletion, 100);
              }
            };
            checkCompletion();
            return state;
          }

          // Use cache if valid and not forcing refresh
          if (!forceRefresh && 
              state.protocolStatus && 
              (now - state.protocolStatusLastFetch) < CACHE_DURATION) {
            console.log('📦 Using cached protocol status');
            resolve(state.protocolStatus);
            return state;
          }

          // Start new request
          console.log('🆕 Fetching fresh protocol status...');
          state.activeRequests.add(requestKey);
          
          return {
            ...state,
            protocolStatusLoading: true,
            protocolStatusError: null,
          };
        });

        // Make the actual API call
        this._fetchProtocolStatusFromAPI()
          .then(protocolStatus => {
            update(state => {
              state.activeRequests.delete(requestKey);
              resolve(protocolStatus);
              return {
                ...state,
                protocolStatus,
                protocolStatusLoading: false,
                protocolStatusError: null,
                protocolStatusLastFetch: Date.now(),
              };
            });
          })
          .catch(error => {
            update(state => {
              state.activeRequests.delete(requestKey); // Now requestKey is accessible
              const errorMsg = error.message || 'Failed to fetch protocol status';
              reject(new Error(errorMsg));
              return {
                ...state,
                protocolStatusLoading: false,
                protocolStatusError: errorMsg,
              };
            });
          });
      });
    },

    /**
     * PER-COLLATERAL DEBT TOTALS - for the borrow panel's debt-ceiling headroom.
     *
     * Light public query (no wallet). Cached like the rest; on error we keep and
     * return whatever is cached (possibly empty) so the borrow panel degrades to
     * the collateral-only cap rather than breaking.
     */
    async fetchCollateralTotals(forceRefresh = false): Promise<Record<string, CollateralTotalsEntry>> {
      const requestKey = 'collateral_totals';
      const state = get(appDataStore);
      const now = Date.now();

      if (!forceRefresh &&
          Object.keys(state.collateralTotals).length > 0 &&
          (now - state.collateralTotalsLastFetch) < CACHE_DURATION) {
        return state.collateralTotals;
      }

      // If a fetch is already in flight, wait for it instead of racing a second call.
      if (state.activeRequests.has(requestKey)) {
        return new Promise(resolve => {
          const check = () => {
            const s = get(appDataStore);
            if (!s.activeRequests.has(requestKey)) resolve(s.collateralTotals);
            else setTimeout(check, 100);
          };
          check();
        });
      }

      update(s => {
        s.activeRequests.add(requestKey);
        return { ...s, collateralTotalsLoading: true };
      });

      try {
        const totals = await this._fetchCollateralTotalsFromAPI();
        update(s => {
          s.activeRequests.delete(requestKey);
          return {
            ...s,
            collateralTotals: totals,
            collateralTotalsLoading: false,
            collateralTotalsLastFetch: Date.now(),
          };
        });
        return totals;
      } catch (error) {
        update(s => {
          s.activeRequests.delete(requestKey);
          return { ...s, collateralTotalsLoading: false };
        });
        console.warn('Failed to fetch collateral totals:', error);
        return get(appDataStore).collateralTotals;
      }
    },

    /**
     * USER VAULTS - Single source of truth
     */
    async fetchUserVaults(
      principal: Principal,
      forceRefresh = false,
      context?: { assertCurrent: () => boolean },
    ): Promise<VaultDTO[]> {
      const requestScope = walletSessionScope;
      const requestEpoch = walletSessionEpoch;
      const requestKey = `user_vaults_${principal.toString()}_${requestScope}_${requestEpoch}`;
      const isCurrent = () => isCurrentUserRequest(principal, requestScope, requestEpoch, context);
      if (!isCurrent()) throw new Error('Wallet session changed before user vaults could be refreshed.');

      const inFlight = activeUserVaultPromises.get(requestKey);
      if (inFlight) return inFlight;

      const state = get(appDataStore);
      const now = Date.now();
      if (!forceRefresh && state.walletPrincipal?.toText() === principal.toText() &&
          now - state.userVaultsLastFetch < CACHE_DURATION) {
        return state.userVaults;
      }

      update(current => ({
        ...current,
        activeRequests: new Set([...current.activeRequests, requestKey]),
        userVaultsLoading: true,
        userVaultsError: null,
      }));

      let trackedRequest!: Promise<VaultDTO[]>;
      trackedRequest = this._fetchUserVaultsFromAPI(principal)
        .then(userVaults => {
          update(current => {
            const activeRequests = new Set(current.activeRequests);
            activeRequests.delete(requestKey);
            if (!isCurrent()) return { ...current, activeRequests };
            return {
              ...current,
              activeRequests,
              userVaults,
              userVaultsLoading: false,
              userVaultsError: null,
              userVaultsLastFetch: Date.now(),
            };
          });
          return userVaults;
        })
        .catch(error => {
          update(current => {
            const activeRequests = new Set(current.activeRequests);
            activeRequests.delete(requestKey);
            if (!isCurrent()) return { ...current, activeRequests };
            const errorMsg = error instanceof Error ? error.message : 'Failed to fetch user vaults';
            return { ...current, activeRequests, userVaultsLoading: false, userVaultsError: errorMsg };
          });
          throw error;
        })
        .finally(() => {
          if (activeUserVaultPromises.get(requestKey) === trackedRequest) {
            activeUserVaultPromises.delete(requestKey);
          }
        });
      activeUserVaultPromises.set(requestKey, trackedRequest);
      return trackedRequest;
    },

    /**
     * WALLET BALANCES - Single source of truth
     *
     * `skipCache` bypasses TokenService's 30s balance cache (separate from this
     * store's 5s cache controlled by `forceRefresh`). Required after balance-
     * mutating user actions — otherwise the UI shows the pre-op balance until
     * the TokenService cache expires.
     */
    async fetchBalances(
      principal: Principal,
      forceRefresh = false,
      opts?: { skipCache?: boolean },
      isRequestCurrent: () => boolean = () => true,
      requestScope?: string,
    ): Promise<{ icpBalance: bigint; icusdBalance: bigint }> {
      // Bound refreshes pass a unique request identity; unbound callers share
      // within the current wallet session so useful same-session dedupe stays.
      const requestKey = `balances_${principal.toString()}_${requestScope ?? walletSessionScope}`;
      const inFlight = activeBalancePromises.get(requestKey);
      if (inFlight) return inFlight;

      const state = get(appDataStore);
      if (walletSessionTransitioning || !state.walletConnected ||
          state.walletPrincipal?.toText() !== principal.toText() || !isRequestCurrent()) {
        throw new Error('Wallet session changed before balances could be refreshed.');
      }
      const requestSessionScope = walletSessionScope;
      const requestSessionEpoch = walletSessionEpoch;
      const isStillCurrent = () => !walletSessionTransitioning &&
        walletSessionScope === requestSessionScope && walletSessionEpoch === requestSessionEpoch &&
        get(appDataStore).walletConnected &&
        get(appDataStore).walletPrincipal?.toText() === principal.toText() && isRequestCurrent();
      const now = Date.now();
      if (!forceRefresh && state.icpBalance !== null && state.icusdBalance !== null &&
          state.walletPrincipal?.toString() === principal.toString() &&
          now - state.balancesLastFetch < CACHE_DURATION) {
        return { icpBalance: state.icpBalance, icusdBalance: state.icusdBalance };
      }

      update(current => ({
        ...current,
        activeRequests: new Set([...current.activeRequests, requestKey]),
        balancesLoading: true,
        balancesError: null,
      }));

      let trackedRequest!: Promise<{ icpBalance: bigint; icusdBalance: bigint }>;
      trackedRequest = this._fetchBalancesFromAPI(principal, opts)
        .then(result => {
          update(current => {
            const activeRequests = new Set(current.activeRequests);
            activeRequests.delete(requestKey);
            // A response from an old owner/session retires only its own marker;
            // it cannot write cache data or clear the newer request's loading bit.
            if (!isStillCurrent()) return { ...current, activeRequests };
            return {
              ...current,
              activeRequests,
              icpBalance: result.icpBalance,
              icusdBalance: result.icusdBalance,
              balancesLoading: false,
              balancesError: null,
              balancesLastFetch: Date.now(),
            };
          });
          return result;
        })
        .catch(error => {
          update(current => {
            const activeRequests = new Set(current.activeRequests);
            activeRequests.delete(requestKey);
            if (!isStillCurrent()) return { ...current, activeRequests };
            const errorMsg = error instanceof Error ? error.message : 'Failed to fetch balances';
            return { ...current, activeRequests, balancesLoading: false, balancesError: errorMsg };
          });
          throw error;
        })
        .finally(() => {
          if (activeBalancePromises.get(requestKey) === trackedRequest) activeBalancePromises.delete(requestKey);
        });
      activeBalancePromises.set(requestKey, trackedRequest);
      return trackedRequest;
    },

    /**
     * WALLET CONNECTION STATE
     */
    setWalletState(connected: boolean, principal: Principal | null, sessionScope?: string | number) {
      walletSessionTransitioning = false;
      update(state => {
        const principalChanged = state.walletPrincipal?.toText() !== principal?.toText();
        const nextSessionScope = sessionScope === undefined
          ? `owner:${principal?.toText() ?? 'disconnected'}`
          : `session:${String(sessionScope)}`;
        const sessionChanged = principalChanged || nextSessionScope !== walletSessionScope || state.walletConnected !== connected;
        if (sessionChanged) {
          walletSessionScope = nextSessionScope;
          walletSessionEpoch += 1;
        }
        const activeRequests = sessionChanged
          ? new Set([...state.activeRequests].filter(key => !key.startsWith('balances_') && !key.startsWith('user_vaults_')))
          : state.activeRequests;
        return {
          ...state,
          activeRequests,
          walletConnected: connected,
          walletPrincipal: principal,
          // Never show one owner's balances after another owner connects.
          // Reset the aggregate loading bit on owner changes; only the current
          // owner's guarded request may set it again.
          ...(sessionChanged && {
            icpBalance: null,
            icusdBalance: null,
            balancesError: null,
            balancesLoading: false,
            balancesLastFetch: 0,
          }),
          // User vaults belong to a wallet session, even when reconnecting the same principal.
          ...(sessionChanged && {
          userVaults: [],
          userVaultsLoading: false,
          userVaultsError: null,
          userVaultsLastFetch: 0,
          }),
          // Clear wallet data when disconnected.
          ...(!connected && {
          icpBalance: null,
          icusdBalance: null,
          balancesError: null,
          balancesLoading: false,
          balancesLastFetch: 0,
          })
        };
      });
    },

    /** Invalidate user-specific in-flight requests while preserving published wallet connection state. */
    invalidateWalletSessionTransition(generation: number): void {
      walletSessionTransitioning = true;
      walletSessionScope = `transition:${generation}`;
      walletSessionEpoch += 1;
      update(state => ({
        ...state,
        activeRequests: new Set([...state.activeRequests].filter(
          key => !key.startsWith('balances_') && !key.startsWith('user_vaults_'),
        )),
        userVaults: [],
        userVaultsLoading: false,
        userVaultsError: null,
        userVaultsLastFetch: 0,
        icpBalance: null,
        icusdBalance: null,
        balancesLoading: false,
        balancesError: null,
        balancesLastFetch: 0,
      }));
    },

    /**
     * FORCE REFRESH ALL DATA
     */
    async refreshAll(principal?: Principal, context?: { assertCurrent: () => boolean }): Promise<void> {
      const currentState = get(appDataStore);
      const targetPrincipal = principal || currentState.walletPrincipal;
      
      if (!targetPrincipal) {
        console.warn('No principal available for refresh');
        return;
      }

      console.log('🔄 Force refreshing all data...');
      
      try {
        const requestScope = walletSessionScope;
        const requestEpoch = walletSessionEpoch;
        const isCurrent = () => isCurrentUserRequest(targetPrincipal, requestScope, requestEpoch, context);
        await Promise.all([
          this.fetchProtocolStatus(true),
          this.fetchCollateralTotals(true),
          this.fetchUserVaults(targetPrincipal, true, context),
          this.fetchBalances(targetPrincipal, true, undefined, isCurrent, `${requestScope}:${requestEpoch}`),
        ]);
        console.log('✅ All data refreshed successfully');
      } catch (error) {
        console.error('❌ Error refreshing data:', error);
      }
    },

    /**
     * CLEAR ALL DATA
     */
    clearAll(): void {
      walletSessionScope = `session:cleared:${++walletSessionEpoch}`;
      walletSessionTransitioning = false;
      set(INITIAL_STATE);
    },

    // PRIVATE API METHODS - These are the only places that should make backend calls
    async _fetchProtocolStatusFromAPI(): Promise<ProtocolStatusDTO> {
      // Import here to avoid circular dependencies
      const { QueryOperations } = await import('../services/protocol/queryOperations');

      // Also trigger collateral store fetch (non-blocking) on protocol status load
      import('./collateralStore').then(({ collateralStore }) => {
        collateralStore.fetchSupportedCollateral().catch(err => {
          console.warn('Background collateral fetch failed:', err);
        });
      });

      return QueryOperations.getProtocolStatus();
    },

    async _fetchCollateralTotalsFromAPI(): Promise<Record<string, CollateralTotalsEntry>> {
      const { publicActor, E8S } = await import('../services/protocol/apiClient');
      const rows = await publicActor.get_collateral_totals();
      const map: Record<string, CollateralTotalsEntry> = {};
      for (const row of rows) {
        map[row.collateral_type.toText()] = {
          totalDebt: Number(row.total_debt) / E8S,
          vaultCount: Number(row.vault_count),
        };
      }
      return map;
    },

    async _fetchUserVaultsFromAPI(principal: Principal): Promise<VaultDTO[]> {
      // Import here to avoid circular dependencies
      const { ApiClient } = await import('../services/protocol/apiClient');
      const { collateralStore } = await import('./collateralStore');
      // Ensure collateral configs (decimals/symbol/price) are loaded BEFORE mapping
      // vaults — getUserVaults divides raw collateral by 10^decimals from the store,
      // so an empty store bakes a wrong scale (e.g. XRP at 8 instead of 6 decimals =>
      // 100x off) and the synthetic-principal symbol fallback. Cached 30s, so cheap.
      await collateralStore.fetchSupportedCollateral().catch(() => {});
      return ApiClient.getUserVaults(true); // Always force refresh from API
    },

    async _fetchBalancesFromAPI(
      principal: Principal,
      opts?: { skipCache?: boolean },
    ): Promise<{ icpBalance: bigint; icusdBalance: bigint }> {
      // Import here to avoid circular dependencies
      const { TokenService } = await import('../services/tokenService');
      const { CONFIG } = await import('../config');

      const [icpBalance, icusdBalance] = await Promise.all([
        TokenService.getTokenBalance(CONFIG.currentIcpLedgerId, principal, opts),
        TokenService.getTokenBalance(CONFIG.currentIcusdLedgerId, principal, opts)
      ]);

      return { icpBalance, icusdBalance };
    }
  };
}

export const appDataStore = createAppDataStore();
registerWalletSessionTransitionListener((generation) => appDataStore.invalidateWalletSessionTransition(generation));

// DERIVED STORES - These are reactive and update automatically
export const protocolStatus = derived(appDataStore, $store => $store.protocolStatus);
export const collateralTotals = derived(appDataStore, $store => $store.collateralTotals);
export const userVaults = derived(appDataStore, $store => $store.userVaults);
export const icpBalance = derived(appDataStore, $store => $store.icpBalance);
export const icusdBalance = derived(appDataStore, $store => $store.icusdBalance);
export const walletConnected = derived(appDataStore, $store => $store.walletConnected);

// LOADING STATES
export const isLoadingProtocol = derived(appDataStore, $store => $store.protocolStatusLoading);
export const isLoadingVaults = derived(appDataStore, $store => $store.userVaultsLoading);
export const isLoadingBalances = derived(appDataStore, $store => $store.balancesLoading);
