import { defineChain } from "viem";

// ── Targets (staging only — the chains rail is experimental/testnet) ───────────

/** Rumi backend on mainnet-STAGING (kvg63). NOT production tfesu. */
export const BACKEND_CANISTER_ID = "kvg63-wiaaa-aaaao-bbabq-cai";
/** IC HTTP gateway. Anonymous agent — the EVM signature is the only auth. */
export const IC_HOST = "https://icp0.io";

/** Conflux eSpace testnet. */
export const CHAIN_ID = 71;
export const ESPACE_RPC = "https://evmtestnet.confluxrpc.com";
export const ESPACE_EXPLORER = "https://evmtestnet.confluxscan.org";

/** Deployed IcUSD ERC-20 (the M2 per-op-idempotency 4-arg-mint build). */
export const ICUSD_CONTRACT =
  "0xBD02222D388BC43095A4758C3e977d5dF8f68f7a" as const;

/** EIP-712 domain — MUST match the backend's `chains/evm/eip712.rs` exactly. */
export const EIP712_DOMAIN = {
  name: "Rumi icUSD CDP",
  version: "1",
  chainId: CHAIN_ID,
  verifyingContract: ICUSD_CONTRACT,
} as const;

// ── CDP params (mirror the backend's ICP-mirrored CFX collateral config) ───────

/** Minimum collateral ratio (1.33 = 133%). */
export const MIN_CR = 1.33;
/** Minimum vault debt: 0.1 icUSD (e8s). */
export const MIN_DEBT_E8S = 10_000_000n;
/** icUSD / CFX decimals. */
export const ICUSD_DECIMALS = 8; // 1 base unit == 1 e8s
export const CFX_DECIMALS = 18;

// ── viem chain ─────────────────────────────────────────────────────────────────

export const confluxESpaceTestnet = defineChain({
  id: CHAIN_ID,
  name: "Conflux eSpace Testnet",
  nativeCurrency: { name: "Conflux", symbol: "CFX", decimals: CFX_DECIMALS },
  rpcUrls: { default: { http: [ESPACE_RPC] } },
  blockExplorers: { default: { name: "ConfluxScan", url: ESPACE_EXPLORER } },
  testnet: true,
});

// ── Intent action discriminants (must match IntentAction in the backend) ───────

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
