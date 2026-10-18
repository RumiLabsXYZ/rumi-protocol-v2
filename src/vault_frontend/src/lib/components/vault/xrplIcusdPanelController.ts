/**
 * Controller for the XRPL icUSD panel (Task 6).
 *
 * Pure TypeScript (no Svelte) so every money/authority rule is unit-testable:
 *
 *  - Backend operation views are the source of truth. Wallet results are hints:
 *    a "submitted" payment never marks debt paid or icUSD delivered, and a
 *    TrustSet "submitted" never marks the trust line present (only an XRPL read
 *    does, and even that is only a UI hint; the backend re-checks).
 *  - Every wallet request is built from a FROZEN snapshot (config + connection
 *    at proposal time). A wallet account/network change invalidates unsigned
 *    proposals; a request already handed to the wallet is tracked by id.
 *  - Updates are never issued from timers. `pollTick` makes queries only (and
 *    for Oisy, only the public config query: caller-scoped reads need the
 *    signer popup). Oisy updates happen only in click-driven methods.
 *  - A stale full-close quote never triggers a second payment: nothing here
 *    signs or prepares automatically after a payment; "Add payment" is an
 *    explicit call that prepares a child intent and still waits for an explicit
 *    sign click.
 */

import {
  buildRepaymentPayment,
  buildTrustSetTx,
  configGate,
  createNonceStore,
  mapOperation,
  RepaymentMismatchError,
  type ApiResult,
  type ClaimActionName,
  type ConfigGate,
  type NonceStore,
  type OperationUi,
  type RepaymentIntent,
  type XrplIcusdApi,
  type XrplIcusdConfigView,
} from '$lib/services/xrplIcusdService';
import type {
  WalletSigningPrompt,
  WalletSubmission,
  XrplConnection,
  XrplWalletAdapter,
} from '$lib/services/xrplWalletService';

// --- public XRPL reads (hints) ----------------------------------------------

/** Fallback XRPL owner reserve when a live read is unavailable (labelled in the UI). */
export const FALLBACK_OWNER_RESERVE_DROPS = 200_000n;

async function rpc(
  method: string,
  params: Record<string, unknown>,
  endpoints: string[],
  fetchImpl: typeof fetch
): Promise<any> {
  let last: unknown = new Error('No XRPL endpoint configured.');
  for (const url of endpoints) {
    try {
      const res = await fetchImpl(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ method, params: [params] }),
      });
      if (!res.ok) throw new Error(`XRPL RPC HTTP ${res.status}`);
      const result = (await res.json())?.result;
      if (result?.status === 'success' || result?.error === 'actNotFound') return result;
      throw new Error(`XRPL RPC unusable result: ${result?.error ?? 'unknown'}`);
    } catch (e) {
      last = e;
    }
  }
  throw last instanceof Error ? last : new Error('XRPL RPC failed');
}

export interface TrustLineRead {
  present: boolean;
  limit: string | null;
  balance: string | null;
}

/** Browser-side trust-line read for the exact issuer + currency. HINT ONLY. */
export async function fetchTrustLine(
  address: string,
  issuer: string,
  currencyHex: string,
  endpoints: string[],
  fetchImpl: typeof fetch = fetch
): Promise<TrustLineRead> {
  const currency = currencyHex.toUpperCase();
  const result = await rpc('account_lines', { account: address, peer: issuer, ledger_index: 'validated' }, endpoints, fetchImpl);
  if (result.error === 'actNotFound') return { present: false, limit: null, balance: null };
  const lines: any[] = Array.isArray(result.lines) ? result.lines : [];
  const line = lines.find((l) => l?.account === issuer && String(l?.currency ?? '').toUpperCase() === currency);
  return line
    ? { present: true, limit: String(line.limit ?? ''), balance: line.balance !== undefined ? String(line.balance) : null }
    : { present: false, limit: null, balance: null };
}

export interface OwnerReserve {
  drops: bigint;
  /** False when the live read failed and the documented fallback is shown. */
  live: boolean;
}

/** XRPL owner reserve (per trust line) from a live server_state read, else a labelled fallback. */
export async function fetchOwnerReserve(endpoints: string[], fetchImpl: typeof fetch = fetch): Promise<OwnerReserve> {
  try {
    const result = await rpc('server_state', {}, endpoints, fetchImpl);
    const inc = result?.state?.validated_ledger?.reserve_inc;
    if (typeof inc === 'number' && Number.isInteger(inc) && inc > 0) return { drops: BigInt(inc), live: true };
    if (typeof inc === 'string' && /^\d+$/.test(inc) && BigInt(inc) > 0n) return { drops: BigInt(inc), live: true };
  } catch { /* fall through */ }
  return { drops: FALLBACK_OWNER_RESERVE_DROPS, live: false };
}

// --- import disclosure --------------------------------------------------------

export interface ImportDisclosure {
  amountE8s: bigint;
  feeE8s: bigint;
  /** ICRC-2 allowance granted to Rumi: amount plus the transfer fee it will pull. */
  allowanceE8s: bigint;
  /** Approval fee + transfer fee, both paid by the holder on the icUSD ledger. */
  totalLedgerFeesE8s: bigint;
  /** Total leaving the holder's canonical balance. */
  debitedE8s: bigint;
}

export function importDisclosure(amountE8s: bigint, feeE8s: bigint): ImportDisclosure {
  return {
    amountE8s,
    feeE8s,
    allowanceE8s: amountE8s + feeE8s,
    totalLedgerFeesE8s: feeE8s * 2n,
    debitedE8s: amountE8s + feeE8s * 2n,
  };
}

// --- state --------------------------------------------------------------------

export type TrustLineStatus = 'unknown' | 'checking' | 'missing' | 'present' | 'error';
export type TrustSetPhase = 'idle' | 'proposed' | 'awaiting_wallet' | 'wallet_pending' | 'submitted' | 'rejected' | 'invalidated' | 'error';
export type RepayPhase =
  | 'idle' | 'preparing' | 'prepared' | 'awaiting_wallet' | 'wallet_pending' | 'submitted'
  | 'observing' | 'observed' | 'rejected' | 'invalidated' | 'error';
export type ImportPhase = 'idle' | 'approving' | 'approved' | 'submitting' | 'done' | 'invalidated' | 'error';
export type BorrowPhase = 'idle' | 'submitting' | 'done' | 'error';

export interface XrplIcusdPanelState {
  config: XrplIcusdConfigView | null;
  gate: ConfigGate | null;
  configError: string | null;
  connection: XrplConnection | null;
  trustLine: { status: TrustLineStatus; limit: string | null; balance: string | null; error: string | null };
  reserve: OwnerReserve;
  trustSet: { phase: TrustSetPhase; proposal: { account: string; network: string } | null; requestId: string | null; hash: string | null; error: string | null };
  operations: OperationUi[];
  operationsSource: 'none' | 'cache' | 'backend';
  operationsError: string | null;
  repay: {
    phase: RepayPhase;
    operationId: bigint | null;
    intent: RepaymentIntent | null;
    snapshot: { account: string; network: string } | null;
    requestId: string | null;
    hash: string | null;
    error: string | null;
  };
  borrow: { phase: BorrowPhase; operationId: bigint | null; error: string | null };
  importFlow: {
    phase: ImportPhase;
    amountE8s: bigint | null;
    disclosure: ImportDisclosure | null;
    snapshot: { account: string; network: string } | null;
    error: string | null;
  };
  /** Deep link / QR for the wallet request that is currently open, if any. */
  prompt: (WalletSigningPrompt & { flow: 'trustset' | 'repay' | 'other' }) | null;
  /** Transient message for actions that were skipped (e.g. needs a click). */
  notice: string | null;
  busy: string | null;
}

export interface XrplIcusdControllerDeps {
  api: XrplIcusdApi;
  wallet: XrplWalletAdapter;
  /** Gesture-gated signer login (Oisy): authenticated calls only from clicks. */
  isOisy: () => boolean;
  ownerKey: string;
  storage?: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'> | null;
  fetchImpl?: typeof fetch;
  endpoints: (network: string) => string[];
  sleep?: (ms: number) => Promise<void>;
  /** Wall clock (ms) used for the repayment quote lifetime check. */
  now?: () => number;
  /** ICRC-2 approve on the icUSD ledger for the backend as spender. */
  approve: (amountE8s: bigint) => Promise<{ success: boolean; error?: string }>;
  /** Current icUSD ledger fee. */
  ledgerFee: () => Promise<bigint>;
}

export interface XrplIcusdController {
  getState(): XrplIcusdPanelState;
  subscribe(fn: (s: XrplIcusdPanelState) => void): () => void;
  init(): Promise<void>;
  /** Explicit user click: may perform the caller-scoped read even for Oisy. */
  refresh(): Promise<void>;
  /** Timer-safe: queries only. */
  pollTick(): Promise<void>;
  checkTrustLine(): Promise<void>;
  proposeTrustSet(): void;
  signTrustSet(): Promise<void>;
  borrow(a: { vaultId: bigint; grossE8s: bigint }): Promise<void>;
  prepareRepayment(a: { vaultId: bigint; maxE8s: bigint; fullClose: boolean }): Promise<void>;
  addPayment(parentOperationId: bigint, a: { vaultId: bigint; maxE8s: bigint; fullClose?: boolean }): Promise<void>;
  payRepayment(): Promise<void>;
  observeRepayment(opts?: { auto?: boolean }): Promise<void>;
  cancelRepayment(): void;
  importCanonical(amountE8s: bigint): Promise<void>;
  finishImport(): Promise<void>;
  resume(operationId: bigint): Promise<void>;
  resolveClaim(operationId: bigint, action: ClaimActionName): Promise<void>;
  dispose(): void;
}

const OPS_CACHE_PREFIX = 'rumi_xrpl_icusd_ops_v1:';
const REPAY_HINT_PREFIX = 'rumi_xrpl_icusd_repay_v1:';
const TRUST_WATCH_TRIES = 8;
const TRUST_WATCH_MS = 3000;
/** A prepared repayment intent is honoured for 7 days from its quote time. */
const INTENT_TTL_NS = 7n * 24n * 3600n * 1_000_000_000n;

interface RepayHint {
  requestId?: string;
  hash?: string;
  /** The exact Payment handed to the wallet (validated by buildRepaymentPayment). */
  paymentJson?: string;
  /** Account the request was created for. */
  account?: string;
  /** Request-nonce key kept until the operation leaves IntentPrepared. */
  intentKey?: string;
  /** Backend intent (bigints as decimal strings) so a reload can re-derive it. */
  intent?: {
    operationId: string; issuer: string; currency: string; payer: string; destinationTag: number;
    maxE8s: string; network: 'mainnet' | 'testnet'; paymentJson: string; quoteTimeNs: string;
  };
}

function dehydrateIntent(i: RepaymentIntent): NonNullable<RepayHint['intent']> {
  return {
    operationId: i.operationId.toString(), issuer: i.issuer, currency: i.currency, payer: i.payer,
    destinationTag: i.destinationTag, maxE8s: i.maxE8s.toString(), network: i.network,
    paymentJson: i.paymentJson, quoteTimeNs: i.quoteTimeNs.toString(),
  };
}
function hydrateIntent(h: RepayHint | undefined): RepaymentIntent | null {
  const i = h?.intent;
  if (!i) return null;
  try {
    return {
      operationId: BigInt(i.operationId), issuer: i.issuer, currency: i.currency, payer: i.payer,
      destinationTag: i.destinationTag, maxE8s: BigInt(i.maxE8s), network: i.network,
      paymentJson: i.paymentJson, quoteTimeNs: BigInt(i.quoteTimeNs),
    };
  } catch { return null; }
}

// bigint-safe JSON for the operation cache (hints only).
function ser(value: unknown): string {
  return JSON.stringify(value, (_k, v) => {
    if (typeof v === 'bigint') return { $b: v.toString() };
    if (ArrayBuffer.isView(v)) return Array.from(v as any, (x: any) => (typeof x === 'bigint' ? { $b: x.toString() } : x));
    return v;
  });
}
function de(text: string): unknown {
  return JSON.parse(text, (_k, v) => (v && typeof v === 'object' && typeof v.$b === 'string' ? BigInt(v.$b) : v));
}

const INFLIGHT_REPAY: RepayPhase[] = ['preparing', 'awaiting_wallet', 'observing'];

export function createXrplIcusdController(deps: XrplIcusdControllerDeps): XrplIcusdController {
  const storage = deps.storage === undefined ? safeLocalStorage() : deps.storage;
  const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const nowMs = deps.now ?? (() => Date.now());
  const fetchImpl = deps.fetchImpl ?? ((...a: Parameters<typeof fetch>) => fetch(...a));
  const nonces: NonceStore = createNonceStore({ storage, owner: deps.ownerKey });
  const opsKey = `${OPS_CACHE_PREFIX}${deps.ownerKey}`;
  const hintKey = `${REPAY_HINT_PREFIX}${deps.ownerKey}`;

  let state: XrplIcusdPanelState = {
    config: null, gate: null, configError: null,
    connection: deps.wallet.getConnection(),
    trustLine: { status: 'unknown', limit: null, balance: null, error: null },
    reserve: { drops: FALLBACK_OWNER_RESERVE_DROPS, live: false },
    trustSet: { phase: 'idle', proposal: null, requestId: null, hash: null, error: null },
    operations: [], operationsSource: 'none', operationsError: null,
    repay: { phase: 'idle', operationId: null, intent: null, snapshot: null, requestId: null, hash: null, error: null },
    borrow: { phase: 'idle', operationId: null, error: null },
    importFlow: { phase: 'idle', amountE8s: null, disclosure: null, snapshot: null, error: null },
    prompt: null, notice: null, busy: null,
  };
  const listeners = new Set<(s: XrplIcusdPanelState) => void>();
  let disposed = false;
  let trustWatchRun = 0;
  let trustCheckRun = 0;

  function set(patch: Partial<XrplIcusdPanelState>) {
    state = { ...state, ...patch };
    listeners.forEach((l) => l(state));
  }
  const WALLET_OPEN = ['awaiting_wallet', 'wallet_pending'];
  const setRepay = (p: Partial<XrplIcusdPanelState['repay']>) => {
    const dropPrompt = p.phase !== undefined && !WALLET_OPEN.includes(p.phase) && state.prompt?.flow === 'repay';
    set({ repay: { ...state.repay, ...p }, ...(dropPrompt ? { prompt: null } : {}) });
  };
  const setTrustSet = (p: Partial<XrplIcusdPanelState['trustSet']>) => {
    const dropPrompt = p.phase !== undefined && !WALLET_OPEN.includes(p.phase) && state.prompt?.flow === 'trustset';
    set({ trustSet: { ...state.trustSet, ...p }, ...(dropPrompt ? { prompt: null } : {}) });
  };
  const setImport = (p: Partial<XrplIcusdPanelState['importFlow']>) => set({ importFlow: { ...state.importFlow, ...p } });
  const setBorrow = (p: Partial<XrplIcusdPanelState['borrow']>) => set({ borrow: { ...state.borrow, ...p } });

  // --- local hints ---
  function readHints(): Record<string, RepayHint> {
    try {
      const raw = storage?.getItem(hintKey);
      const p = raw ? JSON.parse(raw) : {};
      return p && typeof p === 'object' ? p : {};
    } catch { return {}; }
  }
  function writeHints(h: Record<string, RepayHint>) {
    try { storage?.setItem(hintKey, JSON.stringify(h)); } catch { /* hints only */ }
  }
  function saveHint(opId: bigint, patch: Partial<RepayHint>) {
    const h = readHints();
    h[String(opId)] = { ...(h[String(opId)] ?? {}), ...patch };
    writeHints(h);
  }
  function clearHint(opId: bigint) {
    const h = readHints();
    delete h[String(opId)];
    writeHints(h);
  }
  function readOpsCache(): OperationUi[] {
    try {
      const raw = storage?.getItem(opsKey);
      const raws = raw ? (de(raw) as any[]) : [];
      return Array.isArray(raws) ? raws.map(mapOperation) : [];
    } catch { return []; }
  }
  function writeOpsCache(ops: OperationUi[]) {
    try { storage?.setItem(opsKey, ser(ops.map((o) => o.raw))); } catch { /* hints only */ }
  }

  function setOps(ops: OperationUi[], source: 'cache' | 'backend') {
    const sorted = [...ops].sort((a, b) => (a.id < b.id ? 1 : a.id > b.id ? -1 : 0));
    set({ operations: sorted, operationsSource: source, operationsError: null });
    if (source === 'backend') {
      writeOpsCache(sorted);
      const hints = readHints();
      for (const o of sorted) {
        const h = hints[String(o.id)];
        if (!h) continue;
        if (o.terminal) {
          if (h.intentKey) nonces.forget(h.intentKey);
          clearHint(o.id);
        } else if (o.phase !== 'IntentPrepared' && (h.intentKey || h.intent)) {
          // The intent was used: its request nonce has done its job.
          if (h.intentKey) nonces.forget(h.intentKey);
          saveHint(o.id, { intentKey: undefined, intent: undefined });
        }
      }
    }
  }
  function mergeOp(op: OperationUi) {
    const rest = state.operations.filter((o) => o.id !== op.id);
    setOps([op, ...rest], 'backend');
  }

  // --- gating helpers ---
  function activeConfig(): XrplIcusdConfigView | null {
    return state.gate?.state === 'active' ? state.config : null;
  }
  /** Connection that may be used for a new action, or an explanation. */
  function usableConnection(): { conn: XrplConnection; cfg: XrplIcusdConfigView } | string {
    const cfg = activeConfig();
    if (!cfg) return state.gate?.message || 'XRPL icUSD is not available.';
    const conn = deps.wallet.getConnection();
    if (!conn) return 'Connect an XRPL wallet first.';
    if (conn.network !== cfg.network) return `Your wallet is on ${conn.network}; XRPL icUSD is on ${cfg.network}. Switch network and reconnect.`;
    return { conn, cfg };
  }
  function sameConn(c: XrplConnection | null, snap: { account: string; network: string } | null) {
    return !!c && !!snap && c.address === snap.account && c.network === snap.network;
  }
  function failedMessage(r: Extract<ApiResult<unknown>, { ok: false }>): string {
    return r.error;
  }

  // --- wallet change ---
  const offWallet = deps.wallet.onChange((conn) => {
    set({ connection: conn });
    // Anything not yet handed to the wallet is stale for the new identity.
    if (state.trustSet.phase === 'proposed' && !sameConn(conn, state.trustSet.proposal)) {
      setTrustSet({ phase: 'invalidated', proposal: null, error: 'Wallet account or network changed. Review and enable icUSD again.' });
    }
    if (state.repay.phase === 'prepared' && !sameConn(conn, state.repay.snapshot)) {
      setRepay({ phase: 'invalidated', error: 'Wallet account or network changed. Prepare the repayment again with the wallet you want to pay from.' });
    }
    if (state.importFlow.phase === 'approved' && !sameConn(conn, state.importFlow.snapshot)) {
      setImport({ phase: 'invalidated', error: 'Wallet account or network changed after approval. Start the import again.' });
    }
    trustWatchRun++;
    set({ trustLine: { status: 'unknown', limit: null, balance: null, error: null } });
    void checkTrustLine();
  });

  // --- wallet prompts (deep link / QR) ---
  /** The Payment handed to the wallet for the repayment currently being signed. */
  let inflightRepay: { operationId: bigint; paymentJson: string; account: string } | null = null;
  const offPrompt = deps.wallet.onPrompt?.((p) => {
    const flow: 'trustset' | 'repay' | 'other' = WALLET_OPEN.includes(state.repay.phase)
      ? 'repay'
      : WALLET_OPEN.includes(state.trustSet.phase) ? 'trustset' : 'other';
    set({ prompt: { ...p, flow } });
    // Persist the signed-payment bookkeeping the moment the request exists, so a
    // reload while the user is approving in Xaman resumes this same request.
    if (flow === 'repay' && inflightRepay) {
      saveHint(inflightRepay.operationId, {
        requestId: p.requestId, paymentJson: inflightRepay.paymentJson, account: inflightRepay.account,
      });
    }
  });

  // --- reads ---
  async function loadConfig() {
    const r = await deps.api.getConfig();
    if (r.ok) set({ config: r.data, gate: configGate(r.data), configError: null });
    else set({ configError: failedMessage(r), gate: state.config ? state.gate : null });
  }

  async function refreshOps(): Promise<void> {
    const r = await deps.api.listMyOperations();
    if (r.ok) setOps(r.data, 'backend');
    else set({ operationsError: failedMessage(r) });
  }

  async function checkTrustLine() {
    const cfg = state.config;
    const conn = deps.wallet.getConnection();
    set({ connection: conn });
    if (!cfg || !cfg.issuerAddress || !cfg.network || !conn || conn.network !== cfg.network) {
      set({ trustLine: { status: 'unknown', limit: null, balance: null, error: null } });
      return;
    }
    const run = ++trustCheckRun;
    set({ trustLine: { ...state.trustLine, status: 'checking', error: null } });
    try {
      const line = await fetchTrustLine(conn.address, cfg.issuerAddress, cfg.currencyHex, deps.endpoints(conn.network), fetchImpl);
      // Invalidate on account/network change while the read was in flight.
      const now = deps.wallet.getConnection();
      if (run !== trustCheckRun || now?.address !== conn.address || now?.network !== conn.network) return;
      set({ trustLine: { status: line.present ? 'present' : 'missing', limit: line.limit, balance: line.balance, error: null } });
    } catch (e) {
      if (run !== trustCheckRun) return;
      set({ trustLine: { status: 'error', limit: null, balance: null, error: e instanceof Error ? e.message : 'Could not read the XRP Ledger.' } });
    }
  }

  async function watchTrustLine() {
    const run = ++trustWatchRun;
    for (let i = 0; i < TRUST_WATCH_TRIES; i++) {
      await sleep(TRUST_WATCH_MS);
      if (disposed || run !== trustWatchRun) return;
      await checkTrustLine();
      if (state.trustLine.status === 'present') return;
    }
    if (disposed || run !== trustWatchRun) return;
    // The line never appeared: do not leave the user on "Submitted. Waiting".
    // Offer "Enable icUSD" again (a new TrustSet request is created on demand).
    setTrustSet({
      phase: 'idle',
      error: 'The XRP Ledger has not shown the icUSD trust line yet. If your wallet approved it, press Re-check in a moment; otherwise enable icUSD again.',
    });
  }

  // --- repay wallet handling ---
  async function handleSubmission(sub: WalletSubmission, opId: bigint) {
    if (sub.status === 'rejected') {
      // Keep the prepared intent (the operation is still IntentPrepared); drop only the request.
      saveHint(opId, { requestId: undefined, hash: undefined, paymentJson: undefined, account: undefined });
      setRepay({
        phase: 'rejected', requestId: sub.requestId, hash: null,
        error: sub.warning
          ? `${sub.warning} Your debt is unchanged.`
          : 'The wallet request was rejected or expired. Nothing was sent and your debt is unchanged.',
      });
      return;
    }
    saveHint(opId, { requestId: sub.requestId, hash: sub.hash ?? undefined });
    if (sub.status === 'pending' || !sub.hash) {
      setRepay({ phase: 'wallet_pending', requestId: sub.requestId, hash: null, error: null });
      return;
    }
    setRepay({ phase: 'submitted', requestId: sub.requestId, hash: sub.hash, error: null });
    await controller.observeRepayment({ auto: true });
  }

  /**
   * Before building or resuming a repayment payment: the backend operation must
   * still be waiting for it, and the quote must not be older than 7 days.
   * Non-Oisy re-reads the operation by query; Oisy (no passive authenticated
   * read) uses the cached view. Returns a refusal message, or null if fine.
   */
  async function repayStaleness(opId: bigint, quoteTimeNs: bigint | null, alreadySigned: boolean): Promise<string | null> {
    const oisy = deps.isOisy();
    if (!oisy) {
      await refreshOps();
      if (state.operationsError) return `Could not confirm that this repayment is still open (${state.operationsError}). Nothing was sent. Try again.`;
    }
    const cur = state.operations.find((o) => o.id === opId);
    if (!cur && !oisy) return 'Rumi no longer lists this repayment. Prepare it again; do not pay this one.';
    if (cur && cur.phase !== 'IntentPrepared') {
      return `This repayment is no longer waiting for a payment (${cur.label}). Do not pay it again.`;
    }
    if (!alreadySigned && quoteTimeNs !== null && BigInt(nowMs()) * 1_000_000n > quoteTimeNs + INTENT_TTL_NS) {
      const key = readHints()[String(opId)]?.intentKey;
      if (key) nonces.forget(key);
      return 'This repayment quote has expired (older than 7 days). Prepare a new one.';
    }
    return null;
  }

  async function startRepayIntent(
    a: { vaultId: bigint; maxE8s: bigint; fullClose: boolean; topUp: bigint | null }
  ) {
    const u = usableConnection();
    if (typeof u === 'string') { setRepay({ phase: 'error', error: u }); return; }
    if (INFLIGHT_REPAY.includes(state.repay.phase) || state.repay.phase === 'wallet_pending' || state.repay.phase === 'submitted') {
      setRepay({ error: 'Finish or dismiss the current repayment payment first.' });
      return;
    }
    if (a.maxE8s <= 0n) { setRepay({ phase: 'error', error: 'Enter an amount to repay.' }); return; }
    const { conn, cfg } = u;
    const intentKey = `repay|${a.vaultId}|${a.maxE8s}|${a.fullClose}|${a.topUp ?? ''}|${conn.address}|${cfg.network}`;
    const requestNonce = nonces.getOrCreate(intentKey);
    setRepay({ phase: 'preparing', error: null, intent: null, hash: null, requestId: null, operationId: null, snapshot: { account: conn.address, network: conn.network } });
    const r = await deps.api.prepareRepayment({
      vaultId: a.vaultId, maxE8s: a.maxE8s, payer: conn.address, network: cfg.network!, requestNonce,
      fullClose: a.fullClose, topUpOperation: a.topUp,
    });
    if (!r.ok) {
      if (r.definitive) nonces.forget(intentKey);
      setRepay({ phase: 'error', error: failedMessage(r) });
      return;
    }
    // Keep the nonce until the operation leaves IntentPrepared: a reload and a
    // repeated click then get the SAME operation back instead of a second one.
    saveHint(r.data.operationId, { intentKey, intent: dehydrateIntent(r.data) });
    setRepay({ phase: 'prepared', intent: r.data, operationId: r.data.operationId, error: null });
    void refreshOpsIfAllowed();
  }

  async function refreshOpsIfAllowed() {
    if (!deps.isOisy()) await refreshOps();
  }

  const controller: XrplIcusdController = {
    getState: () => state,
    subscribe(fn) {
      listeners.add(fn);
      fn(state);
      return () => { listeners.delete(fn); };
    },

    async init() {
      set({ connection: deps.wallet.getConnection() });
      await loadConfig();
      // Source of truth is the authenticated backend. A gesture-gated signer
      // (Oisy) cannot make that read without a popup, so show the cache until
      // the user clicks Refresh.
      if (deps.isOisy()) {
        const cached = readOpsCache();
        if (cached.length) set({ operations: cached, operationsSource: 'cache' });
      } else {
        await refreshOps();
      }
      if (state.gate?.state === 'active') {
        void fetchOwnerReserve(deps.endpoints(state.config!.network!), fetchImpl).then((reserve) => { if (!disposed) set({ reserve }); });
      }
      await checkTrustLine();
      // Recover a repayment that was prepared / handed to the wallet before a reload.
      const hints = readHints();
      for (const o of state.operations) {
        const h = hints[String(o.id)];
        if (o.kind !== 'Repayment' || o.state !== 'repay_payment_pending' || !h || state.repay.phase !== 'idle') continue;
        const intent = hydrateIntent(h);
        const snapshot = h.account ? { account: h.account, network: state.config?.network ?? '' }
          : intent ? { account: intent.payer, network: intent.network } : null;
        if (h.requestId) {
          // A signed (or open) payment exists: resume that request, never prompt a second payment.
          const stale = await repayStaleness(o.id, intent?.quoteTimeNs ?? null, !!h.hash);
          if (stale) { setRepay({ operationId: o.id, intent, snapshot, phase: 'error', error: stale }); break; }
          setRepay({ operationId: o.id, intent, snapshot, requestId: h.requestId, hash: h.hash ?? null, phase: 'wallet_pending' });
          try {
            const sub = await (h.account ? deps.wallet.resumeRequest(h.requestId, h.account) : deps.wallet.resumeRequest(h.requestId));
            await handleSubmission(sub, o.id);
          } catch {
            setRepay({ phase: 'wallet_pending' });
          }
          break;
        }
        if (intent) {
          // Prepared but not yet paid: restore the card instead of making a second operation.
          setRepay({ operationId: o.id, intent, snapshot, phase: 'prepared', error: null });
          break;
        }
      }
    },

    async refresh() {
      set({ busy: 'refresh' });
      try {
        await loadConfig();
        await refreshOps();
        await checkTrustLine();
      } finally { set({ busy: null }); }
    },

    async pollTick() {
      await loadConfig();
      // Caller-scoped reads route through the Oisy popup: never from a timer.
      if (!deps.isOisy()) await refreshOps();
    },

    checkTrustLine,

    proposeTrustSet() {
      const u = usableConnection();
      if (typeof u === 'string') { setTrustSet({ phase: 'idle', error: u }); return; }
      try {
        buildTrustSetTx(u.cfg, u.conn);
      } catch (e) {
        setTrustSet({ phase: 'error', error: e instanceof Error ? e.message : 'Cannot enable icUSD.' });
        return;
      }
      setTrustSet({ phase: 'proposed', proposal: { account: u.conn.address, network: u.conn.network }, error: null, hash: null, requestId: null });
    },

    async signTrustSet() {
      const t = state.trustSet;
      if (t.phase !== 'proposed' && t.phase !== 'rejected') {
        return;
      }
      const u = usableConnection();
      if (typeof u === 'string') { setTrustSet({ phase: 'error', error: u }); return; }
      // The request is rebuilt from backend config + the CURRENT connection and
      // must be for the identity the user reviewed.
      if (t.proposal && !sameConn(u.conn, t.proposal)) {
        setTrustSet({ phase: 'invalidated', proposal: null, error: 'Wallet account or network changed. Review and enable icUSD again.' });
        return;
      }
      let tx;
      try { tx = buildTrustSetTx(u.cfg, u.conn); } catch (e) {
        setTrustSet({ phase: 'error', error: e instanceof Error ? e.message : 'Cannot enable icUSD.' });
        return;
      }
      setTrustSet({ phase: 'awaiting_wallet', error: null });
      try {
        const sub = await deps.wallet.signAndSubmit(tx, {
          key: `xrpl-icusd-trustset:${u.conn.address}:${u.conn.network}:${u.cfg.issuerAddress}:${u.cfg.currencyHex.toUpperCase()}`,
        });
        if (sub.status === 'rejected') {
          setTrustSet({
            phase: 'rejected', requestId: sub.requestId,
            error: sub.warning
              ? `${sub.warning} No trust line was created.`
              : 'The wallet request was rejected or expired. No trust line was created.',
          });
          return;
        }
        setTrustSet({
          phase: sub.status === 'submitted' ? 'submitted' : 'wallet_pending',
          requestId: sub.requestId, hash: sub.hash, error: null,
        });
        // The line only counts once an XRPL read sees it.
        void watchTrustLine();
      } catch (e) {
        setTrustSet({ phase: 'error', error: e instanceof Error ? e.message : 'The wallet request failed.' });
      }
    },

    async borrow({ vaultId, grossE8s }) {
      const u = usableConnection();
      if (typeof u === 'string') { setBorrow({ phase: 'error', error: u }); return; }
      // The trust-line read is a hint only (RPC can lag); the backend checks the
      // real line and answers TrustLineMissing, which is shown verbatim.
      if (grossE8s <= 0n) { setBorrow({ phase: 'error', error: 'Enter an amount to borrow.' }); return; }
      if (state.borrow.phase === 'submitting') return;
      const intentKey = `borrow|${vaultId}|${grossE8s}|${u.conn.address}|${u.cfg.network}`;
      const requestNonce = nonces.getOrCreate(intentKey);
      setBorrow({ phase: 'submitting', error: null });
      const r = await deps.api.borrowToXrpl({ vaultId, grossE8s, recipient: u.conn.address, network: u.cfg.network!, requestNonce });
      if (!r.ok) {
        if (r.definitive) nonces.forget(intentKey);
        setBorrow({ phase: 'error', error: failedMessage(r) });
        return;
      }
      nonces.forget(intentKey);
      mergeOp(r.data);
      setBorrow({ phase: 'done', operationId: r.data.id, error: null });
    },

    prepareRepayment({ vaultId, maxE8s, fullClose }) {
      return startRepayIntent({ vaultId, maxE8s, fullClose, topUp: null });
    },

    addPayment(parentOperationId, a) {
      return startRepayIntent({ vaultId: a.vaultId, maxE8s: a.maxE8s, fullClose: a.fullClose ?? false, topUp: parentOperationId });
    },

    async payRepayment() {
      const r = state.repay;
      if (r.phase === 'awaiting_wallet' || r.phase === 'observing' || r.phase === 'preparing') return;
      const hint = r.operationId !== null ? readHints()[String(r.operationId)] : undefined;
      try {
        // A request already handed to the wallet is resumed, never re-created.
        if (hint?.requestId && r.operationId !== null) {
          const stale = await repayStaleness(r.operationId, r.intent?.quoteTimeNs ?? hydrateIntent(hint)?.quoteTimeNs ?? null, !!hint.hash);
          if (stale) { setRepay({ phase: 'error', error: stale }); return; }
          setRepay({ phase: 'awaiting_wallet', error: null });
          inflightRepay = hint.paymentJson && r.snapshot
            ? { operationId: r.operationId, paymentJson: hint.paymentJson, account: hint.account ?? r.snapshot.account }
            : null;
          const sub = await (hint.account ? deps.wallet.resumeRequest(hint.requestId, hint.account) : deps.wallet.resumeRequest(hint.requestId));
          await handleSubmission(sub, r.operationId);
          return;
        }
        if (!r.intent || r.operationId === null) {
          setRepay({ phase: 'error', error: 'Prepare the repayment first.' });
          return;
        }
        const u = usableConnection();
        if (typeof u === 'string') { setRepay({ phase: 'error', error: u }); return; }
        let tx;
        try {
          tx = buildRepaymentPayment(r.intent, u.cfg, u.conn);
        } catch (e) {
          if (e instanceof RepaymentMismatchError && /wallet is not the payer|connected wallet/.test(e.message)) {
            setRepay({ phase: 'invalidated', error: e.message });
          } else {
            setRepay({ phase: 'error', error: e instanceof Error ? e.message : 'Refused to sign.' });
          }
          return;
        }
        // Operation still open and quote still fresh? (refuse before the wallet is asked)
        const stale = await repayStaleness(r.operationId, r.intent.quoteTimeNs, false);
        if (stale) { setRepay({ phase: 'error', error: stale }); return; }
        inflightRepay = { operationId: r.operationId, paymentJson: JSON.stringify(tx), account: u.conn.address };
        setRepay({ phase: 'awaiting_wallet', error: null });
        const sub = await deps.wallet.signAndSubmit(tx, { key: `xrpl-icusd-repay:${r.operationId}` });
        await handleSubmission(sub, r.operationId);
      } catch (e) {
        setRepay({ phase: 'error', error: e instanceof Error ? e.message : 'The wallet request failed.' });
      } finally {
        inflightRepay = null;
      }
    },

    async observeRepayment(opts) {
      const r = state.repay;
      if (opts?.auto && deps.isOisy()) {
        set({ notice: 'Payment submitted. Click "Tell Rumi about my payment" to have Rumi check it.' });
        return;
      }
      if (!r.hash || r.operationId === null) return;
      if (r.phase === 'observing') return;
      const prev = r.phase;
      setRepay({ phase: 'observing', error: null });
      const res = await deps.api.observeRepayment(r.operationId, r.hash);
      if (!res.ok) {
        setRepay({ phase: prev === 'observed' ? 'observed' : 'submitted', error: `${failedMessage(res)} Your payment is safe; try again in a moment.` });
        return;
      }
      mergeOp(res.data);
      // The backend decides what the payment did; show its state, not ours.
      // Oisy: the observe click was the one authenticated call; the returned
      // view is already merged, so no second signer request (a refresh needs its own click).
      if (!deps.isOisy()) await refreshOps();
      setRepay({ phase: 'observed', error: null });
    },

    cancelRepayment() {
      const p = state.repay.phase;
      if (p === 'awaiting_wallet' || p === 'observing' || p === 'preparing') return;
      setRepay({ phase: 'idle', intent: null, snapshot: null, requestId: null, hash: null, error: null, operationId: null });
    },

    async importCanonical(amountE8s) {
      if (state.importFlow.phase === 'approving' || state.importFlow.phase === 'submitting') return;
      const u = usableConnection();
      if (typeof u === 'string') { setImport({ phase: 'error', error: u }); return; }
      if (amountE8s <= 0n) { setImport({ phase: 'error', error: 'Enter an amount to move.' }); return; }
      setImport({ phase: 'approving', amountE8s, error: null, snapshot: { account: u.conn.address, network: u.conn.network } });
      let disclosure: ImportDisclosure;
      try {
        disclosure = importDisclosure(amountE8s, await deps.ledgerFee());
      } catch (e) {
        setImport({ phase: 'error', error: e instanceof Error ? e.message : 'Could not read the icUSD ledger fee.' });
        return;
      }
      const ap = await deps.approve(disclosure.allowanceE8s);
      if (!ap.success) {
        setImport({ phase: 'error', error: ap.error ?? 'The icUSD approval failed. Nothing was moved.' });
        return;
      }
      setImport({ phase: 'approved', disclosure });
      // Gesture rule: Oisy needs a fresh click for the second signer request.
      if (!deps.isOisy()) await controller.finishImport();
    },

    async finishImport() {
      const f = state.importFlow;
      if (f.phase !== 'approved' || f.amountE8s === null || !f.snapshot) return;
      const u = usableConnection();
      if (typeof u === 'string') { setImport({ phase: 'error', error: u }); return; }
      if (!sameConn(u.conn, f.snapshot)) {
        setImport({ phase: 'invalidated', error: 'Wallet account or network changed after approval. Start the import again.' });
        return;
      }
      const intentKey = `import|${f.amountE8s}|${f.snapshot.account}|${f.snapshot.network}`;
      const requestNonce = nonces.getOrCreate(intentKey);
      setImport({ phase: 'submitting', error: null });
      const r = await deps.api.issueFromCanonical({
        amountE8s: f.amountE8s, recipient: f.snapshot.account, network: u.cfg.network!, requestNonce,
      });
      if (!r.ok) {
        if (r.definitive) nonces.forget(intentKey);
        setImport({ phase: 'approved', error: failedMessage(r) });
        return;
      }
      nonces.forget(intentKey);
      mergeOp(r.data);
      setImport({ phase: 'done', error: null });
    },

    async resume(operationId) {
      set({ busy: `resume:${operationId}` });
      try {
        const r = await deps.api.resume(operationId);
        if (r.ok) mergeOp(r.data);
        else set({ operationsError: failedMessage(r) });
      } finally { set({ busy: null }); }
    },

    async resolveClaim(operationId, action) {
      set({ busy: `claim:${operationId}` });
      try {
        const r = await deps.api.resolveClaim(operationId, action);
        if (r.ok) mergeOp(r.data);
        else set({ operationsError: failedMessage(r) });
      } finally { set({ busy: null }); }
    },

    dispose() {
      disposed = true;
      trustWatchRun++;
      offWallet();
      offPrompt?.();
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
