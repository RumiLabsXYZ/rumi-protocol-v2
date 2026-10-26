# Base icUSD bridge: isolated staging UI recipe (BB-26)

A generic, placeholder-only recipe for testing the real Bridge page against the synthetic test
ledger and the test bridge canister on ICP, with Base Sepolia (chain 84532), without touching
production canisters, ledgers or wallet whitelists.

- **Authority:** Rob, relayed and verified by Miles: "Great do it up. Keep me posted", in reply to
  testing the Bridge page through an isolated test UI (wallets and approvals, both directions,
  reload recovery, rejected transactions, wrong network and account changes).
- **Roles:** Miles is the only person who creates the copy, runs the dev server, uses wallets or
  touches any canister. This document was produced by read-only source inspection; nothing in it
  has been executed. Private targets (test canister ids, the dev-server port, accounts) are
  supplied by Miles and never written to the repository.
- **Status:** source-verified design, not compiled and not run (proof level `source`).

Placeholders: `<WT>` the bridge worktree root; `<TEST_LEDGER_ID>`, `<TEST_BRIDGE_ID>`; `<PORT>` a
free port that is NOT 4943; `<STAGING_ORIGIN>` = `http://localhost:<PORT>`; `<SCRATCH>` a private
scratch directory outside the repository.

## 1. Verdict

Editing only the ids and the network switch in a copy is not enough. A small isolation patch is
needed, confined to a throwaway copy. No tracked repository file changes, the production
`ICUSD_BRIDGE` stays `""`, and the bridge route, components and services stay byte-identical.

Why ids alone are not enough (all reads are on every route, not just the bridge):

- The root layout (`src/vault_frontend/src/routes/+layout.svelte`, `onMount` at lines 45-69)
  initializes the wallet, queries the protocol canister for the ICP price and the ICPSwap routing
  flag, and polls `get_liquidatable_vaults` every 60 s. It also mounts the wallet panel, whose send
  form can make `icrc1_transfer` update calls on real ledgers.
- `walletStore` starts a 30 s balance poll on every connect (`src/lib/stores/wallet.ts:271-276`):
  protocol status, ICP, icUSD, ckUSDT, ckUSDC, 3pool and collateral ledger balances.
- The Plug whitelist and wallet delegation targets (`src/lib/services/pnp.ts:54-79`) list about 20
  production canisters.
- Internet Identity uses the production `derivationOrigin` whenever `CONFIG.isLocal` is false
  (`src/lib/services/auth.ts:399-401`). The production alternative-origins file
  (`src/vault_frontend/static/.well-known/ii-alternative-origins`) lists `http://localhost:4943`,
  so a copy served on port 4943 would sign in with the user's PRODUCTION principal. On any other
  port, II login fails until `derivationOrigin` is removed.
- There is no in-place switch: `CONFIG.isLocal` is decided by hostname only
  (`src/lib/config.ts:130-137`) and the canister ids are compile-time constants
  (`config.ts:15`, `config.ts:63`; `BRIDGE_ENABLED` at `config.ts:73`).

## 2. Isolation design (four layers, all in the copy)

1. **Poisoned config.** Every `CANISTER_IDS` entry points at `<TEST_LEDGER_ID>` except
   `ICUSD_BRIDGE = <TEST_BRIDGE_ID>`; `isLocal` returns `false`. Ids stay valid principals
   (`apiClient.ts:120-123` builds an actor from `CONFIG.currentCanisterId` at import, so an empty
   string would break it). Any stray service call then lands on the test ledger and is rejected as
   an unknown method, and the Plug whitelist and delegation targets can only contain the two test
   ids. The production whitelist file is not changed.
2. **Bridge-only shell.** A replacement root layout with a visible staging banner and a minimal
   ICP wallet connect that calls the real `walletStore.initialize`, `connect` and `disconnect`;
   `ssr = false`; every route except `bridge` removed.
3. **No polling.** `walletStore`'s `startBalanceRefresh` becomes a no-op.
4. **Request guard.** An inline script in `app.html`, installed before any agent is built, rejects
   every `/api/v*/canister/<id>/...` request whose id is not one of the two test ids, and records
   what it saw and blocked. It covers requests made by the page (anonymous agents, the II agent,
   wallet-adapter agents). It does not cover calls a wallet makes outside the page (the Plug
   extension, the Oisy popup, Base wallets); those are bounded by the whitelist, the wallet's own
   approval prompts and the user.

Kept unmodified: `routes/bridge/+page.svelte`, `lib/components/bridge/*`,
`lib/services/baseBridge/*`, `lib/services/oisySigner.ts`; `wallet.ts`, `auth.ts` and `pnp.ts`
except the single edits below.

## 3. Create and run the copy (Miles only)

The copy must sit at the same depth as `src/vault_frontend`, because `$declarations` resolves to
`../../declarations` from the working directory (`svelte.config.js:32`) and the Vite config uses
`../declarations` (`vite.config.js:36-44`). `node_modules` is symlinked, not copied: the
frontend's own `node_modules` pins `@dfinity/auth-client` and `@dfinity/identity` 2.3.0 (the
workspace root has 2.4.1), and a copy that missed them would silently change II behavior. No
install, download or build is needed; the copy is about 11 MB plus a Vite cache.

```bash
ST="<WT>/src/vault_frontend_stage"
mkdir "$ST"
cd "<WT>/src/vault_frontend"
cp -R src static package.json svelte.config.js vite.config.js tsconfig.json tailwind.config.js postcss.config.* "$ST"/
ln -s ../vault_frontend/node_modules "$ST/node_modules"
```

Do not copy `vite.config.ts`, `dist`, `.svelte-kit` or `node_modules`. The copy shows as untracked
in the worktree: never run `git add -A` or `git add .` there, and do not edit `.git/info/exclude`
(it is shared with the main checkout). Apply section 4, then:

```bash
cd "$ST" && "<WT>/node_modules/.bin/vite" dev --config vite.config.js
```

Use the absolute `vite` binary (the frontend `node_modules` has no `.bin`; `npx` could download).
On first load Vite may re-optimize dependencies and reload the page: open
`<STAGING_ORIGIN>/bridge` once and let it settle before any wallet action.

## 4. Edits inside the copy

Line numbers refer to the original files in `src/vault_frontend`.

- **E1** `src/lib/config.ts` line 8: delete the `rumi_cycle_sentinel` declarations import.
- **E2** `src/lib/config.ts` lines 11-64: above `CANISTER_IDS` add
  `const TEST_LEDGER = '<TEST_LEDGER_ID>'; const TEST_BRIDGE = '<TEST_BRIDGE_ID>';`, then set every
  key of `CANISTER_IDS` to `TEST_LEDGER` except `ICUSD_BRIDGE: TEST_BRIDGE` (keep every key name).
- **E3** `src/lib/config.ts` lines 104-114 (`LOCAL_CANISTER_IDS`): set the production-id keys to
  `TEST_LEDGER` and `ICUSD_BRIDGE` to `TEST_BRIDGE` (defense in depth if E3b is missed).
- **E3b** `src/lib/config.ts` lines 130-137: the `isLocal` getter returns `false`. The host
  getter then returns `https://icp0.io` (`config.ts:215-220`).
- **E4** `src/routes`: replace `+layout.svelte` with the shell below; add `+layout.ts` containing
  `export const ssr = false; export const prerender = false;`; replace `+page.svelte` with a link
  to `/bridge`; delete every other route directory except `bridge`.
- **E5** `src/lib/stores/wallet.ts` lines 271-276: make `startBalanceRefresh()` an empty function.
- **E6** `src/lib/services/auth.ts` lines 399-401: `derivationOrigin: undefined,`. II then
  derives a staging-only principal from `<STAGING_ORIGIN>` (different from production and from
  any other port).
- **E7** (optional) `src/lib/services/pnp.ts` line 79: de-duplicate the targets,
  `].filter((v, i, a) => Boolean(v) && a.indexOf(v) === i);`, so Plug lists exactly two ids.
- **E8** `vite.config.js`: delete the `server.proxy` block (lines 22-29, the `/api` proxy to a
  local replica); set `server: { host: 'localhost', port: <PORT>, strictPort: true }` and
  `cacheDir: '<SCRATCH>/vite-stage-cache'`.
- **E9** `src/app.html`: the guard below as the first script in `<head>`, before
  `%sveltekit.head%`.

Request guard (E9):

```html
<script>
(function () {
  var ALLOW = ['<TEST_LEDGER_ID>', '<TEST_BRIDGE_ID>'];
  var re = /\/api\/v[0-9]+\/canister\/([^\/?#]+)\/([a-z_]+)/;
  var orig = window.fetch.bind(window);
  window.__stageBlocked = [];
  window.__stageSeen = {};
  window.fetch = function (input, init) {
    var url = typeof input === 'string' ? input : (input && input.url) || String(input);
    var m = re.exec(url);
    if (m) {
      var key = m[1] + ' ' + m[2];
      window.__stageSeen[key] = (window.__stageSeen[key] || 0) + 1;
      if (ALLOW.indexOf(m[1]) === -1) {
        window.__stageBlocked.push(key);
        return Promise.reject(new TypeError('staging guard blocked ' + key));
      }
    }
    return orig(input, init);
  };
})();
</script>
```

Staging shell (E4, `src/routes/+layout.svelte`; legacy syntax like the original layout; not
compiled yet):

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import '../app.css';
  import { CONFIG, CANISTER_IDS } from '$lib/config';
  import { walletStore } from '$lib/stores/wallet';
  import { initializePNP, pnp, getAllDelegationTargets } from '$lib/services/pnp';
  import { WALLET_TYPES } from '$lib/services/auth';
  import { preWarmOisySigner } from '$lib/services/oisySigner';
  import { isOisyWallet } from '$lib/services/protocol/walletOperations';

  let wallets: { id: string; name: string }[] = [{ id: WALLET_TYPES.INTERNET_IDENTITY, name: 'Internet Identity' }];
  let connecting = false;
  let error = '';
  let blocked = 0;
  const targets = getAllDelegationTargets();
  $: isConnected = $walletStore.isConnected;
  $: principal = $walletStore.principal;
  $: if (isConnected && principal && isOisyWallet()) void preWarmOisySigner(principal);

  onMount(() => {
    try {
      initializePNP();
      const extra = pnp
        .getEnabledWallets()
        .filter((w) => ['plug', 'oisy'].includes(String(w.id).toLowerCase()))
        .map((w) => ({ id: w.id, name: String(w.id).toLowerCase() === 'oisy' ? 'Oisy' : 'Plug' }));
      wallets = [wallets[0], ...extra];
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
    void walletStore.initialize();
    const timer = setInterval(() => { blocked = ((window as any).__stageBlocked ?? []).length; }, 1000);
    return () => clearInterval(timer);
  });

  async function connect(id: string) {
    if (connecting) return;
    connecting = true; error = '';
    try { await walletStore.connect(id); }
    catch (e) { error = e instanceof Error ? e.message : String(e); }
    finally { connecting = false; }
  }
  async function disconnect() {
    try { await walletStore.disconnect(); } catch (e) { error = e instanceof Error ? e.message : String(e); }
  }
</script>

<div class="stage-banner" role="note">
  <strong>STAGING TEST UI. Not the Rumi app.</strong>
  Synthetic test canisters on ICP, Base Sepolia only.
  host={CONFIG.host} isLocal={String(CONFIG.isLocal)} ledger={CONFIG.currentIcusdLedgerId} bridge={CANISTER_IDS.ICUSD_BRIDGE}
  whitelist={[...new Set(targets)].join(', ')} blockedRequests={blocked}
</div>
<header class="stage-wallet">
  {#if isConnected && principal}
    <span>ICP principal: <code>{principal.toText()}</code></span>
    <button type="button" class="btn-secondary" on:click={disconnect}>Disconnect ICP wallet</button>
  {:else}
    {#each wallets as w (w.id)}
      <button type="button" class="btn-secondary" disabled={connecting} on:click={() => connect(w.id)}>Connect {w.name}</button>
    {/each}
  {/if}
  {#if error}<span role="alert">{error}</span>{/if}
</header>
<div class="stage-main"><slot /></div>

<style>
  .stage-banner { position: sticky; top: 0; z-index: 1000; padding: 0.5rem 1rem; background: #7a1f1f; color: #fff; font-size: 0.75rem; word-break: break-all; }
  .stage-wallet { display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap; padding: 0.75rem 1rem; font-size: 0.8rem; }
  .stage-main { max-width: 1200px; margin: 0 auto; padding: 1rem 2rem 2rem; }
</style>
```

## 5. Acceptance checks before any wallet action

1. The banner shows `host=https://icp0.io`, `isLocal=false`, `ledger=<TEST_LEDGER_ID>`,
   `bridge=<TEST_BRIDGE_ID>`, and a whitelist of exactly those two ids.
2. `/bridge` shows the direction tabs (not "Bridge is not available yet"), "Base Sepolia
   (chain 84532)" and the expected test contract. The page reads the chain from
   `get_bridge_info` (`lib/services/baseBridge/chains.ts:157-172`).
3. Browser network panel, filter `canister/`: only `<TEST_BRIDGE_ID>` and `<TEST_LEDGER_ID>`
   requests reach the network; nothing goes to `localhost:4943` or another local `/api`.
4. Console: `window.__stageSeen` lists only the two test ids, plus at most
   `ryjl3-tyaaa-aaaaa-aaaba-cai read_state`. `window.__stageBlocked` contains at most
   `ryjl3-tyaaa-aaaaa-aaaba-cai read_state` entries. Those are agent-js time syncs: with no
   canister given, `HttpAgent.create` syncs time against the ICP ledger, catches the failure and
   uses the local clock (`node_modules/@dfinity/agent/lib/esm/agent/http/index.js:306-316` and
   `:659-690`), so blocking them is harmless. Any blocked `call`, or any other blocked id, is a
   stop: a staging gap to fix before continuing.
5. A fresh browser profile has no `rumi_last_wallet` in `localStorage`, and no request goes to a
   production Rumi origin.
6. With no wallet connected for a minute, only the 60 s `get_bridge_info` repeat appears
   (`routes/bridge/+page.svelte:63`).

After each wallet connect, repeat checks 3 and 4 and read the console lines `Delegation targets:`
and `Requesting comprehensive Plug permissions:` (`pnp.ts:141`, `:215`).

Reachability rule: at every handoff, and before every wallet step, confirm the dev server is
still serving (reload `/bridge`, or fetch any module it serves) and that a dynamically loaded
wallet module downloads without error. A previously rendered tab is not proof that the server is
still running; a stopped server shows up only as a failed module download at the next dynamic
import, typically when a wallet is first connected.

## 6. Wallets, principals and real user gestures

- **Internet Identity.** With E6 the principal is derived from `<STAGING_ORIGIN>`: new, empty and
  separate from production. A change of port or host gives a new principal and an empty recovery
  journal. II offers no canister targets, so for II the guard and the fresh principal are the
  confinement. Connect needs a click (popup, passkey).
- **Plug.** The user's real Plug account principal. Use a dedicated test account in a dedicated
  browser profile. The whitelist is the two test ids. Plug's per-origin storage should keep the
  production approval untouched (not verified).
- **Oisy.** The user's real Oisy principal (dedicated test account). Each signer call must be the
  first signer call of its own click: ICP to Base is click 1 approve, click 2 bridge
  (`lib/services/baseBridge/bridgeActor.ts:39-41`, `lib/services/baseBridge/icpToBase.ts`).
  Refund and "Tell the bridge" are each their own click. The bridge canister's
  `icrc28_trusted_origins` returns production origins only
  (`src/rumi_icusd_bridge/src/icrc21.rs:194-204`). If Oisy enforces it, the bridge calls from
  `<STAGING_ORIGIN>` will be refused while the ledger approval may still succeed, leaving a short
  allowance on the test ledger. Record the outcome; Oisy on a production origin then stays
  unverified. Do not keep a production Rumi tab with Oisy open (the signer handles one request at
  a time).
- **Base wallet** (EIP-1193 / EIP-6963). Connect, switch or add Base Sepolia, and every
  `bridgeOut` send need a wallet approval. Account and network changes are made in the wallet.
- Test assets: Miles sends synthetic test icUSD to the principal the connected wallet actually
  shows, and Base Sepolia ETH to the test Base account. The test bridge must be running with the
  pauses the test needs.

## 7. BB-27 browser test matrix (Miles)

Each row: action, then what the page must show. Run checks 3 and 4 after each row.

1. ICP to Base with II or Plug: Review shows a quote (this also proves the bridge escrows the
   app's ledger, `bridgeActor.ts:121-126`); Confirm runs approve then bridge in one click; the
   deposit moves through escrowed, minting, delivered.
2. ICP to Base with Oisy: two clicks as above; record the ICRC-28 outcome.
3. Reload mid-deposit: the saved intent restores by principal and request id; no second deposit.
4. Rejected approval or rejected bridge call: clear message, no deposit record, nothing pulled.
5. Base to ICP: connect the Base wallet, burn, then released after Base finality.
6. Rejected `bridgeOut` in the wallet: shown as rejected, nothing saved as sent.
7. Wrong network: the page asks to switch to Base Sepolia and blocks sending.
8. Account change before sending: the pending send aborts; a saved unsent intent for another
   account is reported as an account mismatch.
9. Reload after a burn is sent: recovery shows the transfer and its status.
10. Refund of a held deposit, if the run can produce one: its own click and signer call.

## 8. Risks and untested items

- Nothing here was compiled or run. The staging shell, the guard and the edits are source-checked
  only.
- Oisy's ICRC-28 enforcement on `http://localhost:<PORT>`, and its handling of `icrc2_approve` on
  a synthetic ledger, are unknown.
- Plug's per-call prompts for whitelisted calls, and whether its per-origin storage leaves
  production sessions untouched, are unknown.
- II's default session length (no `maxTimeToLive` is passed) is not verified.
- The intent journal (`rumi:base-bridge:v1:<principal>`, `intents.ts:20`) stores no bridge or
  ledger id for ICP to Base intents (`intents.ts:28-43`): clear it if the test canisters change.
- The page checks the ledger only on the ICP to Base review step; page load and Base to ICP do
  not compare ledgers.
- A localhost run proves nothing about the production derivation origin, the production CSP
  (`static/.ic-assets.json`; the dev server applies none), asset-canister routing or production
  ICRC-28 matching.

## 9. Teardown

Stop the dev server, delete `<WT>/src/vault_frontend_stage` and `<SCRATCH>/vite-stage-cache`, then
confirm `git status --porcelain` in `<WT>` shows nothing new.

## 10. Provenance

- W8, Sonnet requested and Sonnet actual: staging reconnaissance and the first draft of this
  recipe (read-only).
- W9, Haiku requested and Haiku actual: mechanical inventory of canister ids, actor creation
  sites, wallet target lists, load-time work, routes and environment variables (read-only).
- Coordinator (Opus 5.5): checked the load-bearing claims against source (layout load work,
  wallet poll, II derivation origin and alternative origins, delegation targets, ledger check,
  Base Sepolia parameters, dependency versions, staging-layout imports), found the agent-js time
  sync behavior behind acceptance check 4, refined the guard to record request kinds, and wrote
  this document.
