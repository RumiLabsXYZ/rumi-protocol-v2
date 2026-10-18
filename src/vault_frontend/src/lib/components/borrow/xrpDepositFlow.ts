/**
 * Click-to-deposit controller for native XRP vaults (Task 1).
 *
 * Pure TypeScript (no Svelte) so every money/authority rule is unit-testable:
 *
 *  - The vault is opened once; a reload recovers the same pending vault from a
 *    local hint cross-checked against the backend's pending list.
 *  - The Payment is built only from a FROZEN snapshot (connected account,
 *    network, backend custody address, integer drops). If the wallet account or
 *    network changes after the proposal, the proposal is invalidated.
 *  - A wallet hash never credits collateral. Browser XRPL reads are a readiness
 *    HINT; the backend `confirm_xrp_deposit` result is the only authority.
 *  - Oisy (gesture-gated signer) confirms ONLY from a click; delegated sessions
 *    confirm once automatically. No timer ever calls an authenticated update.
 */

import { WALLET_TYPES, currentWalletType } from '$lib/services/auth';
import { get } from 'svelte/store';
import { XrpVaultService, type XrpOpResult, type XrpVaultOpenView } from '$lib/services/xrpVaultService';
import {
  expectedXrplNetwork,
  getXrplWalletAdapter,
  type XrplConnection,
  type XrplWalletAdapter,
  type XrplPaymentTx,
  type WalletSubmission,
} from '$lib/services/xrplWalletService';

// --- exact breakdown ----------------------------------------------------

export interface XrpDepositBreakdown {
  sentDrops: bigint;
  reservedDrops: bigint;
  /** What the backend is expected to credit: sent minus the account reserve. */
  expectedCreditedDrops: bigint;
  viable: boolean;
}

export function xrpDepositBreakdown(sentDrops: bigint, reserveDrops: bigint): XrpDepositBreakdown {
  const expected = sentDrops > reserveDrops ? sentDrops - reserveDrops : 0n;
  return { sentDrops, reservedDrops: reserveDrops, expectedCreditedDrops: expected, viable: sentDrops > reserveDrops };
}

// --- public XRPL JSON-RPC readiness hint ----------------------------------

export const DEFAULT_XRPL_RPC_ENDPOINTS: Record<string, string[]> = {
  testnet: ['https://s.altnet.rippletest.net:51234'],
  mainnet: ['https://xrplcluster.com', 'https://s1.ripple.com:51234'],
  devnet: ['https://s.devnet.rippletest.net:51234'],
};

export function xrplRpcEndpoints(network: string): string[] {
  const override = String((import.meta as any).env?.VITE_XRPL_RPC_ENDPOINTS ?? '')
    .split(',')
    .map((s: string) => s.trim())
    .filter((s: string) => /^https:\/\//.test(s));
  return override.length ? override : DEFAULT_XRPL_RPC_ENDPOINTS[network] ?? DEFAULT_XRPL_RPC_ENDPOINTS.testnet;
}

/**
 * Balance (drops) of `address` at the latest VALIDATED ledger, or 0 for an
 * unfunded account. Throws if no endpoint produced a validated answer.
 */
export async function fetchValidatedBalanceDrops(
  address: string,
  endpoints: string[],
  fetchImpl: typeof fetch = fetch
): Promise<bigint> {
  let lastError: unknown = new Error('No XRPL endpoint configured.');
  for (const url of endpoints) {
    try {
      const res = await fetchImpl(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ method: 'account_info', params: [{ account: address, ledger_index: 'validated' }] }),
      });
      if (!res.ok) throw new Error(`XRPL RPC HTTP ${res.status}`);
      const result = (await res.json())?.result;
      if (result?.status === 'success' && result.validated === true && typeof result.account_data?.Balance === 'string') {
        return BigInt(result.account_data.Balance);
      }
      if (result?.error === 'actNotFound') return 0n;
      throw new Error(`XRPL RPC unusable result: ${result?.error ?? 'unvalidated'}`);
    } catch (e) {
      lastError = e;
    }
  }
  throw lastError instanceof Error ? lastError : new Error('XRPL RPC failed');
}

// --- controller -----------------------------------------------------------

export type XrpDepositPhase =
  | 'idle'
  | 'opening'
  | 'awaiting_payment'
  | 'signing'
  | 'wallet_pending'
  | 'polling'
  | 'ready_to_confirm'
  | 'confirming'
  | 'confirmed'
  | 'rejected'
  | 'invalidated'
  | 'timed_out'
  | 'error';

export interface XrpDepositProposal {
  vaultId: number;
  destination: string;
  sendDrops: bigint;
  account: string;
  network: string;
}

export interface XrpDepositState {
  phase: XrpDepositPhase;
  vault: XrpVaultOpenView | null;
  proposal: XrpDepositProposal | null;
  sentDrops: bigint | null;
  reservedDrops: bigint | null;
  expectedCreditedDrops: bigint | null;
  /** Filled ONLY from the backend confirm result. */
  creditedDrops: bigint | null;
  alreadyConfirmed: boolean;
  /** True when a signer false-negative was recovered: confirmed, credited figure unknown. */
  oisyResilient: boolean;
  txHash: string | null;
  requestId: string | null;
  error: string | null;
  /** True when the backend pending-deposit check failed: nothing was opened; retry via prepare(). */
  pendingReadFailed: boolean;
}

/** Tri-state: a failed read is never treated as "no pending deposits". */
export type PendingDepositsResult =
  | { ok: true; deposits: Array<{ vaultId: number; custodyAddress: string; reserveBaseDrops: bigint; openedAtMs?: number }> }
  | { ok: false; error: string };

export interface XrpDepositVaultApi {
  open(): Promise<XrpOpResult<XrpVaultOpenView>>;
  /** Backend pending deposits, unfiltered by user-hidden ids. */
  readPendingDeposits(): Promise<PendingDepositsResult>;
  confirm(vaultId: number): Promise<XrpOpResult<{ creditedDrops: bigint }>>;
  ownsVault(vaultId: number): Promise<boolean>;
}

export interface XrpDepositDeps {
  wallet: XrplWalletAdapter;
  vault: XrpDepositVaultApi;
  /** True for gesture-gated signer logins (Oisy): confirm only from a click. */
  isOisy: () => boolean;
  network: string;
  ownerKey: string;
  storage?: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'> | null;
  endpoints?: string[];
  fetchImpl?: typeof fetch;
  sleep?: (ms: number) => Promise<void>;
  now?: () => number;
  minCollateralDrops?: bigint;
}

interface DepositHint {
  vaultId: number;
  custodyAddress: string;
  reserveBaseDrops: string;
  sendDrops: string;
  requestId?: string;
  hash?: string;
  confirmed?: boolean;
  /** Set the moment a wallet request is about to be created: the amount is then committed. */
  payStarted?: boolean;
}

const HINT_PREFIX = 'rumi_xrpl_deposit_hint_v1:';
const POLL_START_MS = 2000;
const POLL_CAP_MS = 15_000;
const POLL_STOP_MS = 120_000;
const NO_PENDING = /no pending xrp deposit/i;

export interface XrpDepositController {
  getState(): XrpDepositState;
  subscribe(fn: (s: XrpDepositState) => void): () => void;
  prepare(sendDrops: bigint): Promise<void>;
  propose(): void;
  pay(): Promise<void>;
  resume(): Promise<void>;
  confirmFromClick(): Promise<void>;
  /** Explicit user click on the manual "I've sent the XRP" path (any pre-confirm phase). */
  confirmNow(): Promise<void>;
  dispose(): void;
}

export function createXrpDepositController(deps: XrpDepositDeps): XrpDepositController {
  const storage = deps.storage === undefined ? safeLocalStorage() : deps.storage;
  const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const now = deps.now ?? (() => Date.now());
  const minCollateral = deps.minCollateralDrops ?? 1n;
  const hintKey = `${HINT_PREFIX}${deps.ownerKey}`;

  let state: XrpDepositState = {
    phase: 'idle', vault: null, proposal: null, sentDrops: null, reservedDrops: null,
    expectedCreditedDrops: null, creditedDrops: null, alreadyConfirmed: false, oisyResilient: false,
    txHash: null, requestId: null, error: null, pendingReadFailed: false,
  };
  const listeners = new Set<(s: XrpDepositState) => void>();
  let disposed = false;
  let preparing: Promise<void> | null = null;
  let confirmInFlight = false;
  let pollRun = 0;
  let desiredSend: bigint | null = null;
  /** Amount already committed to a wallet request for this vault (survives reload via the hint). */
  let committedSend: bigint | null = null;

  function set(patch: Partial<XrpDepositState>) {
    state = { ...state, ...patch };
    listeners.forEach((l) => l(state));
  }

  function readHint(): DepositHint | null {
    try {
      const raw = storage?.getItem(hintKey);
      const h = raw ? JSON.parse(raw) : null;
      return h && Number.isFinite(h.vaultId) && typeof h.custodyAddress === 'string' ? h : null;
    } catch {
      return null;
    }
  }
  function writeHint(patch: Partial<DepositHint> | null) {
    try {
      if (patch === null) { storage?.removeItem(hintKey); return; }
      const cur = readHint();
      const next = { ...(cur ?? {}), ...patch };
      storage?.setItem(hintKey, JSON.stringify(next));
    } catch { /* hints only */ }
  }

  const unsubscribeWallet = deps.wallet.onChange((conn) => {
    // Only a not-yet-submitted proposal is invalidated; an in-flight/sent payment
    // is tracked by its request id regardless of who is connected now.
    if (state.proposal && state.phase === 'awaiting_payment' && !sameConnection(conn, state.proposal)) {
      set({ phase: 'invalidated', proposal: null, error: 'Wallet account or network changed. Review and propose the deposit again.' });
    }
  });

  function sameConnection(conn: XrplConnection | null, p: XrpDepositProposal) {
    return !!conn && conn.address === p.account && conn.network === p.network;
  }

  function applyVault(vault: XrpVaultOpenView, sendDrops: bigint, extra: Partial<XrpDepositState> = {}) {
    const b = xrpDepositBreakdown(sendDrops, vault.reserveBaseDrops);
    const viable = b.viable && b.expectedCreditedDrops >= minCollateral;
    set({
      vault,
      sentDrops: null,
      reservedDrops: b.reservedDrops,
      expectedCreditedDrops: b.expectedCreditedDrops,
      phase: viable ? 'awaiting_payment' : 'error',
      error: viable ? null : 'The amount must exceed the XRPL account reserve plus the minimum collateral deposit.',
      ...extra,
    });
  }

  function parseDrops(text: string | undefined, fallback: bigint): bigint {
    try { return text ? BigInt(text) : fallback; } catch { return fallback; }
  }

  /**
   * Adopt a pending deposit from the BACKEND list. The custody address and
   * reserve always come from the backend, never from the local hint; the hint
   * only contributes the wallet-request bookkeeping for the same vault.
   */
  function adoptPending(
    p: { vaultId: number; custodyAddress: string; reserveBaseDrops: bigint },
    hint: DepositHint | null,
    sendDrops: bigint
  ) {
    const vault: XrpVaultOpenView = { vaultId: p.vaultId, custodyAddress: p.custodyAddress, reserveBaseDrops: p.reserveBaseDrops };
    const same = !!hint && hint.vaultId === p.vaultId;
    const requested = same && !!(hint!.requestId || hint!.hash);
    const committed = same && (requested || !!hint!.payStarted);
    const send = committed ? parseDrops(hint!.sendDrops, sendDrops) : sendDrops;
    committedSend = committed ? send : null;
    writeHint({
      vaultId: vault.vaultId,
      custodyAddress: vault.custodyAddress,
      reserveBaseDrops: vault.reserveBaseDrops.toString(),
      sendDrops: send.toString(),
      requestId: same ? hint!.requestId : undefined,
      hash: same ? hint!.hash : undefined,
      payStarted: committed ? true : undefined,
      confirmed: false,
    });
    applyVault(vault, send, { requestId: requested ? hint!.requestId ?? null : null, txHash: requested ? hint!.hash ?? null : null });
    if (state.phase === 'awaiting_payment' && requested) {
      set({ phase: 'wallet_pending', sentDrops: send });
    }
  }

  async function doPrepare(sendDrops: bigint) {
    desiredSend = sendDrops;
    if (state.vault) {
      // Already prepared: idempotent (never a second open).
      return;
    }
    set({ phase: 'opening', error: null, pendingReadFailed: false });
    const hint = readHint();

    if (hint?.confirmed) {
      const vault = hintVault(hint);
      set({ vault, phase: 'confirmed', alreadyConfirmed: true, txHash: hint.hash ?? null, requestId: hint.requestId ?? null,
        reservedDrops: vault.reserveBaseDrops, creditedDrops: null });
      return;
    }

    // ALWAYS consult the backend before opening: a hint can be missing or stale
    // and a failed read must never be mistaken for "nothing pending".
    let read: PendingDepositsResult;
    try {
      read = await deps.vault.readPendingDeposits();
    } catch (e) {
      read = { ok: false, error: e instanceof Error ? e.message : String(e) };
    }
    if (!read.ok) {
      set({
        phase: 'error',
        pendingReadFailed: true,
        error: `Could not check for an existing XRP deposit address (${read.error}). Nothing was opened. Try again.`,
      });
      return;
    }

    const pending = read.deposits;
    const hinted = hint ? pending.find((p) => p.vaultId === hint.vaultId) : undefined;
    if (hinted) {
      adoptPending(hinted, hint, sendDrops);
      return;
    }

    if (hint) {
      // Not pending any more: either confirmed elsewhere or gone.
      let owned = false;
      try {
        owned = await deps.vault.ownsVault(hint.vaultId);
      } catch (e) {
        // Cannot tell: keep the hint and do not open another vault.
        set({
          phase: 'error',
          pendingReadFailed: true,
          error: `Could not check whether your earlier XRP vault was already created (${e instanceof Error ? e.message : String(e)}). Nothing was opened. Try again.`,
        });
        return;
      }
      if (owned) {
        const vault = hintVault(hint);
        writeHint({ confirmed: true });
        set({ vault, phase: 'confirmed', alreadyConfirmed: true, reservedDrops: vault.reserveBaseDrops,
          txHash: hint.hash ?? null, requestId: hint.requestId ?? null });
        return;
      }
      writeHint(null);
    }

    if (pending.length > 0) {
      // An existing pending deposit (even one the user hid or lost the hint for)
      // is reused instead of opening a second vault.
      const newest = [...pending].sort((a, b) => (b.openedAtMs ?? 0) - (a.openedAtMs ?? 0) || b.vaultId - a.vaultId)[0];
      adoptPending(newest, null, sendDrops);
      return;
    }

    const opened = await deps.vault.open();
    if (!opened.success || !opened.data) {
      set({ phase: 'error', error: opened.error ?? 'Could not reserve an XRP custody address.' });
      return;
    }
    committedSend = null;
    writeHint({
      vaultId: opened.data.vaultId,
      custodyAddress: opened.data.custodyAddress,
      reserveBaseDrops: opened.data.reserveBaseDrops.toString(),
      sendDrops: sendDrops.toString(),
      requestId: undefined, hash: undefined, payStarted: undefined, confirmed: false,
    });
    applyVault(opened.data, sendDrops);
  }

  function hintVault(h: DepositHint): XrpVaultOpenView {
    return { vaultId: h.vaultId, custodyAddress: h.custodyAddress, reserveBaseDrops: BigInt(h.reserveBaseDrops) };
  }

  function handleSubmission(sub: WalletSubmission, sendDrops: bigint) {
    set({ requestId: sub.requestId, txHash: sub.hash });
    if (sub.status === 'rejected') {
      writeHint({ requestId: undefined, hash: undefined, payStarted: undefined });
      committedSend = null;
      set({ phase: 'rejected', proposal: null, error: sub.warning ?? 'The wallet request was rejected or expired. Nothing was sent and no collateral was credited.' });
      return Promise.resolve();
    }
    writeHint({ requestId: sub.requestId, hash: sub.hash ?? undefined, sendDrops: sendDrops.toString() });
    if (sub.status === 'pending' || !sub.hash) {
      set({ phase: 'wallet_pending', sentDrops: sendDrops, error: null });
      return Promise.resolve();
    }
    set({ sentDrops: sendDrops, error: null });
    return startPolling();
  }

  async function startPolling() {
    const vault = state.vault;
    if (!vault) return;
    const run = ++pollRun;
    const endpoints = deps.endpoints ?? xrplRpcEndpoints(deps.network);
    const needed = vault.reserveBaseDrops + minCollateral;
    const started = now();
    let delay = POLL_START_MS;
    set({ phase: 'polling', error: null });
    while (!disposed && run === pollRun) {
      try {
        const balance = await fetchValidatedBalanceDrops(vault.custodyAddress, endpoints, deps.fetchImpl ?? fetch);
        if (disposed || run !== pollRun) return;
        if (balance >= needed) {
          // Browser balance is a hint: it only enables the confirm step.
          if (deps.isOisy()) {
            set({ phase: 'ready_to_confirm' });
          } else {
            await doConfirm();
          }
          return;
        }
      } catch { /* transient RPC trouble: keep backing off */ }
      if (now() - started >= POLL_STOP_MS) break;
      await sleep(delay);
      delay = Math.min(delay * 2, POLL_CAP_MS);
      if (now() - started >= POLL_STOP_MS) break;
    }
    if (!disposed && run === pollRun) set({ phase: 'timed_out' });
  }

  async function doConfirm() {
    const vault = state.vault;
    if (!vault || confirmInFlight || state.phase === 'confirmed') return;
    confirmInFlight = true;
    set({ phase: 'confirming', error: null });
    try {
      const res = await deps.vault.confirm(vault.vaultId);
      if (res.success) {
        writeHint({ confirmed: true });
        set({
          phase: 'confirmed',
          // A signer false-negative recovery has no credited figure; never invent one.
          creditedDrops: res.oisyResilient ? null : res.data?.creditedDrops ?? null,
          oisyResilient: res.oisyResilient ?? false,
        });
        return;
      }
      const message = res.error ?? 'Deposit not confirmed yet.';
      if (NO_PENDING.test(message)) {
        let owned = false;
        try { owned = await deps.vault.ownsVault(vault.vaultId); } catch { /* unknown */ }
        if (owned) {
          writeHint({ confirmed: true });
          set({ phase: 'confirmed', alreadyConfirmed: true, creditedDrops: null });
          return;
        }
      }
      set({ phase: 'ready_to_confirm', error: message });
    } catch (e) {
      set({ phase: 'ready_to_confirm', error: e instanceof Error ? e.message : String(e) });
    } finally {
      confirmInFlight = false;
    }
  }

  const controller: XrpDepositController = {
    getState: () => state,
    subscribe(fn) {
      listeners.add(fn);
      fn(state);
      return () => { listeners.delete(fn); };
    },
    prepare(sendDrops) {
      preparing ??= doPrepare(sendDrops).finally(() => { preparing = null; });
      return preparing;
    },
    propose() {
      if (!state.vault || !['awaiting_payment', 'invalidated', 'rejected'].includes(state.phase)) return;
      // An amount already committed to a wallet request wins over a re-typed one.
      const sendDrops = committedSend ?? desiredSend ?? 0n;
      const b = xrpDepositBreakdown(sendDrops, state.vault.reserveBaseDrops);
      if (!(b.viable && b.expectedCreditedDrops >= minCollateral)) {
        set({ phase: 'error', error: 'The amount must exceed the XRPL account reserve plus the minimum collateral deposit.' });
        return;
      }
      const conn = deps.wallet.getConnection();
      if (!conn) { set({ error: 'Connect an XRPL wallet first.' }); return; }
      if (conn.network !== deps.network) {
        set({ error: `Wallet is on ${conn.network}; Rumi expects ${deps.network}.` });
        return;
      }
      set({
        phase: 'awaiting_payment',
        error: null,
        reservedDrops: b.reservedDrops,
        expectedCreditedDrops: b.expectedCreditedDrops,
        // Frozen snapshot: nothing below reads live wallet/vault state again.
        proposal: Object.freeze({
          vaultId: state.vault.vaultId,
          destination: state.vault.custodyAddress,
          sendDrops,
          account: conn.address,
          network: conn.network,
        }) as XrpDepositProposal,
      });
    },
    async pay() {
      const proposal = state.proposal;
      if (!proposal || state.phase !== 'awaiting_payment') return;
      if (!sameConnection(deps.wallet.getConnection(), proposal)) {
        set({ phase: 'invalidated', proposal: null, error: 'Wallet account or network changed. Review and propose the deposit again.' });
        return;
      }
      const tx: XrplPaymentTx = {
        TransactionType: 'Payment',
        Account: proposal.account,
        Destination: proposal.destination,
        Amount: proposal.sendDrops.toString(),
      };
      set({ phase: 'signing', error: null });
      // Commit the amount BEFORE the wallet request exists, so a reload while the
      // request is open resumes that same amount instead of a re-typed one.
      committedSend = proposal.sendDrops;
      writeHint({ sendDrops: proposal.sendDrops.toString(), payStarted: true });
      const offPrompt = deps.wallet.onPrompt?.((p) => {
        writeHint({ requestId: p.requestId });
        set({ requestId: p.requestId });
      });
      let sub: WalletSubmission;
      try {
        sub = await deps.wallet.signAndSubmit(tx, { key: `xrp-deposit-vault-${proposal.vaultId}` });
      } catch (e) {
        set({ phase: 'awaiting_payment', error: e instanceof Error ? e.message : String(e) });
        return;
      } finally {
        offPrompt?.();
      }
      await handleSubmission(sub, proposal.sendDrops);
    },
    async resume() {
      if (state.phase === 'timed_out') {
        await startPolling();
        return;
      }
      const requestId = state.requestId;
      if (state.phase !== 'wallet_pending' || !requestId) return;
      const send = state.sentDrops ?? desiredSend ?? 0n;
      let sub: WalletSubmission;
      try {
        sub = await deps.wallet.resumeRequest(requestId);
      } catch (e) {
        set({ error: e instanceof Error ? e.message : String(e) });
        return;
      }
      if (sub.status === 'pending' && sub.hash === null && state.txHash) {
        await startPolling();
        return;
      }
      await handleSubmission(sub, send);
    },
    async confirmFromClick() {
      if (state.phase !== 'ready_to_confirm' && state.phase !== 'timed_out') return;
      await doConfirm();
    },
    async confirmNow() {
      if (!state.vault || ['opening', 'signing', 'confirming', 'confirmed', 'idle'].includes(state.phase)) return;
      pollRun++; // stop background polling; the user asked explicitly
      await doConfirm();
    },
    dispose() {
      disposed = true;
      pollRun++;
      unsubscribeWallet();
      listeners.clear();
    },
  };
  return controller;
}

function safeLocalStorage(): Pick<Storage, 'getItem' | 'setItem' | 'removeItem'> | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage;
  } catch {
    return null;
  }
}

/** Production wiring used by XrpBorrowModal.svelte. */
export function createDefaultXrpDepositController(ownerKey: string, minCollateralDrops?: bigint): XrpDepositController {
  const network = expectedXrplNetwork();
  return createXrpDepositController({
    wallet: getXrplWalletAdapter(),
    vault: {
      open: () => XrpVaultService.openXrpVault(),
      // Strict read (failure observable, hidden ids included). Oisy stays cache-only (no popup).
      readPendingDeposits: () => XrpVaultService.readMyPendingDeposits(),
      confirm: (id) => XrpVaultService.confirmXrpDeposit(id),
      ownsVault: (id) => XrpVaultService.ownsVault(id),
    },
    isOisy: () => get(currentWalletType) === WALLET_TYPES.OISY,
    network,
    ownerKey,
    minCollateralDrops,
  });
}
