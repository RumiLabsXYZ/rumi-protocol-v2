<script lang="ts">
  import { onDestroy } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import { isConnected as isConnectedStore, principal as principalStore } from '$lib/stores/wallet';
  import { CANISTER_IDS } from '$lib/config';
  import { getPublicMinterActor, redeemDogeWithApproval, updateDogeBalanceForOwner } from '$lib/services/ckdogeMinterActors';
  import { fetchLedgerFee, getCachedLedgerFee } from '$lib/services/ledgerFeeService';
  import { ICRC1_IDL as ckdogeLedgerIdl } from '$lib/idls/ledger.idl.js';
  import {
    KOINU_DECIMALS,
    POLL_INTERVAL_MS,
    betaRiskNotice,
    buildAccountArgs,
    buildApproveArgs,
    buildRetrieveWithApprovalArgs,
    classifyRetrieveDogeStatus,
    classifyUtxoStatus,
    computeApprovalAmount,
    computeConfirmationDisplay,
    computeMintStepIndex,
    confirmationMeterPercent,
    disconnectedWalletCopy,
    formatKoinuAsDoge,
    isPlausibleDogecoinAddress,
    isPollingExhausted,
    isRetryableUpdateBalanceError,
    isTerminalUtxoKind,
    parseDogeAmountInput,
    summarizeMinterInfo,
    summarizeUpdateBalanceError,
    summarizeWithdrawalFeeEstimate,
    type MinterInfoSummary,
    type UpdateBalanceErrorSummary,
    type UtxoStatusSummary,
    type RetrieveStatusSummary,
  } from '$lib/utils/dogeBorrowFlow';

  const CKDOGE_LEDGER_FEE_REF = { ledgerId: CANISTER_IDS.CKDOGE_LEDGER, decimals: KOINU_DECIMALS, symbol: 'ckDOGE' };

  let isConnected = false;
  let ownerPrincipal: Principal | null = null;

  function principalKey(p: Principal | null): string | null {
    return p ? p.toText() : null;
  }

  let ckDogeLogoFailed = false;
  function handleCkDogeLogoError() {
    ckDogeLogoFailed = true;
  }

  // ── Tab state ─────────────────────────────────────────────────────────
  let activeTab: 'mint' | 'redeem' = 'mint';
  function handleTabKeydown(event: KeyboardEvent) {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault();
    activeTab = activeTab === 'mint' ? 'redeem' : 'mint';
    const nextId = activeTab === 'mint' ? 'doge-tab-mint' : 'doge-tab-redeem';
    queueMicrotask(() => document.getElementById(nextId)?.focus());
  }

  // ── Deposit / address state ──────────────────────────────────────────
  let depositAddress: string | null = null;
  let addressLoading = false;
  let addressError = '';
  let minterInfoSummary: MinterInfoSummary | null = null;
  let minterInfoLoading = false;
  let minterInfoError = false;

  // ── Confirmation polling state ───────────────────────────────────────
  let isPolling = false;
  let pollAttempt = 0;
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let utxoStatuses: UtxoStatusSummary[] = [];
  let mintedSummary: UtxoStatusSummary | null = null;
  let lastUpdateBalanceError: UpdateBalanceErrorSummary | null = null;
  let pollingStopped = false;
  let pollFatalMessage = '';
  let destroyed = false;
  // Snapshotted at the "i sent the DOGE" click — every poll call binds to this,
  // never to whatever wallet happens to be connected when the timer fires.
  let pollingPrincipal: Principal | null = null;

  $: mintStepIndex = computeMintStepIndex({ isPolling, pollingStopped, hasMinted: !!mintedSummary });

  $: confirmation = computeConfirmationDisplay({
    isPolling,
    pollingStopped,
    pollAttempt,
    mintedSummary,
    pollFatalMessage,
    lastUpdateBalanceError,
    utxoStatuses,
    minConfirmationsHint: minterInfoSummary?.minConfirmationsCount,
  });

  // ── Copy-address state ───────────────────────────────────────────────
  let addressCopied = false;
  let addressCopyError = false;
  let addressCopyTimer: ReturnType<typeof setTimeout> | null = null;
  async function copyDepositAddress() {
    if (!depositAddress) return;
    if (addressCopyTimer !== null) clearTimeout(addressCopyTimer);
    try {
      await navigator.clipboard.writeText(depositAddress);
      addressCopied = true;
      addressCopyError = false;
    } catch {
      addressCopied = false;
      addressCopyError = true;
    }
    addressCopyTimer = setTimeout(() => {
      addressCopied = false;
      addressCopyError = false;
    }, 2000);
  }

  // ── Redemption state ─────────────────────────────────────────────────
  let redeemAddress = '';
  let redeemDogeAmount = '';
  let redeemAddressError = '';
  let redeemAmountError = '';
  let withdrawalFeeSummary = '';
  let redeemBusy = false;
  let redeemError = '';
  let approveBlockIndex: bigint | null = null;
  let burnBlockIndex: bigint | null = null;
  let retrieveStatusSummary: RetrieveStatusSummary | null = null;
  let retrieveStatusLoading = false;

  // Any change to isConnected/ownerPrincipal (including disconnect) must wipe every
  // piece of client-specific UI state so a new/different wallet never sees stale data.
  function resetClientState() {
    depositAddress = null;
    addressLoading = false;
    addressError = '';
    minterInfoSummary = null;
    minterInfoLoading = false;
    minterInfoError = false;

    stopPolling();
    pollAttempt = 0;
    utxoStatuses = [];
    mintedSummary = null;
    lastUpdateBalanceError = null;
    pollingStopped = false;
    pollFatalMessage = '';
    pollingPrincipal = null;

    addressCopied = false;
    addressCopyError = false;

    redeemAddress = '';
    redeemDogeAmount = '';
    redeemAddressError = '';
    redeemAmountError = '';
    withdrawalFeeSummary = '';
    redeemBusy = false;
    redeemError = '';
    approveBlockIndex = null;
    burnBlockIndex = null;
    retrieveStatusSummary = null;
    retrieveStatusLoading = false;
  }

  let lastPrincipalKey: string | null = null;
  const unsubConnected = isConnectedStore.subscribe((v) => {
    isConnected = v;
    if (!v) resetClientState();
  });
  const unsubPrincipal = principalStore.subscribe((v) => {
    const key = principalKey(v);
    ownerPrincipal = v;
    if (key !== lastPrincipalKey) {
      lastPrincipalKey = key;
      resetClientState();
    }
  });

  // Auto-fetch the deposit address the moment a non-anonymous principal is
  // connected — on initial connect, on wallet restore after reload, and again
  // for whatever new principal a switch resolves to (resetClientState above
  // clears depositAddress/addressError on every principal change, which is
  // what lets this re-fire for the new wallet). Never retries on its own
  // after a failure — addressError blocks it, and the Retry button in the
  // template is the only way to try again. get_doge_address is a public,
  // anonymous-actor query (see ckdogeMinterActors.ts), so this never opens a
  // wallet prompt.
  $: if (isConnected && ownerPrincipal && !depositAddress && !addressLoading && !addressError) {
    requestDepositAddress();
  }

  // Keep the redeem-tab ledger fee warm from the moment a wallet connects, via
  // the public/anonymous ledger actor (see ledgerFeeService) — never the
  // wallet/Oisy consent actor, which rejects icrc1_fee as an unsupported
  // canister call. Warm-by-connect (rather than only on amount blur) means
  // submitRedeem's click handler can read the fee synchronously from cache
  // instead of awaiting a live query, which would burn the Oisy user-gesture
  // window before the first consent screen opens.
  $: if (isConnected) {
    fetchLedgerFee(CKDOGE_LEDGER_FEE_REF).catch(() => {});
  }

  async function requestDepositAddress() {
    if (!isConnected || !ownerPrincipal || addressLoading) return;
    const sessionKey = principalKey(ownerPrincipal);
    const requestPrincipal = ownerPrincipal;
    const isLive = () => !destroyed && principalKey(ownerPrincipal) === sessionKey;

    addressLoading = true;
    addressError = '';
    try {
      const actor = await getPublicMinterActor();
      if (!isLive()) return;
      const args = buildAccountArgs(requestPrincipal);
      const address: string = await actor.get_doge_address(args);
      if (!isLive()) return;
      depositAddress = address;
      minterInfoLoading = true;
      minterInfoError = false;
      try {
        const info = await actor.get_minter_info();
        if (!isLive()) return;
        minterInfoSummary = summarizeMinterInfo(info);
      } catch {
        if (isLive()) {
          minterInfoSummary = null;
          minterInfoError = true;
        }
      } finally {
        if (isLive()) minterInfoLoading = false;
      }
    } catch (err) {
      if (isLive()) {
        addressError = err instanceof Error ? `Could not fetch your DOGE address: ${err.message}` : 'Could not fetch your DOGE address.';
      }
    } finally {
      if (isLive()) addressLoading = false;
    }
  }

  function stopPolling() {
    isPolling = false;
    if (pollTimer !== null) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
  }

  // Only starts on this explicit "I sent the DOGE" click — never automatically.
  async function beginSentDogeFlow() {
    if (!isConnected || !ownerPrincipal || !depositAddress || isPolling) return;
    stopPolling();
    pollAttempt = 0;
    pollingStopped = false;
    pollFatalMessage = '';
    mintedSummary = null;
    lastUpdateBalanceError = null;
    utxoStatuses = [];
    pollingPrincipal = ownerPrincipal;
    isPolling = true;
    await runUpdateBalanceCycle();
  }

  // True only if this exact poll session (component alive, polling flag on, connected
  // principal unchanged) is still the one that kicked off the in-flight call.
  function isPollSessionLive(sessionPrincipal: Principal): boolean {
    return (
      !destroyed &&
      isPolling &&
      pollingPrincipal !== null &&
      pollingPrincipal.toText() === sessionPrincipal.toText() &&
      !!ownerPrincipal &&
      ownerPrincipal.toText() === sessionPrincipal.toText()
    );
  }

  async function runUpdateBalanceCycle() {
    if (destroyed || !isPolling || !pollingPrincipal) return;
    if (!ownerPrincipal || ownerPrincipal.toText() !== pollingPrincipal.toText()) {
      pollFatalMessage = 'Wallet changed mid-check. Stopped watching the original address, reconnect that wallet to resume.';
      stopPolling();
      pollingStopped = true;
      return;
    }
    const sessionPrincipal = pollingPrincipal;
    pollAttempt += 1;

    try {
      const result = await updateDogeBalanceForOwner(sessionPrincipal, () => isPollSessionLive(sessionPrincipal));
      if (!isPollSessionLive(sessionPrincipal)) return;

      if ('Ok' in result) {
        utxoStatuses = (result.Ok as Array<Record<string, any>>).map(classifyUtxoStatus);
        lastUpdateBalanceError = null;
        const minted = utxoStatuses.find((s) => s.kind === 'Minted');
        if (minted) {
          mintedSummary = minted;
          stopPolling();
          pollingStopped = true;
          return;
        }
        const terminalNonMint = utxoStatuses.find((s) => isTerminalUtxoKind(s.kind));
        if (terminalNonMint) {
          pollFatalMessage = terminalNonMint.label;
          stopPolling();
          pollingStopped = true;
          return;
        }
      } else if ('Err' in result) {
        const summary = summarizeUpdateBalanceError(result.Err);
        lastUpdateBalanceError = summary;
        if (!isRetryableUpdateBalanceError(summary.kind)) {
          pollFatalMessage = summary.message;
          stopPolling();
          pollingStopped = true;
          return;
        }
      }
    } catch (err) {
      if (!isPollSessionLive(sessionPrincipal)) return;
      pollFatalMessage = err instanceof Error ? `Error checking your balance: ${err.message}` : 'Error checking your balance.';
      stopPolling();
      pollingStopped = true;
      return;
    }

    if (!isPollSessionLive(sessionPrincipal)) return;

    if (isPollingExhausted(pollAttempt)) {
      stopPolling();
      pollingStopped = true;
      return;
    }

    if (isPolling && !destroyed) {
      pollTimer = setTimeout(runUpdateBalanceCycle, POLL_INTERVAL_MS);
    }
  }

  // Explicit recheck after the bounded poll expires — never resumes automatically,
  // and always binds to whoever is connected NOW (a fresh session), never the stale pollingPrincipal.
  function recheckAfterTimeout() {
    if (!isConnected || !ownerPrincipal || isPolling) return;
    pollingPrincipal = ownerPrincipal;
    pollAttempt = 0;
    pollingStopped = false;
    pollFatalMessage = '';
    isPolling = true;
    runUpdateBalanceCycle();
  }

  function validateRedeemForm(): boolean {
    redeemAddressError = isPlausibleDogecoinAddress(redeemAddress)
      ? ''
      : 'That does not look like a valid Dogecoin address. Double check it.';
    const parsed = parseDogeAmountInput(redeemDogeAmount);
    redeemAmountError = parsed === null ? 'Enter a valid ckDOGE amount (up to 8 decimal places).' : '';
    return !redeemAddressError && !redeemAmountError;
  }

  async function loadWithdrawalEstimate() {
    const parsed = parseDogeAmountInput(redeemDogeAmount);
    if (parsed === null || !isConnected) {
      withdrawalFeeSummary = '';
      return;
    }

    const capturedPrincipalKey = principalKey(ownerPrincipal);
    const capturedRaw = redeemDogeAmount;
    const isLive = () =>
      !destroyed &&
      isConnected &&
      principalKey(ownerPrincipal) === capturedPrincipalKey &&
      redeemDogeAmount === capturedRaw;

    try {
      const minterActor = await getPublicMinterActor();
      const [feeResult, ledgerFee] = await Promise.all([
        minterActor.estimate_withdrawal_fee({ amount: [parsed] }),
        fetchLedgerFee(CKDOGE_LEDGER_FEE_REF),
      ]);
      if (!isLive()) return;
      const outcome = summarizeWithdrawalFeeEstimate(feeResult);
      if (outcome.success) {
        const ledgerFeeStr = formatKoinuAsDoge(ledgerFee);
        withdrawalFeeSummary = `${outcome.label} + ${ledgerFeeStr} ledger fee. The approval step also charges ${ledgerFeeStr} ledger fee separately from the withdrawal transfer fee above, so two ledger fees total.`;
      } else {
        withdrawalFeeSummary = outcome.label;
      }
    } catch {
      if (isLive()) withdrawalFeeSummary = '';
    }
  }

  // The ledger fee is read synchronously from the warm cache (kept warm on connect
  // and on amount blur, both via the public/anonymous ledger actor — never a live
  // icrc1_fee() query here, and never through the Oisy consent actor, which rejects
  // it as an unsupported canister call). The approval amount is exactly requested +
  // ledger fee, never more. Actor routing (Oisy signer vs standard wallet actor) and
  // the approve-then-retrieve sequencing live in redeemDogeWithApproval so the
  // click-to-popup gesture chain stays testable outside a component.
  async function submitRedeem() {
    if (redeemBusy) return;
    if (!validateRedeemForm()) return;
    if (!isConnected || !ownerPrincipal) return;

    const sessionKey = principalKey(ownerPrincipal);
    const sessionPrincipal = ownerPrincipal;
    const isLive = () => !destroyed && principalKey(ownerPrincipal) === sessionKey;

    redeemBusy = true;
    redeemError = '';
    approveBlockIndex = null;
    burnBlockIndex = null;
    retrieveStatusSummary = null;

    try {
      const requestedKoinu = parseDogeAmountInput(redeemDogeAmount)!;
      const ledgerFee = getCachedLedgerFee(CKDOGE_LEDGER_FEE_REF);
      const approvalAmount = computeApprovalAmount(requestedKoinu, ledgerFee);
      const approveArgs = buildApproveArgs(Principal.fromText(CANISTER_IDS.CKDOGE_MINTER), approvalAmount);
      const retrieveArgs = buildRetrieveWithApprovalArgs(redeemAddress, requestedKoinu);

      const outcome = await redeemDogeWithApproval({
        ownerPrincipal: sessionPrincipal,
        ledgerCanisterId: CANISTER_IDS.CKDOGE_LEDGER,
        minterCanisterId: CANISTER_IDS.CKDOGE_MINTER,
        ledgerIdl: ckdogeLedgerIdl,
        approveArgs,
        retrieveArgs,
        isLive,
      });

      if (!isLive()) return;
      switch (outcome.kind) {
        case 'stale':
          return;
        case 'approve-error':
          redeemError = `Approval failed: ${outcome.message}`;
          return;
        case 'retrieve-error':
          approveBlockIndex = outcome.approveBlockIndex;
          redeemError = outcome.message;
          return;
        case 'success':
          approveBlockIndex = outcome.approveBlockIndex;
          burnBlockIndex = outcome.burnBlockIndex;
          return;
      }
    } catch (err) {
      if (isLive()) {
        redeemError = err instanceof Error ? `Redemption failed: ${err.message}` : 'Redemption failed.';
      }
    } finally {
      if (isLive()) redeemBusy = false;
    }
  }

  // Manual refresh only — no background tracking of the retrieval.
  async function refreshRetrieveStatus() {
    if (burnBlockIndex === null || retrieveStatusLoading || !ownerPrincipal) return;
    const sessionKey = principalKey(ownerPrincipal);
    const isLive = () => !destroyed && principalKey(ownerPrincipal) === sessionKey;
    const blockIndex = burnBlockIndex;

    retrieveStatusLoading = true;
    try {
      const minterActor = await getPublicMinterActor();
      if (!isLive()) return;
      const status = await minterActor.retrieve_doge_status({ block_index: blockIndex });
      if (!isLive()) return;
      retrieveStatusSummary = classifyRetrieveDogeStatus(status);
    } catch (err) {
      if (isLive()) {
        redeemError = err instanceof Error ? `Error checking withdrawal status: ${err.message}` : 'Error checking withdrawal status.';
      }
    } finally {
      if (isLive()) retrieveStatusLoading = false;
    }
  }

  onDestroy(() => {
    destroyed = true;
    stopPolling();
    if (addressCopyTimer !== null) clearTimeout(addressCopyTimer);
    unsubConnected();
    unsubPrincipal();
  });
</script>

<div class="doge-page">
  <div class="doge-hero">
    <div class="doge-hero-logo">
      {#if !ckDogeLogoFailed}
        <img src="/ckdoge-logo.svg" alt="ckDOGE" on:error={handleCkDogeLogoError} />
      {:else}
        <span class="doge-logo-fallback" aria-hidden="true">ckÐ</span>
      {/if}
    </div>
    <h1>ckDOGE Bridge</h1>
    <p class="doge-subtitle">Move DOGE onto the Internet Computer and back.</p>
  </div>

  <div class="doge-tabs" role="tablist" aria-label="ckDOGE bridge direction">
    <button
      role="tab"
      id="doge-tab-mint"
      type="button"
      aria-selected={activeTab === 'mint'}
      aria-controls="doge-panel-mint"
      tabindex={activeTab === 'mint' ? 0 : -1}
      class="doge-tab"
      class:doge-tab--active={activeTab === 'mint'}
      on:click={() => (activeTab = 'mint')}
      on:keydown={handleTabKeydown}
    >
      Mint ckDOGE
    </button>
    <button
      role="tab"
      id="doge-tab-redeem"
      type="button"
      aria-selected={activeTab === 'redeem'}
      aria-controls="doge-panel-redeem"
      tabindex={activeTab === 'redeem' ? 0 : -1}
      class="doge-tab"
      class:doge-tab--active={activeTab === 'redeem'}
      on:click={() => (activeTab = 'redeem')}
      on:keydown={handleTabKeydown}
    >
      Redeem DOGE
    </button>
  </div>

  <div class="doge-stage">
    {#if activeTab === 'mint'}
      <div id="doge-panel-mint" role="tabpanel" aria-labelledby="doge-tab-mint" class="doge-panel" tabindex="-1">
        {#if !isConnected}
          <p class="doge-connect-copy">{disconnectedWalletCopy()}</p>
        {:else}
          <div class="doge-stepper">
            <span class="doge-stepper-caption">DOGE &rarr; ckDOGE</span>
            <div class="doge-steps" role="list">
              <div class="doge-step" role="listitem" class:is-active={mintStepIndex === 1} class:is-done={mintStepIndex > 1}>
                <span class="doge-step-circle">{#if mintStepIndex > 1}&check;{:else}1{/if}</span>
                <span class="doge-step-label">Deposit</span>
              </div>
              <span class="doge-step-line" class:is-done={mintStepIndex > 1}></span>
              <div class="doge-step" role="listitem" class:is-active={mintStepIndex === 2} class:is-done={mintStepIndex > 2}>
                <span class="doge-step-circle">{#if mintStepIndex > 2}&check;{:else}2{/if}</span>
                <span class="doge-step-label">Confirmations</span>
              </div>
              <span class="doge-step-line" class:is-done={mintStepIndex > 2}></span>
              <div class="doge-step" role="listitem" class:is-active={mintStepIndex === 3}>
                <span class="doge-step-circle">3</span>
                <span class="doge-step-label">Minted</span>
              </div>
            </div>
          </div>

          <p class="doge-risk">{betaRiskNotice()}</p>

          {#if !depositAddress}
            <h2>Your deposit address</h2>
            {#if addressLoading}
              <p class="doge-panel-sub" aria-live="polite">Fetching your deposit address…</p>
            {:else if addressError}
              <p class="doge-error" role="alert">{addressError}</p>
              <button class="doge-btn doge-btn--primary" on:click={requestDepositAddress}>Retry</button>
            {/if}
          {:else}
            <h2>Send DOGE to your address</h2>
            <p class="doge-panel-sub">Your ckDOGE will arrive in your connected wallet.</p>

            <div class="doge-row doge-row--annotated">
              <span class="doge-row-label" id="doge-address-label">Your DOGE deposit address</span>
              <div class="doge-addr-field">
                <code class="doge-addr-text" aria-labelledby="doge-address-label">{depositAddress}</code>
                <button
                  type="button"
                  class="doge-copy-btn"
                  on:click={copyDepositAddress}
                  aria-label="Copy address"
                >
                  {addressCopied ? 'Copied' : 'Copy address'}
                </button>
              </div>
              <span class="sr-only" aria-live="polite">
                {#if addressCopied}Address copied to clipboard.{:else if addressCopyError}Could not copy the address automatically. Select and copy it manually.{/if}
              </span>
              <span class="doge-annotation doge-annotation--purple" aria-hidden="true">
                <svg class="doge-annotation-arrow" viewBox="0 0 28 32" preserveAspectRatio="none" aria-hidden="true" focusable="false">
                  <path d="M2,4 C 18,4 18,20 26,26" />
                </svg>
                such address.<br />very yours.
              </span>
            </div>

            <div class="doge-row">
              <span class="doge-row-label">Recipient principal</span>
              <code class="doge-addr-text doge-addr-text--muted">{ownerPrincipal?.toText()}</code>
            </div>

            <div class="doge-row">
              <span class="doge-row-label">Requirements</span>
              <div class="doge-stats-row">
                <div class="doge-stat">
                  <span class="doge-stat-label">Minimum deposit</span>
                  <span class="doge-stat-value">
                    {#if minterInfoSummary}{minterInfoSummary.minDepositValue}{:else if minterInfoLoading}Loading…{:else if minterInfoError}Unavailable{:else}Loading…{/if}
                  </span>
                </div>
                <div class="doge-stat">
                  <span class="doge-stat-label">Required confirmations</span>
                  <span class="doge-stat-value">
                    {#if minterInfoSummary}{minterInfoSummary.minConfirmationsValue}{:else if minterInfoLoading}Loading…{:else if minterInfoError}Unavailable{:else}Loading…{/if}
                  </span>
                </div>
              </div>
            </div>

            <p class="doge-only-send">Only send DOGE to this address.</p>

            {#if !isPolling && !pollingStopped}
              <div class="doge-row doge-row--annotated">
                <button class="doge-btn doge-btn--cta" on:click={beginSentDogeFlow}>I sent the DOGE</button>
                <p class="doge-cta-helper">Starts checking for your deposit.</p>
                <span class="doge-annotation doge-annotation--gold" aria-hidden="true">
                  <svg class="doge-annotation-arrow" viewBox="0 0 28 32" preserveAspectRatio="none" aria-hidden="true" focusable="false">
                    <path d="M26,4 C 10,4 10,20 2,26" />
                  </svg>
                  sent it?<br />tell the dog.
                </span>
              </div>
            {/if}
          {/if}

          {#if isPolling || pollingStopped}
            <div class="doge-poll-status doge-ticket" aria-live="polite">
              <div class="doge-row doge-row--annotated doge-ticket-head">
                <span
                  class="doge-status-pill"
                  class:doge-status-pill--confirming={confirmation.phase === 'confirming'}
                  class:doge-status-pill--stopped={confirmation.phase === 'stopped'}
                  class:doge-status-pill--minted={confirmation.phase === 'minted'}
                  class:doge-status-pill--error={confirmation.phase === 'error'}
                >
                  <span class="doge-status-dot" aria-hidden="true"></span>
                  {confirmation.statusLabel}
                </span>

                {#if confirmation.phase === 'confirming' || confirmation.phase === 'stopped'}
                  <span class="doge-annotation doge-annotation--purple" aria-hidden="true">
                    <svg class="doge-annotation-arrow" viewBox="0 0 28 32" preserveAspectRatio="none" aria-hidden="true" focusable="false">
                      <path d="M2,4 C 18,4 18,20 26,26" />
                    </svg>
                    much patience,<br />very checking
                  </span>
                {/if}

                {#if confirmation.meter}
                  <div
                    class="doge-meter-track"
                    role="progressbar"
                    aria-label="Deposit confirmations"
                    aria-valuenow={confirmation.meter.confirmations}
                    aria-valuemin={0}
                    aria-valuemax={confirmation.meter.requiredConfirmations}
                  >
                    <div class="doge-meter-fill" style="width: {confirmationMeterPercent(confirmation.meter)}%"></div>
                  </div>
                  <p class="doge-meter-label">
                    {confirmation.meter.confirmations} / {confirmation.meter.requiredConfirmations} confirmations
                  </p>
                {:else if confirmation.phase !== 'minted' && confirmation.phase !== 'error'}
                  <p class="doge-panel-sub">No deposit detected on this address yet.</p>
                {/if}
              </div>

              {#if confirmation.utxoCountLabel || confirmation.amountDetectedLabel || confirmation.nextCheckLabel}
                <dl class="doge-ticket-details">
                  {#if confirmation.utxoCountLabel}
                    <div class="doge-ticket-row"><dt>UTXOs</dt><dd>{confirmation.utxoCountLabel}</dd></div>
                  {/if}
                  {#if confirmation.amountDetectedLabel}
                    <div class="doge-ticket-row"><dt>Amount detected</dt><dd>{confirmation.amountDetectedLabel}</dd></div>
                  {/if}
                  {#if confirmation.nextCheckLabel}
                    <div class="doge-ticket-row"><dt>Next check</dt><dd>{confirmation.nextCheckLabel}</dd></div>
                  {/if}
                </dl>
              {/if}

              {#if utxoStatuses.length}
                <div class="doge-ticket-divider" aria-hidden="true"></div>
                {#each utxoStatuses as status}
                  <p class="doge-utxo-line">{status.label}</p>
                {/each}
              {/if}

              {#if lastUpdateBalanceError?.pendingUtxos?.length}
                <div class="doge-ticket-divider" aria-hidden="true"></div>
                {#each lastUpdateBalanceError.pendingUtxos as pending}
                  <p class="doge-utxo-line">{pending.label}</p>
                {/each}
              {/if}

              {#if mintedSummary}
                <div class="doge-ticket-divider" aria-hidden="true"></div>
                <p class="doge-success">{mintedSummary.label} (block {mintedSummary.blockIndex?.toString()})</p>
              {/if}

              {#if pollFatalMessage}
                <div class="doge-ticket-divider" aria-hidden="true"></div>
                <p class="doge-error" role="alert">{pollFatalMessage}</p>
              {/if}

              {#if pollingStopped && !mintedSummary}
                <button class="doge-btn" on:click={recheckAfterTimeout}>Check again</button>
              {/if}
            </div>
          {/if}
        {/if}
      </div>
    {:else}
      <div id="doge-panel-redeem" role="tabpanel" aria-labelledby="doge-tab-redeem" class="doge-panel" tabindex="-1">
        {#if !isConnected}
          <p class="doge-connect-copy">{disconnectedWalletCopy()}</p>
        {:else}
          <h2>Send it back to Dogecoin</h2>
          {#if minterInfoSummary}
            <p class="doge-panel-sub">{minterInfoSummary.minWithdrawalLabel}</p>
          {/if}

          <label class="doge-field" for="doge-redeem-address">
            Dogecoin destination address
            <input id="doge-redeem-address" type="text" bind:value={redeemAddress} placeholder="D..." />
          </label>
          {#if redeemAddressError}<p class="doge-error" role="alert">{redeemAddressError}</p>{/if}

          <label class="doge-field" for="doge-redeem-amount">
            Amount (ckDOGE)
            <input
              id="doge-redeem-amount"
              type="text"
              inputmode="decimal"
              bind:value={redeemDogeAmount}
              on:blur={loadWithdrawalEstimate}
              placeholder="e.g. 50"
            />
          </label>
          {#if redeemAmountError}<p class="doge-error" role="alert">{redeemAmountError}</p>{/if}
          {#if withdrawalFeeSummary}<p class="doge-panel-sub">{withdrawalFeeSummary}</p>{/if}

          <button class="doge-btn doge-btn--cta" on:click={submitRedeem} disabled={redeemBusy}>
            {redeemBusy ? 'Processing…' : 'Send it back to Dogecoin'}
          </button>

          {#if redeemError}<p class="doge-error" role="alert">{redeemError}</p>{/if}
          {#if approveBlockIndex !== null}
            <p class="doge-panel-sub">Approval block index: {approveBlockIndex.toString()}</p>
          {/if}
          {#if burnBlockIndex !== null}
            <p class="doge-success">Burn block index: {burnBlockIndex.toString()}, confirmed on ICP.</p>
            <button class="doge-btn" on:click={refreshRetrieveStatus} disabled={retrieveStatusLoading}>
              {retrieveStatusLoading ? 'Checking…' : 'Refresh withdrawal status'}
            </button>
            {#if retrieveStatusSummary}
              <p class="doge-panel-sub">
                {retrieveStatusSummary.label}
                {#if retrieveStatusSummary.txid}(txid: {retrieveStatusSummary.txid}){/if}
              </p>
            {/if}
          {/if}
        {/if}
      </div>
    {/if}
  </div>

  <p class="doge-footnote">Deposits mint to your connected wallet.</p>
</div>

<style>
  .doge-page {
    position: relative;
    max-width: 700px;
    margin: 0 auto;
    padding: 2rem 1.5rem 3rem;
    font-family: 'Inter', system-ui, -apple-system, sans-serif;
    color: var(--rumi-text-primary);
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }

  /* ── Hero ── */
  .doge-hero {
    text-align: center;
    margin-bottom: 1.5rem;
  }

  .doge-hero-logo {
    display: flex;
    align-items: center;
    justify-content: center;
    margin: 0 auto 0.75rem;
  }

  .doge-hero-logo img {
    width: 72px;
    height: 72px;
    display: block;
  }

  .doge-logo-fallback {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 72px;
    height: 72px;
    border-radius: 50%;
    background: var(--rumi-bg-surface2);
    border: 2px solid var(--rumi-border-hover);
    font-size: 1.5rem;
    font-weight: 700;
    color: var(--rumi-purple-accent);
  }

  .doge-hero h1 {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-weight: 700;
    letter-spacing: -0.02em;
    font-size: 2rem;
    margin: 0.25rem 0;
    color: var(--rumi-text-primary);
  }

  .doge-subtitle {
    font-size: 1rem;
    color: var(--rumi-text-secondary);
    margin: 0;
  }

  /* ── Tabs ── */
  .doge-tabs {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.75rem;
    margin-bottom: 1.25rem;
  }

  .doge-tab {
    font-family: 'Inter', sans-serif;
    font-size: 0.9375rem;
    font-weight: 600;
    padding: 0.75rem 1rem;
    border-radius: 0.625rem;
    border: 1px solid var(--rumi-border);
    background: var(--rumi-bg-surface1);
    color: var(--rumi-text-secondary);
    cursor: pointer;
    transition: border-color 0.15s ease, color 0.15s ease;
  }

  .doge-tab:hover {
    border-color: var(--rumi-border-hover);
    color: var(--rumi-text-primary);
  }

  .doge-tab:focus-visible {
    outline: 2px solid var(--rumi-action);
    outline-offset: 2px;
  }

  .doge-tab--active {
    border-color: var(--rumi-action);
    color: var(--rumi-text-primary);
  }

  /* ── Panel ── */
  .doge-stage {
    position: relative;
  }

  .doge-panel {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.75rem 1.75rem 1.5rem;
  }

  .doge-panel:focus-visible {
    outline: none;
  }

  .doge-panel h2 {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-weight: 600;
    letter-spacing: -0.02em;
    font-size: 1.125rem;
    margin: 0 0 0.375rem;
    color: var(--rumi-text-primary);
  }

  .doge-panel-sub {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    margin: 0 0 1rem;
  }

  .doge-connect-copy {
    font-size: 0.9375rem;
    color: var(--rumi-text-secondary);
    text-align: center;
    margin: 0.5rem 0;
  }

  .doge-risk {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    padding: 0.625rem 0.875rem;
    margin: 0 0 1.25rem;
  }

  /* ── Step tracker ── */
  .doge-stepper {
    margin-bottom: 1.5rem;
    padding-bottom: 1.25rem;
    border-bottom: 1px solid var(--rumi-border);
  }

  .doge-stepper-caption {
    display: block;
    font-size: 0.6875rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--rumi-text-muted);
    margin-bottom: 0.75rem;
  }

  .doge-steps {
    display: flex;
    align-items: flex-start;
  }

  .doge-step {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.375rem;
    flex: none;
    width: 5.5rem;
  }

  .doge-step-line {
    flex: 1 1 auto;
    height: 1px;
    background: var(--rumi-border);
    margin-top: 0.875rem;
  }

  .doge-step-line.is-done {
    background: var(--rumi-action);
  }

  .doge-step-circle {
    width: 1.75rem;
    height: 1.75rem;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 0.8125rem;
    font-weight: 600;
    border: 1px solid var(--rumi-border-hover);
    color: var(--rumi-text-secondary);
    background: var(--rumi-bg-surface2);
  }

  .doge-step.is-active .doge-step-circle {
    border-color: var(--rumi-action);
    color: var(--rumi-action-bright);
  }

  .doge-step.is-done .doge-step-circle {
    background: var(--rumi-action);
    border-color: var(--rumi-action);
    color: var(--rumi-bg-primary);
  }

  .doge-step-label {
    font-size: 0.75rem;
    color: var(--rumi-text-secondary);
  }

  .doge-step.is-active .doge-step-label {
    color: var(--rumi-text-primary);
    font-weight: 600;
  }

  /* ── Rows / address / stats ── */
  .doge-row {
    position: relative;
    margin-bottom: 1.25rem;
  }

  .doge-row-label {
    display: block;
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--rumi-text-muted);
    margin-bottom: 0.5rem;
  }

  .doge-addr-field {
    display: flex;
    align-items: center;
    gap: 0.625rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    padding: 0.625rem 0.75rem;
  }

  .doge-addr-text {
    flex: 1;
    font-family: 'SFMono-Regular', Consolas, 'Liberation Mono', Menlo, monospace;
    font-size: 0.8125rem;
    color: var(--rumi-text-primary);
    word-break: break-all;
    background: transparent;
    padding: 0;
    border: none;
  }

  .doge-addr-text--muted {
    color: var(--rumi-text-secondary);
    font-size: 0.75rem;
  }

  .doge-copy-btn {
    flex: none;
    font-family: 'Inter', sans-serif;
    font-size: 0.8125rem;
    font-weight: 500;
    padding: 0.4375rem 0.75rem;
    border-radius: 0.375rem;
    border: 1px solid var(--rumi-border);
    background: var(--rumi-bg-surface3);
    color: var(--rumi-text-secondary);
    cursor: pointer;
    /* Keeps this control clear of the app shell's fixed bottom mobile nav
       (.mobile-nav, +layout.svelte) when the browser's native focus-scroll
       (keyboard Tab) brings it into view on narrow viewports. */
    scroll-margin-bottom: 72px;
  }

  .doge-copy-btn:hover {
    color: var(--rumi-teal-bright);
    border-color: var(--rumi-border-hover);
  }

  .doge-copy-btn:focus-visible {
    outline: 2px solid var(--rumi-action);
    outline-offset: 2px;
  }

  .doge-stats-row {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1rem;
  }

  .doge-stat {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .doge-stat-label {
    font-size: 0.75rem;
    color: var(--rumi-text-secondary);
  }

  .doge-stat-value {
    font-size: 1.0625rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
  }

  .doge-only-send {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    margin: 0 0 1rem;
  }

  /* ── Buttons ── */
  .doge-btn {
    font-family: 'Inter', sans-serif;
    font-size: 0.9375rem;
    font-weight: 500;
    padding: 0.625rem 1.125rem;
    border-radius: 0.5rem;
    border: 1px solid var(--rumi-border);
    background: var(--rumi-bg-surface2);
    color: var(--rumi-text-primary);
    cursor: pointer;
    margin: 0.25rem 0;
  }

  .doge-btn:hover:not(:disabled) {
    border-color: var(--rumi-border-hover);
  }

  .doge-btn:focus-visible {
    outline: 2px solid var(--rumi-action);
    outline-offset: 2px;
  }

  .doge-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .doge-btn--primary {
    background: var(--rumi-bg-surface2);
    border-color: var(--rumi-border-hover);
  }

  .doge-btn--cta {
    display: block;
    width: 100%;
    background: var(--rumi-action);
    border-color: var(--rumi-action);
    color: var(--rumi-bg-primary);
    font-weight: 600;
    padding: 0.75rem 1.125rem;
  }

  .doge-btn--cta:hover:not(:disabled) {
    background: var(--rumi-action-bright);
    border-color: var(--rumi-action-bright);
  }

  .doge-cta-helper {
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
    margin: 0.375rem 0 0;
  }

  /* ── Status text ── */
  .doge-error {
    color: var(--rumi-danger);
    font-size: 0.8125rem;
    font-weight: 500;
    margin: 0.5rem 0;
  }

  .doge-success {
    color: var(--rumi-teal-bright);
    font-size: 0.8125rem;
    font-weight: 500;
    margin: 0.5rem 0;
  }

  .doge-utxo-line {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    margin: 0.15rem 0;
  }

  .doge-poll-status {
    margin-top: 1rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    padding: 0.875rem 1rem;
  }

  /* ── Confirmation ticket: status pill, meter, ticket-stub rows ── */
  .doge-ticket-head {
    margin-bottom: 0;
  }

  .doge-status-pill {
    display: inline-flex;
    align-items: center;
    gap: 0.4375rem;
    padding: 0.3125rem 0.75rem;
    border-radius: 999px;
    font-size: 0.8125rem;
    font-weight: 600;
    background: var(--rumi-bg-surface3);
    color: var(--rumi-text-secondary);
    border: 1px solid var(--rumi-border);
  }

  .doge-status-dot {
    width: 0.4375rem;
    height: 0.4375rem;
    border-radius: 50%;
    background: currentColor;
    flex: none;
  }

  .doge-status-pill--confirming {
    color: var(--rumi-purple-accent);
    border-color: var(--rumi-purple-accent);
    background: rgba(209, 118, 232, 0.08);
  }

  .doge-status-pill--stopped {
    color: #d9a53c;
    border-color: #d9a53c;
    background: rgba(217, 165, 60, 0.08);
  }

  .doge-status-pill--minted {
    color: var(--rumi-teal-bright);
    border-color: var(--rumi-teal-bright);
    background: rgba(45, 212, 191, 0.08);
  }

  .doge-status-pill--error {
    color: var(--rumi-danger);
    border-color: var(--rumi-danger);
    background: rgba(224, 82, 82, 0.08);
  }

  .doge-meter-track {
    margin-top: 0.75rem;
    height: 0.375rem;
    border-radius: 999px;
    background: var(--rumi-bg-surface3);
    overflow: hidden;
  }

  .doge-meter-fill {
    height: 100%;
    border-radius: 999px;
    background: var(--rumi-teal-bright);
    transition: width 0.3s ease;
  }

  .doge-meter-label {
    font-size: 0.75rem;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-secondary);
    margin: 0.375rem 0 0;
  }

  .doge-ticket-details {
    margin: 0.75rem 0 0;
  }

  .doge-ticket-row {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.75rem;
    font-size: 0.8125rem;
    padding: 0.25rem 0;
  }

  .doge-ticket-row dt {
    color: var(--rumi-text-muted);
  }

  .doge-ticket-row dd {
    margin: 0;
    color: var(--rumi-text-primary);
    font-variant-numeric: tabular-nums;
    text-align: right;
  }

  /* Ticket-stub perforation: a dashed rule between sections, echoing the
     Ticket Stub reference layout. */
  .doge-ticket-divider {
    margin: 0.75rem 0;
    border-top: 1px dashed var(--rumi-border-hover);
  }

  /* ── Redeem form ── */
  .doge-field {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
    margin-bottom: 1rem;
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    font-weight: 500;
  }

  .doge-field input {
    font-family: 'SFMono-Regular', Consolas, 'Liberation Mono', Menlo, monospace;
    font-size: 0.8125rem;
    padding: 0.625rem 0.75rem;
    border-radius: 0.5rem;
    border: 1px solid var(--rumi-border);
    background: var(--rumi-bg-surface2);
    color: var(--rumi-text-primary);
  }

  .doge-field input:focus-visible {
    outline: none;
    border-color: var(--rumi-teal);
  }

  .doge-footnote {
    text-align: center;
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
    margin-top: 1.25rem;
  }

  /* ── Margin annotations ──
     Hand-drawn Comic Sans callouts. Purely decorative (aria-hidden), never
     the primary voice for instructions or safety copy. Gold is a one-off
     accent reserved for these notes, not a design-system token. */
  .doge-annotation {
    display: none;
    font-family: 'Comic Sans MS', 'Comic Sans', 'Chalkboard SE', cursive;
    font-size: 0.875rem;
    line-height: 1.25;
    pointer-events: none;
  }

  .doge-annotation--purple {
    color: var(--rumi-purple-accent);
    opacity: 0.85;
  }

  .doge-annotation--gold {
    color: #d9a53c;
  }

  /* Curved connector line, desktop-margin layout only (see width:1220px block
     below). Narrow/inline layout never shows it — omitted, not just hidden,
     since it has no sensible position once the note becomes an inline chip. */
  .doge-annotation-arrow {
    display: none;
  }

  /* Inline fallback: directly under the target row, all viewports up to the
     wide-desktop breakpoint below, and always on narrow/mobile widths. */
  @media (max-width: 1219px) {
    .doge-annotation {
      display: block;
      margin-top: 0.5rem;
      padding: 0.375rem 0.625rem;
      border-left: 2px solid currentColor;
      border-radius: 0.25rem;
      background: rgba(209, 118, 232, 0.06);
    }

    .doge-annotation--gold {
      background: rgba(217, 165, 60, 0.08);
    }
  }

  /* True margins on generously wide desktop viewports (1280/1440 have ample
     room either side of the 700px panel within the app's 1200px content well). */
  @media (min-width: 1220px) {
    .doge-row--annotated {
      overflow: visible;
    }

    .doge-annotation {
      display: block;
      position: absolute;
      top: 0;
      width: 148px;
    }

    .doge-annotation--purple {
      right: calc(100% + 28px);
      text-align: right;
    }

    .doge-annotation--gold {
      left: calc(100% + 28px);
      text-align: left;
    }

    .doge-annotation-arrow {
      display: block;
      position: absolute;
      top: 2px;
      width: 28px;
      height: 32px;
      overflow: visible;
      pointer-events: none;
    }

    .doge-annotation--purple .doge-annotation-arrow {
      right: -28px;
    }

    .doge-annotation--gold .doge-annotation-arrow {
      left: -28px;
    }

    .doge-annotation-arrow path {
      fill: none;
      stroke: currentColor;
      stroke-width: 1.5;
      stroke-linecap: round;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .doge-tab,
    .doge-btn,
    .doge-copy-btn,
    .doge-meter-fill {
      transition: none;
    }
  }

  @media (max-width: 480px) {
    .doge-page {
      padding: 1.25rem 1rem 2.5rem;
    }

    .doge-stats-row {
      grid-template-columns: 1fr;
      gap: 0.75rem;
    }

    .doge-step {
      width: 4.25rem;
    }

    .doge-step-label {
      font-size: 0.6875rem;
    }
  }
</style>
