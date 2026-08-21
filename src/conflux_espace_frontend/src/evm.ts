// viem integration: injected wallets in every mode, plus a testnet-only dev-key
// signer. Every write is initiated by an explicit user click and confirmed by
// the connected wallet.

import {
  createPublicClient,
  createWalletClient,
  custom,
  http,
  parseEther,
  parseUnits,
  formatEther,
  formatUnits,
  type Address,
  type Hex,
  type WalletClient,
} from "viem";
import { privateKeyToAccount, type PrivateKeyAccount } from "viem/accounts";
import {
  CHAIN_ID,
  ESPACE_EXPLORER,
  ESPACE_RPC,
  ICUSD_ABI,
  ICUSD_CONTRACT,
  ICUSD_DECIMALS,
  IS_PRODUCTION_CANARY,
  DEPLOYMENT,
  confluxESpaceChain,
} from "./config";

export const publicClient = createPublicClient({
  chain: confluxESpaceChain,
  transport: http(ESPACE_RPC),
});

export interface Wallet {
  address: Address;
  kind: "injected" | "devkey";
  walletName: string;
  client: WalletClient;
  account: Address | PrivateKeyAccount;
}

// ── EIP-6963 multi-injected-provider discovery ──────────────────────────────
// Wallets (Rabby, MetaMask, Phantom, ...) all inject into `window.ethereum` and
// race for the slot, so the legacy grab connects whoever won (often Phantom,
// which doesn't even support eSpace). EIP-6963 lets us enumerate EVERY installed
// wallet and let the user pick the one they want.
// https://eips.ethereum.org/EIPS/eip-6963

export interface EIP6963ProviderInfo {
  uuid: string;
  name: string;
  icon: string; // data-URI image
  rdns: string; // reverse-DNS id, e.g. "io.rabby"
}
export interface EIP6963ProviderDetail {
  info: EIP6963ProviderInfo;
  provider: any; // EIP-1193 provider
}

const discovered = new Map<string, EIP6963ProviderDetail>();
const listeners = new Set<(wallets: EIP6963ProviderDetail[]) => void>();

function snapshot(): EIP6963ProviderDetail[] {
  // Name-sorted so the picker order is stable as wallets announce asynchronously.
  return [...discovered.values()].sort((a, b) => a.info.name.localeCompare(b.info.name));
}
function notify() {
  const s = snapshot();
  for (const cb of listeners) cb(s);
}

if (typeof window !== "undefined") {
  window.addEventListener("eip6963:announceProvider", (e: any) => {
    const detail = e?.detail as EIP6963ProviderDetail | undefined;
    if (detail?.info?.rdns && detail.provider) {
      discovered.set(detail.info.rdns, detail);
      notify();
    }
  });
  // Ask any already-loaded wallets to announce themselves.
  window.dispatchEvent(new Event("eip6963:requestProvider"));
}

/** Current list of EIP-6963 wallets that have announced (name-sorted). */
export function getInjectedWallets(): EIP6963ProviderDetail[] {
  return snapshot();
}
/** Re-ask wallets to announce (call when the connect view mounts, in case a
 * wallet injected after this module loaded). */
export function refreshInjectedWallets(): void {
  if (typeof window !== "undefined") window.dispatchEvent(new Event("eip6963:requestProvider"));
}
/** Subscribe to discovery changes; returns an unsubscribe fn. */
export function subscribeWallets(cb: (wallets: EIP6963ProviderDetail[]) => void): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

/** Connect to the specific EIP-6963 wallet the user picked. */
export async function connectInjected(detail: EIP6963ProviderDetail): Promise<Wallet> {
  const eth = detail.provider;
  const client = createWalletClient({ chain: confluxESpaceChain, transport: custom(eth) });
  const [address] = await client.requestAddresses();
  await ensureChain(client, eth);
  return { address, kind: "injected", walletName: detail.info.name, client, account: address };
}

/** True if a legacy `window.ethereum` exists (pre-EIP-6963 wallets / fallback). */
export function hasLegacyInjected(): boolean {
  return typeof window !== "undefined" && typeof (window as any).ethereum !== "undefined";
}
/** Fallback connect for a wallet that injects `window.ethereum` but never
 * announced via EIP-6963. Best-effort labels Rabby/MetaMask from provider flags. */
export async function connectLegacyInjected(): Promise<Wallet> {
  const eth = (window as any).ethereum;
  if (!eth) throw new Error("No EVM wallet found. Install Rabby (or another EVM wallet), then reload.");
  const client = createWalletClient({ chain: confluxESpaceChain, transport: custom(eth) });
  const [address] = await client.requestAddresses();
  await ensureChain(client, eth);
  const name = eth.isRabby ? "Rabby" : eth.isMetaMask ? "MetaMask" : "Injected wallet";
  return { address, kind: "injected", walletName: name, client, account: address };
}

export function connectDevKey(pk: string): Wallet {
  if (IS_PRODUCTION_CANARY) {
    throw new Error("Private-key entry is disabled in production-canary builds. Use an injected wallet.");
  }
  const hex = (pk.startsWith("0x") ? pk : `0x${pk}`) as Hex;
  const account = privateKeyToAccount(hex);
  const client = createWalletClient({ account, chain: confluxESpaceChain, transport: http(ESPACE_RPC) });
  return { address: account.address, kind: "devkey", walletName: "Dev key", client, account };
}

async function ensureChain(client: WalletClient, eth: any) {
  try {
    await client.switchChain({ id: CHAIN_ID });
  } catch (e: any) {
    if (e?.code === 4902 || /Unrecognized|add this chain/i.test(String(e?.message ?? ""))) {
      await eth.request({
        method: "wallet_addEthereumChain",
        params: [
          {
            chainId: `0x${CHAIN_ID.toString(16)}`,
            chainName: DEPLOYMENT.chainName,
            nativeCurrency: { name: "Conflux", symbol: "CFX", decimals: 18 },
            rpcUrls: [ESPACE_RPC],
            blockExplorerUrls: [ESPACE_EXPLORER],
          },
        ],
      });
    } else {
      throw e;
    }
  }
}

const txArgs = (w: Wallet) => ({ account: w.account as any, chain: confluxESpaceChain });

export async function sendDeposit(w: Wallet, custody: Address, amountWei: bigint): Promise<Hex> {
  return w.client.sendTransaction({ ...txArgs(w), to: custody, value: amountWei });
}

export async function burnIcusd(w: Wallet, amountE8s: bigint, vaultId: bigint): Promise<Hex> {
  return w.client.writeContract({
    ...txArgs(w),
    address: ICUSD_CONTRACT,
    abi: ICUSD_ABI,
    functionName: "burn",
    args: [amountE8s, vaultId],
  });
}

/** EIP-1193 code 4001 is the only write failure that proves no transaction was
 * authorized. Every other provider error is ambiguous and must remain locked. */
export function isExplicitWalletRejection(error: unknown): boolean {
  let current: any = error;
  for (let depth = 0; current && depth < 8; depth++) {
    if (current.code === 4001 || current.name === "UserRejectedRequestError") return true;
    current = current.cause;
  }
  return false;
}

export async function icusdBalance(addr: Address): Promise<bigint> {
  return (await publicClient.readContract({
    address: ICUSD_CONTRACT,
    abi: ICUSD_ABI,
    functionName: "balanceOf",
    args: [addr],
  })) as bigint;
}

export async function cfxBalance(addr: Address): Promise<bigint> {
  return publicClient.getBalance({ address: addr });
}

export type TransactionFinality = {
  hash: Hex;
  ok: boolean;
  replacementReason: "repriced" | "replaced" | "cancelled" | null;
};

/** Wait for one wallet-submitted transaction. Repriced transactions retain the
 * action; semantic replacement/cancellation fails closed and requires retry. */
export async function waitForTransactionFinality(hash: Hex): Promise<TransactionFinality> {
  let finalHash = hash;
  let replacementReason: TransactionFinality["replacementReason"] = null;
  const receipt = await publicClient.waitForTransactionReceipt({
    hash,
    confirmations: 1,
    onReplaced: ({ reason, transactionReceipt }) => {
      replacementReason = reason;
      finalHash = transactionReceipt.transactionHash;
    },
  });
  return {
    hash: finalHash,
    ok: receipt.status === "success" && (replacementReason === null || replacementReason === "repriced"),
    replacementReason,
  };
}

// Decimal-string -> base units via integer parsing (no float precision loss).
// Returns 0n for empty/invalid/non-positive input so callers can gate on `=== 0n`.
export function toE8s(s: string): bigint {
  const n = parseFloat(s);
  if (!Number.isFinite(n) || n <= 0) return 0n;
  try { return parseUnits(s as `${number}`, ICUSD_DECIMALS); } catch { return 0n; }
}
export function toWei(s: string): bigint {
  const n = parseFloat(s);
  if (!Number.isFinite(n) || n <= 0) return 0n;
  try { return parseEther(s as `${number}`); } catch { return 0n; }
}

export const fmtCfx = (wei: bigint) => Number(formatEther(wei)).toLocaleString(undefined, { maximumFractionDigits: 4 });
export const fmtIcusd = (e8s: bigint) => Number(formatUnits(e8s, ICUSD_DECIMALS)).toLocaleString(undefined, { maximumFractionDigits: 4 });
export const txUrl = (hash: string) => `${ESPACE_EXPLORER}/tx/${hash}`;
export const addressUrl = (address: string) => `${ESPACE_EXPLORER}/address/${address}`;
export { parseEther };
