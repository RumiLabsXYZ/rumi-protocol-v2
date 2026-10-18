/** ckSOL uses SOL's 9-decimal lamports. Never route token amounts through Number. */
export const LAMPORTS_PER_SOL = 1_000_000_000n;
export const NAT64_MAX = 18_446_744_073_709_551_615n;
export function parseSol(value: string): bigint {
  if (!/^(?:\d{1,20}(?:\.\d{0,9})?|\.\d{1,9})$/.test(value)) throw new Error('Enter a SOL amount with up to 9 decimal places.');
  const [whole, fraction = ''] = value.split('.');
  const amount = BigInt(whole || '0') * LAMPORTS_PER_SOL + BigInt(fraction.padEnd(9, '0'));
  if (amount <= 0n || amount > NAT64_MAX) throw new Error('Amount is outside the supported SOL range.');
  return amount;
}
export function formatSol(value: bigint): string {
  const fraction = (value % LAMPORTS_PER_SOL).toString().padStart(9, '0').replace(/0+$/, '');
  return `${value / LAMPORTS_PER_SOL}${fraction ? `.${fraction}` : ''}`;
}
export function validateSolanaAddress(address: string): boolean {
  if (address.length < 32 || address.length > 44) return false;
  const alphabet = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
  let value = 0n;
  for (const char of address) {
    const digit = alphabet.indexOf(char);
    if (digit < 0) return false;
    value = value * 58n + BigInt(digit);
  }
  let length = 0;
  for (; value > 0n; value >>= 8n) length++;
  for (const char of address) { if (char !== '1') break; length++; }
  return length === 32;
}
export interface StatusView { kind?: string; label: string; terminal: boolean; success: boolean; signature?: string }
export function depositView(status: any): StatusView {
  const state = Object.keys(status)[0];
  const signature = status[state]?.signature;
  const label: Record<string, string> = {
    NotFound: 'Deposit ID not found. Keep the ID and check again; do not submit another deposit.',
    Queued: 'Deposit queued for a sweep.', Swept: 'Sweep submitted to Solana.',
    Finalized: 'Sweep finalized; ckSOL mint is pending.',
    Minted: 'ckSOL minted to your ICP account.',
    Dropped: 'Sweep dropped. Check the deposit address before explicitly queuing it again.',
    Quarantined: 'Deposit quarantined by the official minter. Keep the deposit ID and contact minter support.',
  };
  return { kind: state, label: label[state] ?? 'Unrecognized deposit status. Keep your deposit ID and contact support.',
    terminal: ['Minted', 'Dropped', 'Quarantined', 'NotFound'].includes(state) || !label[state], success: state === 'Minted', signature };
}
export function withdrawalView(status: any): StatusView {
  if ('TxFinalized' in status) {
    if ('Success' in status.TxFinalized) return { label: 'SOL withdrawal finalized successfully.', terminal: true, success: true, signature: status.TxFinalized.Success.transaction_id };
    if ('Failure' in status.TxFinalized) return { label: 'SOL transaction finalized with a failure. Keep the burn block and contact minter support; no automatic reimbursement is assumed.', terminal: true, success: false, signature: status.TxFinalized.Failure.transaction_id };
  }
  if ('Pending' in status) return { label: 'Withdrawal accepted; awaiting Solana submission.', terminal: false, success: false };
  if ('TxSent' in status) return { label: 'Withdrawal sent; awaiting Solana finality.', terminal: false, success: false, signature: status.TxSent.transaction_id };
  return { label: 'Withdrawal status unavailable. Keep the burn block and check again; do not repeat the withdrawal.', terminal: true, success: false };
}
export function describeCksolError(error: unknown): string {
  return JSON.stringify(error, (_key, value) => typeof value === 'bigint' ? value.toString() : value);
}
