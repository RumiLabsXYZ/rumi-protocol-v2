<script lang="ts">
  import { onDestroy } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import { walletStore, isConnected as isConnectedStore, principal as principalStore } from '$lib/stores/wallet';
  import { CANISTER_IDS } from '$lib/config';
  import { getPublicMinterActor, getWalletMinterActor } from '$lib/services/ckdogeMinterActors';
  import { ICRC1_IDL as ckdogeLedgerIdl } from '$lib/idls/ledger.idl.js';
  import {
    POLL_INTERVAL_MS,
    betaRiskNotice,
    buildAccountArgs,
    buildApproveArgs,
    buildRetrieveWithApprovalArgs,
    classifyRetrieveDogeStatus,
    classifyUtxoStatus,
    computeApprovalAmount,
    disconnectedWalletCopy,
    formatKoinuAsDoge,
    isPlausibleDogecoinAddress,
    isPollingExhausted,
    isRetryableUpdateBalanceError,
    isTerminalUtxoKind,
    parseKoinuInput,
    pollProgressLabel,
    summarizeApproveError,
    summarizeMinterInfo,
    summarizeRetrieveError,
    summarizeUpdateBalanceError,
    summarizeWithdrawalFeeEstimate,
    type MinterInfoSummary,
    type UpdateBalanceErrorSummary,
    type UtxoStatusSummary,
    type RetrieveStatusSummary,
  } from '$lib/utils/dogeBorrowFlow';

  const ICP_LOGO_SRC = '/icp-token-dark.svg';

  let isConnected = false;
  let ownerPrincipal: Principal | null = null;

  function principalKey(p: Principal | null): string | null {
    return p ? p.toText() : null;
  }

  let ckDogeLogoFailed = false;
  function handleCkDogeLogoError() {
    ckDogeLogoFailed = true;
  }

  // ── Deposit / address state ──────────────────────────────────────────
  let depositAddress: string | null = null;
  let addressLoading = false;
  let addressError = '';
  let minterInfoSummary: MinterInfoSummary | null = null;

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

  // ── Redemption state ─────────────────────────────────────────────────
  let redeemAddress = '';
  let redeemKoinuRaw = '';
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

    stopPolling();
    pollAttempt = 0;
    utxoStatuses = [];
    mintedSummary = null;
    lastUpdateBalanceError = null;
    pollingStopped = false;
    pollFatalMessage = '';
    pollingPrincipal = null;

    redeemAddress = '';
    redeemKoinuRaw = '';
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

  async function getLedgerActor(): Promise<any> {
    return walletStore.getActor(CANISTER_IDS.CKDOGE_LEDGER, ckdogeLedgerIdl);
  }

  // Address is fetched ONLY on this explicit click — never on route load.
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
      try {
        const info = await actor.get_minter_info();
        if (!isLive()) return;
        minterInfoSummary = summarizeMinterInfo(info);
      } catch {
        if (isLive()) minterInfoSummary = null;
      }
    } catch (err) {
      if (isLive()) {
        addressError = err instanceof Error ? `much fail: ${err.message}` : 'much fail, could not fetch your DOGE address';
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
      pollFatalMessage = 'wow, wallet changed mid-check — stopped watching the original address. reconnect that wallet to resume.';
      stopPolling();
      pollingStopped = true;
      return;
    }
    const sessionPrincipal = pollingPrincipal;
    pollAttempt += 1;

    try {
      const actor = await getPublicMinterActor();
      if (!isPollSessionLive(sessionPrincipal)) return;

      const args = buildAccountArgs(sessionPrincipal);
      const result = await actor.update_balance(args);
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
      pollFatalMessage = err instanceof Error ? `much error checking your balance: ${err.message}` : 'much error checking your balance';
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
      : 'wow, such invalid Dogecoin address — double check it';
    const parsed = parseKoinuInput(redeemKoinuRaw);
    redeemAmountError = parsed === null ? 'much bad number — enter a positive whole number of koinu' : '';
    return !redeemAddressError && !redeemAmountError;
  }

  async function loadWithdrawalEstimate() {
    const parsed = parseKoinuInput(redeemKoinuRaw);
    if (parsed === null || !isConnected) {
      withdrawalFeeSummary = '';
      return;
    }

    const capturedPrincipalKey = principalKey(ownerPrincipal);
    const capturedRaw = redeemKoinuRaw;
    const isLive = () =>
      !destroyed &&
      isConnected &&
      principalKey(ownerPrincipal) === capturedPrincipalKey &&
      redeemKoinuRaw === capturedRaw;

    try {
      const [minterActor, ledgerActor] = await Promise.all([getPublicMinterActor(), getLedgerActor()]);
      const [feeResult, ledgerFee] = await Promise.all([
        minterActor.estimate_withdrawal_fee({ amount: [parsed] }),
        ledgerActor.icrc1_fee(),
      ]);
      if (!isLive()) return;
      const outcome = summarizeWithdrawalFeeEstimate(feeResult);
      withdrawalFeeSummary = outcome.success
        ? `${outcome.label} + ${formatKoinuAsDoge(BigInt(ledgerFee))} ledger fee. much wow: the icrc2_approve step itself also charges ${formatKoinuAsDoge(BigInt(ledgerFee))} ledger fee, separate from the withdrawal transfer fee above — so two ledger fees total, very charge`
        : outcome.label;
    } catch {
      if (isLive()) withdrawalFeeSummary = '';
    }
  }

  async function submitRedeem() {
    if (redeemBusy) return;
    if (!validateRedeemForm()) return;
    if (!isConnected || !ownerPrincipal) return;

    const sessionKey = principalKey(ownerPrincipal);
    const isLive = () => !destroyed && principalKey(ownerPrincipal) === sessionKey;

    redeemBusy = true;
    redeemError = '';
    approveBlockIndex = null;
    burnBlockIndex = null;
    retrieveStatusSummary = null;

    try {
      const requestedKoinu = parseKoinuInput(redeemKoinuRaw)!;
      const ledgerActor = await getLedgerActor();
      if (!isLive()) return;
      const ledgerFee: bigint = BigInt(await ledgerActor.icrc1_fee());
      if (!isLive()) return;
      const approvalAmount = computeApprovalAmount(requestedKoinu, ledgerFee);
      const approveArgs = buildApproveArgs(Principal.fromText(CANISTER_IDS.CKDOGE_MINTER), approvalAmount);

      const approveResult = await ledgerActor.icrc2_approve(approveArgs);
      if (!isLive()) return;
      if ('Err' in approveResult) {
        redeemError = `much approval fail: ${summarizeApproveError(approveResult.Err)}`;
        return;
      }
      approveBlockIndex = BigInt(approveResult.Ok);

      // Session must still belong to the wallet that just approved — otherwise a
      // principal switch mid-flight must not advance to retrieve under a new client.
      if (!isLive()) return;

      const minterActor = await getWalletMinterActor();
      if (!isLive()) return;
      const retrieveArgs = buildRetrieveWithApprovalArgs(redeemAddress, requestedKoinu);
      const retrieveResult = await minterActor.retrieve_doge_with_approval(retrieveArgs);
      if (!isLive()) return;
      if ('Err' in retrieveResult) {
        redeemError = summarizeRetrieveError(retrieveResult.Err);
        return;
      }
      burnBlockIndex = BigInt(retrieveResult.Ok.block_index);
    } catch (err) {
      if (isLive()) {
        redeemError = err instanceof Error ? `much error, very failed redemption: ${err.message}` : 'much error, very failed redemption';
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
        redeemError = err instanceof Error ? `much error checking withdrawal status: ${err.message}` : 'much error checking withdrawal status';
      }
    } finally {
      if (isLive()) retrieveStatusLoading = false;
    }
  }

  onDestroy(() => {
    destroyed = true;
    stopPolling();
    unsubConnected();
    unsubPrincipal();
  });
</script>

<div class="doge-page">
  <div class="doge-paw doge-paw--tl" aria-hidden="true">🐾</div>
  <div class="doge-paw doge-paw--br" aria-hidden="true">🐾</div>

  <div class="doge-hero">
    <div class="doge-coin doge-coin--hero" aria-hidden="true">Ð</div>
    <h1>ckDOGE: much bridge, very Dogecoin</h1>
    <p class="doge-tagline">wow, bring your DOGE onto ICP. such minting, very ckDOGE, much wow.</p>

    <div class="doge-badges">
      <div class="doge-badge">
        {#if !ckDogeLogoFailed}
          <img src="/ckdoge-logo.svg" alt="ckDOGE" on:error={handleCkDogeLogoError} />
        {:else}
          <span class="doge-coin doge-coin--small">ckÐ</span>
        {/if}
      </div>
      <span class="doge-badge-arrow" aria-hidden="true">⇄</span>
      <div class="doge-badge">
        <img src={ICP_LOGO_SRC} alt="ICP" class="icp-sidechain-mark" />
      </div>
    </div>
  </div>

  {#if !isConnected}
    <div class="doge-panel doge-panel--connect">
      <p>{disconnectedWalletCopy()}</p>
      <p class="doge-subtle">Connect your ICP wallet (Plug or Oisy) using the button up top, then come back — much DOGE awaits.</p>
    </div>
  {:else}
    <p class="doge-risk">{betaRiskNotice()}</p>

    <div class="doge-grid">
      <!-- Deposit / mint panel -->
      <section class="doge-panel">
        <h2>much deposit, very DOGE in</h2>

        {#if !depositAddress}
          <button class="doge-button doge-button--gold" on:click={requestDepositAddress} disabled={addressLoading}>
            {addressLoading ? 'wow, fetching...' : 'get my ckDOGE address, such wow'}
          </button>
        {:else}
          <div class="doge-address-box">
            <span class="doge-label">your DOGE deposit address, much unique:</span>
            <code class="doge-address">{depositAddress}</code>
            <span class="doge-label">recipient principal (that's you), wow:</span>
            <code class="doge-address">{ownerPrincipal?.toText()}</code>
            {#if minterInfoSummary}
              <p class="doge-subtle">{minterInfoSummary.minConfirmationsLabel}</p>
              <p class="doge-subtle">{minterInfoSummary.minDepositLabel}</p>
            {/if}
          </div>

          {#if !isPolling && !pollingStopped}
            <button class="doge-button doge-button--gold" on:click={beginSentDogeFlow}>
              i sent the DOGE, such wow
            </button>
          {/if}
        {/if}

        {#if addressError}<p class="doge-error">{addressError}</p>{/if}

        {#if isPolling || pollingStopped}
          <div class="doge-poll-status" aria-live="polite">
            <p class="doge-subtle">{pollProgressLabel(pollAttempt)}</p>

            {#each utxoStatuses as status}
              <p class="doge-utxo-line">{status.label}</p>
            {/each}

            {#if lastUpdateBalanceError}
              <p class="doge-subtle">{lastUpdateBalanceError.message}</p>
              {#if lastUpdateBalanceError.pendingUtxos?.length}
                {#each lastUpdateBalanceError.pendingUtxos as pending}
                  <p class="doge-utxo-line">{pending.label}</p>
                {/each}
              {/if}
            {/if}

            {#if mintedSummary}
              <p class="doge-success">{mintedSummary.label} (block {mintedSummary.blockIndex?.toString()})</p>
            {/if}

            {#if pollFatalMessage}<p class="doge-error">{pollFatalMessage}</p>{/if}

            {#if pollingStopped && !mintedSummary}
              <button class="doge-button" on:click={recheckAfterTimeout}>check again, much patience</button>
            {/if}
          </div>
        {/if}
      </section>

      <!-- Redemption panel -->
      <section class="doge-panel">
        <h2>very redeem, such DOGE out</h2>
        <p class="doge-subtle">1 DOGE = 100,000,000 koinu. much decimals, very precise.</p>
        {#if minterInfoSummary}
          <p class="doge-subtle">{minterInfoSummary.minWithdrawalLabel}</p>
        {/if}

        <label class="doge-field" for="doge-redeem-address">
          your Dogecoin address, much destination
          <input id="doge-redeem-address" type="text" bind:value={redeemAddress} placeholder="D... much wow address" />
        </label>
        {#if redeemAddressError}<p class="doge-error">{redeemAddressError}</p>{/if}

        <label class="doge-field" for="doge-redeem-koinu">
          amount in koinu, much whole number pls
          <input
            id="doge-redeem-koinu"
            type="text"
            inputmode="numeric"
            bind:value={redeemKoinuRaw}
            on:blur={loadWithdrawalEstimate}
            placeholder="e.g. 500000000"
          />
        </label>
        {#if redeemAmountError}<p class="doge-error">{redeemAmountError}</p>{/if}
        {#if withdrawalFeeSummary}<p class="doge-subtle">{withdrawalFeeSummary}</p>{/if}

        <button class="doge-button doge-button--gold" on:click={submitRedeem} disabled={redeemBusy}>
          {redeemBusy ? 'much processing...' : 'send it back to Dogecoin, wow'}
        </button>

        {#if redeemError}<p class="doge-error">{redeemError}</p>{/if}
        {#if approveBlockIndex !== null}
          <p class="doge-subtle">wow, approval block index: {approveBlockIndex.toString()}</p>
        {/if}
        {#if burnBlockIndex !== null}
          <p class="doge-success">burn block index: {burnBlockIndex.toString()}, much confirmed on ICP</p>
          <button class="doge-button" on:click={refreshRetrieveStatus} disabled={retrieveStatusLoading}>
            {retrieveStatusLoading ? 'checking, wow...' : 'refresh withdrawal status, much refresh'}
          </button>
          {#if retrieveStatusSummary}
            <p class="doge-subtle">
              {retrieveStatusSummary.label}
              {#if retrieveStatusSummary.txid}(txid: {retrieveStatusSummary.txid}){/if}
            </p>
          {/if}
        {/if}
      </section>
    </div>
  {/if}
</div>

<style>
  .doge-page {
    font-family: 'Comic Sans MS', 'Comic Sans', cursive;
    position: relative;
    max-width: 960px;
    margin: 0 auto;
    padding: 2rem 1.5rem 3rem;
    color: #4a3200;
    background: radial-gradient(circle at top, #fff6d8 0%, #ffe9a8 45%, #ffd166 100%);
    border-radius: 24px;
    overflow: hidden;
  }

  .doge-paw {
    position: absolute;
    font-size: 2rem;
    opacity: 0.18;
    pointer-events: none;
  }
  .doge-paw--tl { top: 12px; left: 16px; transform: rotate(-15deg); }
  .doge-paw--br { bottom: 12px; right: 16px; transform: rotate(15deg); }

  .doge-hero {
    text-align: center;
    margin-bottom: 1.5rem;
  }

  .doge-hero h1 {
    font-size: 2rem;
    margin: 0.5rem 0;
    color: #7a4b00;
    text-shadow: 2px 2px 0 #fff3c4;
  }

  .doge-tagline {
    font-size: 1.05rem;
    color: #5c3d00;
  }

  .doge-coin {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 76px;
    height: 76px;
    border-radius: 50%;
    background: radial-gradient(circle at 35% 30%, #fff2b0, #f2c14e 55%, #c8891a 100%);
    border: 3px solid #a5690a;
    box-shadow: 0 4px 10px rgba(120, 80, 0, 0.35), inset 0 0 0 4px #ffe9a8;
    font-size: 2.2rem;
    font-weight: bold;
    color: #7a4b00;
  }

  .doge-coin--hero {
    width: 100px;
    height: 100px;
    font-size: 3rem;
    margin-bottom: 0.5rem;
  }

  .doge-coin--small {
    width: 40px;
    height: 40px;
    font-size: 1.1rem;
    border-width: 2px;
  }

  .doge-badges {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 1rem;
    margin-top: 1rem;
  }

  .doge-badge {
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .doge-badge img {
    max-width: 56px;
    max-height: 56px;
  }

  .doge-badge-arrow {
    font-size: 1.4rem;
    color: #a5690a;
  }

  .icp-sidechain-mark {
    border-radius: 50%;
    padding: 6px;
    background: #eef1ff;
    border: 2px dashed #7b8bd6;
  }

  .doge-risk {
    background: #fff3c4;
    border-left: 4px solid #d98c00;
    padding: 0.6rem 0.9rem;
    border-radius: 10px;
    font-size: 0.9rem;
    margin-bottom: 1.25rem;
  }

  .doge-panel {
    background: #fffaf0;
    border: 3px solid #e8b93a;
    border-radius: 18px;
    padding: 1.5rem;
    margin-bottom: 1.5rem;
    box-shadow: 0 6px 0 #e8b93a;
  }

  .doge-panel--connect {
    text-align: center;
  }

  .doge-grid {
    display: grid;
    grid-template-columns: 1fr;
    gap: 1.5rem;
  }

  @media (min-width: 800px) {
    .doge-grid {
      grid-template-columns: 1fr 1fr;
    }
  }

  .doge-panel h2 {
    margin-top: 0;
    color: #7a4b00;
  }

  .doge-button {
    font-family: inherit;
    font-size: 1rem;
    padding: 0.6rem 1.2rem;
    border-radius: 999px;
    border: 2px solid #a5690a;
    background: #ffe08a;
    color: #5c3d00;
    cursor: pointer;
    margin: 0.4rem 0;
  }
  .doge-button:hover:not(:disabled) {
    background: #ffd166;
  }
  .doge-button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
  .doge-button--gold {
    background: linear-gradient(180deg, #ffe9a8, #e8b93a);
    font-weight: bold;
  }

  .doge-address-box {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    background: #fff3c4;
    border-radius: 12px;
    padding: 0.75rem;
    margin: 0.75rem 0;
  }

  .doge-address {
    font-family: monospace;
    word-break: break-all;
    background: #fffaf0;
    padding: 0.4rem;
    border-radius: 8px;
    border: 1px dashed #d98c00;
  }

  .doge-label {
    font-size: 0.85rem;
    color: #7a4b00;
  }
  .doge-subtle {
    font-size: 0.85rem;
    color: #6b5300;
  }
  .doge-error {
    color: #a4262c;
    font-weight: bold;
  }
  .doge-success {
    color: #2e7d32;
    font-weight: bold;
  }
  .doge-utxo-line {
    font-size: 0.9rem;
    margin: 0.15rem 0;
  }

  .doge-field {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    margin-bottom: 0.75rem;
    font-size: 0.9rem;
  }
  .doge-field input {
    font-family: inherit;
    padding: 0.5rem;
    border-radius: 10px;
    border: 2px solid #e8b93a;
  }

  .doge-poll-status {
    margin-top: 1rem;
    background: #fff3c4;
    border-radius: 12px;
    padding: 0.75rem;
  }
</style>
