/**
 * pointsStore.ts — the viewed wallet's points state. Reset/loaded by the
 * /points page as the principal changes.
 *
 * Loads in two phases so the headline tiles render fast:
 *   1. principal state + exclusion (one round trip), then
 *   2. detail — the audit ledger (per-source/per-epoch history), the epoch
 *      calendar, and the live positions read from the same endpoints the
 *      accrual engine snapshots. Each detail slice degrades independently.
 */
import { writable } from 'svelte/store';
import type { Principal } from '@dfinity/principal';
import type {
  EpochSummary,
  PointEntry,
  PrincipalState,
} from '$declarations/rumi_points/rumi_points.did';
import {
  getEpochHistory,
  getPrincipalPointEntries,
  getPrincipalState,
  isExcluded,
} from '$lib/services/pointsService';
import { fetchLiveInputs } from '$lib/services/pointsLive';
import { buildLivePositions, type LivePositions } from '$lib/utils/pointsBreakdown';
import { toastStore } from '$lib/stores/toast';

export interface MyPointsDetail {
  loading: boolean;
  /** Audit-ledger rows (history). null = failed or reader not deployed. */
  ledger: PointEntry[] | null;
  ledgerFailed: boolean;
  /** Closed-epoch calendar, for the timeline's date column. Empty on failure. */
  epochs: EpochSummary[];
  /** Live positions (what the next snapshot would credit). */
  live: LivePositions | null;
}

export interface MyPointsState {
  loading: boolean;
  loaded: boolean;
  state: PrincipalState | null;
  excluded: boolean;
  error: boolean;
  detail: MyPointsDetail;
}

const emptyDetail: MyPointsDetail = {
  loading: false,
  ledger: null,
  ledgerFailed: false,
  epochs: [],
  live: null,
};

const initial: MyPointsState = {
  loading: false,
  loaded: false,
  state: null,
  excluded: false,
  error: false,
  detail: { ...emptyDetail },
};

function createMyPointsStore() {
  const { subscribe, set, update } = writable<MyPointsState>({ ...initial });
  // Monotonic token: a principal switch mid-flight must not let the stale
  // load's results land on the new principal's view.
  let seq = 0;

  async function load(p: Principal): Promise<void> {
    const mySeq = ++seq;
    update((s) => ({ ...s, loading: true, error: false }));
    let state: PrincipalState | null;
    let excluded: boolean;
    try {
      [state, excluded] = await Promise.all([getPrincipalState(p), isExcluded(p)]);
    } catch (e) {
      if (mySeq !== seq) return;
      console.error('[pointsStore] load failed', e);
      set({ ...initial, loaded: true, error: true });
      toastStore.error('Could not load your points. Tap retry.');
      return;
    }
    if (mySeq !== seq) return;

    const wantsDetail = state !== null && !excluded;
    set({
      loading: false,
      loaded: true,
      state,
      excluded,
      error: false,
      detail: { ...emptyDetail, loading: wantsDetail },
    });
    if (!wantsDetail) return;

    // Phase 2 — independent slices; a failure in one never blanks the others.
    const [ledgerRes, epochsRes, liveInputsRes] = await Promise.allSettled([
      getPrincipalPointEntries(p),
      getEpochHistory(0, 200),
      fetchLiveInputs(p, state),
    ]);
    if (mySeq !== seq) return;

    const ledger = ledgerRes.status === 'fulfilled' ? ledgerRes.value : null;
    if (ledgerRes.status === 'rejected') {
      console.error('[pointsStore] ledger load failed', ledgerRes.reason);
    }
    if (epochsRes.status === 'rejected') {
      console.error('[pointsStore] epoch history load failed', epochsRes.reason);
    }
    // fetchLiveInputs never rejects, but guard anyway.
    const live =
      liveInputsRes.status === 'fulfilled' ? buildLivePositions(liveInputsRes.value) : null;

    update((s) => ({
      ...s,
      detail: {
        loading: false,
        ledger,
        ledgerFailed: ledgerRes.status === 'rejected',
        epochs: epochsRes.status === 'fulfilled' ? epochsRes.value : [],
        live,
      },
    }));
  }

  function reset(): void {
    seq++;
    set({ ...initial, detail: { ...emptyDetail } });
  }

  return { subscribe, load, reset };
}

export const myPointsStore = createMyPointsStore();
