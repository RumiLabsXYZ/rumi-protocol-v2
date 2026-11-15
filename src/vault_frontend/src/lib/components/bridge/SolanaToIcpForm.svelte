<script lang="ts">
  import { untrack } from 'svelte';
  import type { Principal } from '@dfinity/principal';
  import type { _SERVICE as SolService } from '$declarations/rumi_icusd_sol_bridge/rumi_icusd_sol_bridge.did';
  import { walletSessionGeneration } from '$lib/services/auth';
  import { formatE8s } from '$lib/services/baseBridge/amounts';
  import { isEnabled, type DestinationView } from '$lib/services/baseBridge/destinations';
  import { captureSessionGuard } from '$lib/services/baseBridge/bridgeActor';
  import { withSigning, type SigningContext } from '$lib/services/baseBridge/signingGuard';
  import { shortAddress } from '$lib/services/baseBridge/status';
  import type { SolanaBridgeConfig } from '$lib/services/solanaBridge/config';
  import { explorerTxUrl } from '$lib/services/solanaBridge/networks';
  import type { SolanaIntentStore, SolReturnIntent } from '$lib/services/solanaBridge/intents';
  import type { HolderTokenAccount, SolanaRpc } from '$lib/services/solanaBridge/rpc';
  import type { SolanaWalletSession, SolanaWalletState } from '$lib/services/solanaBridge/walletStandard';
  import { getAnonymousSolanaBridge } from '$lib/services/solanaBridge/solanaActor';
  import { buildSolanaToIcpDeps, createSolanaSigners, type SolanaSigners } from '$lib/services/solanaBridge/solanaDeps';
  import {
    INITIAL_SOL_RETURN_STATE,
    continuePrepared,
    describeReturnProgress,
    discardReturn,
    evaluateReturnDraft,
    mayDiscard,
    notifyReturn,
    prepareReturn,
    rebroadcastReturn,
    reconcileReturn,
    reduceSolReturn,
    startReturn,
    trackReturn,
    trackedReturns,
    unresolvedReturns,
    verdictOf,
    type ReturnEvidence,
    type ReturnVerdict,
    type SolReturnEvent,
    type SolReturnState,
  } from '$lib/services/solanaBridge/solanaToIcp';

  interface Props {
    config: SolanaBridgeConfig;
    destination: DestinationView;
    readFreshDestination?: () => DestinationView | null;
    onRecheck?: () => void;
    wallet: SolanaWalletState;
    /** The connected Solana wallet as a signing session, read at the moment of use. */
    getSession: () => SolanaWalletSession | null;
    /** The keyless Solana node adapter. Null when this build has none configured. */
    rpc: SolanaRpc | null;
    principal: Principal | null;
    store: SolanaIntentStore | null;
    onConnect: () => void;
    onActivity?: () => void;
  }

  let { config, destination, readFreshDestination, onRecheck, wallet, getSession, rpc, principal, store, onConnect, onActivity }: Props = $props();

  /** A reviewed transfer older than this is prepared again on confirm: its blockhash must still be fresh. */
  const PLAN_MAX_AGE_MS = 45_000;

  let amountText = $state('');
  let principalInput = $state('');
  let principalTouched = $state(false);
  let subaccountText = $state('');
  let sourceAccount = $state('');
  let accounts = $state.raw<HolderTokenAccount[] | null>(null);
  let accountsError = $state('');
  let flow = $state.raw<SolReturnState>(INITIAL_SOL_RETURN_STATE);
  let evidence = $state.raw<ReturnEvidence | null>(null);
  let verdict = $state<ReturnVerdict | null>(null);
  let formError = $state('');
  let note = $state('');
  let checking = $state(false);
  let signers = $state.raw<SolanaSigners | null>(null);
  let anonymous = $state.raw<SolService | null>(null);
  let storeVersion = $state(0);
  let sessionGuard: (() => boolean) | null = null;
  let autoResumed = false;

  const signedIn = $derived(!!principal && !principal.isAnonymous());
  const principalText = $derived(principal ? principal.toText() : '');
  const connected = $derived(wallet.status === 'connected' && !!wallet.address);
  const onNetwork = $derived(connected && wallet.chains.includes(config.network.chain));
  const busy = $derived(flow.kind === 'preparing' || flow.kind === 'signing' || flow.kind === 'signed');
  const locked = $derived(busy || flow.kind === 'held' || flow.kind === 'broadcast_uncertain' || flow.kind === 'prepared' || flow.kind === 'tracking');
  const evaluation = $derived(evaluateReturnDraft({ amountText, principalText: principalInput, subaccountText, sourceAccount }, config, accounts));
  const tokenProgramName = $derived(config.tokenProgram === 'token2022' ? 'Token-2022' : 'SPL Token');

  const deviceAttempts = $derived.by(() => {
    storeVersion;
    return store
      ? store.listFor(destination.id).filter((i): i is SolReturnIntent => i.kind === 'solana_to_icp' && i.status !== 'done')
      : [];
  });

  function dispatch(event: SolReturnEvent) {
    flow = reduceSolReturn(flow, event);
  }
  function touched() {
    storeVersion += 1;
    onActivity?.();
  }

  // Default the ICP destination to the signed-in principal until the user edits it.
  $effect(() => {
    if (!principalTouched && principalText) principalInput = principalText;
  });

  // The holder's token accounts, read when a wallet account is connected.
  $effect(() => {
    const address = wallet.address;
    const node = rpc;
    if (!address || !node || !onNetwork) {
      accounts = null;
      return;
    }
    const controller = new AbortController();
    accountsError = '';
    void node
      .getTokenAccounts(address, config.mint, config.tokenProgramId)
      .then((list) => {
        if (controller.signal.aborted) return;
        accounts = list;
        if (!list.some((a) => a.address === untrack(() => sourceAccount))) sourceAccount = list.find((a) => a.amountE8s > 0n)?.address ?? list[0]?.address ?? '';
      })
      .catch((error) => {
        if (!controller.signal.aborted) accountsError = `Could not read your token accounts: ${error instanceof Error ? error.message : String(error)}`;
      });
    return () => controller.abort();
  });

  // An account or network change in the wallet leaves the saved attempt exactly as it was; only the draft is dropped.
  let seenAccount: string | null = null;
  $effect(() => {
    const key = `${wallet.address ?? ''}|${wallet.chains.join(',')}`;
    if (seenAccount !== null && seenAccount !== key) {
      untrack(() => {
        if (flow.kind === 'review') flow = INITIAL_SOL_RETURN_STATE;
      });
    }
    seenAccount = key;
  });

  // An ICP wallet session change abandons in-flight reads. Attempts stay on disk, keyed by principal.
  let seenSession: string | null = null;
  $effect(() => {
    const key = `${$walletSessionGeneration}:${principalText}`;
    if (seenSession !== null && seenSession !== key) {
      untrack(() => {
        signers = null;
        anonymous = null;
        sessionGuard = null;
        if (flow.kind === 'signing' || flow.kind === 'signed') flow = { kind: 'session_changed' };
        else if (flow.kind === 'review' || flow.kind === 'preparing') flow = INITIAL_SOL_RETURN_STATE;
      });
    }
    seenSession = key;
  });

  async function ensureReady(): Promise<boolean> {
    if (signers && anonymous && sessionGuard && sessionGuard()) return true;
    const guard = captureSessionGuard();
    const [createdSigners, reader] = await Promise.all([createSolanaSigners(), getAnonymousSolanaBridge()]);
    if (!guard()) return false;
    signers = createdSigners;
    anonymous = reader;
    sessionGuard = guard;
    return true;
  }

  function signingContext(): SigningContext {
    const opened = destination;
    return {
      destination: opened,
      freshStatus: () => {
        const fresh = readFreshDestination ? readFreshDestination() : opened;
        return fresh ? { status: fresh.status, reason: fresh.reason } : null;
      },
    };
  }

  function recheckIfUnavailable() {
    const fresh = readFreshDestination ? readFreshDestination() : destination;
    if (!fresh || !isEnabled(fresh)) onRecheck?.();
  }

  function buildDeps(reader: SolService | null = anonymous) {
    if (!store || !rpc || !reader) return null;
    const guard = sessionGuard ?? captureSessionGuard();
    return withSigning(buildSolanaToIcpDeps({ config, store, rpc, wallet: getSession, signers, anonymous: reader, isCurrent: guard }), signingContext());
  }

  async function review() {
    formError = '';
    if (!principal || !signedIn) {
      formError = 'Connect your ICP wallet first. The bridge journal that protects this transfer is kept for your signed-in account.';
      return;
    }
    if (!connected || !onNetwork) {
      formError = connected ? `Switch your wallet to ${config.network.name}.` : 'Connect your Solana wallet first.';
      return;
    }
    if (!evaluation.ok) {
      formError = evaluation.problem ?? 'Check the amount.';
      return;
    }
    dispatch({ type: 'preparing' });
    const guard = captureSessionGuard();
    try {
      // Everything the click on "Sign" needs is made ready here, so the wallet call is the first asynchronous step of that click.
      if (!(await ensureReady())) {
        if (guard()) dispatch({ type: 'plan_failed', message: 'Your ICP wallet session changed. Review the transfer again.' });
        return;
      }
      const reader = anonymous ?? (await getAnonymousSolanaBridge());
      const deps = buildDeps(reader);
      if (!deps) {
        dispatch({ type: 'plan_failed', message: 'This build has no Solana node configured, so a return cannot be prepared.' });
        return;
      }
      const prepared = await prepareReturn(deps, { amountText, principalText: principalInput, subaccountText, sourceAccount });
      if (!guard()) return;
      dispatch(prepared.ok ? { type: 'plan_ready', plan: prepared.plan } : { type: 'plan_failed', message: prepared.message });
    } catch (error) {
      if (!guard()) return;
      dispatch({ type: 'plan_failed', message: `Could not prepare the transfer: ${error instanceof Error ? error.message : String(error)}` });
    }
  }

  /** Click handler. The wallet call is the first asynchronous step inside startReturn. */
  function confirm() {
    if (flow.kind !== 'review') return;
    recheckIfUnavailable();
    const plan = flow.plan;
    if (Date.now() - plan.createdAt > PLAN_MAX_AGE_MS) {
      void review();
      return;
    }
    const deps = buildDeps();
    if (!deps) {
      dispatch({ type: 'blocked', message: 'Your wallet is not ready. Review the transfer again.' });
      return;
    }
    void startReturn(deps, plan, dispatch).then(touched);
  }

  function resend() {
    if (flow.kind !== 'broadcast_uncertain') return;
    const deps = buildDeps();
    if (!deps) return;
    void rebroadcastReturn(deps, flow.intent, dispatch).then(touched);
  }

  function proceed() {
    if (flow.kind !== 'prepared') return;
    recheckIfUnavailable();
    const deps = buildDeps();
    if (!deps) return;
    void continuePrepared(deps, flow.intent, dispatch).then(touched);
  }

  async function checkEvidence(intent: SolReturnIntent) {
    if (!rpc || checking) return;
    checking = true;
    note = '';
    try {
      const reader = anonymous ?? (await getAnonymousSolanaBridge());
      const deps = buildDeps(reader);
      if (!deps) return;
      const found = await reconcileReturn(deps, intent);
      evidence = found;
      verdict = verdictOf(found);
      if (found.signature && (verdict === 'landed' || verdict === 'in_flight' || verdict === 'credited')) {
        dispatch({ type: 'evidence', intent: { ...intent, signature: found.signature }, evidence: found });
      }
    } catch (error) {
      note = `Could not check the evidence: ${error instanceof Error ? error.message : String(error)}`;
    } finally {
      checking = false;
    }
  }

  function checkFlowEvidence() {
    if (flow.kind === 'broadcast_uncertain') void checkEvidence(flow.intent);
  }

  function discardFlowAttempt() {
    if (flow.kind === 'prepared') discard(flow.intent);
  }

  function discard(intent: SolReturnIntent) {
    if (!store) return;
    const result = discardReturn({ store }, intent, verdict);
    if (!result.ok) {
      note = result.message;
      return;
    }
    flow = INITIAL_SOL_RETURN_STATE;
    evidence = null;
    verdict = null;
    touched();
  }

  function open(intent: SolReturnIntent) {
    if (busy) return;
    evidence = null;
    verdict = null;
    dispatch({ type: 'resume', intent });
    void ensureReady().catch(() => false);
  }

  // Reload recovery: show the newest attempt that has no recorded outcome, without sending or signing anything.
  $effect(() => {
    if (autoResumed || !store || flow.kind !== 'draft') return;
    autoResumed = true;
    const newest = untrack(() => [...unresolvedReturns(store), ...trackedReturns(store, destination.id)].sort((a, b) => b.createdAt - a.createdAt)[0]);
    if (newest && newest.destination === destination.id) {
      dispatch({ type: 'resume', intent: newest });
      void ensureReady().catch(() => false);
    }
  });

  // Follow a broadcast attempt until the bridge's own release record is terminal. Read-only.
  const trackedId = $derived(flow.kind === 'tracking' ? flow.intent.id : null);
  $effect(() => {
    const id = trackedId;
    if (id === null || !store || !rpc) return;
    const intent = untrack(() => (flow.kind === 'tracking' ? flow.intent : null));
    if (!intent) return;
    const controller = new AbortController();
    const intentStore = store;
    const node = rpc;
    void (async () => {
      try {
        const reader = anonymous ?? (await getAnonymousSolanaBridge());
        if (controller.signal.aborted) return;
        await trackReturn(
          {
            rpc: node,
            store: intentStore,
            reads: {
              getReturn: async (returnId) => (await reader.get_return(returnId))[0] ?? null,
              getRelease: async (returnId) => (await reader.get_release(returnId))[0] ?? null,
            },
          },
          intent,
          (found) => {
            evidence = found;
            dispatch({ type: 'evidence', intent: found.signature && !intent.signature ? { ...intent, signature: found.signature } : intent, evidence: found });
          },
          controller.signal,
        );
        if (!controller.signal.aborted) touched();
      } catch {
        // The next page visit resumes from the journal.
      }
    })();
    return () => controller.abort();
  });

  async function tellBridge(signature: string) {
    note = '';
    try {
      if (!(await ensureReady())) return;
    } catch (error) {
      note = `Your ICP wallet is not ready: ${error instanceof Error ? error.message : String(error)}`;
      return;
    }
    const deps = buildDeps();
    if (!deps) return;
    const result = await notifyReturn(deps, signature);
    note = result.ok ? 'The bridge was told. It still decides from its own reads of Solana.' : result.message;
  }

  function startOver() {
    flow = INITIAL_SOL_RETURN_STATE;
    evidence = null;
    verdict = null;
    formError = '';
    note = '';
  }

  const fmt = (value: bigint) => formatE8s(value);
  const progress = $derived(flow.kind === 'tracking' ? describeReturnProgress(flow.evidence ?? evidence) : null);
</script>

<section class="flow" aria-label="Bridge icUSD from Solana to the Internet Computer">
  <p class="intro">
    Send icUSD from your Solana wallet to the bridge's return account. The bridge reads the transfer as finalized on Solana, burns it, and only then releases the same amount, minus the release fee, on the Internet Computer.
    The memo in the transaction carries your Internet Computer destination, so check it before you approve.
  </p>

  {#if config.halted}
    <p class="banner bad" role="alert">The bridge is halted for safety: {config.halted}</p>
  {:else if config.releasesPaused}
    <p class="banner warn" role="status">Releases to the Internet Computer are paused right now. The bridge still credits your return and releases it once releases resume.</p>
  {/if}

  {#if !rpc}
    <p class="banner warn" role="status">This build has no Solana node configured, so returns cannot be prepared here.</p>
  {/if}

  {#if deviceAttempts.length > 0 && flow.kind === 'draft'}
    <div class="banner warn" role="status" data-testid="device-attempts">
      <strong>Earlier Solana returns on this device</strong>
      {#each deviceAttempts as intent (intent.id)}
        <div class="row">
          <span>{fmt(intent.amountE8s)} icUSD from <code>{shortAddress(intent.wallet)}</code> · {intent.status === 'sent' ? 'sent' : intent.status === 'prepared' ? 'not sent' : 'no recorded outcome'}</span>
          <button type="button" class="btn-secondary small" onclick={() => open(intent)}>Open</button>
        </div>
      {/each}
      <p class="small-note">Opening an attempt never signs or sends anything. It is shown exactly as it was saved.</p>
    </div>
  {/if}

  {#if !connected}
    <button type="button" class="btn-secondary wide" onclick={onConnect} disabled={wallet.status === 'connecting' || wallet.status === 'unavailable'}>Connect your Solana wallet</button>
  {/if}

  <div class="field">
    <label for="sol-ret-amount">Amount to return (icUSD)</label>
    <input id="sol-ret-amount" type="text" inputmode="decimal" autocomplete="off" placeholder="0.00" bind:value={amountText} disabled={locked}
      oninput={() => { if (flow.kind === 'review') flow = INITIAL_SOL_RETURN_STATE; }} />
  </div>

  <div class="field">
    <label for="sol-ret-source">From your token account</label>
    <select id="sol-ret-source" bind:value={sourceAccount} disabled={locked || !accounts || accounts.length === 0}
      onchange={() => { if (flow.kind === 'review') flow = INITIAL_SOL_RETURN_STATE; }}>
      {#if !accounts || accounts.length === 0}
        <option value="">{connected ? 'No icUSD token account found' : 'Connect a wallet first'}</option>
      {:else}
        {#each accounts as account (account.address)}
          <option value={account.address}>{shortAddress(account.address)} · {fmt(account.amountE8s)} icUSD</option>
        {/each}
      {/if}
    </select>
    {#if accountsError}<p class="banner bad" role="alert">{accountsError}</p>{/if}
  </div>

  <div class="field">
    <label for="sol-ret-principal">Internet Computer principal that receives the icUSD</label>
    <input id="sol-ret-principal" type="text" autocomplete="off" spellcheck="false" placeholder="xxxxx-xxxxx-..." bind:value={principalInput} disabled={locked}
      oninput={() => { principalTouched = true; if (flow.kind === 'review') flow = INITIAL_SOL_RETURN_STATE; }} />
    <details>
      <summary>Subaccount (optional)</summary>
      <input id="sol-ret-subaccount" type="text" autocomplete="off" spellcheck="false" placeholder="64 lowercase hex characters, or empty" bind:value={subaccountText} disabled={locked}
        oninput={() => { if (flow.kind === 'review') flow = INITIAL_SOL_RETURN_STATE; }} />
    </details>
    <p class="small-note">The icUSD is released to exactly this account. Double check it: a mistake cannot be undone.</p>
  </div>

  {#if !signedIn}
    <p class="banner" role="status">Connect your ICP wallet in the page header to return icUSD.</p>
  {/if}

  {#if flow.kind === 'draft' || flow.kind === 'failed' || flow.kind === 'declined' || flow.kind === 'blocked' || flow.kind === 'session_changed'}
    <button type="button" class="btn-primary wide" onclick={review} disabled={!signedIn || !connected || !rpc || config.halted !== null}>Review transfer</button>
  {/if}
  {#if flow.kind === 'preparing'}
    <button type="button" class="btn-primary wide" disabled>Preparing the transaction...</button>
  {/if}
  {#if formError}<p class="banner bad" role="alert">{formError}</p>{/if}
  {#if !evaluation.ok && evaluation.problem && flow.kind === 'draft' && amountText !== ''}
    <p class="small-note" role="status">{evaluation.problem}</p>
  {/if}
  {#if evaluation.ok && evaluation.warning && flow.kind === 'draft'}
    <p class="banner warn" role="status">{evaluation.warning}</p>
  {/if}
  {#if flow.kind === 'failed'}<p class="banner bad" role="alert">{flow.message}</p>{/if}
  {#if flow.kind === 'blocked'}<p class="banner bad" role="alert">{flow.message}</p>{/if}
  {#if flow.kind === 'declined'}<p class="banner" role="status">You declined the transfer in your wallet. Nothing was signed or sent.</p>{/if}
  {#if flow.kind === 'session_changed'}
    <p class="banner warn" role="alert">Your wallet session changed during the transfer. The attempt stays saved on this device exactly as it was; open it below to check it.</p>
  {/if}

  {#if flow.kind === 'review'}
    {@const plan = flow.plan}
    <div class="review" aria-label="Transfer summary">
      <h3>Review (check every line)</h3>
      <dl>
        <div class="total"><dt>Amount sent</dt><dd>{fmt(plan.amountE8s)} icUSD</dd></div>
        <div><dt>From wallet</dt><dd><code>{plan.holder}</code></dd></div>
        <div><dt>From token account</dt><dd><code>{plan.sourceAccount}</code></dd></div>
        <div class="total" data-testid="return-account"><dt>Sent to the bridge's return account</dt><dd><code>{plan.returnAccount}</code></dd></div>
        <div><dt>icUSD mint</dt><dd><code>{plan.config.mint}</code></dd></div>
        <div><dt>Token program</dt><dd>{tokenProgramName} (<code>{plan.config.tokenProgramId}</code>)</dd></div>
        <div class="total" data-testid="return-destination"><dt>Released on the Internet Computer to</dt><dd><code>{plan.icpPrincipal}</code>{plan.icpSubaccount ? ` / ${plan.icpSubaccount}` : ''}</dd></div>
        <div><dt>Bridge release fee, deducted</dt><dd>{fmt(plan.config.releaseFeeE8s)} icUSD</dd></div>
        <div><dt>Memo (written into the transaction)</dt><dd><code data-testid="return-memo">{plan.memo}</code></dd></div>
        <div><dt>Network</dt><dd>{plan.config.network.name}</dd></div>
      </dl>
      <p class="small-note">
        Your Solana wallet signs one transaction: a checked token transfer plus this memo. It is valid for about a minute; if you wait longer it is prepared again. Solana finality takes {plan.config.network.finalityNote}; the release on the Internet Computer follows the bridge's burn.
      </p>
    </div>
    <button type="button" class="btn-primary wide" onclick={confirm}>Sign in your Solana wallet</button>
    <button type="button" class="btn-secondary wide" onclick={startOver}>Edit</button>
  {/if}

  {#if flow.kind === 'signing'}
    <p class="banner" role="status">Approve the transfer in your Solana wallet. Do not close this page. Nothing counts until Solana finalizes it.</p>
  {/if}
  {#if flow.kind === 'signed'}
    <p class="banner" role="status">Signed. Sending the saved transaction to Solana...</p>
  {/if}

  {#if flow.kind === 'broadcast_uncertain'}
    <div class="banner warn" role="alert">
      <strong>The signed transaction may not have reached Solana.</strong>
      <p>{flow.message}</p>
      <div class="row">
        <button type="button" class="btn-primary small" onclick={resend}>Send the saved transaction again</button>
        <button type="button" class="btn-secondary small" onclick={checkFlowEvidence} disabled={checking}>Check the evidence</button>
      </div>
    </div>
  {/if}

  {#if flow.kind === 'prepared'}
    <div class="banner warn" role="status">
      <strong>Saved, but your wallet was never asked.</strong>
      <p>Nothing was signed or sent. You can continue with the exact transaction you reviewed (if its blockhash is still valid) or discard it.</p>
      <div class="row">
        <button type="button" class="btn-primary small" onclick={proceed}>Continue in my wallet</button>
        <button type="button" class="btn-secondary small" onclick={discardFlowAttempt}>Discard</button>
      </div>
    </div>
  {/if}

  {#if flow.kind === 'held'}
    {@const held = flow.intent}
    <div class="banner warn" role="alert" data-testid="held">
      <strong>This attempt has no recorded outcome. It is held.</strong>
      <p>{flow.message}</p>
      <p>Nothing was rebuilt or signed again, and nothing will be. Check the evidence: it looks for the saved signature, or for a transaction with exactly the saved message, on Solana and at the bridge.</p>
      <div class="row">
        <button type="button" class="btn-primary small" onclick={() => checkEvidence(held)} disabled={checking || !rpc}>{checking ? 'Checking...' : 'Check the evidence'}</button>
        {#if mayDiscard(held, verdict)}
          <button type="button" class="btn-secondary small" onclick={() => discard(held)}>Discard this attempt</button>
        {/if}
      </div>
      {#if evidence}
        <p data-testid="evidence">
          {#if evidence.signature}Signature <code>{evidence.signature}</code>{evidence.recovered ? ' (found by its exact message)' : ''}. {/if}
          {describeReturnProgress(evidence).label}. {describeReturnProgress(evidence).detail}
        </p>
      {/if}
    </div>
  {/if}

  {#if flow.kind === 'tracking' && progress}
    {@const signature = flow.intent.signature ?? evidence?.signature ?? null}
    <section class="status-card" class:good={progress.terminal} aria-live="polite" aria-label="Return status" data-testid="tracking">
      <header><span class="kicker">SOLANA RETURN</span><strong class="label">{progress.label}</strong></header>
      <p class="detail">{progress.detail}</p>
      <dl class="facts">
        <div><dt>Sent</dt><dd>{fmt(flow.intent.amountE8s)} icUSD</dd></div>
        <div><dt>To</dt><dd><code>{flow.intent.icpPrincipal}</code></dd></div>
        {#if signature}
          <div><dt>Solana transaction (a hint until the bridge credits it)</dt><dd><a href={explorerTxUrl(config.network, signature)} target="_blank" rel="noopener noreferrer" title={signature}>{shortAddress(signature)}</a></dd></div>
        {/if}
      </dl>
      {#if signature && !progress.terminal}
        <div class="actions"><button type="button" class="btn-secondary small" onclick={() => tellBridge(signature)} disabled={!signedIn}>Tell the bridge now</button></div>
      {/if}
      {#if note}<p class="small-note" role="status">{note}</p>{/if}
    </section>
    {#if progress.terminal}
      <button type="button" class="btn-secondary wide" onclick={startOver}>Start another return</button>
    {/if}
  {/if}
  {#if note && flow.kind !== 'tracking'}<p class="small-note" role="status">{note}</p>{/if}
</section>

<style>
  .flow { display:flex; flex-direction:column; gap:0.9rem; }
  .intro { margin:0; font-size:0.85rem; line-height:1.55; color:var(--rumi-text-secondary); }
  .field { display:flex; flex-direction:column; gap:0.35rem; }
  label { font-size:0.8rem; font-weight:600; color:var(--rumi-text-secondary); }
  input, select { width:100%; padding:0.7rem 0.8rem; color:var(--rumi-text-primary); background:var(--rumi-bg-primary); border:1px solid var(--rumi-border-hover); border-radius:0.5rem; font:inherit; font-size:0.95rem; }
  input:focus-visible, select:focus-visible { outline:none; border-color:var(--rumi-border-accent); box-shadow:0 0 0 2px var(--rumi-action-dim); }
  input:disabled, select:disabled { opacity:0.6; }
  details { font-size:0.78rem; color:var(--rumi-text-secondary); }
  details input { margin-top:0.4rem; }
  .small-note { margin:0.2rem 0 0; font-size:0.72rem; line-height:1.5; color:var(--rumi-text-muted); }
  .banner { margin:0; padding:0.65rem 0.8rem; border:1px solid var(--rumi-border); border-radius:0.5rem; background:var(--rumi-bg-surface2); font-size:0.78rem; line-height:1.5; color:var(--rumi-text-secondary); }
  .banner p { margin:0.35rem 0 0; }
  .banner.warn { border-color:rgba(229,168,90,0.4); color:#ffd9a8; }
  .banner.bad { border-color:rgba(224,107,159,0.4); background:rgba(224,107,159,0.08); color:#ffb3c5; }
  .row { display:flex; flex-wrap:wrap; justify-content:space-between; align-items:center; gap:0.5rem; margin-top:0.5rem; }
  .wide { width:100%; }
  .small { padding:0.4rem 0.8rem; font-size:0.75rem; }
  .review { padding:0.9rem 1rem; border:1px solid var(--rumi-border); border-radius:0.6rem; background:var(--rumi-bg-primary); }
  .review h3 { margin:0 0 0.5rem; font-size:0.9rem; color:var(--rumi-text-primary); }
  dl { display:grid; gap:0.4rem; margin:0; }
  dl div { display:flex; justify-content:space-between; gap:1rem; font-size:0.8rem; }
  dt { color:var(--rumi-text-secondary); }
  dd { margin:0; text-align:right; color:var(--rumi-text-primary); font-variant-numeric:tabular-nums; overflow-wrap:anywhere; }
  .total dt, .total dd { font-weight:600; color:var(--rumi-text-primary); }
  code { font-size:0.72rem; overflow-wrap:anywhere; }
  .status-card { padding:0.9rem 1rem; border:1px solid var(--rumi-border); border-radius:0.6rem; background:var(--rumi-bg-primary); }
  .status-card.good { border-color:var(--rumi-border-teal); }
  .status-card header { display:flex; flex-direction:column; gap:0.15rem; }
  .kicker { font-size:0.65rem; letter-spacing:0.09em; color:var(--rumi-text-muted); font-weight:600; }
  .label { font-size:1rem; color:var(--rumi-text-primary); }
  .detail { margin:0.35rem 0 0; font-size:0.8rem; line-height:1.5; color:var(--rumi-text-secondary); }
  .facts { margin-top:0.6rem; }
  .facts div { font-size:0.75rem; }
  .facts a { color:var(--rumi-action-bright); }
  .actions { margin-top:0.7rem; }
  button:disabled { opacity:0.45; cursor:not-allowed; }
</style>
