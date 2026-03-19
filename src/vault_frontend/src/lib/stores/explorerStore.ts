import { writable } from 'svelte/store';
import { publicActor } from '$lib/services/protocol/apiClient';

const PAGE_SIZE = 100;

// Events
export const explorerEvents = writable<any[]>([]);
export const explorerEventsLoading = writable(false);
export const explorerEventsPage = writable(0);
export const explorerEventsTotalCount = writable(0);

// Snapshots
export const protocolSnapshots = writable<any[]>([]);
export const snapshotsLoading = writable(false);

// All vaults cache
export const allVaults = writable<any[]>([]);
export const allVaultsLoading = writable(false);

export async function fetchEvents(page: number = 0) {
  explorerEventsLoading.set(true);
  try {
    const totalCount = Number(await publicActor.get_event_count());
    explorerEventsTotalCount.set(totalCount);

    if (totalCount === 0) {
      explorerEvents.set([]);
      return;
    }

    // Fetch newest first: calculate start index from the end
    const start = Math.max(0, totalCount - ((page + 1) * PAGE_SIZE));
    const length = Math.min(PAGE_SIZE, totalCount - (page * PAGE_SIZE));

    const events = await publicActor.get_events({
      start: BigInt(start),
      length: BigInt(length)
    });
    // Reverse to show newest first
    explorerEvents.set([...events].reverse());
    explorerEventsPage.set(page);
  } catch (e) {
    console.error('Failed to fetch events:', e);
  } finally {
    explorerEventsLoading.set(false);
  }
}

export async function fetchSnapshots() {
  snapshotsLoading.set(true);
  try {
    const count = Number(await publicActor.get_snapshot_count());
    if (count === 0) {
      protocolSnapshots.set([]);
      return;
    }
    // Fetch all snapshots (hourly, so even a year is only ~8760)
    const batchSize = 2000;
    const allSnaps: any[] = [];
    for (let i = 0; i < count; i += batchSize) {
      const batch = await publicActor.get_protocol_snapshots({
        start: BigInt(i),
        length: BigInt(Math.min(batchSize, count - i))
      });
      allSnaps.push(...batch);
    }
    protocolSnapshots.set(allSnaps);
  } catch (e) {
    console.error('Failed to fetch snapshots:', e);
  } finally {
    snapshotsLoading.set(false);
  }
}

export async function fetchAllVaults() {
  allVaultsLoading.set(true);
  try {
    const vaults = await publicActor.get_all_vaults();
    allVaults.set(vaults);
    return vaults;
  } catch (e) {
    console.error('Failed to fetch all vaults:', e);
    return [];
  } finally {
    allVaultsLoading.set(false);
  }
}

export async function fetchVaultHistory(vaultId: number) {
  try {
    return await publicActor.get_vault_history(BigInt(vaultId));
  } catch (e) {
    console.error(`Failed to fetch history for vault #${vaultId}:`, e);
    return [];
  }
}

export async function fetchVaultsByOwner(principal: any) {
  try {
    return await publicActor.get_vaults([principal]);
  } catch (e) {
    console.error('Failed to fetch vaults by owner:', e);
    return [];
  }
}

export function getEventsTotalPages(): number {
  let total = 0;
  explorerEventsTotalCount.subscribe(v => total = v)();
  return Math.ceil(total / PAGE_SIZE);
}

export { PAGE_SIZE };
