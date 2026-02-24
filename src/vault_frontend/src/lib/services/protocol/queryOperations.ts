import { ApiClient, E8S } from './apiClient';
import type { FeesInfo, ProtocolStatus, FeesDTO, ProtocolStatusDTO, CollateralInfo } from '../types';
import type {
    ProtocolStatus as CanisterProtocolStatus,
    Fees,
    CollateralConfig
  } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did';
import { Principal } from '@dfinity/principal';
import { CONFIG } from '../../config';
import { RequestDeduplicator } from '../RequestDeduplicator';
import { decodeRustDecimal } from '$lib/utils/decimalUtils';

/**
 * Operations focused on querying protocol data
 */
export class QueryOperations {
  /**
   * Get the current protocol status - WITH REQUEST DEDUPLICATION
   */
  static async getProtocolStatus(): Promise<ProtocolStatusDTO> {
    return RequestDeduplicator.deduplicate('get_protocol_status', async () => {
      console.log('Calling get_protocol_status with args:', []);
      const canisterStatus = await ApiClient.getPublicData<CanisterProtocolStatus>('get_protocol_status');
      
      console.log('Raw protocol status:', canisterStatus);
      
      return {
        mode: canisterStatus.mode,
        totalIcpMargin: Number(canisterStatus.total_icp_margin) / E8S,
        totalIcusdBorrowed: Number(canisterStatus.total_icusd_borrowed) / E8S,
        lastIcpRate: Number(canisterStatus.last_icp_rate),
        lastIcpTimestamp: Number(canisterStatus.last_icp_timestamp),
        totalCollateralRatio: Number(canisterStatus.total_collateral_ratio),
        liquidationBonus: Number(canisterStatus.liquidation_bonus),
        recoveryTargetCr: Number(canisterStatus.recovery_target_cr),
        recoveryModeThreshold: Number(canisterStatus.recovery_mode_threshold),
        recoveryLiquidationBuffer: Number(canisterStatus.recovery_liquidation_buffer),
        reserveRedemptionsEnabled: Boolean((canisterStatus as any).reserve_redemptions_enabled),
        reserveRedemptionFee: Number((canisterStatus as any).reserve_redemption_fee),
      };
    });
  }

  /**
   * Get the current ICP price - SIMPLIFIED to directly use protocol status
   */
  static async getICPPrice(): Promise<number> {
    try {
      const status = await ApiClient.getPublicData<CanisterProtocolStatus>('get_protocol_status');
      console.log('Raw protocol status:', status); 
      const rate = Number(status.last_icp_rate);
      console.log('Converted ICP rate:', rate); 
      return rate;
    } catch (err) {
      console.error("Failed to fetch ICP price:", err);
      throw new Error("Could not fetch ICP price");
    }
  }

  // Add a method to get the real price from logs even in mock mode
  static async getRealICPPrice(): Promise<number> {
    try {
      // First try logs approach
      const priceFromLogs = await this.getRealIcpPriceFromLogs();
      if (priceFromLogs !== null) {
        return priceFromLogs;
      }
      
      // Then try metrics endpoint
      const metricsResponse = await fetch(`${CONFIG.host}/api/${CONFIG.currentCanisterId}/metrics`);
      if (metricsResponse.ok) {
        const metricsText = await metricsResponse.text();
        const metricsMatch = metricsText.match(/rumi_icp_rate\s+([0-9.]+)/);
        
        if (metricsMatch && metricsMatch[1]) {
          const price = parseFloat(metricsMatch[1]);
          console.log('Found ICP price in metrics:', price);
          return price;
        }
      }
      
      // Default fallback price
      return 6.41;
    } catch (err) {
      console.error('Error fetching real ICP price:', err);
      return 6.41; // Default fallback value
    }
  }

  /**
   * Method to get real ICP price from logs
   */
  private static async getRealIcpPriceFromLogs(): Promise<number | null> {
    try {
      // Try to get the price directly from the canister logs
      const response = await fetch(`${CONFIG.host}/api/${CONFIG.currentCanisterId}/logs?priority=TraceXrc`);
      
      if (response.ok) {
        const text = await response.text();
        const matches = text.matchAll(/\[FetchPrice\] fetched new ICP rate: ([0-9.]+)/g);
        let latestPrice = null;
        
        for (const match of Array.from(matches)) {
          if (match && match[1]) {
            latestPrice = parseFloat(match[1]);
          }
        }
        
        if (latestPrice !== null) {
          console.log('Found live ICP price in logs:', latestPrice);
          return latestPrice;
        }
      }
      return null;
    } catch (err) {
      console.error('Error fetching ICP price from logs:', err);
      return null;
    }
  }

  /**
   * Get current fee rates
   */
  static async getFees(amount: number): Promise<FeesDTO> {
    try {
      const amountBigInt = BigInt(Math.round(amount * E8S));
      const fees = await ApiClient.getPublicData<Fees>('get_fees', amountBigInt);
      
      return {
        borrowingFee: Number(fees.borrowing_fee),
        redemptionFee: Number(fees.redemption_fee)
      };
    } catch (err) {
      console.error('Failed to get fees:', err);
      return {
        borrowingFee: 0.005, // 0.5%
        redemptionFee: 0.001 // 0.1%
      };
    }
  }

  /**
   * Get pending transfers
   * Note: The backend doesn't have a dedicated endpoint for this yet
   */
  static async getPendingTransfers(): Promise<any[]> {
    try {
      // This is a placeholder for when the backend adds this endpoint
      return [];
    } catch (err) {
      console.error('Error getting pending transfers:', err);
      return [];
    }
  }

  /**
   * Get the list of supported collateral types with their status.
   * Returns an array of [principalText, statusString] pairs.
   */
  static async getSupportedCollateralTypes(): Promise<Array<{ principal: string; status: string }>> {
    return RequestDeduplicator.deduplicate('get_supported_collateral_types', async () => {
      try {
        const result = await ApiClient.getPublicData<Array<[any, any]>>('get_supported_collateral_types');
        return result.map(([principal, status]: [any, any]) => {
          let statusStr = 'Unknown';
          if (status && typeof status === 'object') {
            if ('Active' in status) statusStr = 'Active';
            else if ('Paused' in status) statusStr = 'Paused';
            else if ('Frozen' in status) statusStr = 'Frozen';
            else if ('Sunset' in status) statusStr = 'Sunset';
            else if ('Deprecated' in status) statusStr = 'Deprecated';
          }
          return {
            principal: principal.toText ? principal.toText() : String(principal),
            status: statusStr,
          };
        });
      } catch (err) {
        console.error('Failed to get supported collateral types:', err);
        return [];
      }
    });
  }

  /**
   * Get the CollateralConfig for a specific collateral type, decoded into CollateralInfo.
   * Returns null if the collateral type is not found.
   */
  static async getCollateralConfig(collateralPrincipal: string): Promise<CollateralInfo | null> {
    return RequestDeduplicator.deduplicate(`get_collateral_config_${collateralPrincipal}`, async () => {
      try {
        const principal = Principal.fromText(collateralPrincipal);
        const configOpt = await ApiClient.getPublicData<[] | [CollateralConfig]>(
          'get_collateral_config', principal
        );
        if (!configOpt || (Array.isArray(configOpt) && configOpt.length === 0)) {
          return null;
        }
        const config = Array.isArray(configOpt) ? configOpt[0] : configOpt;

        // Parse status
        let statusStr = 'Unknown';
        if (config.status && typeof config.status === 'object') {
          if ('Active' in config.status) statusStr = 'Active';
          else if ('Paused' in config.status) statusStr = 'Paused';
          else if ('Frozen' in config.status) statusStr = 'Frozen';
          else if ('Sunset' in config.status) statusStr = 'Sunset';
          else if ('Deprecated' in config.status) statusStr = 'Deprecated';
        }

        // Decode blob fields
        const liquidationCr = decodeRustDecimal(config.liquidation_ratio);
        const minimumCr = decodeRustDecimal(config.borrow_threshold_ratio);
        const borrowingFee = decodeRustDecimal(config.borrowing_fee);
        const liquidationBonus = decodeRustDecimal(config.liquidation_bonus);
        const recoveryTargetCr = decodeRustDecimal(config.recovery_target_cr);

        const price = config.last_price.length > 0 ? Number(config.last_price[0]) : 0;
        const priceTimestamp = config.last_price_timestamp.length > 0 ? Number(config.last_price_timestamp[0]) : 0;

        return {
          principal: collateralPrincipal,
          symbol: collateralPrincipal, // Caller should map to friendly name
          decimals: Number(config.decimals),
          ledgerCanisterId: config.ledger_canister_id.toText(),
          price,
          priceTimestamp,
          minimumCr,
          liquidationCr,
          borrowingFee,
          liquidationBonus,
          recoveryTargetCr,
          debtCeiling: Number(config.debt_ceiling),
          minVaultDebt: Number(config.min_vault_debt),
          ledgerFee: Number(config.ledger_fee),
          color: '#94A3B8', // Caller should map to friendly color
          status: statusStr,
        } as CollateralInfo;
      } catch (err) {
        console.error('Failed to get collateral config:', err);
        return null;
      }
    });
  }

  /**
   * Check if an error is an AlreadyProcessing error
   */
  public static isAlreadyProcessingError(error: any): boolean {
    return error && (
      ('AlreadyProcessing' in error) || 
      (error instanceof Error && error.message.toLowerCase().includes('already has an ongoing operation')) ||
      (error instanceof Error && error.message.toLowerCase().includes('operation in progress'))
    );
  }

  /**
   * Check if an error is for a stale processing state
   */
  public static isStaleProcessingState(error: any, timeThresholdSeconds: number = 90): boolean {
    // Add implementation to check if the processing state is stale based on timestamp
    if (error && typeof error === 'object' && 'timestamp' in error) {
      const timestampMs = Number(error.timestamp);
      if (!isNaN(timestampMs)) {
        const ageMs = Date.now() - timestampMs;
        return ageMs > timeThresholdSeconds * 1000;
      }
    }
    
    // Also check error message for clues
    if (error instanceof Error) {
      const msg = error.message.toLowerCase();
      if (msg.includes('stale') || msg.includes('previous operation is being cleaned up')) {
        return true;
      }
    }
    
    return false;
  }
}

