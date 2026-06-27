// viem integration: EIP-6963 injected wallets (Rabby, MetaMask, ...) or an
// in-browser dev-key signer (the path the staging round-trip used), the CFX
// deposit, the IcUSD.burn repay, and reads.

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
  confluxESpaceTestnet,
} from "./config";

export const publicClient = createPublicClient({
  chain: confluxESpaceTestnet,
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
  const client = createWalletClient({ chain: confluxESpaceTestnet, transport: custom(eth) });
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
  const client = createWalletClient({ chain: confluxESpaceTestnet, transport: custom(eth) });
  const [address] = await client.requestAddresses();
  await ensureChain(client, eth);
  const name = eth.isRabby ? "Rabby" : eth.isMetaMask ? "MetaMask" : "Injected wallet";
  return { address, kind: "injected", walletName: name, client, account: address };
}

export function connectDevKey(pk: string): Wallet {
  const hex = (pk.startsWith("0x") ? pk : `0x${pk}`) as Hex;
  const account = privateKeyToAccount(hex);
  const client = createWalletClient({ account, chain: confluxESpaceTestnet, transport: http(ESPACE_RPC) });
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
            chainId: "0x47", // 71
            chainName: "Conflux eSpace Testnet",
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

const txArgs = (w: Wallet) => ({ account: w.account as any, chain: confluxESpaceTestnet });

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
export { parseEther };
