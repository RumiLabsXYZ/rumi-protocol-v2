/**
 * pointsStore.ts — the connected wallet's points state. Reset/loaded by the
 * /points page as the principal changes.
 */
import { writable } from 'svelte/store';
import type { Principal } from '@dfinity/principal';
import type { PrincipalState } from '$declarations/rumi_points/rumi_points.did';
import { getPrincipalState, isExcluded } from '$lib/services/pointsService';
import { toastStore } from '$lib/stores/toast';

export interface MyPointsState {
  loading: boolean;
  loaded: boolean;
  state: PrincipalState | null;
  excluded: boolean;
  error: boolean;
}

const initial: MyPointsState = {
  loading: false,
  loaded: false,
  state: null,
  excluded: false,
  error: false,
};

function createMyPointsStore() {
  const { subscribe, set, update } = writable<MyPointsState>({ ...initial });

  async function load(p: Principal): Promise<void> {
    update((s) => ({ ...s, loading: true, error: false }));
    try {
      const [state, excluded] = await Promise.all([getPrincipalState(p), isExcluded(p)]);
      set({ loading: false, loaded: true, state, excluded, error: false });
    } catch (e) {
      console.error('[pointsStore] load failed', e);
      set({ ...initial, loaded: true, error: true });
      toastStore.error('Could not load your points. Tap retry.');
    }
  }

  function reset(): void {
    set({ ...initial });
  }

  return { subscribe, load, reset };
}

export const myPointsStore = createMyPointsStore();
