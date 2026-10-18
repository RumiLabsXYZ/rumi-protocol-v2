/**
 * XRPL wallet adapter (Task 1 of the XRPL icUSD plan).
 *
 * A thin, UI-agnostic interface over "one connected XRPL wallet". The first
 * concrete adapter is Xaman through the unified `xumm` SDK in browser PKCE mode:
 * the SDK is constructed with the PUBLIC API key only, the user authorizes in
 * the Xaman app (desktop QR / mobile deep link) and payloads are signed and
 * submitted by Xaman. No credential beyond the public key exists in this
 * bundle, and none is needed.
 *
 * Authority notes:
 *  - A wallet result (signed, tx hash) is only a HINT for the UI. Collateral is
 *    credited solely by the Rumi backend after it verifies the XRPL account.
 *  - A signed payload without a transaction id stays 'pending'. It is resumed by
 *    its request id; callers must never create a second payment for the same
 *    intent (the adapter enforces this per idempotency `key`).
 */

export type XrplNetworkName = 'mainnet' | 'testnet' | 'devnet';

export interface XrplConnection {
  address: string;
  network: string;
}

export interface XrplPaymentTx {
  TransactionType: 'Payment';
  Destination: string;
  /**
   * Native XRP amount in drops (decimal string), or an exact issued-token
   * amount (XRPL icUSD repayment). Never carries Paths/SendMax/Flags.
   */
  Amount: string | { currency: string; issuer: string; value: string };
  DestinationTag?: number;
}

export interface XrplTrustSetTx {
  TransactionType: 'TrustSet';
  LimitAmount: { currency: string; issuer: string; value: string };
}

export type XrplPaymentOrTrustSet = XrplPaymentTx | XrplTrustSetTx;

export interface WalletSubmission {
  requestId: string;
  status: 'pending' | 'submitted' | 'rejected';
  signed: boolean;
  hash: string | null;
}

export interface WalletSigningPrompt {
  requestId: string;
  /** Mobile deep link / universal link to open the request in the wallet. */
  deepLink: string;
  /** QR image URL for desktop approval. */
  qrPng: string | null;
}

export interface SignOptions {
  /** Idempotency key: the same key resumes the stored request, never re-creates it. */
  key?: string;
}

export interface WalletAvailability {
  available: boolean;
  reason?: string;
}

export interface XrplWalletAdapter {
  readonly id: string;
  availability(): WalletAvailability;
  connect(): Promise<XrplConnection>;
  getConnection(): XrplConnection | null;
  signAndSubmit(tx: XrplPaymentOrTrustSet, opts?: SignOptions): Promise<WalletSubmission>;
  resumeRequest(requestId: string): Promise<WalletSubmission>;
  disconnect(): void;
  onChange(handler: (conn: XrplConnection | null) => void): () => void;
  /** Deep link / QR for a freshly created request. */
  onPrompt(handler: (prompt: WalletSigningPrompt) => void): () => void;
  /** Restore a previous PKCE session after reload, if there is one. */
  restore?(): Promise<XrplConnection | null>;
}

// --- Xaman ---------------------------------------------------------------

/** The subset of the `xumm` SDK surface this adapter uses. */
export interface XamanSdkLike {
  on(event: string, handler: (...args: any[]) => void): unknown;
  off?(event: string, handler: (...args: any[]) => void): unknown;
  authorize(): Promise<unknown>;
  logout(): Promise<void>;
  user: {
    account: Promise<string | undefined>;
    networkId: Promise<number | undefined>;
    networkType: Promise<string | undefined>;
  };
  payload?: {
    create(body: any): Promise<{ uuid: string; next: { always: string }; refs: { qr_png: string } } | null | undefined>;
    get(uuid: string): Promise<any | null>;
  };
}

export interface XamanAdapterOptions {
  /** Public Xaman app API key (VITE_XAMAN_API_KEY). Empty means unavailable. */
  apiKey: string | undefined;
  expectedNetwork: XrplNetworkName;
  /** Lazy SDK loader; receives the public key only. */
  loadSdk?: (apiKey: string) => Promise<XamanSdkLike>;
  storage?: Pick<Storage, 'getItem' | 'setItem' | 'removeItem'> | null;
  sleep?: (ms: number) => Promise<void>;
  /** Poll cadence while waiting for the user to approve in Xaman. */
  pollIntervalMs?: number;
  /** How long one signAndSubmit/resume call waits before returning 'pending'. */
  maxWaitMs?: number;
}

const REQUEST_STORE_KEY = 'rumi_xrpl_wallet_requests_v1';
const DEFAULT_POLL_MS = 2000;
const DEFAULT_MAX_WAIT_MS = 120_000;
/** Xaman payload lifetime in minutes. */
const PAYLOAD_EXPIRE_MINUTES = 15;

const XAMAN_NETWORK_BY_ID: Record<number, XrplNetworkName> = { 0: 'mainnet', 1: 'testnet', 2: 'devnet' };

function networkFromXaman(networkId: number | undefined, networkType: string | undefined): string {
  if (typeof networkId === 'number' && XAMAN_NETWORK_BY_ID[networkId]) return XAMAN_NETWORK_BY_ID[networkId];
  const t = (networkType ?? '').toUpperCase();
  if (t === 'MAINNET') return 'mainnet';
  if (t === 'TESTNET') return 'testnet';
  if (t === 'DEVNET') return 'devnet';
  return t ? t.toLowerCase() : 'unknown';
}

/**
 * Map a Xaman payload (as returned by `payload.get`) to a WalletSubmission.
 * Only a payload that is signed AND carries a transaction id is 'submitted'.
 */
export function payloadToSubmission(requestId: string, payload: any | null | undefined): WalletSubmission {
  const meta = payload?.meta;
  if (!meta) return { requestId, status: 'pending', signed: false, hash: null };
  const txid = typeof payload?.response?.txid === 'string' && payload.response.txid ? payload.response.txid : null;
  if (meta.signed) {
    return txid
      ? { requestId, status: 'submitted', signed: true, hash: txid }
      : { requestId, status: 'pending', signed: true, hash: null };
  }
  if (meta.cancelled || meta.expired || meta.resolved) {
    return { requestId, status: 'rejected', signed: false, hash: null };
  }
  return { requestId, status: 'pending', signed: false, hash: null };
}

function defaultStorage(): XamanAdapterOptions['storage'] {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage;
  } catch {
    return null;
  }
}

async function defaultLoadSdk(apiKey: string): Promise<XamanSdkLike> {
  const mod = await import('xumm');
  // PKCE browser mode: the public API key is the ONLY constructor argument.
  return new mod.Xumm(apiKey) as unknown as XamanSdkLike;
}

export function createXamanAdapter(options: XamanAdapterOptions): XrplWalletAdapter {
  const apiKey = (options.apiKey ?? '').trim();
  const expected = options.expectedNetwork;
  const loadSdk = options.loadSdk ?? defaultLoadSdk;
  const storage = options.storage === undefined ? defaultStorage() : options.storage;
  const sleep = options.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const pollMs = options.pollIntervalMs ?? DEFAULT_POLL_MS;
  const maxWaitMs = options.maxWaitMs ?? DEFAULT_MAX_WAIT_MS;

  let sdkPromise: Promise<XamanSdkLike> | null = null;
  let connection: XrplConnection | null = null;
  const changeHandlers = new Set<(c: XrplConnection | null) => void>();
  const promptHandlers = new Set<(p: WalletSigningPrompt) => void>();

  // --- request id persistence (hints only) ---
  function readRequests(): Record<string, string> {
    try {
      const raw = storage?.getItem(REQUEST_STORE_KEY);
      const parsed = raw ? JSON.parse(raw) : {};
      return parsed && typeof parsed === 'object' ? parsed : {};
    } catch {
      return {};
    }
  }
  function writeRequests(map: Record<string, string>) {
    try {
      storage?.setItem(REQUEST_STORE_KEY, JSON.stringify(map));
    } catch {
      /* hints only */
    }
  }
  // In-memory mirror so a throwing/absent localStorage still avoids duplicates in-session.
  const memoryRequests = new Map<string, string>();
  function lookupRequest(key: string): string | undefined {
    return memoryRequests.get(key) ?? readRequests()[key];
  }
  function rememberRequest(key: string, requestId: string) {
    memoryRequests.set(key, requestId);
    writeRequests({ ...readRequests(), [key]: requestId });
  }

  function setConnection(next: XrplConnection | null) {
    const same = connection?.address === next?.address && connection?.network === next?.network;
    connection = next;
    if (!same) changeHandlers.forEach((h) => h(next));
  }

  function sdk(): Promise<XamanSdkLike> {
    if (!apiKey) return Promise.reject(new Error('Xaman is not configured (missing VITE_XAMAN_API_KEY).'));
    sdkPromise ??= loadSdk(apiKey).then((s) => {
      s.on('logout', () => setConnection(null));
      return s;
    });
    sdkPromise.catch(() => { sdkPromise = null; });
    return sdkPromise;
  }

  async function readConnection(s: XamanSdkLike): Promise<XrplConnection | null> {
    const [account, networkId, networkType] = await Promise.all([s.user.account, s.user.networkId, s.user.networkType]);
    if (!account) return null;
    return { address: account, network: networkFromXaman(networkId, networkType) };
  }

  async function waitForResolution(requestId: string, s: XamanSdkLike): Promise<WalletSubmission> {
    let waited = 0;
    let last: WalletSubmission = { requestId, status: 'pending', signed: false, hash: null };
    for (;;) {
      let payload: any = null;
      try {
        payload = await s.payload!.get(requestId);
      } catch {
        payload = null;
      }
      last = payloadToSubmission(requestId, payload);
      if (last.status !== 'pending') return last;
      if (waited >= maxWaitMs) return last;
      await sleep(pollMs);
      waited += pollMs;
    }
  }

  /** A rejected/expired request must not pin its key forever: a retry creates a fresh one. */
  function settleKeyed(key: string | undefined, sub: WalletSubmission): WalletSubmission {
    if (key && sub.status === 'rejected') {
      memoryRequests.delete(key);
      const map = readRequests();
      delete map[key];
      writeRequests(map);
    }
    return sub;
  }

  const adapter: XrplWalletAdapter = {
    id: 'xaman',
    availability() {
      return apiKey
        ? { available: true }
        : { available: false, reason: 'Xaman is not configured for this deployment.' };
    },
    async connect() {
      const s = await sdk();
      await s.authorize();
      const conn = await readConnection(s);
      if (!conn) throw new Error('Xaman did not return an account.');
      if (conn.network !== expected) {
        try { await s.logout(); } catch { /* ignore */ }
        setConnection(null);
        throw new Error(`Xaman is on the ${conn.network} network; Rumi expects ${expected}. Switch network in Xaman and reconnect.`);
      }
      setConnection(conn);
      return conn;
    },
    async restore() {
      if (!apiKey) return null;
      try {
        const s = await sdk();
        const conn = await Promise.race([
          readConnection(s),
          new Promise<null>((r) => setTimeout(() => r(null), 4000)),
        ]);
        if (conn && conn.network === expected) {
          setConnection(conn);
          return conn;
        }
      } catch { /* no session */ }
      return null;
    },
    getConnection() {
      return connection;
    },
    async signAndSubmit(tx, opts) {
      if (!connection) throw new Error('Connect an XRPL wallet first.');
      const s = await sdk();
      if (!s.payload) throw new Error('Xaman payload API is unavailable. Reconnect and try again.');
      const key = opts?.key;
      const existing = key ? lookupRequest(key) : undefined;
      if (existing) return settleKeyed(key!, await waitForResolution(existing, s));

      const created = await s.payload.create({
        txjson: tx,
        options: {
          submit: true,
          expire: PAYLOAD_EXPIRE_MINUTES,
          force_network: expected.toUpperCase(),
        },
        custom_meta: { instruction: 'Rumi Protocol' },
      });
      if (!created?.uuid) throw new Error('Xaman did not create the signing request.');
      if (key) rememberRequest(key, created.uuid);
      promptHandlers.forEach((h) =>
        h({ requestId: created.uuid, deepLink: created.next?.always ?? '', qrPng: created.refs?.qr_png ?? null })
      );
      return settleKeyed(key, await waitForResolution(created.uuid, s));
    },
    async resumeRequest(requestId) {
      const s = await sdk();
      if (!s.payload) throw new Error('Xaman payload API is unavailable. Reconnect and try again.');
      return waitForResolution(requestId, s);
    },
    disconnect() {
      const s = sdkPromise;
      setConnection(null);
      if (s) void s.then((sd) => sd.logout()).catch(() => {});
    },
    onChange(handler) {
      changeHandlers.add(handler);
      return () => { changeHandlers.delete(handler); };
    },
    onPrompt(handler) {
      promptHandlers.add(handler);
      return () => { promptHandlers.delete(handler); };
    },
  };
  return adapter;
}

// --- configuration & singleton --------------------------------------------

/** Expected XRPL network: testnet unless explicitly configured as mainnet. */
export function expectedXrplNetwork(): XrplNetworkName {
  const raw = String((import.meta as any).env?.VITE_XRPL_NETWORK ?? '').toLowerCase();
  return raw === 'mainnet' ? 'mainnet' : raw === 'devnet' ? 'devnet' : 'testnet';
}

let singleton: XrplWalletAdapter | null = null;

export function getXrplWalletAdapter(): XrplWalletAdapter {
  singleton ??= createXamanAdapter({
    apiKey: (import.meta as any).env?.VITE_XAMAN_API_KEY,
    expectedNetwork: expectedXrplNetwork(),
  });
  return singleton;
}
