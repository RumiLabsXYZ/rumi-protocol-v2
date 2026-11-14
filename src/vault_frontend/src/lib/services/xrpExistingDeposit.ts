import type { _SERVICE, ForwardFilteredEventsResponse, XrpVaultDepositAddress } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did';
import { idlFactory } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import { get } from 'svelte/store';
import { CONFIG } from '../config';
import { walletStore } from '../stores/wallet';
import { ApiClient, publicActor } from './protocol/apiClient';

export function requireXrpDepositSession(owner: string): void {
  if (!owner || get(walletStore).principal?.toText() !== owner) {
    throw new Error('Wallet changed. Reopen Deposit with the vault owner wallet.');
  }
}

export function normalizeXrpDepositHash(value: string): string {
  const hash = value.trim().toUpperCase();
  if (!/^[0-9A-F]{64}$/.test(hash)) throw new Error('Enter the 64-character XRP transaction hash.');
  return hash;
}

export async function copyXrpVaultAddress(
  owner: string, address: string,
  write: (value: string) => Promise<void> = (value) => navigator.clipboard.writeText(value)
): Promise<void> {
  // Store reads are synchronous, including before Svelte has flushed an account
  // change to the component. Never copy an address from the previous session.
  requireXrpDepositSession(owner);
  await write(address);
  requireXrpDepositSession(owner);
}

/** One bounded public scan. The returned index is a hint, verified again by the backend. */
export async function findXrpVaultOrigin(
  vaultId: number, owner: string, start = 0n,
  read: (...args: Parameters<_SERVICE['get_events_forward_filtered']>) => Promise<ForwardFilteredEventsResponse> = publicActor.get_events_forward_filtered
): Promise<{ index?: bigint; next: bigint; end: boolean }> {
  requireXrpDepositSession(owner);
  const page: ForwardFilteredEventsResponse = await read(start, 2000n, [[{ OpenVault: null }]]);
  requireXrpDepositSession(owner);
  if (page.next_start < start || (!page.reached_end && page.next_start <= start)) {
    throw new Error('Vault history did not advance. Try again later.');
  }
  const opening = page.events.find(([, event]) => 'open_vault' in event
    && event.open_vault.vault.vault_id === BigInt(vaultId)
    && event.open_vault.vault.owner.toText() === owner);
  return { index: opening?.[0], next: page.next_start, end: page.reached_end };
}

async function actor(owner: string): Promise<_SERVICE> {
  requireXrpDepositSession(owner);
  const backend = await walletStore.getActor(CONFIG.currentCanisterId, idlFactory) as _SERVICE;
  requireXrpDepositSession(owner);
  return backend;
}

export async function loadXrpVaultAddress(
  vaultId: number, owner: string, origin?: bigint
): Promise<XrpVaultDepositAddress> {
  return ApiClient.executeSequentialOperation(async () => {
    const backend = await actor(owner);
    const response = await backend.get_xrp_vault_deposit_address(BigInt(vaultId), origin === undefined ? [] : [origin]);
    requireXrpDepositSession(owner);
    if ('Err' in response) throw new Error(ApiClient.formatProtocolError(response.Err));
    return response.Ok;
  });
}

/** Explicit owner action. Lost replies keep the same hash for an idempotent retry. */
export async function confirmXrpVaultTopup(
  vaultId: number, owner: string, txHash: string, origin?: bigint
): Promise<bigint> {
  const hash = normalizeXrpDepositHash(txHash);
  return ApiClient.executeSequentialOperation(async () => {
    const backend = await actor(owner);
    const response = await backend.confirm_xrp_vault_topup(BigInt(vaultId), hash, origin === undefined ? [] : [origin]);
    requireXrpDepositSession(owner);
    if ('Err' in response) throw new Error(ApiClient.formatProtocolError(response.Err));
    return response.Ok;
  });
}
