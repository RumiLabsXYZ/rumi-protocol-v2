import { defineChain } from "viem";

export type DeploymentMode = "testnet" | "production-canary";

export type DeploymentConfig = {
  mode: DeploymentMode;
  productionCanary: boolean;
  backendCanisterId: string;
  chainId: number;
  chainName: string;
  rpcUrl: string;
  explorerUrl: string;
  icusdContract: `0x${string}`;
};

const TESTNET_CONFIG: DeploymentConfig = {
  mode: "testnet",
  productionCanary: false,
  backendCanisterId: "kvg63-wiaaa-aaaao-bbabq-cai",
  chainId: 71,
  chainName: "Conflux eSpace Testnet",
  rpcUrl: "https://evmtestnet.confluxrpc.com",
  explorerUrl: "https://evmtestnet.confluxscan.org",
  icusdContract: "0xBD02222D388BC43095A4758C3e977d5dF8f68f7a",
};

export const CANARY_CHAIN_ID = 1030;
export const CANARY_ICUSD_CONTRACT = "0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff" as const;

const PRODUCTION_CANARY_CONFIG: DeploymentConfig = {
  mode: "production-canary",
  productionCanary: true,
  backendCanisterId: "tfesu-vyaaa-aaaap-qrd7a-cai",
  chainId: CANARY_CHAIN_ID,
  chainName: "Conflux eSpace Mainnet",
  rpcUrl: "https://evm.confluxrpc.com",
  explorerUrl: "https://evm.confluxscan.io",
  icusdContract: CANARY_ICUSD_CONTRACT,
};

/** Resolve a build target. Empty/undefined deliberately preserves the existing testnet default. */
export function resolveDeploymentConfig(mode?: string): DeploymentConfig {
  if (!mode || mode === "testnet") return TESTNET_CONFIG;
  if (mode === "production-canary") return PRODUCTION_CANARY_CONFIG;
  throw new Error(`Unsupported VITE_DEPLOYMENT_MODE: ${mode}`);
}

export const DEPLOYMENT = resolveDeploymentConfig(import.meta.env.VITE_DEPLOYMENT_MODE);
export const IS_PRODUCTION_CANARY = DEPLOYMENT.productionCanary;

export const BACKEND_CANISTER_ID = DEPLOYMENT.backendCanisterId;
/** IC HTTP gateway. Anonymous agent — the EVM signature is the only auth. */
export const IC_HOST = "https://icp0.io";
export const CHAIN_ID = DEPLOYMENT.chainId;
export const ESPACE_RPC = DEPLOYMENT.rpcUrl;
export const ESPACE_EXPLORER = DEPLOYMENT.explorerUrl;
export const ICUSD_CONTRACT = DEPLOYMENT.icusdContract;

/** EIP-712 domain — MUST match the backend's `chains/evm/eip712.rs` exactly. */
export const EIP712_DOMAIN = {
  name: "Rumi icUSD CDP",
  version: "1",
  chainId: CHAIN_ID,
  verifyingContract: ICUSD_CONTRACT,
} as const;

// ── CDP params (mirror the backend's chain collateral config) ────────────────

/** Minimum collateral ratio (1.33 = 133%). */
export const MIN_CR = 1.33;
/** Minimum vault debt: 0.1 icUSD (e8s). */
export const MIN_DEBT_E8S = 10_000_000n;
export const ICUSD_DECIMALS = 8;
export const CFX_DECIMALS = 18;

/** Immutable production-canary envelope. These values are sent in the Open intent. */
export const CANARY_COLLATERAL_WEI = 5n * 10n ** 18n;
export const CANARY_DEBT_E8S = 10_000_000n; // exactly 0.10 icUSD

export function openTermsFor(config: DeploymentConfig, requestedCollateral: bigint, requestedDebt: bigint) {
  return config.productionCanary
    ? { collateralWei: CANARY_COLLATERAL_WEI, debtE8s: CANARY_DEBT_E8S }
    : { collateralWei: requestedCollateral, debtE8s: requestedDebt };
}

export const confluxESpaceChain = defineChain({
  id: CHAIN_ID,
  name: DEPLOYMENT.chainName,
  nativeCurrency: { name: "Conflux", symbol: "CFX", decimals: CFX_DECIMALS },
  rpcUrls: { default: { http: [ESPACE_RPC] } },
  blockExplorers: { default: { name: "ConfluxScan", url: ESPACE_EXPLORER } },
  testnet: !IS_PRODUCTION_CANARY,
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
