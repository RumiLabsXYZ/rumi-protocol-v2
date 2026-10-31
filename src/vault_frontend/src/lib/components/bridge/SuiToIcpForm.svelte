<script lang="ts">
  import { untrack } from 'svelte';
  import type { Principal } from '@dfinity/principal';
  import type { _SERVICE as SuiService } from '$declarations/rumi_icusd_sui_bridge/rumi_icusd_sui_bridge.did';
  import { walletSessionGeneration } from '$lib/services/auth';
  import { formatE8s } from '$lib/services/baseBridge/amounts';
  import { isEnabled, type DestinationView } from '$lib/services/baseBridge/destinations';
  import { captureSessionGuard } from '$lib/services/baseBridge/bridgeActor';
  import { withSigning, type SigningContext } from '$lib/services/baseBridge/signingGuard';
  import { shortAddress } from '$lib/services/baseBridge/status';
  import { formatMist } from '$lib/services/suiBridge/units';
  import type { SuiBridgeConfig } from '$lib/services/suiBridge/config';
  import { explorerTxUrl } from '$lib/services/suiBridge/networks';
  import type { SuiIntentStore, SuiReturnIntent } from '$lib/services/suiBridge/intents';
  import type { SuiCoin, SuiRpc } from '$lib/services/suiBridge/rpc';
  import type { SuiWalletSession, SuiWalletState } from '$lib/services/suiBridge/walletStandard';
  import { getAnonymousSuiBridge } from '$lib/services/suiBridge/suiActor';
  import { buildSuiToIcpDeps, createSuiSigners, type SuiSigners } from '$lib/services/suiBridge/suiDeps';
  import {
    INITIAL_SUI_RETURN_STATE,
    continuePrepared,
    describeReturnProgress,
    discardReturn,
    evaluateReturnDraft,
    mayDiscard,
    notifyReturn,
    prepareReturn,
    rebroadcastReturn,
    reconcileReturn,
    reduceSuiReturn,
    startReturn,
    trackReturn,
    trackedReturns,
    unresolvedReturns,
    verdictOf,
    type ReturnEvidence,
    type ReturnVerdict,
    type SuiReturnEvent,
    type SuiReturnState,
  } from '$lib/services/suiBridge/suiToIcp';

  interface Props {
    config: SuiBridgeConfig;
    destination: DestinationView;
    readFreshDestination?: () => DestinationView | null;
    onRecheck?: () => void;
    wallet: SuiWalletState;
    /** The connected Sui wallet as a signing session, read at the moment of use. */
    getSession: () => SuiWalletSession | null;
    /** The keyless Sui node adapter. Null when this build has none configured. */
    rpc: SuiRpc | null;
    principal: Principal | null;
    store: SuiIntentStore | null;
    onConnect: () => void;
    onActivity?: () => void;
  }

  let { config, destination, readFreshDestination, onRecheck, wallet, getSession, rpc, principal, store, onConnect, onActivity }: Props = $props();

  /** A reviewed transfer older than this is prepared again on confirm: the coin versions it references must still be current. */
  const PLAN_MAX_AGE_MS = 120_000;

  let amountText = $state('');
  let principalInput = $state('');
  let principalTouched = $state(false);
  let subaccountText = $state('');
  let holdings = $state.raw<SuiCoin[] | null>(null);
  let holdingsError = $state('');
  let flow = $state.raw<SuiReturnState>(INITIAL_SUI_RETURN_STATE);
  let evidence = $state.raw<ReturnEvidence | null>(null);
  let verdict = $state<ReturnVerdict | null>(null);
  let formError = $state('');
  let note = $state('');
  let checking = $state(false);
  let signers = $state.raw<SuiSigners | null>(null);
  /** The ICP wallet's actors exist for the current session. "Tell the bridge now" is offered only then (see `tellBridge`). */
  let actorsReady = $state(false);
  let preparingActors = $state(false);
  let anonymous = $state.raw<SuiService | null>(null);
  let storeVersion = $state(0);
  let sessionGuard: (() => boolean) | null = null;
  let autoResumed = false;

  const signedIn = $derived(!!principal && !principal.isAnonymous());
  const principalText = $derived(principal ? principal.toText() : '');
  const connected = $derived(wallet.status === 'connected' && !!wallet.address);
  const onNetwork = $derived(connected && wallet.chains.includes(config.network.chain));
  const busy = $derived(flow.kind === 'preparing' || flow.kind === 'signing' || flow.kind === 'signed');
  const locked = $derived(busy || flow.kind === 'held' || flow.kind === 'broadcast_uncertain' || flow.kind === 'prepared' || flow.kind === 'tracking');
  const evaluation = $derived(evaluateReturnDraft({ amountText, principalText: principalInput, subaccountText }, config, holdings));
  const heldE8s = $derived(holdings ? holdings.reduce((sum, coin) => sum + coin.balance, 0n) : null);

  const deviceAttempts = $derived.by(() => {
    storeVersion;
    return store
      ? store.listFor(destination.id).filter((i): i is SuiReturnIntent => i.kind === 'sui_to_icp' && i.status !== 'done')
      : [];
  });

  function dispatch(event: SuiReturnEvent) {
    flow = reduceSuiReturn(flow, event);
  }
  function touched() {
    storeVersion += 1;
    onActivity?.();
  }

  // Default the ICP destination to the signed-in principal until the user edits it.
  $effect(() => {
    if (!principalTouched && principalText) principalInput = principalText;
  });

  // The holder's icUSD coin objects, read when a wallet account is connected (a keyless read, no wallet call).
  $effect(() => {
    const address = wallet.address;
    const node = rpc;
    if (!address || !node || !onNetwork) {
      holdings = null;
      return;
    }
    const controller = new AbortController();
    holdingsError = '';
    void node
      .listCoins(address, config.coinType)
      .then((list) => {
        if (!controller.signal.aborted) holdings = list;
      })
      .catch((error) => {
        if (!controller.signal.aborted) holdingsError = `Could not read your icUSD on Sui: ${error instanceof Error ? error.message : String(error)}`;
      });
    return () => controller.abort();
  });

  // An account or network change in the wallet leaves the saved attempt exactly as it was; only the draft is dropped.
  let seenAccount: string | null = null;
  $effect(() => {
    const key = `${wallet.address ?? ''}|${wallet.chains.join(',')}`;
    if (seenAccount !== null && seenAccount !== key) {
      untrack(() => {
        if (flow.kind === 'review') flow = INITIAL_SUI_RETURN_STATE;
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
        actorsReady = false;
        if (flow.kind === 'signing' || flow.kind === 'signed') flow = { kind: 'session_changed' };
        else if (flow.kind === 'review' || flow.kind === 'preparing') flow = INITIAL_SUI_RETURN_STATE;
      });
    }
    seenSession = key;
  });

  async function ensureReady(): Promise<boolean> {
    if (signers && anonymous && sessionGuard && sessionGuard()) {
      actorsReady = true;
      return true;
    }
    const guard = captureSessionGuard();
    preparingActors = true;
    try {
      const [createdSigners, reader] = await Promise.all([createSuiSigners(), getAnonymousSuiBridge()]);
      if (!guard()) return false;
      signers = createdSigners;
      anonymous = reader;
      sessionGuard = guard;
      actorsReady = true;
      return true;
    } finally {
      preparingActors = false;
    }
  }

  // A broadcast attempt on screen: prepare the ICP wallet's actors NOW, so that the explicit "Tell the bridge now" click
  // can call the signer with no await in front of it. Preparing never notifies anything.
  $effect(() => {
    if (flow.kind !== 'tracking' || !signedIn || actorsReady) return;
    untrack(() => {
      if (!preparingActors) void ensureReady().catch(() => false);
    });
  });

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

  function buildDeps(reader: SuiService | null = anonymous) {
    if (!store || !rpc || !reader) return null;
    const guard = sessionGuard ?? captureSessionGuard();
    return withSigning(buildSuiToIcpDeps({ config, store, rpc, wallet: getSession, signers, anonymous: reader, isCurrent: guard }), signingContext());
  }

  async function review() {
    formError = '';
    if (!principal || !signedIn) {
      formError = 'Connect your ICP wallet first. The bridge journal that protects this transfer is kept for your signed-in account.';
      return;
    }
    if (!connected || !onNetwork) {
      formError = connected ? `Switch your wallet to ${config.network.name}.` : 'Connect your Sui wallet first.';
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
      const reader = anonymous ?? (await getAnonymousSuiBridge());
      const deps = buildDeps(reader);
      if (!deps) {
        dispatch({ type: 'plan_failed', message: 'This build has no Sui node configured, so a return cannot be prepared.' });
        return;
      }
      const prepared = await prepareReturn(deps, { amountText, principalText: principalInput, subaccountText });
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

  async function checkEvidence(intent: SuiReturnIntent) {
    if (!rpc || checking) return;
    checking = true;
    note = '';
    try {
      const reader = anonymous ?? (await getAnonymousSuiBridge());
      const deps = buildDeps(reader);
      if (!deps) return;
      const found = await reconcileReturn(deps, intent);
      evidence = found;
      verdict = verdictOf(found);
      if (verdict === 'landed' || verdict === 'in_flight' || verdict === 'credited') {
        dispatch({ type: 'evidence', intent, evidence: found });
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

  function discard(intent: SuiReturnIntent) {
    if (!store) return;
    const result = discardReturn({ store }, intent, verdict);
    if (!result.ok) {
      note = result.message;
      return;
    }
    flow = INITIAL_SUI_RETURN_STATE;
    evidence = null;
    verdict = null;
    touched();
  }

  function open(intent: SuiReturnIntent) {
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
        const reader = anonymous ?? (await getAnonymousSuiBridge());
        if (controller.signal.aborted) return;
        await trackReturn(
          {
            rpc: node,
            store: intentStore,
            reads: {
              getRelease: async (transferId) => (await reader.get_release(transferId))[0] ?? null,
              getCandidate: async (sender, nonce) => (await reader.get_observer()).candidates.find((c) => c.sui_sender === sender && c.client_nonce === BigInt(nonce)) ?? null,
            },
          },
          intent,
          (found) => {
            evidence = found;
            dispatch({ type: 'evidence', intent, evidence: found });
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

  /**
   * A distinct click that only PREPARES the ICP wallet's actors (when they were not ready). It never notifies the bridge, and
   * nothing from it is queued for later: the user clicks "Tell the bridge now" afterwards.
   */
  async function prepareActors() {
    note = '';
    try {
      if (!(await ensureReady())) note = 'Your ICP wallet session changed. Try again.';
    } catch (error) {
      note = `Your ICP wallet is not ready: ${error instanceof Error ? error.message : String(error)}`;
    }
  }

  /**
   * "Tell the bridge now": an explicit click, with the ICP signer call made SYNCHRONOUSLY from it. Everything it needs
   * (the actors) was prepared before this button was offered, so there is no await between the click and
   * `notify_sui_return`: a popup signer keeps the user gesture. If the actors are not ready it says so and does nothing;
   * it never queues a call for later and never runs on its own.
   */
  function tellBridge(digest: string) {
    note = '';
    const deps = actorsReady ? buildDeps() : null;
    if (!deps) {
      note = 'Your ICP wallet is not ready yet. Prepare it first, then click Tell the bridge now.';
      return;
    }
    void notifyReturn(deps, digest).then((result) => {
      note = result.ok ? 'The bridge was told. It still decides from its own reads of Sui.' : result.message;
    });
  }

  function startOver() {
    flow = INITIAL_SUI_RETURN_STATE;
    evidence = null;
    verdict = null;
    formError = '';
    note = '';
  }

  const fmt = (value: bigint) => formatE8s(value);
  const progress = $derived(flow.kind === 'tracking' ? describeReturnProgress(flow.evidence ?? evidence) : null);
</script>

<section class="flow" aria-label="Bridge icUSD from Sui to the Internet Computer">
  <p class="intro">
    Burn icUSD from your Sui wallet through the Gate. The bridge verifies the burn from the Gate's own record, once it is final on Sui, and only then releases the same amount, minus the release fee, on the Internet Computer.
    The call carries your Internet Computer destination, so check it before you approve.
  </p>

  {#if !rpc}
    <p class="banner warn" role="status">This build has no Sui node configured, so returns cannot be prepared here.</p>
  {/if}

  {#if deviceAttempts.length > 0 && flow.kind === 'draft'}
    <div class="banner warn" role="status" data-testid="device-attempts">
      <strong>Earlier Sui returns on this device</strong>
      {#each deviceAttempts as intent (intent.id)}
        <div class="row">
          <span>{fmt(intent.amountE8s)} icUSD from <code>{shortAddress(intent.sender)}</code> · {intent.status === 'sent' ? 'sent' : intent.status === 'prepared' ? 'not sent' : 'no recorded outcome'}</span>
          <button type="button" class="btn-secondary small" onclick={() => open(intent)}>Open</button>
        </div>
      {/each}
      <p class="small-note">Opening an attempt never signs or sends anything. It is shown exactly as it was saved.</p>
    </div>
  {/if}

  {#if !connected}
    <button type="button" class="btn-secondary wide" onclick={onConnect} disabled={wallet.status === 'connecting' || wallet.status === 'unavailable'}>Connect your Sui wallet</button>
  {/if}

  <div class="field">
    <label for="sui-ret-amount">Amount to return (icUSD)</label>
    <input id="sui-ret-amount" type="text" inputmode="decimal" autocomplete="off" placeholder="0.00" bind:value={amountText} disabled={locked}
      oninput={() => { if (flow.kind === 'review') flow = INITIAL_SUI_RETURN_STATE; }} />
  </div>

  <div class="field">
    <span class="plain-label">Your icUSD on Sui</span>
    <p class="holding" data-testid="holdings">
      {#if !connected}Connect a wallet first.{:else if !rpc}Not available in this build.{:else if heldE8s === null}Reading...{:else}{fmt(heldE8s)} icUSD in {holdings?.length ?? 0} coin object{(holdings?.length ?? 0) === 1 ? '' : 's'}{/if}
    </p>
    {#if holdingsError}<p class="banner bad" role="alert">{holdingsError}</p>{/if}
  </div>

  <div class="field">
    <label for="sui-ret-principal">Internet Computer principal that receives the icUSD</label>
    <input id="sui-ret-principal" type="text" autocomplete="off" spellcheck="false" placeholder="xxxxx-xxxxx-..." bind:value={principalInput} disabled={locked}
      oninput={() => { principalTouched = true; if (flow.kind === 'review') flow = INITIAL_SUI_RETURN_STATE; }} />
    <details>
      <summary>Subaccount (optional)</summary>
      <input id="sui-ret-subaccount" type="text" autocomplete="off" spellcheck="false" placeholder="64 lowercase hex characters, or empty" bind:value={subaccountText} disabled={locked}
        oninput={() => { if (flow.kind === 'review') flow = INITIAL_SUI_RETURN_STATE; }} />
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
        <div class="total"><dt>Amount burned</dt><dd>{fmt(plan.amountE8s)} icUSD</dd></div>
        <div><dt>From Sui address</dt><dd><code>{plan.sender}</code></dd></div>
        <div><dt>Network</dt><dd>{plan.config.network.name}</dd></div>
        <div><dt>Package</dt><dd><code>{plan.config.packageId}</code></dd></div>
        <div><dt>Gate</dt><dd><code>{plan.config.gateId}</code></dd></div>
        <div><dt>icUSD coin type</dt><dd><code>{plan.config.coinType}</code></dd></div>
        <div><dt>Coin objects used</dt><dd>{plan.coins.length}{plan.coinTotalE8s > plan.amountE8s ? ` (the exact amount is split off; ${fmt(plan.coinTotalE8s - plan.amountE8s)} icUSD stays in your coin)` : ''}</dd></div>
        <div class="total" data-testid="return-destination"><dt>Released on the Internet Computer to</dt><dd><code>{plan.icpPrincipal}</code>{plan.icpSubaccount ? ` / ${plan.icpSubaccount}` : ' (default subaccount)'}</dd></div>
        <div><dt>Bridge release fee, deducted</dt><dd>{fmt(plan.config.releaseFeeE8s)} icUSD</dd></div>
        <div><dt>Client nonce</dt><dd><code data-testid="return-nonce">{plan.nonce}</code></dd></div>
        <div><dt>Transfer id the bridge will use</dt><dd><code data-testid="return-transfer-id">{plan.transferId}</code></dd></div>
        <div><dt>Gas (paid in SUI, separate from icUSD)</dt><dd data-testid="return-gas">{plan.gasBalanceMist === null ? 'your wallet chooses' : `you hold ${formatMist(plan.gasBalanceMist)} SUI`}</dd></div>
      </dl>
      <p class="small-note">
        Your Sui wallet signs one transaction that burns exactly {fmt(plan.amountE8s)} icUSD through the Gate. The transfer id is the Gate's own id for this attempt, not a transaction digest. Sui finality takes {plan.config.network.finalityNote}; the release on the Internet Computer follows the bridge's verification of the burn.
      </p>
    </div>
    <button type="button" class="btn-primary wide" onclick={confirm}>Sign in your Sui wallet</button>
    <button type="button" class="btn-secondary wide" onclick={startOver}>Edit</button>
  {/if}

  {#if flow.kind === 'signing'}
    <p class="banner" role="status">Approve the transfer in your Sui wallet. Do not close this page. Nothing counts until Sui finalizes it.</p>
  {/if}
  {#if flow.kind === 'signed'}
    <p class="banner" role="status">Signed. Sending the saved transaction to Sui...</p>
  {/if}

  {#if flow.kind === 'broadcast_uncertain'}
    <div class="banner warn" role="alert">
      <strong>The signed transaction may not have reached Sui.</strong>
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
      <p>Nothing was signed or sent. You can continue with the same call you reviewed (same coins, nonce and transfer id) or discard it.</p>
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
      <p>Nothing was rebuilt or signed again, and nothing will be. Check the evidence: it asks the Sui node about the saved digest, if there is one, and the bridge about the transfer id. Not finding it is not proof that it did not run.</p>
      <div class="row">
        <button type="button" class="btn-primary small" onclick={() => checkEvidence(held)} disabled={checking || !rpc}>{checking ? 'Checking...' : 'Check the evidence'}</button>
        {#if mayDiscard(held, verdict)}
          <button type="button" class="btn-secondary small" onclick={() => discard(held)}>Discard this attempt</button>
        {/if}
      </div>
      {#if evidence}
        <p data-testid="evidence">
          {#if evidence.digest}Digest <code>{evidence.digest}</code>. {/if}
          {describeReturnProgress(evidence).label}. {describeReturnProgress(evidence).detail}
        </p>
      {/if}
    </div>
  {/if}

  {#if flow.kind === 'tracking' && progress}
    {@const digest = flow.intent.digest ?? evidence?.digest ?? null}
    <section class="status-card" class:good={progress.terminal} aria-live="polite" aria-label="Return status" data-testid="tracking">
      <header><span class="kicker">SUI RETURN</span><strong class="label">{progress.label}</strong></header>
      <p class="detail">{progress.detail}</p>
      <dl class="facts">
        <div><dt>Sent</dt><dd>{fmt(flow.intent.amountE8s)} icUSD</dd></div>
        <div><dt>To</dt><dd><code>{flow.intent.icpPrincipal}</code></dd></div>
        <div><dt>Transfer id</dt><dd><code>{flow.intent.transferId}</code></dd></div>
        {#if digest}
          <div><dt>Sui transaction (a hint until the bridge records the burn)</dt><dd><a href={explorerTxUrl(config.network, digest)} target="_blank" rel="noopener noreferrer" title={digest}>{shortAddress(digest)}</a></dd></div>
        {/if}
      </dl>
      {#if digest && !progress.terminal}
        <div class="actions">
          {#if actorsReady}
            <button type="button" class="btn-secondary small" onclick={() => tellBridge(digest)} disabled={!signedIn}>Tell the bridge now</button>
          {:else}
            <button type="button" class="btn-secondary small" onclick={prepareActors} disabled={!signedIn || preparingActors}>
              {preparingActors ? 'Preparing your ICP wallet...' : 'Prepare ICP wallet to tell the bridge'}
            </button>
          {/if}
        </div>
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
  input { width:100%; padding:0.7rem 0.8rem; color:var(--rumi-text-primary); background:var(--rumi-bg-primary); border:1px solid var(--rumi-border-hover); border-radius:0.5rem; font:inherit; font-size:0.95rem; }
  input:focus-visible { outline:none; border-color:var(--rumi-border-accent); box-shadow:0 0 0 2px var(--rumi-action-dim); }
  input:disabled { opacity:0.6; }
  details { font-size:0.78rem; color:var(--rumi-text-secondary); }
  details input { margin-top:0.4rem; }
  .plain-label { font-size:0.8rem; font-weight:600; color:var(--rumi-text-secondary); }
  .holding { margin:0; font-size:0.85rem; color:var(--rumi-text-primary); font-variant-numeric:tabular-nums; }
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
