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
  /** Pinned to the frozen connection address by the adapter before Xaman sees it. */
  Account?: string;
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
  /** Pinned to the frozen connection address by the adapter before Xaman sees it. */
  Account?: string;
  LimitAmount: { currency: string; issuer: string; value: string };
}

export type XrplPaymentOrTrustSet = XrplPaymentTx | XrplTrustSetTx;

export interface WalletSubmission {
  requestId: string;
  status: 'pending' | 'submitted' | 'rejected';
  signed: boolean;
  hash: string | null;
  /**
   * Why a request was treated as rejected when it was NOT a plain user
   * rejection (signed by a different account, or refused by the XRP Ledger).
   */
  warning?: string;
  /** Engine result Xaman reported after dispatch (e.g. tesSUCCESS), if any. */
  engineResult?: string | null;
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
  /**
   * Resume a stored request. `expectedAccount` pins the signer; when omitted the
   * account frozen at creation (persisted with the request id) is used.
   * An unsigned request re-emits its signing prompt.
   */
  resumeRequest(requestId: string, expectedAccount?: string): Promise<WalletSubmission>;
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
const REQUEST_ACCOUNT_KEY = 'rumi_xrpl_wallet_request_accounts_v1';
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
 * Only a payload that is signed, carries a transaction id, was signed by the
 * expected account and was not refused by the XRP Ledger is 'submitted'.
 *
 * `expectedAccount` is the connection address frozen when the request was
 * created: a payload signed by any other account is 'rejected' with a warning.
 */
export function payloadToSubmission(
  requestId: string,
  payload: any | null | undefined,
  expectedAccount?: string
): WalletSubmission {
  const meta = payload?.meta;
  if (!meta) return { requestId, status: 'pending', signed: false, hash: null };
  const txid = typeof payload?.response?.txid === 'string' && payload.response.txid ? payload.response.txid : null;
  if (meta.signed) {
    const signer = payload?.response?.account;
    if (expectedAccount && signer !== expectedAccount) {
      return {
        requestId, status: 'rejected', signed: false, hash: null,
        warning: `The request was signed by ${typeof signer === 'string' && signer ? signer : 'a different account'}, not the connected wallet ${expectedAccount}. It was not used.`,
      };
    }
    const engine = typeof payload?.response?.dispatched_result === 'string' && payload.response.dispatched_result
      ? payload.response.dispatched_result
      : null;
    if (engine && engine !== 'tesSUCCESS') {
      return {
        requestId, status: 'rejected', signed: false, hash: null, engineResult: engine,
        warning: `The XRP Ledger refused this transaction (${engine}). Nothing was applied.`,
      };
    }
    const extra = engine ? { engineResult: engine } : {};
    return txid
      ? { requestId, status: 'submitted', signed: true, hash: txid, ...extra }
      : { requestId, status: 'pending', signed: true, hash: null, ...extra };
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
  // requestId -> account that must sign it (frozen when the request was created).
  const memoryAccounts = new Map<string, string>();
  function readAccounts(): Record<string, string> {
    try {
      const raw = storage?.getItem(REQUEST_ACCOUNT_KEY);
      const parsed = raw ? JSON.parse(raw) : {};
      return parsed && typeof parsed === 'object' ? parsed : {};
    } catch {
      return {};
    }
  }
  function rememberAccount(requestId: string, account: string) {
    memoryAccounts.set(requestId, account);
    try {
      storage?.setItem(REQUEST_ACCOUNT_KEY, JSON.stringify({ ...readAccounts(), [requestId]: account }));
    } catch { /* hints only */ }
  }
  function accountFor(requestId: string): string | undefined {
    return memoryAccounts.get(requestId) ?? readAccounts()[requestId];
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

  function emitPrompt(requestId: string, payload: any) {
    const deepLink = payload?.next?.always;
    const qrPng = payload?.refs?.qr_png;
    if (typeof deepLink !== 'string' && typeof qrPng !== 'string') return;
    promptHandlers.forEach((h) =>
      h({ requestId, deepLink: typeof deepLink === 'string' ? deepLink : '', qrPng: typeof qrPng === 'string' ? qrPng : null })
    );
  }

  async function waitForResolution(
    requestId: string,
    s: XamanSdkLike,
    expectedAccount: string | undefined,
    reemitPrompt = false
  ): Promise<WalletSubmission> {
    let waited = 0;
    let last: WalletSubmission = { requestId, status: 'pending', signed: false, hash: null };
    let prompted = !reemitPrompt;
    for (;;) {
      let payload: any = null;
      try {
        payload = await s.payload!.get(requestId);
      } catch {
        payload = null;
      }
      last = payloadToSubmission(requestId, payload, expectedAccount);
      if (!prompted && last.status === 'pending' && !last.signed && payload) {
        prompted = true;
        emitPrompt(requestId, payload);
      }
      if (last.status !== 'pending') return last;
      if (waited >= maxWaitMs) return last;
      await sleep(pollMs);
      waited += pollMs;
    }
  }

  function forgetKey(key: string) {
    memoryRequests.delete(key);
    const map = readRequests();
    delete map[key];
    writeRequests(map);
  }

  /**
   * A rejected/expired request must not pin its key forever: a retry creates a
   * fresh one. A TrustSet is also released once it returns a hash, so an old
   * signed payload can never be replayed as the answer to a later "Enable icUSD".
   */
  function settleKeyed(key: string | undefined, sub: WalletSubmission, tx: XrplPaymentOrTrustSet): WalletSubmission {
    if (!key) return sub;
    if (sub.status === 'rejected' || (tx.TransactionType === 'TrustSet' && sub.hash)) forgetKey(key);
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
      // Freeze the signer: Xaman must sign as exactly this account, and a result
      // from any other account is never accepted.
      const account = connection.address;
      if ((tx as any).Account !== undefined && (tx as any).Account !== account) {
        throw new Error('The transaction account does not match the connected wallet.');
      }
      const existing = key ? lookupRequest(key) : undefined;
      if (existing) {
        const pinned = accountFor(existing) ?? account;
        return settleKeyed(key, await waitForResolution(existing, s, pinned, true), tx);
      }
      const txjson = { ...tx, Account: account };

      const created = await s.payload.create({
        txjson,
        options: {
          submit: true,
          expire: PAYLOAD_EXPIRE_MINUTES,
          force_network: expected.toUpperCase(),
        },
        custom_meta: { instruction: 'Rumi Protocol' },
      });
      if (!created?.uuid) throw new Error('Xaman did not create the signing request.');
      rememberAccount(created.uuid, account);
      if (key) rememberRequest(key, created.uuid);
      promptHandlers.forEach((h) =>
        h({ requestId: created.uuid, deepLink: created.next?.always ?? '', qrPng: created.refs?.qr_png ?? null })
      );
      return settleKeyed(key, await waitForResolution(created.uuid, s, account), tx);
    },
    async resumeRequest(requestId, expectedAccount) {
      const s = await sdk();
      if (!s.payload) throw new Error('Xaman payload API is unavailable. Reconnect and try again.');
      return waitForResolution(requestId, s, expectedAccount ?? accountFor(requestId) ?? connection?.address, true);
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
