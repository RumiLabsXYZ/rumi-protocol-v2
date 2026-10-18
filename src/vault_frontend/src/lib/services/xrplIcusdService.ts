/**
 * XRPL icUSD service (Task 6 of the XRPL icUSD plan).
 *
 * Pure, UI-agnostic building blocks plus typed wrappers over the backend's
 * XRPL icUSD lane. Authority rules enforced here:
 *
 *  - Money is bigint e8s. Decimal text is parsed/formatted exactly (max 8
 *    decimals); JavaScript Number is never used for an amount that is signed,
 *    sent to the backend, or compared against a cap.
 *  - A TrustSet is built ONLY from backend config + the current wallet
 *    connection. A repayment Payment is built ONLY from the backend-prepared
 *    intent (`payment_json`) after it is verified field-by-field against the
 *    configured issuer/currency, the connection and the intent. Anything that
 *    does not match is refused before the wallet is ever asked to sign.
 *  - Backend operation views are the source of truth for state. A wallet
 *    "submitted" result is a hint and never marks debt paid or icUSD delivered.
 *  - Request nonces are 32 random bytes from crypto, persisted per intent so a
 *    retried click reuses them (the backend de-duplicates on the nonce).
 *  - Wire shapes come from the generated declarations. The lane is switched on
 *    and off by `get_xrpl_icusd_config`; when the actor cannot answer it the UI
 *    shows "not available" instead of crashing.
 */

import type {
  _SERVICE,
  ClaimAction,
  OperationPage,
  OperationView,
  PublicXrplIcusdConfig,
  RepaymentIntentView,
  XrplIcusdError,
  XrplNetwork,
} from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did';
import type {
  XrplConnection,
  XrplPaymentTx,
  XrplTrustSetTx,
} from './xrplWalletService';

// --- exact money -------------------------------------------------------------

export const E8S = 100_000_000n;
const U64_MAX = (1n << 64n) - 1n;

/** Parse a decimal icUSD string (max 8 decimals, no sign/exponent) to e8s, or null. */
export function parseE8s(text: string): bigint | null {
  const t = (text ?? '').trim();
  const m = /^(\d+)(?:\.(\d{1,8}))?$/.exec(t);
  if (!m) return null;
  const v = BigInt(m[1]) * E8S + BigInt((m[2] ?? '').padEnd(8, '0') || '0');
  return v <= U64_MAX ? v : null;
}

/** Format e8s as a decimal string with at least 2 decimals. */
export function formatE8s(v: bigint): string {
  const neg = v < 0n;
  const a = neg ? -v : v;
  const whole = a / E8S;
  let frac = (a % E8S).toString().padStart(8, '0').replace(/0+$/, '');
  if (frac.length < 2) frac = frac.padEnd(2, '0');
  return `${neg ? '-' : ''}${whole}.${frac}`;
}

/** XRPL issued-amount value (decimal string) with no trailing zeros. */
function limitDecimal(e8s: bigint): string {
  const whole = e8s / E8S;
  const frac = (e8s % E8S).toString().padStart(8, '0').replace(/0+$/, '');
  return frac ? `${whole}.${frac}` : `${whole}`;
}

/**
 * Parse an XRPL issued-amount value to e8s ONLY when it is exactly
 * representable (plain decimal, no exponent, no non-zero digits beyond 8).
 */
export function parseIssuedValueToE8s(value: unknown): bigint | null {
  if (typeof value !== 'string') return null;
  const m = /^(\d+)(?:\.(\d+))?$/.exec(value.trim());
  if (!m) return null;
  const frac = m[2] ?? '';
  if (frac.length > 8 && /[1-9]/.test(frac.slice(8))) return null;
  const v = BigInt(m[1]) * E8S + BigInt(frac.slice(0, 8).padEnd(8, '0') || '0');
  return v <= U64_MAX ? v : null;
}

// --- config ------------------------------------------------------------------

export type XrplNetworkText = 'mainnet' | 'testnet';

export interface XrplIcusdConfigView {
  enabled: boolean;
  ready: boolean;
  issuancePaused: boolean;
  settlementPaused: boolean;
  issuerAddress: string | null;
  currencyHex: string;
  network: XrplNetworkText | null;
  perOperationCapE8s: bigint;
  outstandingCapE8s: bigint;
  configVersion: bigint;
}

export function networkText(v: unknown): XrplNetworkText | null {
  if (v && typeof v === 'object') {
    if ('Mainnet' in (v as object)) return 'mainnet';
    if ('Testnet' in (v as object)) return 'testnet';
  }
  return null;
}

function networkVariant(n: XrplNetworkText): XrplNetwork {
  return n === 'mainnet' ? { Mainnet: null } : { Testnet: null };
}

function opt<T>(v: [] | [T] | undefined | null): T | null {
  return Array.isArray(v) && v.length > 0 ? (v[0] as T) : null;
}

export function mapConfig(raw: PublicXrplIcusdConfig): XrplIcusdConfigView {
  return {
    enabled: !!raw.enabled,
    ready: !!raw.ready,
    issuancePaused: !!raw.issuance_paused,
    settlementPaused: !!raw.settlement_paused,
    issuerAddress: opt<string>(raw.issuer_address),
    currencyHex: String(raw.currency_hex ?? ''),
    network: networkText(opt(raw.network)),
    perOperationCapE8s: BigInt(raw.per_operation_cap_e8s ?? 0n),
    outstandingCapE8s: BigInt(raw.outstanding_cap_e8s ?? 0n),
    configVersion: BigInt(raw.config_version ?? 0n),
  };
}

export interface ConfigGate {
  state: 'active' | 'disabled' | 'not_ready' | 'paused';
  message: string;
}

/** When not 'active' the UI explains and offers no new actions. */
export function configGate(cfg: XrplIcusdConfigView | null): ConfigGate {
  if (!cfg) return { state: 'not_ready', message: 'XRPL icUSD status is not loaded yet.' };
  if (!cfg.enabled) return { state: 'disabled', message: 'XRPL icUSD is not enabled yet.' };
  if (!cfg.ready || !cfg.issuerAddress || !cfg.network) {
    return { state: 'not_ready', message: 'XRPL icUSD is being set up. Borrowing and repayment on XRPL are not available yet.' };
  }
  if (cfg.issuancePaused || cfg.settlementPaused) {
    return {
      state: 'paused',
      message: 'XRPL icUSD is paused. New borrowing and repayment on XRPL are unavailable; operations already in progress keep reconciling.',
    };
  }
  return { state: 'active', message: '' };
}

// --- trust line --------------------------------------------------------------

const CURRENCY_HEX = /^[0-9A-Fa-f]{40}$/;

/**
 * Build the one-time "Enable icUSD" TrustSet from backend config and the
 * current wallet connection only. Throws on any mismatch.
 */
export function buildTrustSetTx(
  cfg: XrplIcusdConfigView,
  conn: XrplConnection | null,
  limitE8s?: bigint
): XrplTrustSetTx {
  if (!conn) throw new Error('Connect an XRPL wallet first.');
  const gate = configGate(cfg);
  if (gate.state === 'disabled' || gate.state === 'not_ready' || !cfg.issuerAddress || !cfg.network) {
    throw new Error('XRPL icUSD is not available yet.');
  }
  if (conn.network !== cfg.network) {
    throw new Error(`Wallet is on ${conn.network}; XRPL icUSD is on ${cfg.network}. Switch network and reconnect.`);
  }
  if (!CURRENCY_HEX.test(cfg.currencyHex)) throw new Error('Backend returned an invalid currency identifier.');
  const limit = limitE8s ?? cfg.outstandingCapE8s;
  if (limit <= 0n) throw new Error('Trust line limit must be positive.');
  return {
    TransactionType: 'TrustSet',
    LimitAmount: { currency: cfg.currencyHex.toUpperCase(), issuer: cfg.issuerAddress, value: limitDecimal(limit) },
  };
}

// --- borrow math -------------------------------------------------------------

export interface BorrowEstimate {
  /** Debt the vault records. */
  grossE8s: bigint;
  /** Borrowing fee (estimate until the backend returns the operation). */
  feeE8s: bigint;
  /** icUSD to be delivered on XRPL. */
  netE8s: bigint;
}

/** Display estimate: gross debt, fee and net delivered are three different numbers. */
export function estimateBorrow(grossE8s: bigint, feeRate: number): BorrowEstimate {
  const ppm = Number.isFinite(feeRate) && feeRate > 0 ? BigInt(Math.round(feeRate * 1_000_000)) : 0n;
  const fee = (grossE8s * ppm) / 1_000_000n;
  return { grossE8s, feeE8s: fee, netE8s: grossE8s - fee };
}

// --- operation mapping -------------------------------------------------------

export type ClaimActionName = 'ReceiveCanonical' | 'ReissueXrpl' | 'CancelUnexecutedRepayment';

export type OpUiState =
  | 'processing'
  | 'delivery_pending'
  | 'delivered'
  | 'repay_payment_pending'
  | 'repay_xrpl_received'
  | 'repay_debt_updated'
  | 'import_pending'
  | 'complete'
  | 'held'
  | 'refund_pending'
  | 'recoverable_error'
  | 'cancelled';

export interface OperationUi {
  id: bigint;
  kind: 'Borrow' | 'Repayment' | 'Import' | 'Refund';
  phase: string;
  state: OpUiState;
  label: string;
  detail: string;
  terminal: boolean;
  vaultId: bigint | null;
  grossE8s: bigint;
  feeE8s: bigint;
  netE8s: bigint;
  /** Set only once the backend says the token was delivered. */
  deliveredE8s: bigint | null;
  debtAppliedE8s: bigint;
  observedExact: string | null;
  heldExact: string | null;
  recipient: string | null;
  xrplTxHashes: string[];
  nextAction: string;
  /** Held-payment settlement cutoff (ns since epoch), when the backend set one. */
  settlementCutoffNs: bigint | null;
  canResume: boolean;
  claimActions: ClaimActionName[];
  notes: string[];
  createdAtNs: bigint;
  updatedAtNs: bigint;
  raw: OperationView;
}

function variantKey(v: unknown): string {
  return v && typeof v === 'object' ? (Object.keys(v as object)[0] ?? '') : '';
}

const TERMINAL_PHASES = new Set(['Delivered', 'Complete', 'ClaimResolved', 'Cancelled']);

export function claimActionsFor(op: Pick<OperationView, 'next_action' | 'resolution' | 'kind' | 'phase'>): ClaimActionName[] {
  if (variantKey(op.next_action) !== 'ResolveClaim') return [];
  if (opt(op.resolution)) return [];
  const kind = variantKey(op.kind);
  const phase = variantKey(op.phase);
  if (kind === 'Repayment' && phase === 'EscrowBurnPending') return ['CancelUnexecutedRepayment'];
  return ['ReceiveCanonical', 'ReissueXrpl'];
}

export function mapOperation(op: OperationView): OperationUi {
  const kind = variantKey(op.kind) as OperationUi['kind'];
  const phase = variantKey(op.phase);
  const nextAction = variantKey(op.next_action) || 'None';
  const gross = BigInt(op.gross_e8s ?? 0n);
  const fee = BigInt(op.fee_e8s ?? 0n);
  const net = BigInt(op.net_e8s ?? 0n);
  const terminal = TERMINAL_PHASES.has(phase);
  const claimActions = claimActionsFor(op);

  let state: OpUiState = 'processing';
  let label = 'Processing';
  let detail = '';
  let deliveredE8s: bigint | null = null;

  if (phase === 'Held') {
    state = 'held'; label = 'Held for recovery'; detail = 'Your payment was received but could not be applied. Choose how to recover it.';
  } else if (phase === 'RefundPending') {
    state = 'refund_pending'; label = 'Refund pending'; detail = 'Choose how to receive your funds.';
  } else if (phase === 'Cancelled') {
    state = 'cancelled'; label = 'Cancelled'; detail = '';
  } else if (kind === 'Borrow' || kind === 'Import' || kind === 'Refund') {
    if (phase === 'Delivered' || phase === 'Complete' || phase === 'ClaimResolved') {
      state = phase === 'ClaimResolved' ? 'complete' : 'delivered';
      label = phase === 'ClaimResolved' ? 'Resolved' : 'icUSD delivered on XRPL';
      if (state === 'delivered') deliveredE8s = net;
    } else if (phase === 'DebtCommitted' || phase === 'IssuePrepared' || phase === 'Signed' || phase === 'Submitted') {
      state = kind === 'Borrow' ? 'delivery_pending' : 'import_pending';
      label = kind === 'Borrow' ? 'Debt recorded; icUSD delivery pending' : 'Import recorded; icUSD delivery pending';
      detail = 'Waiting for XRPL to validate the issuance. Nothing is lost if you close this page.';
    } else {
      state = 'processing'; label = 'Processing on Rumi'; detail = 'The Rumi canister is preparing this operation.';
    }
  } else if (kind === 'Repayment') {
    if (phase === 'IntentPrepared') {
      state = 'repay_payment_pending'; label = 'Payment pending'; detail = 'Waiting for your icUSD payment on XRPL.';
    } else if (phase === 'ReturnObserved' || phase === 'ReturnVerified' || phase === 'EscrowBurnPending') {
      state = 'repay_xrpl_received'; label = 'XRPL received'; detail = 'Your payment was seen. Rumi is updating the debt.';
    } else if (phase === 'DebtApplied') {
      state = 'repay_debt_updated'; label = 'Debt updated'; detail = '';
    } else if (phase === 'Complete' || phase === 'ClaimResolved') {
      state = 'complete'; label = 'Repayment complete'; detail = '';
    }
  }

  if (nextAction === 'ContactOperator' && !terminal) {
    state = 'recoverable_error';
    label = 'Needs attention';
    detail = 'Rumi could not finish this automatically. Try Resume; if it persists, contact support with the operation number.';
  }

  const canResume =
    !terminal && state !== 'held' && state !== 'refund_pending' && state !== 'cancelled' && claimActions.length === 0;

  return {
    id: BigInt(op.operation_id),
    kind,
    phase,
    state,
    label,
    detail,
    terminal,
    vaultId: opt<bigint>(op.vault_id),
    grossE8s: gross,
    feeE8s: fee,
    netE8s: net,
    deliveredE8s,
    debtAppliedE8s: BigInt(op.debt_applied_e8s ?? 0n),
    observedExact: opt<string>(op.observed_exact_amount),
    heldExact: opt<string>(op.held_exact_amount),
    recipient: opt<string>(op.recipient),
    xrplTxHashes: op.xrpl_tx_hashes.map(String),
    nextAction,
    settlementCutoffNs: opt<bigint>(op.settlement_cutoff_ns),
    canResume,
    claimActions,
    notes: op.notes.map(String),
    createdAtNs: BigInt(op.created_at_ns ?? 0n),
    updatedAtNs: BigInt(op.updated_at_ns ?? 0n),
    raw: op,
  };
}

export interface RepaymentSummary {
  /** Exactly what the backend verified as received. */
  receivedExact: string | null;
  appliedE8s: bigint;
  heldExact: string | null;
  /** Backend/vault residual debt if the caller knows it, else null. */
  remainingDebtE8s: bigint | null;
}

export function repaymentSummary(op: OperationUi, remainingDebtE8s: bigint | null): RepaymentSummary {
  return {
    receivedExact: op.observedExact,
    appliedE8s: op.debtAppliedE8s,
    heldExact: op.heldExact,
    remainingDebtE8s,
  };
}

// --- repayment payment -------------------------------------------------------

export interface RepaymentIntent {
  operationId: bigint;
  issuer: string;
  currency: string;
  payer: string;
  destinationTag: number;
  maxE8s: bigint;
  network: XrplNetworkText;
  paymentJson: string;
  quoteTimeNs: bigint;
}

export class RepaymentMismatchError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'RepaymentMismatchError';
  }
}

const TF_PARTIAL_PAYMENT = 0x00020000;
const MAX_TAG = 0xffffffff;

/**
 * Build the XRPL issued-token Payment from the backend intent. Nothing is taken
 * from the form: every field must match the configured issuer/currency, the
 * intent and the current connection, and the amount never exceeds max_e8s.
 */
export function buildRepaymentPayment(
  intent: RepaymentIntent,
  cfg: XrplIcusdConfigView,
  conn: XrplConnection | null
): XrplPaymentTx {
  const fail = (why: string): never => {
    throw new RepaymentMismatchError(`Refused to sign: ${why}. Nothing was sent.`);
  };
  if (!conn) fail('no XRPL wallet is connected');
  if (!cfg.issuerAddress || !cfg.network) fail('XRPL icUSD is not configured');
  if (!CURRENCY_HEX.test(cfg.currencyHex)) fail('invalid configured currency');
  const issuer = cfg.issuerAddress!;
  const currency = cfg.currencyHex.toUpperCase();

  if (intent.issuer !== issuer) fail('issuer differs from the configured issuer');
  if (intent.currency.toUpperCase() !== currency) fail('currency differs from the configured icUSD');
  if (intent.network !== cfg.network) fail('intent network differs from the configured network');
  if (conn!.network !== cfg.network) fail('wallet network differs from the configured network');
  if (conn!.address !== intent.payer) fail('the connected wallet is not the payer this repayment was prepared for');
  if (!Number.isInteger(intent.destinationTag) || intent.destinationTag < 0 || intent.destinationTag > MAX_TAG) {
    fail('invalid destination tag');
  }

  let tx: any;
  try {
    tx = typeof intent.paymentJson === 'string' ? JSON.parse(intent.paymentJson) : intent.paymentJson;
  } catch {
    return fail('the prepared payment could not be read');
  }
  if (!tx || typeof tx !== 'object') fail('the prepared payment is empty');

  if (tx.TransactionType !== 'Payment') fail('not a Payment');
  if (tx.Account !== undefined && tx.Account !== intent.payer) fail('payment account differs from the payer');
  if (tx.Destination !== issuer) fail('payment destination is not the issuer');
  if (tx.DestinationTag !== intent.destinationTag) fail('payment destination tag differs from the repayment intent');
  for (const forbidden of ['Paths', 'SendMax', 'DeliverMin']) {
    if (tx[forbidden] !== undefined) fail(`${forbidden} is not allowed`);
  }
  if (tx.Flags !== undefined) {
    if (typeof tx.Flags !== 'number' || (tx.Flags & TF_PARTIAL_PAYMENT) !== 0) fail('partial payments are not allowed');
  }
  const amt = tx.Amount;
  if (!amt || typeof amt !== 'object') fail('payment is not an icUSD token amount');
  if (String(amt.currency ?? '').toUpperCase() !== currency) fail('payment currency is not icUSD');
  if (amt.issuer !== issuer) fail('payment issuer is not the configured issuer');
  const e8s = parseIssuedValueToE8s(amt.value);
  if (e8s === null || e8s <= 0n) fail('payment amount is not an exact positive icUSD amount');
  if (e8s! > intent.maxE8s) fail('payment amount exceeds the approved maximum');

  return {
    TransactionType: 'Payment',
    Destination: issuer,
    DestinationTag: intent.destinationTag,
    Amount: { currency, issuer, value: String(amt.value).trim() },
  };
}

// --- request nonces ----------------------------------------------------------

const NONCE_PREFIX = 'rumi_xrpl_icusd_nonces_v1:';

function toHex(b: Uint8Array): string {
  return Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
}
function fromHex(h: string): Uint8Array | null {
  if (!/^[0-9a-f]{64}$/.test(h)) return null;
  const out = new Uint8Array(32);
  for (let i = 0; i < 32; i++) out[i] = parseInt(h.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export interface NonceStore {
  getOrCreate(intentKey: string): Uint8Array;
  forget(intentKey: string): void;
}

export function createNonceStore(opts: {
  storage?: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'> | null;
  owner: string;
  rng?: (buf: Uint8Array) => Uint8Array;
}): NonceStore {
  const storage = opts.storage === undefined ? safeLocalStorage() : opts.storage;
  const key = `${NONCE_PREFIX}${opts.owner}`;
  const rng = opts.rng ?? ((b: Uint8Array) => crypto.getRandomValues(b));
  const memory = new Map<string, string>();

  function read(): Record<string, string> {
    try {
      const raw = storage?.getItem(key);
      const parsed = raw ? JSON.parse(raw) : {};
      return parsed && typeof parsed === 'object' ? parsed : {};
    } catch {
      return {};
    }
  }
  function write(map: Record<string, string>) {
    try { storage?.setItem(key, JSON.stringify(map)); } catch { /* hints only */ }
  }

  return {
    getOrCreate(intentKey) {
      const existing = memory.get(intentKey) ?? read()[intentKey];
      const decoded = existing ? fromHex(existing) : null;
      if (decoded) {
        memory.set(intentKey, existing!);
        return decoded;
      }
      const fresh = rng(new Uint8Array(32));
      const hex = toHex(fresh);
      memory.set(intentKey, hex);
      write({ ...read(), [intentKey]: hex });
      return fresh;
    },
    forget(intentKey) {
      memory.delete(intentKey);
      const map = read();
      delete map[intentKey];
      write(map);
    },
  };
}

function safeLocalStorage(): Pick<Storage, 'getItem' | 'setItem' | 'removeItem'> | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage;
  } catch {
    return null;
  }
}

// --- errors ------------------------------------------------------------------

export function formatXrplIcusdError(err: XrplIcusdError | unknown): string {
  if (!err || typeof err !== 'object') return String(err);
  const key = Object.keys(err as object)[0];
  const val = (err as Record<string, unknown>)[key];
  const text = typeof val === 'string' && val ? val : '';
  switch (key) {
    case 'TrustLineMissing': return `Your XRPL wallet has no icUSD trust line yet. Enable icUSD first.${text ? ` (${text})` : ''}`;
    case 'Disabled': return `XRPL icUSD is not enabled.${text ? ` ${text}` : ''}`;
    case 'Paused': return `XRPL icUSD is paused.${text ? ` ${text}` : ''}`;
    case 'WrongNetwork': return `Wrong XRPL network.${text ? ` ${text}` : ''}`;
    case 'Unauthorized': return 'This operation belongs to a different account.';
    case 'AnonymousCaller': return 'Sign in to Rumi first.';
    case 'NotFound': return 'Operation not found.';
    case 'NonceConflict': return 'This request was already used with different details. Start a new request.';
    case 'LimitExceeded': return `Limit reached. ${text}`.trim();
    case 'Ambiguous': return `Outcome is not certain yet; Rumi is reconciling. ${text}`.trim();
    case 'WrongPhase': return `This operation cannot do that right now. ${text}`.trim();
    case 'InvalidArgument': return `Invalid request. ${text}`.trim();
    case 'NotApplicable': return `Not available for this vault. ${text}`.trim();
    case 'Policy': return `Blocked by protocol policy. ${text}`.trim();
    case 'CoverageUnavailable': return `Backing check is unavailable right now; try again shortly. ${text}`.trim();
    case 'MintingAccount': return `XRPL icUSD issuer is not verified yet. ${text}`.trim();
    case 'Unavailable': return `Temporarily unavailable; try again shortly. ${text}`.trim();
    case 'Internal': return text ? `Internal error: ${text}` : 'Internal error';
    default: return text ? `${key}: ${text}` : key || 'Unknown error';
  }
}

/** Error variants after which the request definitely did not (or cannot) proceed. */
function isDefinitiveError(err: unknown): boolean {
  const key = err && typeof err === 'object' ? Object.keys(err as object)[0] : '';
  return !['Internal', 'Unavailable', 'Ambiguous'].includes(key);
}

// --- API wrappers ------------------------------------------------------------

export type ApiResult<T> =
  | { ok: true; data: T }
  | { ok: false; kind: 'unavailable' | 'backend' | 'network'; error: string; definitive: boolean };

export interface XrplIcusdApi {
  getConfig(): Promise<ApiResult<XrplIcusdConfigView>>;
  listMyOperations(): Promise<ApiResult<OperationUi[]>>;
  issueFromCanonical(a: { amountE8s: bigint; recipient: string; network: XrplNetworkText; requestNonce: Uint8Array }): Promise<ApiResult<OperationUi>>;
  resume(operationId: bigint): Promise<ApiResult<OperationUi>>;
  resolveClaim(operationId: bigint, action: ClaimActionName): Promise<ApiResult<OperationUi>>;
  borrowToXrpl(a: { vaultId: bigint; grossE8s: bigint; recipient: string; network: XrplNetworkText; requestNonce: Uint8Array }): Promise<ApiResult<OperationUi>>;
  prepareRepayment(a: {
    vaultId: bigint; maxE8s: bigint; payer: string; network: XrplNetworkText; requestNonce: Uint8Array;
    fullClose: boolean; topUpOperation: bigint | null;
  }): Promise<ApiResult<RepaymentIntent>>;
  observeRepayment(operationId: bigint, txHash: string): Promise<ApiResult<OperationUi>>;
}

export interface XrplIcusdApiDeps {
  /** Authenticated actor (caller-scoped reads and all updates). */
  getActor: () => Promise<Partial<_SERVICE>>;
  /** Anonymous actor for public config reads. */
  getPublicActor: () => Promise<Partial<_SERVICE>>;
}

const MAX_OPERATION_PAGES = 4;
const PAGE_LIMIT = 50;

export function createXrplIcusdApi(deps: XrplIcusdApiDeps): XrplIcusdApi {
  const networkFail = (e: unknown): ApiResult<never> => ({
    ok: false, kind: 'network', definitive: false,
    error: e instanceof Error ? e.message : String(e),
  });
  const backendFail = (err: XrplIcusdError): ApiResult<never> => ({
    ok: false, kind: 'backend', definitive: isDefinitiveError(err), error: formatXrplIcusdError(err),
  });

  /** Run an update call that returns XrplOperationResult and map it. */
  async function opResult(
    call: (actor: _SERVICE) => Promise<{ Ok: OperationView } | { Err: XrplIcusdError }>
  ): Promise<ApiResult<OperationUi>> {
    try {
      const actor = (await deps.getActor()) as _SERVICE;
      const r = await call(actor);
      if ('Err' in r) return backendFail(r.Err);
      return { ok: true, data: mapOperation(r.Ok) };
    } catch (e) {
      return networkFail(e);
    }
  }

  return {
    async getConfig() {
      try {
        const actor = await deps.getPublicActor();
        if (typeof actor?.get_xrpl_icusd_config !== 'function') {
          return { ok: false, kind: 'unavailable', definitive: true, error: 'XRPL icUSD is not available on this deployment yet.' };
        }
        return { ok: true, data: mapConfig(await actor.get_xrpl_icusd_config()) };
      } catch (e) {
        return networkFail(e);
      }
    },

    async listMyOperations() {
      try {
        const actor = (await deps.getActor()) as _SERVICE;
        const out: OperationUi[] = [];
        let cursor: [] | [bigint] = [];
        for (let i = 0; i < MAX_OPERATION_PAGES; i++) {
          const page: OperationPage = await actor.get_my_xrpl_operations(cursor, PAGE_LIMIT);
          out.push(...page.items.map(mapOperation));
          const next: bigint | null = opt<bigint>(page.next_cursor);
          if (next === null) break;
          cursor = [next];
        }
        return { ok: true, data: out };
      } catch (e) {
        return networkFail(e);
      }
    },

    issueFromCanonical(a) {
      return opResult((actor) => actor.issue_xrpl_from_canonical({
        request_nonce: a.requestNonce, recipient: a.recipient, network: networkVariant(a.network), amount_e8s: a.amountE8s,
      }));
    },
    resume(operationId) {
      return opResult((actor) => actor.resume_xrpl_operation(operationId));
    },
    resolveClaim(operationId, action) {
      return opResult((actor) => actor.resolve_xrpl_claim({
        operation_id: operationId, action: { [action]: null } as ClaimAction,
      }));
    },
    borrowToXrpl(a) {
      return opResult((actor) => actor.borrow_to_xrpl({
        vault_id: a.vaultId, gross_e8s: a.grossE8s, recipient: a.recipient,
        network: networkVariant(a.network), request_nonce: a.requestNonce,
      }));
    },
    async prepareRepayment(a) {
      try {
        const actor = (await deps.getActor()) as _SERVICE;
        const r = await actor.prepare_xrpl_repayment({
          vault_id: a.vaultId, max_e8s: a.maxE8s, payer: a.payer, network: networkVariant(a.network),
          request_nonce: a.requestNonce, full_close: a.fullClose,
          top_up_operation: a.topUpOperation === null ? [] : [a.topUpOperation],
        });
        if ('Err' in r) return backendFail(r.Err);
        const v: RepaymentIntentView = r.Ok;
        const net = networkText(v.network);
        if (!net) return { ok: false, kind: 'backend', definitive: false, error: 'Backend returned an unknown network.' };
        return {
          ok: true,
          data: {
            operationId: v.operation_id, issuer: v.issuer, currency: v.currency,
            payer: v.payer, destinationTag: v.destination_tag, maxE8s: v.max_e8s,
            network: net, paymentJson: v.payment_json, quoteTimeNs: v.quote_time_ns,
          },
        };
      } catch (e) {
        return networkFail(e);
      }
    },
    /** Hint only (the backend throttles it per operation); returns the current operation view. */
    async observeRepayment(operationId, txHash) {
      return opResult((actor) => actor.observe_xrpl_repayment({ operation_id: operationId, tx_hash: txHash }));
    },
  };
}

/** Default API wired to the app's backend actors (lazy, so tests need no stores). */
export async function createDefaultXrplIcusdApi(): Promise<XrplIcusdApi> {
  const [{ walletStore }, { CONFIG }, idl, { publicActor }] = await Promise.all([
    import('../stores/wallet'),
    import('../config'),
    import('$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js'),
    import('./protocol/apiClient'),
  ]);
  return createXrplIcusdApi({
    getActor: () => walletStore.getActor(CONFIG.currentCanisterId, idl.idlFactory) as Promise<_SERVICE>,
    getPublicActor: async () => publicActor as unknown as _SERVICE,
  });
}

export { networkVariant };
