import { writable, derived, get } from 'svelte/store';
import type { CollateralInfo } from '../services/types';
import { decodeRustDecimal } from '../utils/decimalUtils';
import { TokenService } from '../services/tokenService';
import { ICRC1_IDL } from '../idls/ledger.idl';

/**
 * Extract a display-friendly status string from the Candid CollateralStatus variant.
 */
function parseCollateralStatus(status: any): string {
  if (!status || typeof status !== 'object') return 'Unknown';
  if ('Active' in status) return 'Active';
  if ('Paused' in status) return 'Paused';
  if ('Frozen' in status) return 'Frozen';
  if ('Sunset' in status) return 'Sunset';
  if ('Deprecated' in status) return 'Deprecated';
  return 'Unknown';
}

interface CollateralStoreState {
  collaterals: CollateralInfo[];
  loading: boolean;
  error: string | null;
  lastFetch: number;
}

const INITIAL_STATE: CollateralStoreState = {
  collaterals: [],
  loading: false,
  error: null,
  lastFetch: 0,
};

const CACHE_DURATION = 30_000; // 30 seconds

function createCollateralStore() {
  const { subscribe, set, update } = writable<CollateralStoreState>(INITIAL_STATE);

  return {
    subscribe,

    /**
     * Fetch all supported collateral types and their configs from the backend.
     * Caches for CACHE_DURATION ms unless forceRefresh is true.
     */
    async fetchSupportedCollateral(forceRefresh = false): Promise<CollateralInfo[]> {
      const state = get({ subscribe });
      const now = Date.now();

      // Use cache if still fresh
      if (!forceRefresh && state.collaterals.length > 0 && now - state.lastFetch < CACHE_DURATION) {
        return state.collaterals;
      }

      update(s => ({ ...s, loading: true, error: null }));

      try {
        // Dynamic imports to avoid circular deps
        const { publicActor } = await import('../services/protocol/apiClient');

        // 1. Fetch supported types: Vec<(Principal, CollateralStatus)>
        const supportedTypes = await publicActor.get_supported_collateral_types();

        // 2. For each active type, fetch its CollateralConfig
        const collaterals: CollateralInfo[] = [];

        for (const [principal, status] of supportedTypes) {
          const statusStr = parseCollateralStatus(status);

          // Fetch config for this collateral type
          const configOpt = await publicActor.get_collateral_config(principal);
          if (!configOpt || configOpt.length === 0) {
            console.warn(`No config found for collateral ${principal.toText()}`);
            continue;
          }

          const config = configOpt[0];
          const principalText = principal.toText();
          const ledgerId = config.ledger_canister_id.toText();

          // Fetch symbol dynamically from the ledger's icrc1_symbol
          let symbol = principalText.substring(0, 5).toUpperCase();
          try {
            const ledgerActor = await TokenService.createAnonymousActor(ledgerId, ICRC1_IDL);
            symbol = await (ledgerActor as any).icrc1_symbol();
          } catch (err) {
            console.warn(`Failed to fetch icrc1_symbol for ${ledgerId}:`, err);
          }

          // Read display_color from backend config, fall back to neutral gray
          const color = (config.display_color && config.display_color.length > 0)
            ? config.display_color[0]
            : '#94A3B8';

          // Decode blob fields using Rust Decimal decoder
          const liquidationCr = decodeRustDecimal(config.liquidation_ratio);
          const minimumCr = decodeRustDecimal(config.borrow_threshold_ratio);
          const borrowingFee = decodeRustDecimal(config.borrowing_fee);
          const liquidationBonus = decodeRustDecimal(config.liquidation_bonus);
          const recoveryTargetCr = decodeRustDecimal(config.recovery_target_cr);
          const interestRateApr = config.interest_rate_apr ? decodeRustDecimal(config.interest_rate_apr) : 0;

          const price = config.last_price.length > 0 ? Number(config.last_price[0]) : 0;
          const priceTimestamp = config.last_price_timestamp.length > 0 ? Number(config.last_price_timestamp[0]) : 0;

          collaterals.push({
            principal: principalText,
            symbol,
            decimals: Number(config.decimals),
            ledgerCanisterId: ledgerId,
            price,
            priceTimestamp,
            minimumCr,
            liquidationCr,
            borrowingFee,
            liquidationBonus,
            recoveryTargetCr,
            interestRateApr,
            debtCeiling: Number(config.debt_ceiling),
            minVaultDebt: Number(config.min_vault_debt),
            ledgerFee: Number(config.ledger_fee),
            color,
            status: statusStr,
          });
        }

        update(s => ({
          ...s,
          collaterals,
          loading: false,
          error: null,
          lastFetch: Date.now(),
        }));

        return collaterals;
      } catch (err) {
        const errorMsg = err instanceof Error ? err.message : 'Failed to fetch collateral types';
        console.error('Error fetching collateral types:', err);
        update(s => ({ ...s, loading: false, error: errorMsg }));

        // Return cached data if available
        const cached = get({ subscribe }).collaterals;
        return cached.length > 0 ? cached : [];
      }
    },

    /**
     * Get CollateralInfo for a specific collateral type by its principal text.
     * Returns undefined if not loaded or not found.
     */
    getCollateralInfo(principalText: string): CollateralInfo | undefined {
      const state = get({ subscribe });
      return state.collaterals.find(c => c.principal === principalText);
    },

    /**
     * Get price for a specific collateral type. Falls back to 0 if unknown.
     */
    getCollateralPrice(principalText: string): number {
      const info = this.getCollateralInfo(principalText);
      return info?.price ?? 0;
    },

    /**
     * Get symbol for a collateral type. Falls back to abbreviated principal.
     */
    getCollateralSymbol(principalText: string): string {
      const info = this.getCollateralInfo(principalText);
      return info?.symbol ?? principalText.substring(0, 5);
    },

    /**
     * Get decimals for a collateral type. Falls back to 8 (ICP default).
     */
    getCollateralDecimals(principalText: string): number {
      const info = this.getCollateralInfo(principalText);
      return info?.decimals ?? 8;
    },

    /**
     * Get the color for a collateral type's UI badge.
     */
    getCollateralColor(principalText: string): string {
      const info = this.getCollateralInfo(principalText);
      return info?.color ?? '#94A3B8';
    },

    /**
     * Reset the store (e.g., on wallet disconnect).
     */
    reset() {
      set(INITIAL_STATE);
    },
  };
}

export const collateralStore = createCollateralStore();

/**
 * Derived store: only Active collateral types (for dropdowns, etc.)
 */
export const activeCollateralTypes = derived(
  collateralStore,
  $store => $store.collaterals.filter(c => c.status === 'Active')
);

/**
 * Derived store: loading state
 */
export const collateralLoading = derived(
  collateralStore,
  $store => $store.loading
);
