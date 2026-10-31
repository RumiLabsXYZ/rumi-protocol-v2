<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import type { Principal } from '@dfinity/principal';
  import type { _SERVICE as BridgeService } from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
  import { walletSessionGeneration } from '$lib/services/auth';
  import { formatE8s, formatWeiAsEth, tryParseE8s } from '$lib/services/baseBridge/amounts';
  import type { BridgeConfig } from '$lib/services/baseBridge/chains';
  import { isEnabled, type DestinationView } from '$lib/services/baseBridge/destinations';
  import { displayAddress } from '$lib/services/baseBridge/eip55';
  import type { Eip1193Provider, EvmWalletState } from '$lib/services/baseBridge/evmWallet';
  import { intentDestination, type BaseToIcpIntent, type IntentStore } from '$lib/services/baseBridge/intents';
  import { withSigning, type SigningContext } from '$lib/services/baseBridge/signingGuard';
  import {
    buildBaseToIcpDeps,
    captureSessionGuard,
    createSigners,
    getAnonymousBridge,
    type Signers,
  } from '$lib/services/baseBridge/bridgeActor';
  import {
    INITIAL_BASE_TO_ICP_STATE,
    checkIntentOnChain,
    classifyUnsentIntent,
    discardUnsentIntent,
    evaluateBaseToIcpDraft,
    notifyBridge,
    readBaseSnapshot,
    reduceBaseToIcp,
    sendBridgeOut,
    trackBaseToIcp,
    type BaseSnapshot,
    type BaseToIcpEvent,
    type BaseToIcpState,
    type IntentRecovery,
  } from '$lib/services/baseBridge/baseToIcp';
  import { shortHash } from '$lib/services/baseBridge/status';
  import BaseTrackingCard from './BaseTrackingCard.svelte';

  interface Props {
    config: BridgeConfig;
    /** The destination this form is for. It is enabled while this form exists. */
    destination: DestinationView;
    /** The newest known state of that destination (the page's latest read), or null if it is no longer reported. */
    readFreshDestination?: () => DestinationView | null;
    /** Ask the page to read the destination's status again. */
    onRecheck?: () => void;
    wallet: EvmWalletState;
    getProvider: () => Eip1193Provider | null;
    principal: Principal | null;
    store: IntentStore | null;
    onConnect: () => void;
    onSwitch: () => void;
    onActivity?: () => void;
  }

  let { config, destination, readFreshDestination, onRecheck, wallet, getProvider, principal, store, onConnect, onSwitch, onActivity }: Props =
    $props();

  interface RecoveryItem {
    intent: BaseToIcpIntent;
    recovery: IntentRecovery;
  }

  let amountText = $state('');
  let flow = $state.raw<BaseToIcpState>(INITIAL_BASE_TO_ICP_STATE);
  let snapshot = $state.raw<BaseSnapshot | null>(null);
  let snapshotLoading = $state(false);
  let recoveries = $state.raw<RecoveryItem[]>([]);
  let recoveryMessage = $state('');
  let notifyBusy = $state(false);
  let notifyMessage = $state('');
  let storeVersion = $state(0);
  let anonymous = $state.raw<BridgeService | null>(null);
  let signers = $state.raw<Signers | null>(null);
  let sessionGuard: (() => boolean) | null = null;
  let snapshotSeq = 0;

  const signedIn = $derived(!!principal && !principal.isAnonymous());
  const principalText = $derived(principal ? principal.toText() : '');
  const provider = $derived.by(() => {
    // Re-read the provider whenever the wallet connection changes.
    wallet.status;
    wallet.walletId;
    return getProvider();
  });
  const locked = $derived(flow.kind === 'confirming' || flow.kind === 'awaiting_wallet' || flow.kind === 'tracking');

  const evaluation = $derived(
    evaluateBaseToIcpDraft(
      {
        walletAvailable: wallet.status !== 'unavailable',
        account: wallet.account,
        chainId: wallet.chainId,
        expectedChain: config.chain,
        icpPrincipal: principal ? principal.toText() : null,
        icpPrincipalIsAnonymous: !!principal && principal.isAnonymous(),
        amountText,
        balanceE8s: snapshot?.balanceE8s ?? null,
        minimumE8s: config.minBridgeOutE8s,
        ethBalanceWei: snapshot?.ethBalanceWei ?? null,
        gasCostWei: snapshot?.gasCostWei ?? null,
        preflightError: snapshot?.preflightError ?? null,
      },
      formatE8s,
    ),
  );

  const balanceLine = $derived.by(() => {
    if (snapshot && snapshot.balanceE8s !== null) return `Balance on Base: ${formatE8s(snapshot.balanceE8s)} icUSD · `;
    return snapshotLoading ? 'Reading your balance... · ' : '';
  });
  const gasLine = $derived(snapshot && snapshot.gasCostWei !== null ? `about ${formatWeiAsEth(snapshot.gasCostWei)} ETH` : null);

  /** Estimated amount that reaches the ICP account: the release fee and the ledger fee are deducted. */
  const estimatedReceive = $derived.by(() => {
    if (!evaluation.amountE8s) return null;
    const fees = config.releaseFeeE8s + (config.ledgerFeeE8s ?? 0n);
    return evaluation.amountE8s > fees ? evaluation.amountE8s - fees : 0n;
  });

  function dispatch(event: BaseToIcpEvent) {
    flow = reduceBaseToIcp(flow, event);
  }

  function touched() {
    storeVersion += 1;
    onActivity?.();
  }

  onMount(() => {
    void getAnonymousBridge()
      .then((reader) => (anonymous = reader))
      .catch(() => {});
  });

  // Read balances and the gas estimate through the wallet provider, a moment after the inputs settle.
  $effect(() => {
    const activeProvider = provider;
    const account = wallet.account;
    const chainId = wallet.chainId;
    const owner = principal;
    const amount = tryParseE8s(amountText);
    const sequence = ++snapshotSeq;
    if (!activeProvider || !account || chainId !== config.chain.chainId || !owner || owner.isAnonymous()) {
      snapshot = null;
      snapshotLoading = false;
      return;
    }
    const timer = setTimeout(async () => {
      snapshotLoading = true;
      try {
        const next = await readBaseSnapshot(
          { provider: activeProvider, contract: config.contract, account, icpPrincipal: owner },
          amount.ok ? amount.value : null,
        );
        if (sequence === snapshotSeq) snapshot = next;
      } finally {
        if (sequence === snapshotSeq) snapshotLoading = false;
      }
    }, 250);
    return () => clearTimeout(timer);
  });

  // An ICP session change abandons in-flight work. Saved intents stay on disk, keyed by principal.
  let seenSession: string | null = null;
  $effect(() => {
    const key = `${$walletSessionGeneration}:${principalText}`;
    if (seenSession !== null && seenSession !== key) {
      untrack(() => {
        signers = null;
        sessionGuard = null;
        notifyMessage = '';
        if (flow.kind !== 'awaiting_wallet') flow = INITIAL_BASE_TO_ICP_STATE;
      });
    }
    seenSession = key;
  });

  /** Saved Base -> ICP transfers that are not finished, other than the one being tracked. */
  const pending = $derived.by(() => {
    storeVersion;
    if (!store) return [] as BaseToIcpIntent[];
    // The transfer being sent or tracked right now is shown by the flow itself, not as a saved leftover.
    const trackedId = flow.kind === 'tracking' || flow.kind === 'awaiting_wallet' ? flow.intent.transferId : null;
    return store
      .list()
      .filter(
        (i): i is BaseToIcpIntent =>
          i.kind === 'base_to_icp' && i.status !== 'done' && i.transferId !== trackedId && intentDestination(i) === destination.id,
      );
  });

  // Work out what each saved transfer needs (track, resume, mismatch, wallet) by asking Base.
  $effect(() => {
    const items = pending;
    const activeProvider = provider;
    const account = wallet.account;
    const chainId = wallet.chainId;
    const walletAvailable = wallet.status !== 'unavailable';
    let cancelled = false;
    void (async () => {
      const resolved: RecoveryItem[] = [];
      for (const intent of items) {
        let used: boolean | null = null;
        if (!intent.txHash && activeProvider) {
          const check = await checkIntentOnChain(activeProvider, intent);
          used = check === 'used' ? true : check === 'unused' ? false : null;
        }
        resolved.push({ intent, recovery: classifyUnsentIntent(intent, { walletAvailable, account, chainId, used }) });
      }
      if (!cancelled) recoveries = resolved;
    })();
    return () => {
      cancelled = true;
    };
  });

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

  // Follow the transfer: the wallet receipt is a hint, the canister's release record is the authority.
  const trackedTransferId = $derived(flow.kind === 'tracking' ? flow.intent.transferId : null);
  $effect(() => {
    const id = trackedTransferId;
    if (id === null || !store) return;
    const controller = new AbortController();
    const intentStore = store;
    void (async () => {
      const tracked = untrack(() => (flow.kind === 'tracking' ? flow.intent : null));
      if (!tracked) return;
      try {
        // Prepare the signing actors now so the optional "Tell the bridge" click can sign immediately.
        try {
          await untrack(() => ensureReady());
        } catch {
          // Notifying stays unavailable until the ICP wallet reconnects; tracking continues.
        }
        const reader = untrack(() => anonymous) ?? (await getAnonymousBridge());
        if (controller.signal.aborted) return;
        await trackBaseToIcp(
          {
            provider: untrack(() => getProvider()),
            reads: { getRelease: async (transferId) => (await reader.get_release(transferId))[0] ?? null },
            store: intentStore,
          },
          tracked,
          (tracking) => dispatch({ type: 'tracking_update', tracking }),
          controller.signal,
        );
        if (!controller.signal.aborted) touched();
      } catch {
        // Tracking restarts from the saved intent on the next visit.
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
   * to read again. The flow itself refuses to ask the wallet anything meanwhile.
   */
  function recheckIfUnavailable() {
    const fresh = readFreshDestination ? readFreshDestination() : destination;
    if (!fresh || !isEnabled(fresh)) onRecheck?.();
  }

  function buildDeps() {
    const activeProvider = getProvider();
    if (!activeProvider || !wallet.account || !principal || !store || !anonymous || !sessionGuard) return null;
    return withSigning(
      buildBaseToIcpDeps({
        provider: activeProvider,
        config,
        account: wallet.account,
        principal,
        store,
        signers,
        anonymous,
        isCurrent: sessionGuard,
      }),
      signingContext(),
    );
  }

  /** Anonymous reader plus (when the ICP wallet allows) the signing actors, bound to the current session. */
  async function prepareReaders(): Promise<boolean> {
    try {
      return await ensureReady();
    } catch {
      // Sending needs only the anonymous reader; signing actors matter only for the optional notify step.
      const guard = captureSessionGuard();
      try {
        const reader = anonymous ?? (await getAnonymousBridge());
        if (!guard()) return false;
        anonymous = reader;
        sessionGuard = guard;
        return true;
      } catch {
        return false;
      }
    }
  }

  async function startReview() {
    if (!evaluation.canSend || evaluation.amountE8s === null) return;
    const amount = evaluation.amountE8s;
    if (!(await prepareReaders())) {
      dispatch({ type: 'blocked', message: 'Could not reach the bridge canister to prepare this transfer. Try again in a moment.' });
      return;
    }
    dispatch({ type: 'confirm', amountE8s: amount });
  }

  /** Click handler for the final send. */
  function send() {
    if (flow.kind !== 'confirming') return;
    recheckIfUnavailable();
    const deps = buildDeps();
    if (!deps) {
      dispatch({ type: 'aborted', reason: 'check_failed', message: 'Your wallets are not ready. Review the transfer again.' });
      return;
    }
    void sendBridgeOut(deps, { amountE8s: flow.amountE8s }, dispatch).then(touched);
  }

  async function resume(item: RecoveryItem) {
    if (flow.kind === 'awaiting_wallet') return;
    recheckIfUnavailable();
    if (!(await prepareReaders())) {
      recoveryMessage = 'Could not reach the bridge canister. Try again in a moment.';
      return;
    }
    const deps = buildDeps();
    if (!deps) return;
    dispatch({ type: 'confirm', amountE8s: item.intent.amountE8s, resume: true });
    void sendBridgeOut(deps, { amountE8s: item.intent.amountE8s, resume: item.intent }, dispatch).then(touched);
  }

  function track(item: RecoveryItem) {
    if (flow.kind === 'awaiting_wallet') return;
    dispatch({ type: 'track', intent: item.intent });
  }

  async function discard(item: RecoveryItem) {
    if (!store) return;
    recoveryMessage = '';
    const result = await discardUnsentIntent({ store, provider: getProvider() }, item.intent);
    if (result === 'discarded') {
      recoveryMessage = 'The unsent transfer was removed from this device.';
      dispatch({ type: 'dismiss' });
    } else if (result === 'sent_on_chain') {
      recoveryMessage = 'That transfer exists on Base, so it was kept. Track it instead.';
    } else {
      recoveryMessage = `Connect your Base wallet on ${config.chain.name} so the transfer can be checked before it is removed.`;
    }
    touched();
  }

  async function notify() {
    if (flow.kind !== 'tracking' || !signers || notifyBusy) return;
    const deps = buildDeps();
    if (!deps) return;
    notifyBusy = true;
    notifyMessage = '';
    // notifyBridge issues the signer call before any await.
    const result = await notifyBridge(deps, flow.intent);
    notifyBusy = false;
    if (result.ok) {
      if (flow.kind === 'tracking') flow = { ...flow, intent: result.intent };
      notifyMessage = 'The bridge was told about this transaction.';
    } else {
      notifyMessage = result.message;
    }
    touched();
  }

  function backToForm() {
    flow = INITIAL_BASE_TO_ICP_STATE;
    notifyMessage = '';
  }

  function recoveryText(item: RecoveryItem): string {
    const r = item.recovery;
    switch (r.action) {
      case 'track':
        return 'This transfer is on Base. The bridge settles it after finality.';
      case 'resume':
        return 'This transfer was prepared but never sent. It can be sent now with the same request.';
      case 'account_mismatch':
        return `This unsent transfer was prepared by ${r.expected}. It cannot be sent from the connected account because the transfer id depends on the sender. Connect that account to resume it, or discard it.`;
      case 'needs_wallet':
        return r.reason === 'wrong_network'
          ? `Switch your wallet to ${config.chain.name} so this transfer can be checked on Base.`
          : r.reason === 'not_connected'
            ? 'Connect your Base wallet so this transfer can be checked on Base.'
            : 'No Base wallet was found, so this transfer cannot be checked.';
      default:
        return 'Could not read this transfer from Base. Check your wallet connection.';
    }
  }

  const fmt = (value: bigint) => formatE8s(value);
</script>

<section class="flow" aria-label="Bridge icUSD from Base to the Internet Computer">
  <p class="intro">
    Burn icUSD on Base and receive the same amount, minus fees, as icUSD on the Internet Computer. The bridge releases it to the ICP account you are signed in with
    after Base L1 batch finality, about 20 minutes.
  </p>

  {#if config.halted}
    <p class="banner bad" role="alert">The bridge is halted for safety: {config.halted}</p>
  {:else if config.releasesPaused}
    <p class="banner warn" role="status">Releases to the Internet Computer are paused right now. A transfer sent now would wait until releases resume.</p>
  {/if}

  {#if recoveries.length > 0}
    <div class="banner warn" role="status">
      <strong>Saved transfers on this device</strong>
      {#each recoveries as item (item.intent.transferId)}
        <div class="recovery">
          <div class="recovery-head">
            <span>{fmt(item.intent.amountE8s)} icUSD · <code title={item.intent.transferId}>{shortHash(item.intent.transferId)}</code></span>
          </div>
          <p>{recoveryText(item)}</p>
          <div class="row">
            {#if item.recovery.action === 'track'}
              <button type="button" class="btn-secondary small" onclick={() => track(item)}>Track</button>
            {:else if item.recovery.action === 'resume'}
              <button type="button" class="btn-primary small" onclick={() => resume(item)} disabled={flow.kind === 'awaiting_wallet'}>Resume (same request)</button>
              <button type="button" class="btn-secondary small" onclick={() => discard(item)}>Discard</button>
            {:else if item.recovery.action === 'account_mismatch'}
              <button type="button" class="btn-secondary small" onclick={() => discard(item)}>Discard unsent transfer</button>
            {:else if item.recovery.action === 'needs_wallet' && item.recovery.reason === 'wrong_network'}
              <button type="button" class="btn-secondary small" onclick={onSwitch}>Switch to {config.chain.name}</button>
            {:else if item.recovery.action === 'needs_wallet' && item.recovery.reason === 'not_connected'}
              <button type="button" class="btn-secondary small" onclick={onConnect}>Connect Base wallet</button>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}
  {#if recoveryMessage}<p class="banner" role="status">{recoveryMessage}</p>{/if}

  {#if flow.kind === 'tracking'}
    <BaseTrackingCard
      intent={flow.intent}
      tracking={flow.tracking}
      chain={config.chain}
      onNotify={notify}
      {notifyBusy}
      {notifyMessage}
    />
    {#if flow.tracking.terminal}
      <button type="button" class="btn-secondary wide" onclick={backToForm}>Start another transfer</button>
    {/if}
  {:else}
    <div class="field">
      <label for="base-amount">Amount to bridge (icUSD on {config.chain.name})</label>
      <input id="base-amount" type="text" inputmode="decimal" autocomplete="off" placeholder="0.00" bind:value={amountText} disabled={locked} />
      <p class="small-note">
        {balanceLine}Minimum {fmt(config.minBridgeOutE8s)} icUSD
      </p>
    </div>

    <div class="destination">
      <span class="kicker">ICP DESTINATION</span>
      {#if signedIn}
        <code>{principalText}</code>
        <span class="small-note">Default subaccount. Fixed to the account you are signed in with.</span>
      {:else}
        <span class="small-note">Sign in with your ICP wallet in the page header. The bridge releases icUSD to that account.</span>
      {/if}
    </div>

    {#if flow.kind === 'draft' || flow.kind === 'rejected' || flow.kind === 'aborted' || flow.kind === 'blocked' || flow.kind === 'failed'}
      {#if evaluation.state === 'ready'}
        {#each evaluation.warnings as warning (warning)}<p class="small-note">{warning}</p>{/each}
      {:else if amountText.trim() !== '' || evaluation.state === 'wrong_network' || evaluation.state === 'not_connected' || evaluation.state === 'no_wallet' || evaluation.state === 'icp_signed_out'}
        <p class="banner" class:warn={evaluation.state === 'insufficient_gas' || evaluation.state === 'wrong_network'} role="status" data-state={evaluation.state}>{evaluation.message}</p>
      {/if}
      {#if evaluation.state === 'wrong_network'}
        <button type="button" class="btn-primary wide" onclick={onSwitch}>Switch to {config.chain.name}</button>
      {:else if evaluation.state === 'not_connected'}
        <button type="button" class="btn-primary wide" onclick={onConnect}>Connect Base wallet</button>
      {:else if !(flow.kind === 'failed' && flow.uncertain)}
        <button type="button" class="btn-primary wide" onclick={startReview} disabled={!evaluation.canSend}>Review transfer</button>
      {/if}
    {/if}

    {#if flow.kind === 'rejected'}
      <p class="banner" role="status">You declined the request in your wallet. Nothing was sent.</p>
    {/if}
    {#if flow.kind === 'aborted'}
      <p class="banner warn" role="alert">{flow.message} Nothing was sent.</p>
    {/if}
    {#if flow.kind === 'blocked'}
      <p class="banner bad" role="alert">{flow.message} Nothing was sent.</p>
    {/if}
    {#if flow.kind === 'failed'}
      <p class="banner bad" role="alert">{flow.message}</p>
      {#if flow.uncertain}
        <button type="button" class="btn-secondary wide" onclick={() => dispatch({ type: 'dismiss' })}>Check saved transfers above</button>
      {/if}
    {/if}

    {#if flow.kind === 'confirming'}
      <div class="review" aria-label="Confirm Base to ICP transfer">
        <h3>Confirm</h3>
        <p class="destination-line">
          <strong>icUSD will be released to</strong> <code>{principalText}</code>
          <span class="small-note">on the Internet Computer, default subaccount.</span>
        </p>
        <dl>
          <div><dt>You burn on {config.chain.name}</dt><dd>{fmt(flow.amountE8s)} icUSD</dd></div>
          <div><dt>From Base account</dt><dd><code>{wallet.account ? displayAddress(wallet.account) : ''}</code></dd></div>
          <div><dt>Bridge release fee (deducted)</dt><dd>{fmt(config.releaseFeeE8s)} icUSD</dd></div>
          <div><dt>icUSD ledger fee (deducted)</dt><dd>{config.ledgerFeeE8s !== null ? `${fmt(config.ledgerFeeE8s)} icUSD` : 'charged by the ledger'}</dd></div>
          {#if estimatedReceive !== null}
            <div class="total"><dt>You receive on the Internet Computer</dt><dd>{config.ledgerFeeE8s !== null ? '' : 'about '}{fmt(estimatedReceive)} icUSD</dd></div>
          {/if}
          {#if gasLine}
            <div><dt>Base network gas (paid in ETH, estimate)</dt><dd>{gasLine}</dd></div>
          {:else}
            <div><dt>Base network gas</dt><dd>paid in ETH from your wallet</dd></div>
          {/if}
        </dl>
        <p class="small-note">
          Your wallet will ask you to confirm one transaction that burns the tokens on Base. The release on the Internet Computer happens only after Base L1 batch finality (about 20 minutes) and cannot be undone. Wallet receipts shown afterwards are hints; the bridge canister decides.
        </p>
      </div>
      <button type="button" class="btn-primary wide" onclick={send}>Burn on Base and bridge</button>
      <button type="button" class="btn-secondary wide" onclick={() => dispatch({ type: 'back' })}>Back</button>
    {/if}

    {#if flow.kind === 'awaiting_wallet'}
      <p class="banner" role="status">Confirm the transaction in your Base wallet. The transfer is saved on this device first, so it can be resumed if this page closes.</p>
    {/if}
  {/if}
</section>

<style>
  .flow { display:flex; flex-direction:column; gap:0.9rem; }
  .intro { margin:0; font-size:0.85rem; line-height:1.55; color:var(--rumi-text-secondary); }
  .field { display:flex; flex-direction:column; gap:0.35rem; }
  label { font-size:0.8rem; font-weight:600; color:var(--rumi-text-secondary); }
  input { width:100%; padding:0.7rem 0.8rem; color:var(--rumi-text-primary); background:var(--rumi-bg-primary); border:1px solid var(--rumi-border-hover); border-radius:0.5rem; font:inherit; font-size:1rem; font-variant-numeric:tabular-nums; }
  input:focus-visible { outline:none; border-color:var(--rumi-border-accent); box-shadow:0 0 0 2px var(--rumi-action-dim); }
  input:disabled { opacity:0.6; }
  .small-note { margin:0.2rem 0 0; font-size:0.72rem; line-height:1.5; color:var(--rumi-text-muted); }
  .kicker { font-size:0.65rem; letter-spacing:0.09em; color:var(--rumi-text-muted); font-weight:600; }
  .destination { display:flex; flex-direction:column; gap:0.25rem; padding:0.6rem 0.8rem; border:1px solid var(--rumi-border); border-radius:0.5rem; background:rgba(255,255,255,0.015); }
  .destination code, .destination-line code { overflow-wrap:anywhere; font-size:0.72rem; color:var(--rumi-text-secondary); }
  .destination-line { margin:0 0 0.6rem; font-size:0.85rem; display:flex; flex-direction:column; gap:0.2rem; color:var(--rumi-text-primary); }
  .banner { margin:0; padding:0.65rem 0.8rem; border:1px solid var(--rumi-border); border-radius:0.5rem; background:var(--rumi-bg-surface2); font-size:0.78rem; line-height:1.5; color:var(--rumi-text-secondary); }
  .banner p { margin:0.35rem 0 0; }
  .banner.warn { border-color:rgba(229,168,90,0.4); color:#ffd9a8; }
  .banner.bad { border-color:rgba(224,107,159,0.4); background:rgba(224,107,159,0.08); color:#ffb3c5; }
  .recovery { margin-top:0.6rem; padding-top:0.5rem; border-top:1px solid var(--rumi-border); }
  .recovery-head { font-size:0.8rem; color:var(--rumi-text-primary); }
  .row { display:flex; flex-wrap:wrap; gap:0.5rem; margin-top:0.5rem; }
  .wide { width:100%; }
  .small { padding:0.4rem 0.8rem; font-size:0.75rem; }
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
