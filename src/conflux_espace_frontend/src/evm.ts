// viem integration: MetaMask (primary) or an in-browser dev-key signer (the path
// the staging round-trip used), the CFX deposit, the IcUSD.burn repay, and reads.

import {
  createPublicClient,
  createWalletClient,
  custom,
  http,
  parseEther,
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
  kind: "metamask" | "devkey";
  client: WalletClient;
  account: Address | PrivateKeyAccount;
}

export function hasMetaMask(): boolean {
  return typeof (window as any).ethereum !== "undefined";
}

export async function connectMetaMask(): Promise<Wallet> {
  const eth = (window as any).ethereum;
  if (!eth) throw new Error("No injected wallet. Install MetaMask, or use the dev-key signer below.");
  const client = createWalletClient({ chain: confluxESpaceTestnet, transport: custom(eth) });
  const [address] = await client.requestAddresses();
  await ensureChain(client, eth);
  return { address, kind: "metamask", client, account: address };
}

export function connectDevKey(pk: string): Wallet {
  const hex = (pk.startsWith("0x") ? pk : `0x${pk}`) as Hex;
  const account = privateKeyToAccount(hex);
  const client = createWalletClient({ account, chain: confluxESpaceTestnet, transport: http(ESPACE_RPC) });
  return { address: account.address, kind: "devkey", client, account };
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

export const fmtCfx = (wei: bigint) => Number(formatEther(wei)).toLocaleString(undefined, { maximumFractionDigits: 4 });
export const fmtIcusd = (e8s: bigint) => Number(formatUnits(e8s, ICUSD_DECIMALS)).toLocaleString(undefined, { maximumFractionDigits: 4 });
export const txUrl = (hash: string) => `${ESPACE_EXPLORER}/tx/${hash}`;
export { parseEther };
