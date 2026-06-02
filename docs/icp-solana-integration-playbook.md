# ICP ↔ Solana Integration Playbook — v2 (Motoko)

*Field guide to the seam between an Internet Computer canister app and Solana.
v2 adds everything learned taking a real integration devnet → mainnet-beta,
including a live soft-launch and dogfooded deposit. Motoko backend + React/TS
frontend. Does NOT cover Rust canisters.*

> Why this exists: the IC chain-fusion docs cover the IC half, the Solana dev
> guides cover the Solana half. Every expensive bug lives at the SEAM between
> them — and most of v2's new material is the BROWSER-side seam, which neither
> set of docs addresses and which only bites on mainnet.

---

## The four-actor model (the single most important mental model)

Get who-does-what straight and most of the frontend bugs below disappear:

| Actor | Does | Never |
|-------|------|-------|
| **User's wallet (Phantom)** | SIGNS + BROADCASTS the user's transactions (`signAndSendTransaction`) | — |
| **RPC provider (e.g. domain-locked Helius)** | Browser-side READS (`getBalance`) + fetching the **recent blockhash** the wallet needs | submit the user's tx itself (that's the wallet's job) |
| **Your canister** (`*_sol`) | Server-side everything: deposit detection, threshold-Ed25519 withdrawals, sweeps. Pays cycles to call the DFINITY sol-rpc canister | — |
| **DFINITY sol-rpc canister** | The canister's gateway to Solana (consensus over many providers) | be called **from the browser** — see below |
| **Public RPC** (`api.mainnet-beta.solana.com`) | nothing you should depend on | be called from the browser — it **403s** browser origins |

Crystallized rule: **wallet for signing + broadcasting; an RPC provider for reads
and the blockhash; your canister for server-side chain ops.** Two things the
browser physically cannot do: call the public mainnet RPC (403), or call the
DFINITY sol-rpc canister (needs cycles + update call; a browser ingress call
carries neither).

There are no webhooks. Every "did the money land?" is a PULL — a timer in your
canister polling the DFINITY sol-rpc canister.

---

## Part 1 — The canister ↔ sol-rpc seam

**1. The sol-rpc canister speaks `Multi*Result`, not plain results.** 3-arg calls
`(RpcSources, ?RpcConfig, params) -> async Multi<X>Result`, every result wrapped
in `#Consistent` / `#Inconsistent`. Pull the LIVE candid
(`dfx canister metadata <sol-rpc> candid:service`) before hand-typing the actor.

**2. Decode-trap AFTER broadcast is the scariest class.** For `sendTransaction`
the tx hits the network during the `await`; if the RESPONSE decode then traps,
state rolls back but the tx is on-chain → durable-nonce desync / double-spend.
Guards: exact types (1,3); tolerate `#Inconsistent` on sends (4); mutate
nonce/state only after a good decode; treat decode-fail as "maybe sent."

**3. Candid does NOT widen `nat64`/`int64` → `Nat`/`Int`.** Declaring `Nat` for a
`nat64` field traps at type-table unification — even on a variant arm you never
read. Mirror exact widths: lamports = `Nat64`, signed offsets = `Int`.

**4. Consensus tolerance is asymmetric — AND per-slot values can't reach
consensus at all.**
- Reads (`getBalance`): `#Inconsistent` → error (you want agreement).
- Sends (`sendTransaction`): a signature is deterministic from the signed bytes,
  so the first `#Ok` IS the broadcast — accept it, ignore sibling errors.
- **`getLatestBlockhash` is special:** the value changes every slot, so providers
  almost never agree → chronic `#Inconsistent`. This will fail your `bootstrap()`
  and any canister-side blockhash fetch. **Build an admin/override path that
  fetches one fresh finalized blockhash and hands it straight into the call in the
  same shell** (blockhashes expire ~60s, so be fast). Don't rely on multi-provider
  consensus for anything that mutates every slot.

---

## Part 2 — Deposit detection (no webhooks)

**5. The loop: timer (~60s) + per-address signature cursor + generous intent TTL +
at-least-once credit.** Results come newest-first → reverse to oldest-first. Match
inbound delta to an open intent (± tolerance). Advance the cursor ONLY after credit
(or a deliberate, logged skip for confirmed-but-unmatched). A 10-min TTL is too
short (deposits silently expire); ~2h is right — budget for timer gaps, slow users,
wallet confirmations.

**6. The "intent" pattern decouples user action from detection.**
`prepare(intent)` → `{ depositAddress, intentId }`; user sends whenever; timer
reconciles; frontend polls a `pokeMyDeposit`-style query. The client never waits
on the chain.

**7. EVERY deposit *type* needs its own intent type AND its own detection wiring.**
Because SOL deposits are asynchronous (funds land at a threshold-derived address,
a timer detects later) there is NO synchronous pull — unlike an ICP ledger
`transfer_from`. So you can't trivially port a "pull funds + record" call. To add a
new deposit kind (e.g. an investor/"backer" deposit alongside game deposits) you
must, at minimum:
- add a new intent type + stable var (additive → **no migration** if the canister's
  empty/upgrade-safe),
- add `prepareXDeposit` + a `getMyPendingXIntents` query,
- add a credit branch in the unmatched-deposit path,
- **add the new intent set to BOTH the `pokeMyDeposit` open-intent gate AND the
  timer's scan-set builder.** Miss the timer scan-set and the deposit type simply
  never auto-detects — the single easiest bug to ship here.
- Match the other path's accounting exactly (e.g. backer deposits take **no
  front-end-load/cover-charge**, entitlement = gross × bonus).

**8. Detection is not instant, and it can stall. Have an admin backstop + admin
visibility.** A real deposit took ~5 min once (transient sol-rpc flakiness +
observer tick). Symptoms of "spinning forever": (a) frontend on the wrong cluster
(Part 4 #11), (b) transient sol-rpc consensus, (c) the type isn't in the timer
scan-set (#7). Ship `adminCreditManualDeposit` and an **admin getter for open
intents** — without them you cannot debug a stuck deposit (you can't query as the
user's SIWS principal).

---

## Part 3 — Signing & withdrawals

**9. Threshold Ed25519 + durable nonce + caller-supplied target.** Sign with the IC
threshold Ed25519 API + derivation path (never raw keys). Use a durable nonce
account (bootstrap once; idempotent; see #4 for the flaky-blockhash escape hatch).
Withdrawals must take the caller's OWN validated wallet address — the canister-
derived deposit address isn't user-spendable. Refresh the nonce AFTER a good
broadcast decode.

**10. The cycles signature: inbound state mutations succeed while outbound sol-rpc
calls fail.** If crediting a position works but the *sweep* throws `IC0406` /
"could not perform remote call", suspect either (a) the canister near its freezing
threshold (it can process messages but can't afford ~20B-cycle outcalls), or (b)
transient sol-rpc/subnet flakiness — **retry distinguishes them** (transient ones
clear on retry). sol-rpc calls are expensive; stale open intents keep the 60s
scanner burning until TTL, so size your CycleOps top-up threshold for that burn,
not idle baseline.

---

## Part 4 — The browser-side Solana boundary (where M3 actually bit us)

**11. The devnet→mainnet flip is THREE coordinated changes, and missing the
frontend pair fails SILENTLY.** The flip is: (a) the canister's install arg
(`solRpcProvider = variant { mainnet }`), (b) the frontend `SOLANA_RPC_ENDPOINT`,
(c) the CSP `connect-src` host. We flipped the canister but not the frontend → the
one-click flow built a **devnet** transaction; with no devnet funds it died
silently and never reached mainnet (no Phantom popup, nothing on-chain, UI spun on
"Registering…"). Flip all three together. After flipping, grep the whole UI for
stale "devnet"/"do not send real SOL" banners — they become actively wrong.

**12. The browser can't use the public mainnet RPC, and can't call the DFINITY
sol-rpc canister.** `api.mainnet-beta.solana.com` returns **403** for browser-
origin `getLatestBlockhash`/submit (devnet's public RPC is permissive, which is
exactly why this anti-pattern survives all of M1/M2 testing and only explodes at
mainnet). And the DFINITY sol-rpc canister can't be called from a browser (cycles +
update-call; ingress carries neither). So browser-side Solana needs a real RPC
provider — see #13/#14.

**13. Sending SOL: hand it to the wallet — but YOU still supply the blockhash.**
Anti-pattern: the app fetches a blockhash and calls
`adapter.sendTransaction`/`sendRawTransaction` itself (works on devnet, 403s on
mainnet). Correct: Phantom's `signAndSendTransaction` broadcasts via the wallet's
own RPC. **Caveat that cost us real time: Phantom does NOT fill in the blockhash** —
its provider serializes client-side and throws `Transaction recentBlockhash
required` if it's missing. So you still fetch a recent blockhash from your RPC and
set it; the wallet only does the broadcast.

```ts
const provider =
  (window as any)?.phantom?.solana?.isPhantom ? (window as any).phantom.solana
  : (window as any)?.solana?.isPhantom ? (window as any).solana
  : undefined;

const connection = new Connection(SOLANA_RPC_ENDPOINT, 'confirmed'); // your Helius endpoint
const { blockhash } = await connection.getLatestBlockhash('confirmed');
const tx = new Transaction({ feePayer: fromPubkey, recentBlockhash: blockhash });
tx.add(SystemProgram.transfer({ fromPubkey, toPubkey, lamports: Number(lamports) }));

if (provider?.isConnected === false) await provider.connect();
// verify provider.publicKey?.toString() === your expected SIWS session pubkey
const res = await provider.signAndSendTransaction(tx); // wallet broadcasts
const signature = typeof res === 'string' ? res : res.signature;
```

- Do NOT `confirmTransaction` to await success — it subscribes over `wss://`
  (`signatureSubscribe`), and CSP `connect-src` is **scheme-specific** (an
  `https://` allowance does NOT cover `wss://`), so it hangs even when the tx
  landed. Take the returned signature as success; confirm server-side via your
  canister's detection.

**14. Reading Solana state from the browser: domain-locked provider key.** There's
no wallet API for arbitrary reads (`getBalance`), and the public RPC 403s, so reads
need a provider. Ship the key in the bundle but **lock it to your origins** so a
scraped key is useless elsewhere — that's the accepted client-side pattern.
- In the provider dashboard (Helius/QuickNode/Alchemy): Allowed Domains → your
  custom domain AND your deployment origin (e.g. `<canister-id>.icp0.io`).
- Same principle restated: **wallet for signing; RPC for reading + the blockhash.**
  Using a provider key for reads is consistent with refusing to use one to submit
  sends — opposite operations, same rule.
- Vite env traps that burned an hour:
  - `.env.local` is gitignored; `VITE_*` is baked into the public bundle at build —
    expected; the domain-lock is the protection, not secrecy.
  - **If `vite.config` sets a non-default `root` (e.g. `root: 'frontend'`),
    `import.meta.env` loads `.env` files from `envDir` (= root), NOT the repo root.**
    Repo-root env file → `import.meta.env.VITE_X` is empty.
  - If the project already injects env via `loadEnv(mode, process.cwd(), '') +
    define` (the common IC pattern, e.g. for `DFX_NETWORK`), MIRROR that pattern
    rather than fighting Vite: `define: { 'process.env.VITE_SOLANA_RPC_URL':
    JSON.stringify(env.VITE_SOLANA_RPC_URL || '') }` and read `process.env.…` in
    code (type it in `vite-env.d.ts`).
  - dfx writes a repo-root `.env` (DFX_NETWORK) that `loadEnv` merges. Build output
    may go to `frontend/dist`, not repo-root `dist` — verify the key is embedded in
    the RIGHT dist before concluding it "didn't work."

**15. The CSP blocks Solana by default.** The asset canister's `standard` policy
allows IC domains only in `connect-src`; add your Solana RPC host(s) in
`.ic-assets.json` (both the public endpoint as fallback and your provider host).
After deploy, verify the LIVE header (`curl -sI https://yourapp | grep -i
content-security-policy`) actually lists the host — don't trust the source file.

```json
"Content-Security-Policy": "default-src 'self';script-src 'self';connect-src 'self' http://localhost:* https://icp0.io https://*.icp0.io https://icp-api.io https://api.mainnet-beta.solana.com https://mainnet.helius-rpc.com;img-src 'self' data:;style-src * 'unsafe-inline';font-src *;object-src 'none';base-uri 'self';frame-ancestors 'none';form-action 'self';upgrade-insecure-requests;"
```

**16. Always keep a manual-deposit fallback.** When the one-click flakes (provider
rate-limit, transient RPC), a "get a deposit address, send from Phantom yourself"
path bypasses the app's RPC entirely (Phantom uses its own) and is perfectly normal
crypto UX. It's the same intent under the hood, so it auto-credits identically.

**17. SIWS: byte-exact message, pure ASCII — and Ledger can't sign it.** Drive the
`siws_provider` directly if a turnkey manager won't take your wallet: `prepare ->
signMessage(canonical text) -> login -> get_delegation -> DelegationIdentity`. The
canonical message must byte-match what the canister reconstructs; `bs58`-encode the
signature; persist session key + delegation separately.
- **Keep the SIWS message pure ASCII and under ~1212 bytes.** A fancy apostrophe,
  em-dash, or emoji in `domain`/`statement` pushes it out of the Ledger's
  restricted-ASCII format → `0x6a81`.
- **Hardware wallets (Ledger) cannot sign the off-chain SIWS *message*** — the
  Solana app gates off-chain message signing and returns `0x6a81` even with blind
  signing ON, across app versions. Phantom relays it as `ledgerUnknownSignError`.
  (Ledger signs *transactions* fine — deposits work; only message-signing/sign-in
  fails.) You cannot fix this from the dApp if your message is already spec-clean.
  Mitigation: catch the error and steer users to a hot wallet / Internet Identity /
  Plug; the real fix is a separate **transaction-based sign-in** (Ledger signs a tx
  it *can* sign, verified server-side off-chain so login costs no SOL).

**18. Lamports in bigint; one denomination branch per page, not per card.** Use
`bigint` everywhere (`LAMPORTS_PER_SOL = 1_000_000_000n`); reject >9 decimals at
parse. When you add a SOL/SIWS session type, EVERY page that reads canister data
needs a denomination branch — not just the deposit widget. Stats, ledgers, position
lists, and explainer copy all silently keep reading the ICP canister otherwise.
Mirror the read hooks per canister and make components denomination-aware.

**19. Declarations sync.** `dfx generate` writes to a gitignored `.dfx/...` path;
the frontend imports the checked-in copy. After any candid change, hand-copy
`.did`/`.did.d.ts`/`.did.js` into `frontend/src/declarations/<name>/` and commit —
but NOT `index.ts` if it's hand-maintained (add new exported types there manually).
Alias SOL types (`GamePlan as SolGamePlan`) to disambiguate from ICP equivalents.

---

## Part 5 — Migration, deploy & the mainnet cutover

**20. Variant-set drift.** Adding a Motoko variant case is an implicit migration
with no glue. In each `runVN`, enumerate the FULL current variant set — never alias
the prior version, or a stored historical record fails to deserialize (M0170)
*after* `stop` has drained the canister. Backfill new fields with a
correct-by-construction default.

**21. The deploy dance.** Canisters with live timers/observers: `stop` → `upgrade
--wasm-memory-persistence keep` → `start`. Attach the migration, deploy, then
unwire it for the next normal upgrade. **Operator-gate every mainnet deploy**
touching shared state. Asset deploys are idempotent; module hash unchanged is
normal for asset canisters — **verify a frontend deploy by comparing the live
content-hashed bundle name against your local build**, not by trusting deploy log
output.

**22. devnet→mainnet is a REINSTALL, not an upgrade — and reinstall skips
`postupgrade`.** The cluster is baked in at install, so flipping it requires
`uninstall`/`install` (destructive — wipes state; fine for throwaway devnet data,
NEVER once real users exist). Because reinstall does NOT run `postupgrade`, you must
MANUALLY redo whatever it normally does: arm the detection timer, register the seed,
re-prime cursors. (For a feature add on an already-mainnet canister, use UPGRADE so
state and the verified install args are preserved.)

**23. Surface every money address character-by-character before any send.** A
safety classifier will (correctly) block an agent-supplied treasury/pool address on
a "trust me" basis. Print the full address and call out first-4/last-4; have the
operator compare against their wallet before reinstalling or funding.

**24. Cutover ordering (runbook it — the reinstall is irreversible):**
1. Pre-flight (read-only): confirm identity/canister IDs, snapshot state, verify the
   treasury address in-conversation.
2. Go/no-go gate (a conscious commitment, not a technical step): "this wipes state;
   I'm putting real money in; I'll keep it topped up with cycles + liquidity."
3. **Reinstall** with mainnet install args.
4. Derive the pool address → operator sends real seed SOL (the one step no agent can
   do — it's the user's wallet).
5. `bootstrap()` the durable nonce (flaky — use the admin-blockhash path #4) +
   **manually** arm the timer + register the seed (reinstall skipped postupgrade).
6. Upgrade the observer canister (additive, no migration) and **reset its SOL
   cursors AFTER reinstall, BEFORE the first deposit** — a stale cursor silently
   skips game 0 / mis-mints.
7. Flip frontend RPC endpoint + CSP (#11), remove devnet banners, deploy, verify
   live CSP by curl.
8. Smoke test with a tiny REAL deposit end-to-end (best: dogfood through the actual
   app UI, which tests the feature and lays the seed at once). Expect transient
   sol-rpc retries; have the admin backstop ready.
9. Soft-launch.

**25. Repo hygiene at launch.** A live frontend can be AHEAD of `origin/main` (it
carries merged work main is missing); a naive PR/cherry-pick from your deploy branch
will conflict. Don't force-merge — establish the true mainline first, then open a
clean PR. Keep the secret out of git (`.env.local`), commit only the wiring.

---

## Pre-flight checklist

```
CANISTER ↔ SOL-RPC
[ ] Actor types from LIVE candid; exact int widths (Nat64/Int, never bare Nat)
[ ] sendTransaction tolerates #Inconsistent (first #Ok wins); reads strict
[ ] getLatestBlockhash has an admin/override path (per-slot value can't reach consensus)
[ ] state/nonce mutated only after good decode; decode-fail = "maybe sent"

DEPOSITS
[ ] timer (~60s) + per-address cursor + generous TTL (hours) + at-least-once credit
[ ] results reversed to oldest-first; cursor advances only after credit/skip
[ ] EACH deposit type wired into BOTH the poke gate AND the timer scan-set
[ ] adminCreditManualDeposit backstop + admin getter for open intents

SIGNING
[ ] threshold Ed25519; durable nonce bootstrapped via the blockhash escape hatch
[ ] withdrawals take the caller's own validated address
[ ] CycleOps threshold sized for sol-rpc burn (not idle baseline)

BROWSER (the part that bites on mainnet)
[ ] cluster flip = canister arg + frontend SOLANA_RPC_ENDPOINT + CSP host, together
[ ] sends via wallet signAndSendTransaction; app sets the blockhash itself; no app submit
[ ] no confirmTransaction (wss blocked); no public mainnet RPC from browser; no sol-rpc canister from browser
[ ] reads/blockhash via a DOMAIN-LOCKED provider key (Allowed Domains = your origins)
[ ] Vite env: right envDir/define pattern; key embedded in the actual dist; .env.local gitignored
[ ] live CSP verified by curl after deploy
[ ] manual-deposit fallback present
[ ] SIWS message pure ASCII + under length; Ledger error caught and steered to hot wallet
[ ] every page denomination-branched, not just the deposit card
[ ] declarations hand-synced after candid changes (not index.ts)

DEPLOY / CUTOVER
[ ] migration enumerates full current variant set; unwired after
[ ] timer/observer canisters: stop → upgrade(keep) → start; operator-gated
[ ] devnet→mainnet = REINSTALL (wipes state) and redo postupgrade work MANUALLY
[ ] money addresses verified character-by-character before any send
[ ] observer cursors reset after reinstall, before first deposit
[ ] frontend deploy verified by live bundle hash; repo mainline sorted before PR
```

## What the official docs already cover (don't re-learn here)
- IC side: threshold Ed25519, the DFINITY sol-rpc canister, HTTPS-outcall
  cost/consensus — IC docs (the EVM-RPC integration is a near-identical sibling).
- Solana side: tx construction, @solana/kit / @solana/web3.js, wallet-standard.
This guide is only the seam — especially the browser-side seam, which neither set
of docs covers and which only fails on mainnet.
