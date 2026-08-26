import { defineChain, parseEther } from "viem";

export type DeploymentMode = "testnet" | "production-canary" | "production-public";

export type DeploymentConfig = {
  mode: DeploymentMode;
  mainnet: boolean;
  guidedLifecycle: boolean;
  backendCanisterId: string;
  chainId: number;
  chainName: string;
  rpcUrl: string;
  explorerUrl: string;
  icusdContract: `0x${string}`;
  receiptConfirmations: number;
};

const TESTNET_CONFIG: DeploymentConfig = {
  mode: "testnet",
  mainnet: false,
  guidedLifecycle: false,
  backendCanisterId: "kvg63-wiaaa-aaaao-bbabq-cai",
  chainId: 71,
  chainName: "Conflux eSpace Testnet",
  rpcUrl: "https://evmtestnet.confluxrpc.com",
  explorerUrl: "https://evmtestnet.confluxscan.org",
  icusdContract: "0xBD02222D388BC43095A4758C3e977d5dF8f68f7a",
  receiptConfirmations: 1,
};

export const CANARY_CHAIN_ID = 1030;
export const CANARY_ICUSD_CONTRACT = "0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff" as const;

const PRODUCTION_CANARY_CONFIG: DeploymentConfig = {
  mode: "production-canary",
  mainnet: true,
  guidedLifecycle: true,
  backendCanisterId: "tfesu-vyaaa-aaaap-qrd7a-cai",
  chainId: CANARY_CHAIN_ID,
  chainName: "Conflux eSpace Mainnet",
  rpcUrl: "https://evm.confluxrpc.com",
  explorerUrl: "https://evm.confluxscan.io",
  icusdContract: CANARY_ICUSD_CONTRACT,
  receiptConfirmations: 400,
};

const PRODUCTION_PUBLIC_CONFIG: DeploymentConfig = {
  mode: "production-public",
  mainnet: true,
  /* production-public-prune:start public-guided-flag */
  guidedLifecycle: false,
  /* production-public-prune:end public-guided-flag */
  backendCanisterId: "tfesu-vyaaa-aaaap-qrd7a-cai",
  chainId: CANARY_CHAIN_ID,
  chainName: "Conflux eSpace Mainnet",
  rpcUrl: "https://evm.confluxrpc.com",
  explorerUrl: "https://evm.confluxscan.io",
  icusdContract: CANARY_ICUSD_CONTRACT,
  receiptConfirmations: 400,
};

/** Resolve a build target. Empty/undefined deliberately preserves the existing testnet default. */
export function resolveDeploymentConfig(mode?: string): DeploymentConfig {
  if (!mode || mode === "testnet") return TESTNET_CONFIG;
  if (mode === "production-canary") return PRODUCTION_CANARY_CONFIG;
  if (mode === "production-public") return PRODUCTION_PUBLIC_CONFIG;
  throw new Error(`Unsupported VITE_DEPLOYMENT_MODE: ${mode}`);
}

export const DEPLOYMENT = resolveDeploymentConfig(import.meta.env.VITE_DEPLOYMENT_MODE);
export const IS_MAINNET = DEPLOYMENT.mainnet;
export const IS_PRODUCTION_CANARY = DEPLOYMENT.guidedLifecycle;
export const IS_PRODUCTION_PUBLIC = DEPLOYMENT.mode === "production-public";
// vite.config.ts validates the exact production-public origin before either a
// dev server or build starts. Embed only the already-validated value here so
// deployment-verification sentinels and origin-policy machinery cannot leak
// into the production bundle.
export const PUBLIC_CANONICAL_ORIGIN = IS_PRODUCTION_PUBLIC
  ? (import.meta.env.VITE_PUBLIC_CANONICAL_ORIGIN || null)
  : null;

/** A single click may prompt at most once on mainnet. Testnet retains one
 * convenience retry for local/staging nonce races. */
export function signatureAttemptLimit(mainnet: boolean): 1 | 2 {
  return mainnet ? 1 : 2;
}

export const BACKEND_CANISTER_ID = DEPLOYMENT.backendCanisterId;
/** IC HTTP gateway. Anonymous agent — the EVM signature is the only auth. */
export const IC_HOST = "https://icp0.io";
export const CHAIN_ID = DEPLOYMENT.chainId;
export const ESPACE_RPC = DEPLOYMENT.rpcUrl;
export const ESPACE_EXPLORER = DEPLOYMENT.explorerUrl;
export const ICUSD_CONTRACT = DEPLOYMENT.icusdContract;
export const RECEIPT_CONFIRMATIONS = DEPLOYMENT.receiptConfirmations;

/** Mirrors viem's confirmation count: the inclusion block counts as one. */
export function receiptHasRequiredConfirmations(
  receiptBlock: bigint,
  headBlock: bigint,
  requiredConfirmations: number,
): boolean {
  if (!Number.isSafeInteger(requiredConfirmations) || requiredConfirmations < 1 || headBlock < receiptBlock) return false;
  return headBlock - receiptBlock + 1n >= BigInt(requiredConfirmations);
}

/** EIP-712 domain — MUST match the backend's `chains/evm/eip712.rs` exactly. */
export const EIP712_DOMAIN = {
  name: "Rumi icUSD CDP",
  version: "1",
  chainId: CHAIN_ID,
  verifyingContract: ICUSD_CONTRACT,
} as const;

// ── CDP params (mirror the backend's chain collateral config) ────────────────

/**
 * Minimum collateral ratio (1.50 = 150%). Mirrors the backend's open/borrow
 * gate `min_cr_e4` (15_000) in `src/rumi_protocol_backend/src/chains/collateral_config.rs`
 * for Conflux (chain 71 and 1030). This is NOT the liquidation threshold
 * (`liquidation_threshold_e4`, 133%) — it is the higher bar the backend
 * enforces when a vault is opened or borrowed against. Must move in lockstep
 * with `min_cr_e4`; a mismatch under-collateralizes the suggested amount and
 * the backend rejects the Open intent after the EVM nonce is already spent,
 * forcing a re-sign.
 */
export const MIN_CR = 1.5;
/** Minimum vault debt: 0.1 icUSD (e8s). */
export const MIN_DEBT_E8S = 10_000_000n;
export const ICUSD_DECIMALS = 8;
export const CFX_DECIMALS = 18;

/** Safety buffer applied above MIN_CR when suggesting collateral, to absorb
 * price/rounding drift between the UX hint and the backend's authoritative
 * check. Matches the buffer already used by the pre-existing suggestion
 * formula (kept as-is, not a new product behavior). */
export const SUGGESTED_COLLATERAL_BUFFER = 1.02;

/**
 * Suggested CFX collateral (in wei) for a requested icUSD debt at a given
 * CFX/USD price, sized to clear the backend's MIN_CR floor plus
 * SUGGESTED_COLLATERAL_BUFFER. Returns 0n for non-positive inputs.
 */
export function suggestedCollateralWei(debtIcusd: number, cfxPriceUsd: number): bigint {
  return suggestedCollateralWeiAtRatio(debtIcusd, cfxPriceUsd, MIN_CR);
}

export function suggestedCollateralWeiAtRatio(
  debtIcusd: number,
  cfxPriceUsd: number,
  minimumCollateralRatio: number,
): bigint {
  if (!(debtIcusd > 0) || !(cfxPriceUsd > 0)) return 0n;
  if (!(minimumCollateralRatio > 0)) return 0n;
  const cfxNeeded = (debtIcusd * minimumCollateralRatio / cfxPriceUsd) * SUGGESTED_COLLATERAL_BUFFER;
  return parseEther(cfxNeeded.toFixed(6));
}

/* production-public-prune:start guided-open-terms */
/** Immutable production-canary envelope. These values are sent in the Open intent. */
export const CANARY_COLLATERAL_WEI = 5n * 10n ** 18n;
export const CANARY_DEBT_E8S = 10_000_000n; // exactly 0.10 icUSD

export function openTermsFor(config: DeploymentConfig, requestedCollateral: bigint, requestedDebt: bigint) {
  return config.guidedLifecycle
    ? { collateralWei: CANARY_COLLATERAL_WEI, debtE8s: CANARY_DEBT_E8S }
    : { collateralWei: requestedCollateral, debtE8s: requestedDebt };
}
/* production-public-prune:end guided-open-terms */

export const confluxESpaceChain = defineChain({
  id: CHAIN_ID,
  name: DEPLOYMENT.chainName,
  nativeCurrency: { name: "Conflux", symbol: "CFX", decimals: CFX_DECIMALS },
  rpcUrls: { default: { http: [ESPACE_RPC] } },
  blockExplorers: { default: { name: "ConfluxScan", url: ESPACE_EXPLORER } },
  /* production-public-prune:start testnet-chain-flag */
  testnet: !IS_MAINNET,
  /* production-public-prune:end testnet-chain-flag */
});

// Compatibility alias for any external testnet-only imports.
export const confluxESpaceTestnet = confluxESpaceChain;

// ── Intent action discriminants (must match IntentAction in the backend) ─────

export const ACTION = {
  Open: 0,
  Borrow: 1,
  WithdrawCollateral: 2,
  Close: 3,
} as const;

/** Minimal ABI for the bits of IcUSD.sol the UI touches. */
export const ICUSD_ABI = [
  {
    type: "function",
    name: "balanceOf",
    stateMutability: "view",
    inputs: [{ name: "account", type: "address" }],
    outputs: [{ name: "", type: "uint256" }],
  },
  {
    type: "function",
    name: "burn",
    stateMutability: "nonpayable",
    inputs: [
      { name: "amount", type: "uint256" },
      { name: "target_vault_id", type: "uint64" },
    ],
    outputs: [],
  },
] as const;
