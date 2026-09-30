import { Principal } from '@dfinity/principal';
import { flushSync, tick } from 'svelte';

export const OWNER_A = Principal.fromUint8Array(Uint8Array.from([1, 1, 1, 1]));
export const OWNER_B = Principal.fromUint8Array(Uint8Array.from([2, 2, 2, 2]));
export const OWNER_A_TEXT = OWNER_A.toText();
export const OWNER_B_TEXT = OWNER_B.toText();
export const BITCOIN_ADDRESS = '1BoatSLRHtKNngkdXEeobR76b53LETtpyT';
export function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => { resolve = res; });
  return { promise, resolve };
}
export async function settle(rounds = 12) {
  for (let i = 0; i < rounds; i++) { await Promise.resolve(); await tick(); flushSync(); }
}
export function inputValue(input: HTMLInputElement, value: string) {
  Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')!.set!.call(input, value);
  input.dispatchEvent(new Event('input', { bubbles: true }));
}
