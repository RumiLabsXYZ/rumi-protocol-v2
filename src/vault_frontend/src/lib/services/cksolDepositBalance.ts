import { validateSolanaAddress } from '../utils/cksolMinterFlow';

// Advisory display only. The official minter independently reads the address
// and enforces its minimum; this RPC never authorizes or submits a transaction.
export async function readSolDepositBalance(address: string): Promise<bigint> {
  if (!validateSolanaAddress(address)) throw new Error('Invalid SOL deposit address.');
  const response = await fetch('https://solana-rpc.publicnode.com', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'getBalance', params: [address, { commitment: 'finalized' }] }),
    signal: AbortSignal.timeout(8_000)
  });
  if (!response.ok) throw new Error('SOL balance check is temporarily unavailable.');
  const reply = await response.json();
  const value = reply?.result?.value;
  if (reply.error || !Number.isSafeInteger(value) || value < 0)
    throw new Error('SOL balance check returned an invalid response.');
  return BigInt(value);
}
