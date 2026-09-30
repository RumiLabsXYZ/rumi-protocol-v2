import { Principal } from '@dfinity/principal';
import { flushSync, tick } from 'svelte';

export const PRINCIPAL_A = Principal.fromUint8Array(Uint8Array.from([1, 1, 1, 1]));
export const PRINCIPAL_B = Principal.fromUint8Array(Uint8Array.from([2, 2, 2, 2]));
export const PRINCIPAL_A_TEXT = PRINCIPAL_A.toText();
export const PRINCIPAL_B_TEXT = PRINCIPAL_B.toText();
export const CKBTC_LEDGER_TEXT = 'mxzaz-hqaaa-aaaar-qaada-cai';

export function fakeCollateralInfo(overrides: Record<string, unknown> = {}) {
  return { principal: CKBTC_LEDGER_TEXT, symbol: 'ckBTC', price: 60_000, liquidationCr: 1.2, minimumCr: 1.35,
    borrowingFee: 0.005, debtCeiling: 1_000_000, ledgerFee: 10, status: 'Active', ...overrides };
}
export function fakeRawVault(vaultId: number, collateralAmount: bigint, borrowedIcusd: bigint) {
  return { vault_id: BigInt(vaultId), collateral_type: { toText: () => CKBTC_LEDGER_TEXT }, collateral_amount: collateralAmount, borrowed_icusd_amount: borrowedIcusd };
}
export function fakeMinterInfo() { return { min_confirmations: 4, deposit_btc_min_amount: [10_000n], retrieve_btc_min_amount: 10_000n }; }
export function fakeMinted(satoshi = 1_000_000n, blockIndex = 7n) {
  return { Ok: [{ Minted: { block_index: blockIndex, minted_amount: satoshi } }] };
}
export function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => { resolve = res; });
  return { promise, resolve };
}
export async function settle(rounds = 12) {
  for (let i = 0; i < rounds; i++) { await Promise.resolve(); await tick(); flushSync(); }
}
export function setInputValue(input: HTMLInputElement, value: string) {
  Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!.call(input, value);
  input.dispatchEvent(new Event('input', { bubbles: true }));
}
