<script lang="ts">
  import { untrack } from 'svelte';
  import type { Principal } from '@dfinity/principal';
  import type { _SERVICE as BridgeService } from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
  import { walletSessionGeneration } from '$lib/services/auth';
  import { formatE8s, tryParseE8s } from '$lib/services/baseBridge/amounts';
  import type { BridgeConfig } from '$lib/services/baseBridge/chains';
  import { isEnabled, type DestinationView } from '$lib/services/baseBridge/destinations';
  import { displayAddress } from '$lib/services/baseBridge/eip55';
  import type { EvmWalletState } from '$lib/services/baseBridge/evmWallet';
  import { intentDestination, type IcpToBaseIntent, type IntentStore } from '$lib/services/baseBridge/intents';
  import { withSigning, type SigningContext } from '$lib/services/baseBridge/signingGuard';
  import {
    buildIcpToBaseDeps,
    buildPrepareDeps,
    captureSessionGuard,
    createSigners,
    getAnonymousBridge,
    needsSeparateApprovalClick,
    type Signers,
  } from '$lib/services/baseBridge/bridgeActor';
  import {
    INITIAL_ICP_TO_BASE_STATE,
    bridgeAfterApproval,
    discardIntent,
    isQuoteStale,
    prepareIcpToBase,
    reduceIcpToBase,
    requestDepositRefund,
    resumeIcpToBase,
    retryIcpToBase,
    startIcpToBase,
    trackDeposit,
    APPROVAL_TTL_MS,
    unresolvedIntents,
    validateRecipient,
    type IcpToBaseEvent,
    type IcpToBaseState,
  } from '$lib/services/baseBridge/icpToBase';
  import { DEPOSIT_DELIVERY_NOTE, describeDeposit } from '$lib/services/baseBridge/status';
  import DepositStatusCard from './DepositStatusCard.svelte';

  interface Props {
    config: BridgeConfig;
    /** The destination this form is for. It is enabled while this form exists. */
    destination: DestinationView;
    /** The newest known state of that destination (the page's latest read), or null if it is no longer reported. */
    readFreshDestination?: () => DestinationView | null;
    /** Ask the page to read the destination's status again. */
    onRecheck?: () => void;
    wallet: EvmWalletState;
    principal: Principal | null;
    store: IntentStore | null;
    onActivity?: () => void;
  }

  let { config, destination, readFreshDestination, onRecheck, wallet, principal, store, onActivity }: Props = $props();

  let amountText = $state('');
  let recipientText = $state('');
  let recipientTouched = $state(false);
  let formError = $state('');
  let flow = $state.raw<IcpToBaseState>(INITIAL_ICP_TO_BASE_STATE);
  let signers = $state.raw<Signers | null>(null);
  let anonymous = $state.raw<BridgeService | null>(null);
  let refundBusy = $state(false);
  let refundMessage = $state('');
  let storeVersion = $state(0);
  /** True for popup-signer wallets: the approval and the transfer are two separate clicks. */
  let twoStep = $state(needsSeparateApprovalClick());
  let sessionGuard: (() => boolean) | null = null;

  const signedIn = $derived(!!principal && !principal.isAnonymous());
  const principalText = $derived(principal ? principal.toText() : '');
  const bridgeBlocked = $derived(config.depositsPaused || !!config.halted);
  const busy = $derived(flow.kind === 'quoting' || flow.kind === 'approving' || flow.kind === 'submitting');
  const inputsLocked = $derived(busy || flow.kind === 'uncertain' || flow.kind === 'approved');
  const trackedDisplay = $derived(
    flow.kind === 'tracking' ? describeDeposit(flow.deposit, { principal: principalText, awaitingGas: config.minterAwaitingGas }) : null,
  );
  /**
   * A recipient that mixes upper and lower case must carry a valid EIP-55 checksum. This is checked while
   * typing and blocks the review step, so a typo cannot reach the bridge. (Other problems are reported on submit.)
   */
  const recipientChecksumProblem = $derived.by(() => {
    if (recipientText.trim() === '') return null;
    const check = validateRecipient(recipientText, config.contract);
    return !check.ok && (check.code === 'checksum' || check.code === 'checksum_unverifiable') ? check : null;
  });

  /** Earlier requests on this device that never recorded an outcome (not the one on screen right now). */
  const unresolved = $derived.by(() => {
    storeVersion;
    const onScreen =
      flow.kind === 'approving' || flow.kind === 'approved' || flow.kind === 'submitting' || flow.kind === 'uncertain' || flow.kind === 'tracking'
        ? flow.intent.clientRequestId
        : null;
    return store ? unresolvedIntents(store, destination.id).filter((intent) => intent.clientRequestId !== onScreen) : [];
  });
  /** Deposits started on this device that are still in progress. */
  const inProgress = $derived.by(() => {
    storeVersion;
    return store
      ? store
          .list()
          .filter(
            (i): i is IcpToBaseIntent =>
              i.kind === 'icp_to_base' && i.status === 'submitted' && i.depositId !== undefined && intentDestination(i) === destination.id,
          )
      : [];
  });

  function dispatch(event: IcpToBaseEvent) {
    flow = reduceIcpToBase(flow, event);
  }

  function touched() {
    storeVersion += 1;
    onActivity?.();
  }

  // Default the recipient to the connected Base account until the user edits the field.
  $effect(() => {
    if (!recipientTouched && wallet.account) recipientText = displayAddress(wallet.account);
  });

  // An ICP wallet session change abandons in-flight work. Intents stay on disk, keyed by principal.
  let seenSession: string | null = null;
  $effect(() => {
    const key = `${$walletSessionGeneration}:${principalText}`;
    if (seenSession !== null && seenSession !== key) {
      untrack(() => {
        const wasInFlight = flow.kind === 'approving' || flow.kind === 'submitting';
        signers = null;
        anonymous = null;
        sessionGuard = null;
        refundMessage = '';
        flow = wasInFlight ? { kind: 'session_changed' } : INITIAL_ICP_TO_BASE_STATE;
      });
    }
    seenSession = key;
  });

  /**
   * Make sure the wallet's actors and the anonymous reader exist for the current
   * session. Called ahead of any click that needs to sign, so the click handler
   * never has to wait for them. Returns false if the session changed meanwhile.
   */
  async function ensureReady(): Promise<boolean> {
    if (signers && anonymous && sessionGuard && sessionGuard()) return true;
    const guard = captureSessionGuard();
    const [createdSigners, reader] = await Promise.all([createSigners(), getAnonymousBridge()]);
    if (!guard()) return false;
    signers = createdSigners;
    anonymous = reader;
    sessionGuard = guard;
    return true;
  }

  // Poll the canister for the deposit once there is one. Stops on terminal states or when the view goes away.
  const trackedDepositId = $derived(flow.kind === 'tracking' ? flow.deposit.id : null);
  $effect(() => {
    const id = trackedDepositId;
    if (id === null || !principal || !store) return;
    const controller = new AbortController();
    const owner = principal;
    const intentStore = store;
    void (async () => {
      try {
        let reader: BridgeService | null = null;
        try {
          if (await untrack(() => ensureReady())) reader = anonymous;
        } catch {
          // Refund stays unavailable until the wallet reconnects; progress can still be read anonymously.
        }
        if (!reader) reader = await getAnonymousBridge();
        if (controller.signal.aborted) return;
        const anonymousReader = reader;
        await trackDeposit(
          {
            reads: { getDeposit: async (depositId) => (await anonymousReader.get_deposit(depositId))[0] ?? null },
            owner,
            store: intentStore,
          },
          id,
          (deposit) => dispatch({ type: 'deposit_updated', deposit }),
          controller.signal,
        );
        if (!controller.signal.aborted) touched();
      } catch {
        // The next page visit resumes from the intent journal.
      }
    })();
    return () => controller.abort();
  });

  /** What every signing flow carries: this destination, and its newest status read at the moment of use. */
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

  /**
   * Before a click that would sign: if the newest read no longer shows the destination as enabled, ask the page
   * to read again (the way a changed fee asks for a new review). The flow itself refuses to sign meanwhile.
   */
  function recheckIfUnavailable() {
    const fresh = readFreshDestination ? readFreshDestination() : destination;
    if (!fresh || !isEnabled(fresh)) onRecheck?.();
  }

  function buildDeps() {
    if (!principal || !store || !signers || !anonymous || !sessionGuard) return null;
    return withSigning(buildIcpToBaseDeps({ principal, store, signers, anonymous, isCurrent: sessionGuard }), signingContext());
  }

  async function review() {
    formError = '';
    if (!principal || !signedIn) {
      formError = 'Connect your ICP wallet first. The icUSD is taken from the account you are signed in with.';
      return;
    }
    const amount = tryParseE8s(amountText);
    if (!amount.ok) {
      formError = amount.message;
      return;
    }
    const recipient = validateRecipient(recipientText, config.contract);
    if (!recipient.ok) {
      formError = recipient.message;
      return;
    }
    twoStep = needsSeparateApprovalClick();
    dispatch({ type: 'quote_started' });
    const guard = captureSessionGuard();
    try {
      const [prepared, ready] = await Promise.all([
        prepareIcpToBase(buildPrepareDeps(principal), { amountE8s: amount.value, recipient: recipient.address }, formatE8s),
        ensureReady(),
      ]);
      if (!ready || !guard()) return;
      dispatch({ type: 'quote_ready', quote: prepared.quote, problem: prepared.problem });
    } catch (error) {
      if (!guard()) return;
      dispatch({
        type: 'quote_failed',
        message: `Could not prepare the transfer: ${error instanceof Error ? error.message : String(error)}`,
      });
    }
  }

  /**
   * Click handler. The first signer call is issued synchronously inside startIcpToBase.
   * For popup-signer wallets with an approval needed, this click only approves.
   */
  function confirm() {
    if (flow.kind !== 'review' || flow.problem) return;
    recheckIfUnavailable();
    const quote = flow.quote;
    if (isQuoteStale(quote, Date.now())) {
      void review();
      return;
    }
    const deps = buildDeps();
    if (!deps) {
      dispatch({ type: 'blocked', message: 'Your wallet is not ready. Review the transfer again.' });
      return;
    }
    void startIcpToBase(deps, quote, dispatch).then(touched);
  }

  /**
   * Second click of the two-click path. The approval is already confirmed, so the
   * transfer request is this click's first (and only) signer call.
   */
  function bridgeNow() {
    if (flow.kind !== 'approved') return;
    recheckIfUnavailable();
    const deps = buildDeps();
    if (!deps) {
      dispatch({ type: 'blocked', message: 'Your wallet is not ready. Wait a moment and try again.' });
      return;
    }
    void bridgeAfterApproval(deps, flow.intent, dispatch).then(touched);
  }

  function discardApproved() {
    if (flow.kind !== 'approved' || !store) return;
    discardIntent({ store }, flow.intent);
    flow = INITIAL_ICP_TO_BASE_STATE;
    touched();
  }

  function retry() {
    if (flow.kind !== 'uncertain' || flow.lookup === 'looking') return;
    recheckIfUnavailable();
    const deps = buildDeps();
    if (!deps) return;
    void retryIcpToBase(deps, flow.intent, dispatch).then(touched);
  }

  function discard() {
    if (flow.kind !== 'uncertain' || flow.lookup !== 'not_found' || !store) return;
    discardIntent({ store }, flow.intent);
    flow = INITIAL_ICP_TO_BASE_STATE;
    touched();
  }

  async function resume(intent: IcpToBaseIntent) {
    if (busy || !principal || !store) return;
    // Show what the earlier request was for, so the user can review it again if it needs re-approving.
    amountText = formatE8s(intent.amountE8s);
    recipientText = displayAddress(intent.recipient);
    recipientTouched = true;
    twoStep = needsSeparateApprovalClick();
    try {
      if (!(await ensureReady())) return;
      const deps = buildDeps();
      if (!deps) return;
      await resumeIcpToBase(deps, intent, dispatch);
      touched();
    } catch (error) {
      formError = `Could not check the earlier request: ${error instanceof Error ? error.message : String(error)}`;
    }
  }

  async function followDeposit(intent: IcpToBaseIntent) {
    if (busy || intent.depositId === undefined) return;
    try {
      const reader = anonymous ?? (await getAnonymousBridge());
      const deposit = (await reader.get_deposit(intent.depositId))[0];
      if (deposit) dispatch({ type: 'track_existing', intent, deposit });
    } catch (error) {
      formError = `Could not load that deposit: ${error instanceof Error ? error.message : String(error)}`;
    }
  }

  async function refund() {
    if (flow.kind !== 'tracking' || refundBusy) return;
    const deps = buildDeps();
    if (!deps) {
      refundMessage = 'Your wallet is not ready. Wait a moment and try again.';
      return;
    }
    refundBusy = true;
    refundMessage = '';
    // requestDepositRefund issues its signer call before any await.
    const result = await requestDepositRefund(deps, flow.deposit);
    refundBusy = false;
    if (result.ok) {
      dispatch({ type: 'deposit_updated', deposit: result.deposit });
      refundMessage = 'Refund requested. The bridge is returning your escrowed icUSD.';
    } else {
      refundMessage = result.message;
    }
    touched();
  }

  function startOver() {
    flow = INITIAL_ICP_TO_BASE_STATE;
    refundMessage = '';
    formError = '';
  }

  function useConnectedAccount() {
    if (!wallet.account) return;
    recipientTouched = false;
    recipientText = displayAddress(wallet.account);
  }

  const fmt = (value: bigint) => formatE8s(value);
  const approvalMinutes = Math.round(APPROVAL_TTL_MS / 60_000);
</script>

<section class="flow" aria-label="Bridge icUSD from the Internet Computer to Base">
  <p class="intro">
    Lock icUSD in the bridge's escrow on the Internet Computer and receive the same amount, minus the bridge fee, as icUSD on Base.
    This does not borrow or create debt.
  </p>

  {#if config.halted}
    <p class="banner bad" role="alert">The bridge is halted for safety: {config.halted}</p>
  {:else if config.depositsPaused}
    <p class="banner warn" role="status">Deposits to Base are paused right now. Your icUSD has not moved.</p>
  {/if}

  {#if unresolved.length > 0}
    <div class="banner warn" role="status">
      <strong>Earlier request without a recorded outcome</strong>
      {#each unresolved as intent (intent.clientRequestId)}
        <div class="row">
          <span>{fmt(intent.amountE8s)} icUSD to <code>{displayAddress(intent.recipient)}</code></span>
          <button type="button" class="btn-secondary small" onclick={() => resume(intent)} disabled={busy}>Check status</button>
        </div>
      {/each}
      <p class="small-note">Checking looks the request up on the bridge by its saved request id. It never sends anything again by itself.</p>
    </div>
  {/if}

  {#if inProgress.length > 0 && flow.kind !== 'tracking'}
    <div class="banner" role="status">
      <strong>Deposits in progress on this device</strong>
      {#each inProgress as intent (intent.clientRequestId)}
        <div class="row">
          <span>#{intent.depositId} · {fmt(intent.amountE8s)} icUSD</span>
          <button type="button" class="btn-secondary small" onclick={() => followDeposit(intent)}>Track</button>
        </div>
      {/each}
    </div>
  {/if}

  <div class="field">
    <label for="icp-amount">Amount to bridge (icUSD)</label>
    <input
      id="icp-amount"
      type="text"
      inputmode="decimal"
      autocomplete="off"
      placeholder="0.00"
      bind:value={amountText}
      disabled={inputsLocked}
      oninput={() => {
        if (flow.kind === 'review' || flow.kind === 'fee_changed') flow = INITIAL_ICP_TO_BASE_STATE;
      }}
    />
  </div>

  <div class="field">
    <div class="label-row">
      <label for="icp-recipient">Base recipient address</label>
      {#if wallet.account && recipientText.toLowerCase() !== wallet.account}
        <button type="button" class="link" onclick={useConnectedAccount} disabled={inputsLocked}>Use connected account</button>
      {/if}
    </div>
    <input
      id="icp-recipient"
      type="text"
      autocomplete="off"
      spellcheck="false"
      placeholder="0x..."
      bind:value={recipientText}
      disabled={inputsLocked}
      aria-invalid={recipientChecksumProblem ? 'true' : undefined}
      aria-describedby={recipientChecksumProblem ? 'icp-recipient-error' : undefined}
      oninput={() => {
        recipientTouched = true;
        if (flow.kind === 'review' || flow.kind === 'fee_changed') flow = INITIAL_ICP_TO_BASE_STATE;
      }}
    />
    {#if recipientChecksumProblem}
      <p id="icp-recipient-error" class="banner bad" role="alert">{recipientChecksumProblem.message}</p>
    {/if}
    <p class="small-note">The icUSD is minted to this address on {config.chain.name}. Double check it: a mistake cannot be undone.</p>
  </div>

  {#if !signedIn}
    <p class="banner" role="status">Connect your ICP wallet in the page header to bridge icUSD.</p>
  {/if}

  {#if flow.kind === 'draft' || flow.kind === 'error' || flow.kind === 'session_changed'}
    <button type="button" class="btn-primary wide" onclick={review} disabled={!signedIn || bridgeBlocked || inputsLocked || !!recipientChecksumProblem}>Review transfer</button>
  {/if}
  {#if flow.kind === 'quoting'}
    <button type="button" class="btn-primary wide" disabled>Reading fees and allowance...</button>
  {/if}

  {#if formError}<p class="banner bad" role="alert">{formError}</p>{/if}
  {#if flow.kind === 'error'}<p class="banner bad" role="alert">{flow.message}</p>{/if}
  {#if flow.kind === 'session_changed'}
    <p class="banner warn" role="alert">Your wallet session changed during the transfer, so nothing further was submitted. Check the history below for the result.</p>
  {/if}

  {#if flow.kind === 'review' || flow.kind === 'approving' || flow.kind === 'submitting' || flow.kind === 'approval_failed' || flow.kind === 'fee_changed' || flow.kind === 'rejected'}
    {@const quote = flow.quote}
    {#if quote}
      <div class="review" aria-label="Transfer summary">
        <h3>Review</h3>
        <dl>
          <div><dt>Pulled from your icUSD account</dt><dd>{fmt(quote.amountE8s)} icUSD</dd></div>
          <div><dt>icUSD ledger fee, charged on top</dt><dd>{fmt(quote.ledgerFeeE8s)} icUSD</dd></div>
          {#if quote.needsApproval}
            <div><dt>Approval fee (an allowance is needed first)</dt><dd>{fmt(quote.approvalFeeE8s)} icUSD</dd></div>
          {/if}
          <div class="total"><dt>Total leaving your account</dt><dd>{fmt(quote.totalDebitE8s)} icUSD</dd></div>
          <div><dt>Bridge deposit fee, deducted from the mint</dt><dd>{fmt(quote.depositFeeE8s)} icUSD</dd></div>
          <div class="total"><dt>You receive on {config.chain.name}</dt><dd>{fmt(quote.mintAmountE8s)} icUSD</dd></div>
          <div><dt>Recipient</dt><dd><code>{displayAddress(quote.recipient)}</code></dd></div>
        </dl>
        <p class="small-note">
          Expected delivery after the deposit is escrowed: {DEPOSIT_DELIVERY_NOTE} (Base L1 batch finality).
          {#if quote.needsApproval && twoStep}
            Your wallet needs two separate confirmations. First click approves exactly {fmt(quote.requiredAllowanceE8s)} icUSD for the bridge, valid for {approvalMinutes} minutes. Then click Bridge to Base to confirm the transfer.
          {:else if quote.needsApproval}
            Your wallet will first ask you to approve exactly {fmt(quote.requiredAllowanceE8s)} icUSD for the bridge, valid for {approvalMinutes} minutes, and then to confirm the transfer.
          {:else}
            Your existing allowance already covers this transfer, so only the transfer itself needs your approval.
          {/if}
        </p>
        {#if flow.kind === 'review' && flow.problem}
          <p class="banner bad" role="alert">{flow.problem.message}</p>
        {/if}
      </div>
    {/if}

    {#if flow.kind === 'review'}
      <button type="button" class="btn-primary wide" onclick={confirm} disabled={!!flow.problem || bridgeBlocked}>
        {flow.quote.needsApproval ? (twoStep ? 'Approve (step 1 of 2)' : 'Approve and bridge') : 'Bridge to Base'}
      </button>
      <button type="button" class="btn-secondary wide" onclick={startOver}>Edit</button>
    {/if}
    {#if flow.kind === 'approving'}
      <p class="banner" role="status">Approve exactly {fmt(flow.quote.requiredAllowanceE8s)} icUSD in your wallet. Nothing moves until the transfer step{twoStep ? ', which is the next click' : ''}.</p>
    {/if}
    {#if flow.kind === 'submitting'}
      <p class="banner" role="status">Sending the transfer request to the bridge. Please wait; do not close this page.</p>
    {/if}
    {#if flow.kind === 'approval_failed'}
      <p class="banner bad" role="alert">{flow.message}</p>
      <button type="button" class="btn-secondary wide" onclick={review}>Review again</button>
    {/if}
    {#if flow.kind === 'fee_changed'}
      <p class="banner warn" role="alert">
        The bridge deposit fee changed to {fmt(flow.currentFeeE8s)} icUSD (you reviewed {fmt(flow.quote.depositFeeE8s)} icUSD). No icUSD moved. Review the new amounts and confirm again.
      </p>
      <button type="button" class="btn-primary wide" onclick={review}>Review new fee</button>
    {/if}
    {#if flow.kind === 'rejected'}
      <p class="banner bad" role="alert">{flow.message} No icUSD moved.</p>
      {#if flow.reapprove}
        <button type="button" class="btn-primary wide" onclick={review}>Review and approve again</button>
      {/if}
      <button type="button" class="btn-secondary wide" onclick={startOver}>Edit transfer</button>
    {/if}
  {/if}

  {#if flow.kind === 'approved'}
    <div class="review" aria-label="Approved transfer">
      <h3>Approval confirmed</h3>
      <p class="banner" role="status">Approval confirmed. Click Bridge to Base to continue. Nothing has been sent to the bridge yet.</p>
      <dl>
        <div><dt>Amount to bridge</dt><dd>{fmt(flow.intent.amountE8s)} icUSD</dd></div>
        <div><dt>Bridge deposit fee, deducted from the mint</dt><dd>{fmt(flow.intent.expectedDepositFee)} icUSD</dd></div>
        <div class="total"><dt>You receive on {config.chain.name}</dt><dd>{fmt(flow.intent.amountE8s > flow.intent.expectedDepositFee ? flow.intent.amountE8s - flow.intent.expectedDepositFee : 0n)} icUSD</dd></div>
        <div><dt>Recipient</dt><dd><code>{displayAddress(flow.intent.recipient)}</code></dd></div>
      </dl>
      <p class="small-note">The icUSD ledger fee is charged on top. The approval expires about {approvalMinutes} minutes after it was granted; if it has expired the bridge will ask you to approve again.</p>
    </div>
    <button type="button" class="btn-primary wide" onclick={bridgeNow} disabled={bridgeBlocked}>Bridge to Base</button>
    <button type="button" class="btn-secondary wide" onclick={discardApproved}>Discard this request</button>
  {/if}

  {#if flow.kind === 'uncertain'}
    <div class="banner warn" role="alert">
      <strong>The result of this transfer is not confirmed yet.</strong>
      <p>{flow.message}</p>
      {#if flow.lookup === 'looking'}
        <p>Looking for the request on the bridge...</p>
      {:else if flow.lookup === 'not_found'}
        <p>The bridge has no record of this request yet. If you retry, the same request id is used, so it cannot be created twice.</p>
      {:else}
        <p>The bridge could not be asked about this request right now. Retrying is safe: the same request id is used, so the bridge returns the existing deposit instead of creating a second one.</p>
      {/if}
      <div class="row">
        <button type="button" class="btn-primary small" onclick={retry} disabled={flow.lookup === 'looking'}>Retry with the same request</button>
        {#if flow.lookup === 'not_found'}
          <button type="button" class="btn-secondary small" onclick={discard}>Discard this request</button>
        {/if}
      </div>
    </div>
  {/if}

  {#if flow.kind === 'tracking'}
    <DepositStatusCard
      deposit={flow.deposit}
      principal={principalText}
      chain={config.chain}
      awaitingGas={config.minterAwaitingGas}
      onRefund={refund}
      refundBusy={refundBusy || !signers}
      {refundMessage}
    />
    {#if trackedDisplay?.terminal}
      <button type="button" class="btn-secondary wide" onclick={startOver}>
        {trackedDisplay.stage === 'pull_failed' ? 'Try again' : 'Start another transfer'}
      </button>
    {/if}
  {/if}
</section>

<style>
  .flow { display:flex; flex-direction:column; gap:0.9rem; }
  .intro { margin:0; font-size:0.85rem; line-height:1.55; color:var(--rumi-text-secondary); }
  .field { display:flex; flex-direction:column; gap:0.35rem; }
  label { font-size:0.8rem; font-weight:600; color:var(--rumi-text-secondary); }
  .label-row { display:flex; justify-content:space-between; align-items:baseline; gap:0.5rem; }
  input { width:100%; padding:0.7rem 0.8rem; color:var(--rumi-text-primary); background:var(--rumi-bg-primary); border:1px solid var(--rumi-border-hover); border-radius:0.5rem; font:inherit; font-size:1rem; font-variant-numeric:tabular-nums; }
  input:focus-visible { outline:none; border-color:var(--rumi-border-accent); box-shadow:0 0 0 2px var(--rumi-action-dim); }
  input:disabled { opacity:0.6; }
  .small-note { margin:0.2rem 0 0; font-size:0.72rem; line-height:1.5; color:var(--rumi-text-muted); }
  .banner { margin:0; padding:0.65rem 0.8rem; border:1px solid var(--rumi-border); border-radius:0.5rem; background:var(--rumi-bg-surface2); font-size:0.78rem; line-height:1.5; color:var(--rumi-text-secondary); }
  .banner p { margin:0.35rem 0 0; }
  .banner.warn { border-color:rgba(229,168,90,0.4); color:#ffd9a8; }
  .banner.bad { border-color:rgba(224,107,159,0.4); background:rgba(224,107,159,0.08); color:#ffb3c5; }
  .row { display:flex; flex-wrap:wrap; justify-content:space-between; align-items:center; gap:0.5rem; margin-top:0.5rem; }
  .wide { width:100%; }
  .small { padding:0.4rem 0.8rem; font-size:0.75rem; }
  .link { padding:0; border:0; background:none; color:var(--rumi-action-bright); cursor:pointer; font:inherit; font-size:0.75rem; }
  .review { padding:0.9rem 1rem; border:1px solid var(--rumi-border); border-radius:0.6rem; background:var(--rumi-bg-primary); }
  .review h3 { margin:0 0 0.5rem; font-size:0.9rem; color:var(--rumi-text-primary); }
  dl { display:grid; gap:0.4rem; margin:0; }
  dl div { display:flex; justify-content:space-between; gap:1rem; font-size:0.8rem; }
  dt { color:var(--rumi-text-secondary); }
  dd { margin:0; text-align:right; color:var(--rumi-text-primary); font-variant-numeric:tabular-nums; overflow-wrap:anywhere; }
  .total dt, .total dd { font-weight:600; color:var(--rumi-text-primary); }
  code { font-size:0.72rem; }
  button:disabled { opacity:0.45; cursor:not-allowed; }
</style>
