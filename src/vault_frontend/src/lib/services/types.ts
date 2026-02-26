import type { Principal } from '@dfinity/principal';

const E8S = 100_000_000;

// Liquidity provider status
export interface LiquidityStatus {
  liquidityProvided: number;
  totalLiquidityProvided: number;
  liquidityPoolShare: number;
  availableLiquidityReward: number;
}

// Fee information returned by the protocol
export interface FeesInfo {
  borrowingFee: number;
  redemptionFee: number;
}

// Results from vault operations
export interface VaultOperationResult {
  success: boolean;
  vaultId?: number;
  error?: string;
  blockIndex?: number;
  feePaid?: number;
  message?: string;
  // Oisy two-step push-deposit: deposit succeeded, awaiting user gesture to finalize
  pendingDeposit?: boolean;
  depositBlockIndex?: number;
}

export interface VaultHistoryEvent {
  type: string;
  timestamp: number;
  amount: number;
  details: Record<string, any>;
}

export interface UserBalances {
  icp: number;
  icusd: number;
}

// Fees returned to the frontend
export interface FeesDTO {
  borrowingFee: number;
  redemptionFee: number;
}

// Vault as returned to the frontend
export interface VaultDTO {
  vaultId: number;
  owner: string;
  icpMargin: number;              // Kept for backward compat (same as collateralAmount for ICP)
  borrowedIcusd: number;
  timestamp?: number;
  // Multi-collateral fields
  collateralType: string;         // Principal text of collateral token's ledger
  collateralAmount: number;       // Human-readable (divided by decimals)
  collateralSymbol: string;       // "ICP", "ckETH", "ckBTC", etc.
  collateralDecimals: number;     // 8 for ICP, 18 for ckETH, etc.
}

/**
 * Interface for CandidVault as returned by the backend.
 * Matches the regenerated Candid declarations.
 */
export interface CandidVault {
  vault_id: number;
  owner: string;
  borrowed_icusd_amount: number;
  icp_margin_amount: number;        // Kept for backward compat
  collateral_amount: number;        // Raw amount in token's native precision
  collateral_type: string;          // Principal text of collateral token's ledger
}

// Liquidity status as returned to the frontend
export interface LiquidityStatusDTO {
  liquidity_provided: bigint;
  total_liquidity_provided: bigint;
  liquidity_pool_share: number;
  available_liquidity_reward: bigint;
  total_available_returns: bigint;
}

// Alias for compatibility with existing code
export type Vault = VaultDTO;

export interface ProtocolStatusDTO {
  mode: any;
  totalIcpMargin: number;
  totalIcusdBorrowed: number;
  lastIcpRate: number;
  lastIcpTimestamp: number;
  totalCollateralRatio: number;
  liquidationBonus: number;
  recoveryTargetCr: number;
  recoveryModeThreshold: number;
  recoveryLiquidationBuffer: number;
  reserveRedemptionsEnabled: boolean;
  reserveRedemptionFee: number;
}

// Reserve redemption result from backend
export interface ReserveRedemptionResult {
  icusdBlockIndex: number;
  stableAmountSent: number;    // in e6s (ckStable native units)
  feeAmount: number;           // in icUSD e8s
  stableTokenUsed: string;     // principal text of the ledger used
  vaultSpilloverAmount: number; // icUSD e8s that spilled over to vault redemptions
}

// Reserve balance for a given ckStable token
export interface ReserveBalance {
  ledger: string;   // principal text
  balance: number;  // raw native units (e6s for ckStable)
  symbol: string;
}

export type ProtocolStatus = ProtocolStatusDTO;

export interface EnhancedVault {
  vaultId: number;
  owner: string;
  icpMargin: number;
  borrowedIcusd: number;
  timestamp: number;
  lastUpdated: number;
  collateralRatio?: number;
  collateralValueUSD?: number;
  maxBorrowable?: number;
  status?: 'healthy' | 'warning' | 'danger';
  // Multi-collateral fields
  collateralType: string;
  collateralAmount: number;
  collateralSymbol: string;
  collateralDecimals: number;
}

// Per-collateral configuration from backend CollateralConfig
export interface CollateralInfo {
  principal: string;            // Ledger canister principal (text)
  symbol: string;               // "ICP", "ckETH", "ckBTC"
  decimals: number;             // 8 for ICP, 18 for ckETH, etc.
  ledgerCanisterId: string;     // Same as principal for ICRC-1 tokens
  price: number;                // Current USD price
  priceTimestamp: number;       // When price was last updated
  minimumCr: number;            // borrow_threshold_ratio
  liquidationCr: number;        // liquidation_ratio
  borrowingFee: number;         // One-time fee at mint
  liquidationBonus: number;
  recoveryTargetCr: number;
  interestRateApr: number;      // Annual interest rate (0.0 = 0%)
  debtCeiling: number;          // In ICUSD e8s
  minVaultDebt: number;         // Dust threshold in ICUSD e8s
  ledgerFee: number;            // Transfer fee in native units
  color: string;                // UI badge color
  status: string;               // "Active", "Paused", "Frozen", etc.
}