<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import QRCode from 'qrcode';
  import { walletStore, isConnected as isConnectedStore, principal as principalStore } from '$lib/stores/wallet';
  import { collateralStore } from '$lib/stores/collateralStore';
  import { appDataStore, protocolStatus } from '$lib/stores/appDataStore';
  import { protocolService } from '$lib/services/protocol';
  import type { ActionBoundContext } from '$lib/services/protocol';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { CANISTER_IDS, CONFIG } from '$lib/config';
  import { WALLET_TYPES } from '$lib/services/auth';
  import { getPublicCkbtcMinterActor, updateBtcBalanceForOwner } from '$lib/services/ckbtcMinterActors';
  import { formatNumber, formatAddress } from '$lib/utils/format';
  import {
    POLL_INTERVAL_MS,
    buildAccountArgs,
    classifyUtxoStatus,
    computeConfirmationDisplay,
    computeMintStepIndex,
    confirmationMeterPercent,
    parseBtcAmountInput,
    isPollingExhausted,
    isRetryableUpdateBalanceError,
    isTerminalUtxoKind,
    satoshiToBitcoin,
    summarizeMinterInfo,
    summarizeUpdateBalanceError,
    type MinterInfoSummary,
    type UpdateBalanceErrorSummary,
    type UtxoStatusSummary,
  } from '$lib/utils/bitcoinBorrowFlow';
  import {
    createInitialIntent,
    loadIntent,
    saveIntent,
    saveIntentIfSameLineage,
    clearIntent,
    storageKeyForPrincipal,
    addMintedReceipt,
    resolveCollateralAmountForBorrow,
    isStillSamePrincipal,
    isStillLiveSession,
    beginPendingAction,
    nextPendingActionForOutcome,
    icusdAmountToRawE8s,
    hasVaultAlreadyBorrowed,
    classifyLiveOpenAndBorrowOutcomeFromBound,
    classifyOpenVaultV2Status,
    classifyFinishBorrowOutcomeFromBound,
    classifyRecheckOutcome,
    runExclusiveAction,
    bitcoinBorrowActionLockName,
    computeBitcoinBorrowRisk,
    computeMaxBorrow,
    projectRatioAtPriceDrop,
    haveTermsChangedMaterially,
    canSubmitBorrow,
    type BitcoinBorrowIntentRecord,
    type BitcoinBorrowStep,
    type VaultLite,
    type OpenAndBorrowOutcome,
    type TermsSnapshot,
    type ExpectedWire,
    type ExclusiveLocksLike,
  } from '$lib/utils/bitcoinBorrowWizard';
  import { userVaults } from '$lib/stores/appDataStore';
  import VaultCard from '$lib/components/vault/VaultCard.svelte';
  import { toastStore } from '$lib/stores/toast';

  const CKBTC_PRINCIPAL = CANISTER_IDS.CKBTC_LEDGER;
  // Storage/lock scoping: a local-dev environment must never read/write a mainnet record (or
  // vice versa) for the same principal text.
  const NETWORK_SCOPE = CONFIG.isLocal ? 'local' : 'mainnet';
  const OBSOLETE_TOKEN_FUNDS_ERROR = 'Insufficient token funds. Your balance is too low for this amount.';

  function clearObsoleteTokenFundsError() {
    // The BTC flow owns the authoritative success boundary. Retire only the
    // exact terminal transfer error left over from an earlier failed attempt;
    // other current errors and success/info toasts remain visible.
    toastStore.removeError(OBSOLETE_TOKEN_FUNDS_ERROR);
  }

  /** Feature-detects the Web Locks API without depending on a specific lib.dom typings version. */
  function getLocks(): ExclusiveLocksLike | null {
    if (typeof navigator === 'undefined') return null;
    const anyNav = navigator as unknown as { locks?: ExclusiveLocksLike };
    return anyNav.locks ?? null;
  }


  // ── Wallet/principal tracking ────────────────────────────────────────
  let isConnected = false;
  let ownerPrincipal: Principal | null = null;
  let ownerPrincipalText: string | null = null;
  let destroyed = false;

  // Bumped on EVERY principal-store transition (handlePrincipalChanged), including a
  // disconnect-then-reconnect of the SAME account. A mutating action captures this
  // alongside the principal text at entry; if either changes before its post-await
  // continuation runs, the continuation must not write into the (now different) live
  // intent/outcome — it preserves its own captured account's record directly in storage.
  let sessionGeneration = 0;

  function principalKey(p: Principal | null): string | null {
    return p ? p.toText() : null;
  }

  function captureSession(): { principalText: string | null; generation: number } {
    return { principalText: ownerPrincipalText, generation: sessionGeneration };
  }

  function isSessionStillLive(captured: { principalText: string | null; generation: number }): boolean {
    return isStillLiveSession(captured, { principalText: ownerPrincipalText, generation: sessionGeneration });
  }

  function isLiveIntentSession(captured: { principalText: string | null; generation: number }, lineage: number): boolean {
    return isSessionStillLive(captured) && intent?.createdAt === lineage;
  }

  // ── Wizard step + persisted intent ───────────────────────────────────
  let step: BitcoinBorrowStep = 'choose';
  let intent: BitcoinBorrowIntentRecord | null = null;
  let step1Snapshot: TermsSnapshot | null = null;

  function persistIntent(): boolean {
    return intent ? saveIntent(localStorage, intent, NETWORK_SCOPE) : false;
  }

  // ── Step 1: calculator ───────────────────────────────────────────────
  let collateralAmountInput = '0.01';
  let collateralAmount: number;
  $: collateralAmount = Number(collateralAmountInput) || 0;
  let icusdAmount = 50;
  let dropPct = 25;
  let step1Error = '';
  let freshnessNow = Date.now();
  let freshnessTimer: ReturnType<typeof setInterval> | null = null;
  let borrowingTermsRefreshBusy = false;
  let borrowingTermsRefreshError = '';
  let nextAutomaticTermsRefreshAt = 0;
  $: collateralConfigFresh = $collateralStore.lastFetch > 0 && freshnessNow - $collateralStore.lastFetch <= 30_000 && !$collateralStore.error;

  $: ckbtcInfo = $collateralStore.collaterals.find((c) => c.principal === CKBTC_PRINCIPAL);
  $: collateralConfigLoading = $collateralStore.loading && !ckbtcInfo;
  $: collateralConfigMissing = !$collateralStore.loading && (!ckbtcInfo || ckbtcInfo.status !== 'Active');
  $: collateralPrice = ckbtcInfo?.price ?? 0;
  $: liquidationCr = ckbtcInfo?.liquidationCr ?? 0;
  $: minimumCr = ckbtcInfo?.minimumCr ?? 0;
  $: borrowingFeeRate = ckbtcInfo?.borrowingFee ?? 0;
  $: feeCurve = $protocolStatus?.borrowingFeeCurveResolved ?? [];
  // Live ckBTC ledger transfer fee (raw satoshi), from the current backend collateral config.
  // Missing fee data remains zero here and keeps calculatorReady false.
  $: ckbtcLedgerFeeSatoshi = BigInt(Math.trunc(ckbtcInfo?.ledgerFee ?? 0));
  $: calculatorReady = Boolean(
    collateralConfigFresh && ckbtcInfo?.status === 'Active' && Number.isFinite(collateralPrice) && collateralPrice > 0 &&
    Number.isFinite(liquidationCr) && liquidationCr > 0 && Number.isFinite(minimumCr) && minimumCr > 0 &&
    Number.isFinite(borrowingFeeRate) && borrowingFeeRate >= 0 && Number.isFinite(ckbtcInfo?.debtCeiling) && ckbtcInfo.debtCeiling > 0 &&
    Number.isFinite(ckbtcInfo?.ledgerFee) && ckbtcInfo.ledgerFee >= 0 && $protocolStatus && Array.isArray(feeCurve)
  );

  $: risk = computeBitcoinBorrowRisk({
    collateralAmountBitcoin: collateralAmount,
    icusdAmount,
    collateralPriceUsd: collateralPrice,
    liquidationCr,
    minimumCr,
    borrowingFeeRate,
    feeCurve,
  });
  $: maxBorrow = computeMaxBorrow(collateralAmount, collateralPrice, minimumCr);
  $: downside = projectRatioAtPriceDrop(
    { collateralAmountBitcoin: collateralAmount, icusdAmount, collateralPriceUsd: collateralPrice },
    dropPct
  );
  $: crBand =
    risk.collateralRatioPct === Infinity
      ? 'safe'
      : risk.collateralRatioPct < minimumCr * 100
        ? 'danger'
        : risk.collateralRatioPct < minimumCr * 1.234 * 100
          ? 'caution'
          : 'safe';
  $: liqBand = risk.liqPriceRatio > 0.75 ? 'danger' : risk.liqPriceRatio > 0.5 ? 'caution' : 'safe';

  function setMaxBorrowAmount() {
    if (maxBorrow > 0) icusdAmount = maxBorrow;
  }

  function ensureIntentForCurrentPrincipal() {
    if (!ownerPrincipalText) return;
    if (!intent || intent.principal !== ownerPrincipalText) {
      const existing = loadIntent(localStorage, ownerPrincipalText, Date.now(), NETWORK_SCOPE);
      intent = existing ?? createInitialIntent(ownerPrincipalText, collateralAmount, icusdAmount, Date.now());
    } else {
      intent = { ...intent, collateralAmountBitcoin: collateralAmount, icusdAmount, updatedAt: Date.now() };
    }
    persistIntent();
  }

  async function refreshBorrowingTerms(): Promise<boolean> {
    if (borrowingTermsRefreshBusy) return false;
    borrowingTermsRefreshBusy = true;
    borrowingTermsRefreshError = '';
    try {
      await Promise.all([
        collateralStore.fetchSupportedCollateral(true, { strict: true }),
        appDataStore.fetchProtocolStatus(true),
      ]);
      freshnessNow = Date.now();
      await tick();
      if (!calculatorReady) {
        borrowingTermsRefreshError = 'Fresh backend data does not include active ckBTC price and borrowing terms.';
        nextAutomaticTermsRefreshAt = Date.now() + 30_000;
        return false;
      }
      nextAutomaticTermsRefreshAt = 0;
      return true;
    } catch {
      freshnessNow = Date.now();
      borrowingTermsRefreshError = 'Could not refresh live ckBTC borrowing terms. Try again when the backend is available.';
      nextAutomaticTermsRefreshAt = Date.now() + 30_000;
      return false;
    } finally {
      borrowingTermsRefreshBusy = false;
    }
  }

  async function proceedToSignIn() {
    step1Error = '';
    if (!(await refreshBorrowingTerms())) {
      if (!borrowingTermsRefreshBusy) {
        step1Error = borrowingTermsRefreshError || 'Live ckBTC collateral settings are unavailable, stale, or inactive.';
      }
      return;
    }
    if (parseBtcAmountInput(collateralAmountInput) === null || !(collateralAmount > 0)) {
      step1Error = 'Enter how much BTC you plan to send.';
      return;
    }
    if (!(icusdAmount > 0)) {
      step1Error = 'Enter how much icUSD you want to borrow.';
      return;
    }
    step1Snapshot = {
      collateralPriceUsd: collateralPrice,
      collateralRatioPct: risk.collateralRatioPct,
      liquidationPriceUsd: risk.liquidationPriceUsd,
      borrowFeeIcusd: risk.borrowFeeIcusd,
      borrowingFeeRate,
      minimumCr,
      debtCeiling: ckbtcInfo?.debtCeiling,
      collateralStatus: ckbtcInfo?.status,
      borrowingFeeCurve: feeCurve.map(([cr, multiplier]) => [cr, multiplier] as [number, number]),
    };
    if (isConnected && ownerPrincipalText) {
      ensureIntentForCurrentPrincipal();
      step = 'send';
    } else {
      step = 'signin';
    }
  }

  // ── Step 2: sign in ──────────────────────────────────────────────────
  let signInBusy = false;
  let signInError = '';

  async function connectWith(walletId: string) {
    signInBusy = true;
    signInError = '';
    try {
      await walletStore.connect(walletId);
      ensureIntentForCurrentPrincipal();
      step = 'send';
    } catch (err) {
      signInError = err instanceof Error ? err.message : 'Could not connect. Please try again.';
    } finally {
      signInBusy = false;
    }
  }

  function switchWallet() {
    walletStore.disconnect().catch(() => {});
  }

  // ── Step 3: send BTC (deposit address + poll) ───────────────────────
  let depositAddress: string | null = null;
  let addressLoading = false;
  let addressError = '';
  let minterInfoSummary: MinterInfoSummary | null = null;
  let minterInfoLoading = false;
  let minterInfoError = false;
  let qrDataUrl = '';
  let addressCopied = false;
  let addressCopyError = false;
  let addressCopyTimer: ReturnType<typeof setTimeout> | null = null;

  let isPolling = false;
  let pollAttempt = 0;
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let utxoStatuses: UtxoStatusSummary[] = [];
  let mintedSummary: UtxoStatusSummary | null = null;
  let lastUpdateBalanceError: UpdateBalanceErrorSummary | null = null;
  let pollingStopped = false;
  let pollFatalMessage = '';
  let pollingPrincipal: Principal | null = null;
  let pollingSession: { principalText: string | null; generation: number } | null = null;
  let pollingLineage: number | null = null;
  let useAvailableBalanceOptIn = false;

  $: mintStepIndex = computeMintStepIndex({ isPolling, pollingStopped, hasMinted: !!mintedSummary });
  $: confirmationDisplay = computeConfirmationDisplay({
    isPolling,
    pollingStopped,
    pollAttempt,
    mintedSummary,
    pollFatalMessage,
    lastUpdateBalanceError,
    utxoStatuses,
    minConfirmationsHint: minterInfoSummary?.minConfirmationsCount,
  });

  $: sessionMintedSatoshi = intent ? BigInt(intent.sessionMintedSatoshi) : 0n;
  $: walletCkbtcBalanceSatoshi = (() => {
    const symbol = ckbtcInfo?.symbol ?? 'ckBTC';
    return $walletStore.tokenBalances?.[symbol]?.raw ?? 0n;
  })();
  $: collateralResolution = resolveCollateralAmountForBorrow({
    sessionMintedSatoshi,
    walletCkbtcBalanceSatoshi,
    useAvailableBalanceOptIn,
    ledgerFeeSatoshi: ckbtcLedgerFeeSatoshi,
  });
  $: resolvedCollateralBitcoin = satoshiToBitcoin(collateralResolution.satoshiAmount);
  $: feeReservedBitcoin = satoshiToBitcoin(collateralResolution.feeReservedSatoshi);
  // The source balance this collateral was drawn from (session mint or opted-in wallet balance),
  // before the ledger-fee reservation — used only to disclose the reservation transparently.
  $: canOfferExistingBalance = sessionMintedSatoshi === 0n && walletCkbtcBalanceSatoshi > 0n && !useAvailableBalanceOptIn && pollAttempt > 0;
  $: sourceBalanceSatoshi = collateralResolution.source === 'existing_balance_opt_in' ? walletCkbtcBalanceSatoshi : sessionMintedSatoshi;
  // True once we have a real balance to draw from but the fee reservation consumed all of it —
  // the wallet flow must explain this instead of silently hiding the "continue" button.
  $: collateralAllConsumedByFees = collateralResolution.satoshiAmount === 0n && sourceBalanceSatoshi > 0n;

  $: if (step === 'send' && isConnected && ownerPrincipal && !depositAddress && !addressLoading && !addressError) {
    requestDepositAddress();
  }

  async function requestDepositAddress() {
    if (!isConnected || !ownerPrincipal || addressLoading) return;
    const sessionKey = principalKey(ownerPrincipal);
    const requestPrincipal = ownerPrincipal;
    const requestSession = captureSession();
    const requestLineage = intent?.createdAt ?? null;
    const isLive = () => !destroyed && isStillLiveSession(requestSession, captureSession()) &&
      principalKey(ownerPrincipal) === sessionKey && step === 'send' &&
      (requestLineage === null || intent?.createdAt === requestLineage);

    addressLoading = true;
    addressError = '';
    try {
      const actor = await getPublicCkbtcMinterActor();
      if (!isLive()) return;
      const args = buildAccountArgs(requestPrincipal);
      const address: string = await actor.get_btc_address(args);
      if (!isLive()) return;
      depositAddress = address;
      qrDataUrl = '';
      void generateQr(address, requestSession, requestLineage);
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
        addressError =
          err instanceof Error ? `Could not fetch your BTC address: ${err.message}` : 'Could not fetch your BTC address.';
      }
    } finally {
      if (isLive()) addressLoading = false;
    }
    void walletStore.refreshBalance({ skipCache: true }).catch(() => {});
  }

  async function generateQr(
    address: string,
    capturedSession: { principalText: string | null; generation: number },
    capturedLineage: number | null
  ) {
    const isQrLive = () => !destroyed && isStillLiveSession(capturedSession, captureSession()) &&
      step === 'send' && (capturedLineage === null || intent?.createdAt === capturedLineage) && depositAddress === address;
    try {
      const dataUrl = await QRCode.toDataURL(address, {
        width: 196,
        margin: 2,
        color: { dark: '#020617', light: '#ffffff' },
        errorCorrectionLevel: 'M',
      });
      if (isQrLive()) qrDataUrl = dataUrl;
    } catch (err) {
      console.error('BTC QR generation failed:', err);
      if (isQrLive()) qrDataUrl = '';
    }
  }

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

  function stopPolling() {
    isPolling = false;
    if (pollTimer !== null) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
  }

  async function beginSentBitcoinFlow() {
    if (!isConnected || !ownerPrincipal || !depositAddress || isPolling) return;
    stopPolling();
    pollAttempt = 0;
    pollingStopped = false;
    pollFatalMessage = '';
    mintedSummary = null;
    lastUpdateBalanceError = null;
    utxoStatuses = [];
    pollingPrincipal = ownerPrincipal;
    pollingSession = captureSession();
    pollingLineage = intent?.createdAt ?? null;
    isPolling = true;
    if (intent) {
      intent = { ...intent, step: 'send', updatedAt: Date.now() };
      persistIntent();
    }
    await runUpdateBalanceCycle();
  }

  function isPollSessionLive(
    sessionPrincipal: Principal,
    capturedSession: { principalText: string | null; generation: number } | null = pollingSession,
    capturedLineage: number | null = pollingLineage
  ): boolean {
    return (
      !destroyed &&
      isPolling &&
      !!capturedSession &&
      isStillLiveSession(capturedSession, captureSession()) &&
      (capturedLineage === null || intent?.createdAt === capturedLineage) &&
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
    const cycleSession = pollingSession;
    const cycleLineage = pollingLineage;
    const cycleIsLive = () => isPollSessionLive(sessionPrincipal, cycleSession, cycleLineage);
    pollAttempt += 1;

    try {
      const result = await updateBtcBalanceForOwner(sessionPrincipal, cycleIsLive);
      if (!cycleIsLive()) return;

      if ('Ok' in result) {
        utxoStatuses = (result.Ok as Array<Record<string, any>>).map(classifyUtxoStatus);
        lastUpdateBalanceError = null;
        const minted = utxoStatuses.find((s) => s.kind === 'Minted');
        if (minted) {
          mintedSummary = minted;
          if (minted.blockIndex !== undefined && minted.satoshiAmount !== undefined && intent) {
            intent = addMintedReceipt(intent, minted.blockIndex, minted.satoshiAmount, Date.now());
            persistIntent();
          }
          stopPolling();
          pollingStopped = true;
          void walletStore.refreshBalance({ skipCache: true }).catch(() => {});
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
      if (!cycleIsLive()) return;
      pollFatalMessage = err instanceof Error ? `Error checking your balance: ${err.message}` : 'Error checking your balance.';
      stopPolling();
      pollingStopped = true;
      return;
    }

    if (!cycleIsLive()) return;

    if (isPollingExhausted(pollAttempt)) {
      stopPolling();
      pollingStopped = true;
      return;
    }

    if (isPolling && !destroyed) {
      pollTimer = setTimeout(runUpdateBalanceCycle, POLL_INTERVAL_MS);
    }
  }

  function recheckAfterTimeout() {
    if (!isConnected || !ownerPrincipal || isPolling) return;
    pollingPrincipal = ownerPrincipal;
    pollingSession = captureSession();
    pollingLineage = intent?.createdAt ?? null;
    pollAttempt = 0;
    pollingStopped = false;
    pollFatalMessage = '';
    isPolling = true;
    void runUpdateBalanceCycle();
  }

  function useExistingBalance() {
    useAvailableBalanceOptIn = true;
  }

  function proceedToConfirm() {
    if (resolvedCollateralBitcoin <= 0) return;
    if (intent) {
      intent = { ...intent, step: 'confirm', updatedAt: Date.now() };
      persistIntent();
    }
    finalTermsLoaded = false;
    step = 'confirm';
  }

  // ── Step 4: confirm and borrow ───────────────────────────────────────
  let finalTermsLoaded = false;
  let finalTermsError = '';
  let finalRisk: ReturnType<typeof computeBitcoinBorrowRisk> | null = null;
  let finalSnapshot: TermsSnapshot | null = null;
  let termsChanged = false;
  let termsConfirmed = false;
  let confirmIcusdAmount = icusdAmount;
  let finalTermsRefreshToken = 0;
  let actionInProgress = false;
  let confirmError = '';
  let ingressRecoveryWarning = '';
  let outcome: OpenAndBorrowOutcome | null = null;

  async function refreshFinalTerms() {
    const refreshToken = ++finalTermsRefreshToken;
    const refreshSession = captureSession();
    const refreshLineage = intent?.createdAt ?? null;
    const isCurrentRefresh = () => !destroyed && refreshToken === finalTermsRefreshToken &&
      isStillLiveSession(refreshSession, captureSession()) && step === 'confirm' &&
      intent?.createdAt === refreshLineage;

    finalTermsLoaded = false;
    finalTermsError = '';
    try {
      await Promise.all([
        collateralStore.fetchSupportedCollateral(true, { strict: true }),
        appDataStore.fetchProtocolStatus(true),
      ]);
    } catch (err) {
      if (!isCurrentRefresh()) return;
      finalRisk = null;
      finalSnapshot = null;
      termsConfirmed = false;
      finalTermsError = err instanceof Error ? `Could not refresh live borrowing terms: ${err.message}` : 'Could not refresh live borrowing terms.';
      return;
    }
    if (!isCurrentRefresh()) return;
    const info = $collateralStore.collaterals.find((c) => c.principal === CKBTC_PRINCIPAL);
    const freshPrice = info?.price ?? collateralPrice;
    const freshLiqCr = info?.liquidationCr ?? liquidationCr;
    const freshMinCr = info?.minimumCr ?? minimumCr;
    const freshFeeRate = info?.borrowingFee ?? borrowingFeeRate;
    const freshFeeCurve = $protocolStatus?.borrowingFeeCurveResolved ?? [];

    const validFeeCurve = Array.isArray($protocolStatus?.borrowingFeeCurveResolved) &&
      $protocolStatus.borrowingFeeCurveResolved.every((point) =>
        Array.isArray(point) && point.length === 2 && Number.isFinite(point[0]) && Number.isFinite(point[1])
      );
    if (!info || !Number.isFinite(freshPrice) || freshPrice <= 0 || !Number.isFinite(freshLiqCr) || freshLiqCr <= 0 || !Number.isFinite(freshMinCr) || freshMinCr <= 0 ||
      !Number.isFinite(freshFeeRate) || freshFeeRate < 0 || !Number.isFinite(info.debtCeiling) || info.debtCeiling <= 0 ||
      info.status !== 'Active' || !$protocolStatus || !validFeeCurve) {
      if (!isCurrentRefresh()) return;
      finalRisk = null;
      finalSnapshot = null;
      termsConfirmed = false;
      finalTermsError = 'Live borrowing terms are unavailable or this collateral is not active. Refresh before borrowing.';
      return;
    }

    // Keep the user-confirmed amount unchanged. If live terms no longer support it, the final
    // ratio gate blocks submission and the user must edit/reconfirm explicitly.
    confirmIcusdAmount = icusdAmount;

    finalRisk = computeBitcoinBorrowRisk({
      collateralAmountBitcoin: resolvedCollateralBitcoin,
      icusdAmount: confirmIcusdAmount,
      collateralPriceUsd: freshPrice,
      liquidationCr: freshLiqCr,
      minimumCr: freshMinCr,
      borrowingFeeRate: freshFeeRate,
      feeCurve: freshFeeCurve,
    });
    const newSnapshot: TermsSnapshot = {
      collateralPriceUsd: freshPrice,
      collateralRatioPct: finalRisk.collateralRatioPct,
      liquidationPriceUsd: finalRisk.liquidationPriceUsd,
      borrowFeeIcusd: finalRisk.borrowFeeIcusd,
      borrowingFeeRate: freshFeeRate,
      minimumCr: freshMinCr,
      debtCeiling: info.debtCeiling,
      collateralStatus: info.status,
      borrowingFeeCurve: freshFeeCurve.map(([cr, multiplier]) => [cr, multiplier] as [number, number]),
    };
    if (!isCurrentRefresh()) return;
    termsChanged = step1Snapshot ? haveTermsChangedMaterially(step1Snapshot, newSnapshot) : false;
    finalSnapshot = newSnapshot;
    termsConfirmed = !termsChanged;
    finalTermsLoaded = true;
  }

  $: if (step === 'confirm' && !finalTermsLoaded && !outcome) {
    refreshFinalTerms();
  }

  function acknowledgeUpdatedTerms() {
    termsConfirmed = true;
  }

  $: principalMatchesIntent = !!intent && isStillSamePrincipal(intent.principal, ownerPrincipalText);
  // True when this account has an unresolved mutating attempt (this tab or another tab)
  // that this tab hasn't classified into an outcome yet — a fresh submit must never be
  // offered on top of that; the user is routed to a read-only recheck first.
  $: hasUnresolvedPendingAction = !!intent?.pendingAction && !outcome;
  $: canConfirm =
    finalTermsLoaded &&
    !!finalRisk &&
    finalRisk.isValidCr &&
    !hasUnresolvedPendingAction &&
    !ingressRecoveryWarning &&
    canSubmitBorrow({ actionInProgress, isConnected, principalMatchesIntent, termsConfirmed });

  async function fetchVaultLites(owner: Principal): Promise<VaultLite[]> {
    const vaults = await publicActor.get_vaults([owner]);
    return vaults.map((v: any) => ({
      vaultId: Number(v.vault_id),
      collateralPrincipal: v.collateral_type.toText(),
      collateralAmount: BigInt(v.collateral_amount),
      borrowedIcusd: BigInt(v.borrowed_icusd_amount),
    }));
  }

  /** Human-readable message for a mutating action refused by the cross-tab exclusive guard. */
  function exclusiveGuardRefusalMessage(reason: 'locked' | 'unsupported'): string {
    return reason === 'locked'
      ? 'Another tab or window is already completing this action. Wait for it to finish, then recheck.'
      : 'This browser does not support the safety lock needed to prevent a duplicate submission across tabs. Close any other tabs with this page open, then try again.';
  }

  type ConfirmAndBorrowRun =
    | { aborted: true; reason: 'other_tab_pending' | 'preflight_unavailable' | 'session_changed' | 'durability_failed' }
    | { aborted: false; classified: OpenAndBorrowOutcome; resolvedIntent: BitcoinBorrowIntentRecord; approvalMayHaveMutated: boolean };

  async function confirmAndBorrow() {
    if (!canConfirm || !intent || !ownerPrincipal) return;
    actionInProgress = true;
    confirmError = '';
    const session = captureSession();
    const actionOwner = ownerPrincipal; // Principal object, stable for THIS action regardless of a later switch.
    const actionOwnerText = actionOwner.toText();
    const baseIntent = intent;

    const locks = getLocks();
    const lockName = bitcoinBorrowActionLockName(actionOwnerText, NETWORK_SCOPE, CANISTER_IDS.CKBTC_LEDGER);

    const exec = await runExclusiveAction<ConfirmAndBorrowRun>(locks, lockName, async () => {
      // Re-read persisted state now that the lock is held, in case another tab already started
      // (or finished) a mutating action for this account while we were waiting to acquire it.
      const latest = loadIntent(localStorage, actionOwnerText, Date.now(), NETWORK_SCOPE);
      // A completed record for this lineage means another tab already submitted and
      // resolved the borrow. A different lineage means this tab's draft is stale. In
      // either case, do not dispatch a second open-and-borrow call.
      if (latest && (latest.pendingAction || latest.borrowConfirmed || latest.step === 'done' || latest.createdAt !== baseIntent.createdAt)) {
        return { aborted: true, reason: 'other_tab_pending' };
      }

      let beforeIds = new Set<number>();
      let beforeVaults: VaultLite[] = [];
      let beforeVaultsLoaded = false;
      try {
        const before = await fetchVaultLites(actionOwner);
        beforeVaults = before;
        beforeVaultsLoaded = true;
        beforeIds = new Set(before.map((v) => v.vaultId));
      } catch {
        // proceed with an empty before-set; reconciliation below still filters by collateral type + amount.
      }

      if (!isLiveIntentSession(session, baseIntent.createdAt)) {
        return { aborted: true, reason: 'session_changed' };
      }

      // Persist the in-flight attempt BEFORE the mutating call, with enough of a snapshot
      // (pre-action vault ids + exact submitted amounts) to recover a still-unknown vault id
      // after a reload/crash, even if this tab never sees the call resolve.
      const submittedCollateralRaw = collateralResolution.satoshiAmount;
      const submittedIcusdRaw = icusdAmountToRawE8s(confirmIcusdAmount);
      if (!beforeVaultsLoaded) return { aborted: true, reason: 'preflight_unavailable' };
      const ctx: ActionBoundContext = {
        expectedPrincipalText: actionOwnerText,
        assertCurrent: () => isLiveIntentSession(session, baseIntent.createdAt),
      };
      const ingressState = await protocolService.getCollateralIngressStateBound(ctx, CANISTER_IDS.CKBTC_LEDGER);
      if (!isLiveIntentSession(session, baseIntent.createdAt)) return { aborted: true, reason: 'session_changed' };
      const activeIngress = ingressState.active_request[0];
      const latestIngress = ingressState.latest_result[0];
      const savedRequestId = baseIntent.openRequestId ? BigInt(baseIntent.openRequestId) : null;
      if (savedRequestId === null && beforeVaults.some((vault) => vault.collateralPrincipal === CKBTC_PRINCIPAL && vault.borrowedIcusd === 0n)) {
        return { aborted: true, reason: 'other_tab_pending' };
      }
      const matchingSavedIngress = [activeIngress, latestIngress].find((view) =>
        view && savedRequestId !== null && view.request_id === savedRequestId &&
        view.owner.toText() === actionOwnerText && view.ledger.toText() === CANISTER_IDS.CKBTC_LEDGER &&
        view.amount_raw === submittedCollateralRaw && 'Open' in view.operation &&
        view.operation.Open.collateral_type.toText() === CKBTC_PRINCIPAL
      );
      if (activeIngress && !matchingSavedIngress) return { aborted: true, reason: 'other_tab_pending' };
      const requestId = savedRequestId ?? ingressState.next_request_id;
      if (requestId <= 0n || (!savedRequestId && requestId !== ingressState.next_request_id) ||
          (savedRequestId !== null && savedRequestId < ingressState.next_request_id && !matchingSavedIngress)) {
        return { aborted: true, reason: 'other_tab_pending' };
      }

      const pendingIntent = {
        ...beginPendingAction(baseIntent, 'open_and_borrow', Date.now(), {
        preActionVaultIds: Array.from(beforeIds),
        submittedCollateralSatoshi: submittedCollateralRaw.toString(),
        submittedIcusdAmount: confirmIcusdAmount,
        submittedIcusdAmountRaw: submittedIcusdRaw.toString(),
        }),
        openRequestId: requestId.toString(),
        borrowDispatchStarted: false,
      };
      if (isLiveIntentSession(session, baseIntent.createdAt)) {
        intent = pendingIntent;
        if (!persistIntent()) {
          // Do not leave an in-memory pending marker when the durable write failed:
          // the caller must be able to see the failure and retry safely.
          intent = baseIntent;
          return { aborted: true, reason: 'durability_failed' };
        }
      } else {
        if (!saveIntentIfSameLineage(localStorage, pendingIntent, Date.now(), NETWORK_SCOPE)) {
          return { aborted: true, reason: 'durability_failed' };
        }
      }

      const result = await protocolService.openVaultV2Bound(
        ctx,
        requestId,
        submittedCollateralRaw,
        CKBTC_PRINCIPAL
      );

      // Resolve against the CAPTURED owner, regardless of who is connected by the time this awaits resolve.
      let vaultsAfter: VaultLite[] = [];
      try {
        vaultsAfter = await fetchVaultLites(actionOwner);
      } catch {
        // classification below handles an empty after-fetch gracefully (stays ambiguous_pending).
      }

      const classified = result.kind === 'predispatch_aborted' && !result.status
        ? { kind: 'failed' as const, vaultId: null, message: result.errorMessage || 'Collateral approval was not completed.' }
        : classifyOpenVaultV2Status({
        status: result.status,
        ownerText: actionOwnerText,
        ledgerText: CANISTER_IDS.CKBTC_LEDGER,
        expectedRequestId: requestId,
        collateralPrincipal: CKBTC_PRINCIPAL,
        expectedCollateralRaw: submittedCollateralRaw,
        vaults: vaultsAfter,
      });

      const resolvedIntent: BitcoinBorrowIntentRecord = {
        ...pendingIntent,
        openRequestId: classified.kind === 'failed' ? null : pendingIntent.openRequestId,
        vaultId: classified.vaultId,
        borrowConfirmed: false,
        partialBorrowAcknowledged: classified.kind === 'partial_zero_debt',
        step: 'confirm',
        pendingAction: nextPendingActionForOutcome(classified.kind, pendingIntent.pendingAction ?? null),
        updatedAt: Date.now(),
      };

      return { aborted: false, classified, resolvedIntent, approvalMayHaveMutated: result.approvalMayHaveMutated };
    });

    if (!exec.ran) {
      confirmError = exclusiveGuardRefusalMessage(exec.reason);
      actionInProgress = false;
      if (ownerPrincipalText) void reconcileForPrincipal(ownerPrincipalText);
      return;
    }
    if (exec.result.aborted) {
      actionInProgress = false;
      if (exec.result.reason === 'other_tab_pending') {
        confirmError = 'Another tab or window already has an action in progress for this account. Recheck its status before trying again.';
        if (ownerPrincipalText) void reconcileForPrincipal(ownerPrincipalText);
      } else if (exec.result.reason === 'durability_failed') {
        confirmError = 'Your attempt could not be saved safely, so nothing was submitted. Free local storage and try again.';
      }
      // 'session_changed': the connected account switched mid-flow — no error to show, the UI
      // already re-renders for the new/disconnected account via handlePrincipalChanged.
      return;
    }

    const { classified, resolvedIntent, approvalMayHaveMutated } = exec.result;
    if (isLiveIntentSession(session, baseIntent.createdAt)) {
      intent = resolvedIntent;
      outcome = classified;
      persistIntent();
      if (classified.kind === 'success') {
        clearObsoleteTokenFundsError();
        step = 'done';
        void appDataStore.refreshAll(actionOwner).catch(() => {});
      } else {
        confirmError = approvalMayHaveMutated
          ? `${classified.message} (A token approval step may have gone through even though nothing was borrowed.)`
          : classified.message;
        termsConfirmed = false;
      }
    } else {
      // Wallet changed mid-request. Preserve the ORIGINAL account's resolved record directly
      // in storage (never through the live `intent`/`outcome`, which now belong to a different
      // account), and never clobber a newer intent lineage started by that account since.
      saveIntentIfSameLineage(localStorage, resolvedIntent, Date.now(), NETWORK_SCOPE);
    }
    actionInProgress = false;
  }

  type FinishBorrowRun =
    | { aborted: true; reason: 'other_tab_pending' | 'preflight_unavailable' | 'session_changed' | 'durability_failed' }
    | { aborted: false; finalOutcome: OpenAndBorrowOutcome; resolvedIntent: BitcoinBorrowIntentRecord };

  async function finishBorrowOnVault() {
    const knownVaultId = outcome?.vaultId ?? intent?.vaultId ?? null;
    if (knownVaultId === null || !ownerPrincipal || !intent) return;
    actionInProgress = true;
    confirmError = '';
    const session = captureSession();
    const actionOwner = ownerPrincipal;
    const actionOwnerText = actionOwner.toText();
    const baseIntent = intent;
    const vaultId = knownVaultId;

    const writeResolved = (resolved: BitcoinBorrowIntentRecord, finalOutcome: OpenAndBorrowOutcome) => {
      if (isLiveIntentSession(session, baseIntent.createdAt)) {
        intent = resolved;
        outcome = finalOutcome;
        persistIntent();
        if (finalOutcome.kind === 'success') {
          clearObsoleteTokenFundsError();
          step = 'done';
          void appDataStore.refreshAll(actionOwner).catch(() => {});
        } else {
          confirmError = finalOutcome.message;
        }
      } else {
        saveIntentIfSameLineage(localStorage, resolved, Date.now(), NETWORK_SCOPE);
      }
    };

    if (!isSessionStillLive(session)) {
      actionInProgress = false;
      return;
    }

    const locks = getLocks();
    const lockName = bitcoinBorrowActionLockName(actionOwnerText, NETWORK_SCOPE, CANISTER_IDS.CKBTC_LEDGER);

    const exec = await runExclusiveAction<FinishBorrowRun>(locks, lockName, async () => {
      const latest = loadIntent(localStorage, actionOwnerText, Date.now(), NETWORK_SCOPE);
      // Never resubmit on top of a DIFFERENT in-flight attempt from another tab (e.g. a fresh
      // open_and_borrow that this tab does not yet know about).
      if (latest && (
        latest.createdAt !== baseIntent.createdAt ||
        latest.borrowConfirmed ||
        latest.step === 'done' ||
        (latest.pendingAction && !(latest.pendingAction === 'finish_borrow' && latest.partialBorrowAcknowledged))
      )) {
        return { aborted: true, reason: 'other_tab_pending' };
      }

      // Keep the fresh preflight inside the cross-tab lock. Two tabs can otherwise both observe
      // zero debt before either one dispatches, then serialize duplicate borrow calls after the
      // first tab releases the lock. A failed or missing read is fail-closed.
      let preflightVault: VaultLite | null;
      try {
        const current = await fetchVaultLites(actionOwner);
        if (!isLiveIntentSession(session, baseIntent.createdAt)) return { aborted: true, reason: 'session_changed' };
        preflightVault = current.find((v) => v.vaultId === vaultId) ?? null;
      } catch {
        return { aborted: true, reason: 'preflight_unavailable' };
      }
      if (!preflightVault || hasVaultAlreadyBorrowed(preflightVault)) {
        return { aborted: true, reason: preflightVault ? 'other_tab_pending' : 'preflight_unavailable' };
      }

      const submittedIcusdRaw = icusdAmountToRawE8s(confirmIcusdAmount);
      const pendingIntent = beginPendingAction(baseIntent, 'finish_borrow', Date.now(), {
        submittedIcusdAmount: confirmIcusdAmount,
        submittedIcusdAmountRaw: submittedIcusdRaw.toString(),
      });
      if (isLiveIntentSession(session, baseIntent.createdAt)) {
        intent = pendingIntent;
        if (!persistIntent()) {
          intent = baseIntent;
          return { aborted: true, reason: 'durability_failed' };
        }
      } else {
        if (!saveIntentIfSameLineage(localStorage, pendingIntent, Date.now(), NETWORK_SCOPE)) {
          return { aborted: true, reason: 'durability_failed' };
        }
      }

      const ctx: ActionBoundContext = {
        expectedPrincipalText: actionOwnerText,
        assertCurrent: () => isLiveIntentSession(session, baseIntent.createdAt),
      };

      const result = await protocolService.borrowFromVaultBound(ctx, vaultId, submittedIcusdRaw);

      let vaultAfter: VaultLite | null = null;
      try {
        const after = await fetchVaultLites(actionOwner);
        vaultAfter = after.find((v) => v.vaultId === vaultId) ?? null;
      } catch {
        // classification below handles a null vaultAfter gracefully.
      }

      const classifiedBorrowOutcome = classifyFinishBorrowOutcomeFromBound({
        signal: { kind: result.kind, vaultId, errorMessage: result.errorMessage },
        vaultAfter,
        expectedBorrowedRaw: submittedIcusdRaw,
      });
      const borrowReceiptMatches = result.kind === 'dispatched_ok' && result.blockIndex !== null &&
        result.submittedIcusdRaw === submittedIcusdRaw;
      const finalOutcome = classifiedBorrowOutcome.kind === 'success' && !borrowReceiptMatches
        ? { kind: 'ambiguous_pending' as const, vaultId, message: 'The vault has the expected debt, but this borrow reply did not provide an attributable receipt. Recheck before taking further action.' }
        : classifiedBorrowOutcome;

      const resolvedIntent: BitcoinBorrowIntentRecord = {
        ...pendingIntent,
        vaultId,
        borrowConfirmed: finalOutcome.kind === 'success',
        partialBorrowAcknowledged: finalOutcome.kind === 'partial_zero_debt',
        borrowDispatchStarted: finalOutcome.kind === 'ambiguous_pending',
        step: finalOutcome.kind === 'success' ? 'done' : pendingIntent.step,
        pendingAction: nextPendingActionForOutcome(finalOutcome.kind, pendingIntent.pendingAction ?? null),
        updatedAt: Date.now(),
      };
      return { aborted: false, finalOutcome, resolvedIntent };
    });

    if (!exec.ran) {
      confirmError = exclusiveGuardRefusalMessage(exec.reason);
      actionInProgress = false;
      // No mutation occurred when the safety lock is unavailable. Keep the current confirm
      // screen visible so the user can address the guard; reconciliation here can race the
      // pending UI update and incorrectly send them back to the deposit step.
      if (exec.reason === 'locked' && ownerPrincipalText) void reconcileForPrincipal(ownerPrincipalText);
      return;
    }
    if (exec.result.aborted) {
      actionInProgress = false;
      confirmError = exec.result.reason === 'preflight_unavailable'
        ? 'The vault state could not be refreshed safely. Recheck on-chain before trying again.'
        : exec.result.reason === 'durability_failed'
          ? 'Your attempt could not be saved safely, so nothing was submitted. Free local storage and try again.'
        : exec.result.reason === 'session_changed'
          ? ''
          : 'Another tab or window is already completing this action for this account. Recheck its status before trying again.';
      if (ownerPrincipalText) void reconcileForPrincipal(ownerPrincipalText);
      return;
    }

    writeResolved(exec.result.resolvedIntent, exec.result.finalOutcome);
    actionInProgress = false;
  }

  /** The exact wire amounts THIS record's mutating attempt actually submitted (or 0 if never submitted), for exact-match verification — never re-derived from the live, possibly-since-edited calculator inputs. */
  function expectedWireFromIntent(record: BitcoinBorrowIntentRecord): ExpectedWire {
    return {
      collateralAmountRaw: record.submittedCollateralSatoshi ? BigInt(record.submittedCollateralSatoshi) : 0n,
      // Records written before the raw field was introduced retain the human amount. Convert that
      // legacy field only as a compatibility fallback; every new attempt persists and reuses the
      // exact bigint returned to the bound API.
      borrowedAmountRaw: record.submittedIcusdAmountRaw
        ? BigInt(record.submittedIcusdAmountRaw)
        : record.submittedIcusdAmount != null
          ? icusdAmountToRawE8s(record.submittedIcusdAmount)
          : 0n,
    };
  }

  async function recheckOutcome() {
    if (!ownerPrincipal || !intent) return;
    actionInProgress = true;
    const session = captureSession();
    const actionOwner = ownerPrincipal;
    const baseIntent = intent;
    const knownVaultId = outcome?.vaultId ?? baseIntent.vaultId ?? null;
    try {
      const after = await fetchVaultLites(actionOwner);
      // A query-only reconciliation can confirm success but can NEVER manufacture
      // partial_zero_debt or failed — see classifyRecheckOutcome's doc comment.
      const reclassified = baseIntent.pendingAction === 'finish_borrow' && baseIntent.borrowDispatchStarted !== false
        ? { kind: 'ambiguous_pending' as const, vaultId: knownVaultId, message: 'Vault debt alone cannot attribute a borrow whose reply was lost. Continue to recheck; do not retry.' }
        : baseIntent.openRequestId &&
        (baseIntent.pendingAction === 'open_and_borrow' ||
          (baseIntent.pendingAction === 'finish_borrow' && baseIntent.borrowDispatchStarted === false))
        ? classifyOpenVaultV2Status({
            status: await protocolService.getCollateralIngressBound({
              expectedPrincipalText: actionOwner.toText(),
              assertCurrent: () => isLiveIntentSession(session, baseIntent.createdAt),
            }, CANISTER_IDS.CKBTC_LEDGER, BigInt(baseIntent.openRequestId)),
            ownerText: actionOwner.toText(),
            ledgerText: CANISTER_IDS.CKBTC_LEDGER,
            expectedRequestId: BigInt(baseIntent.openRequestId),
            collateralPrincipal: CKBTC_PRINCIPAL,
            expectedCollateralRaw: expectedWireFromIntent(baseIntent).collateralAmountRaw,
            vaults: after,
          })
        : classifyRecheckOutcome({
            knownVaultId,
            vaults: after,
            beforeIds: new Set(baseIntent.preActionVaultIds ?? []),
            ckbtcPrincipal: CKBTC_PRINCIPAL,
            expected: expectedWireFromIntent(baseIntent),
          });

      const resolvedIntent: BitcoinBorrowIntentRecord = {
        ...baseIntent,
        vaultId: reclassified.vaultId,
        borrowConfirmed: reclassified.kind === 'success',
        partialBorrowAcknowledged: false,
        step: reclassified.kind === 'success' ? 'done' : 'confirm',
        pendingAction: nextPendingActionForOutcome(reclassified.kind, baseIntent.pendingAction ?? null),
        updatedAt: Date.now(),
      };

      if (isLiveIntentSession(session, baseIntent.createdAt)) {
        outcome = reclassified;
        intent = resolvedIntent;
        persistIntent();
        if (reclassified.kind === 'success') {
          clearObsoleteTokenFundsError();
          step = 'done';
          void appDataStore.refreshAll(actionOwner).catch(() => {});
        }
      } else {
        saveIntentIfSameLineage(localStorage, resolvedIntent, Date.now(), NETWORK_SCOPE);
      }
    } finally {
      actionInProgress = false;
    }
  }

  // ── Done step: inline ckBTC position view (reuses the existing VaultCard/actions) ──
  let doneExpandedVaultId: number | null = null;
  function handleDoneVaultToggle(e: CustomEvent<{ vaultId: number }>) {
    doneExpandedVaultId = doneExpandedVaultId === e.detail.vaultId ? null : e.detail.vaultId;
  }
  function handleDoneVaultUpdated() {
    if (ownerPrincipal) void appDataStore.fetchUserVaults(ownerPrincipal, true).catch(() => {});
  }
  $: doneCkbtcVaults = $userVaults.filter((v) => (v.collateralType || '') === CKBTC_PRINCIPAL);
  $: confirmedDoneVault = doneCkbtcVaults.find((v) => v.vaultId === (outcome?.vaultId ?? intent?.vaultId)) ?? null;
  $: confirmedCollateralBitcoin = confirmedDoneVault?.collateralAmount ??
    (intent?.submittedCollateralSatoshi ? satoshiToBitcoin(BigInt(intent.submittedCollateralSatoshi)) : resolvedCollateralBitcoin);
  $: confirmedDoneDebt = confirmedDoneVault?.borrowedIcusd ??
    (intent?.submittedIcusdAmountRaw ? Number(BigInt(intent.submittedIcusdAmountRaw)) / 100_000_000 : confirmIcusdAmount || icusdAmount);
  $: if (step === 'done' && ownerPrincipal) {
    appDataStore.fetchUserVaults(ownerPrincipal).catch(() => {});
  }

  function backToAmounts() {
    outcome = null;
    confirmError = '';
    step = 'choose';
  }

  function retryConfirm() {
    outcome = null;
    confirmError = '';
    finalTermsLoaded = false;
  }

  // ── Result / reset ───────────────────────────────────────────────────
  function resetFlow() {
    useAvailableBalanceOptIn = false;
    if (ownerPrincipalText) clearIntent(localStorage, ownerPrincipalText, NETWORK_SCOPE);
    intent = null;
    outcome = null;
    confirmError = '';
    step1Error = '';
    depositAddress = null;
    addressError = '';
    minterInfoSummary = null;
    minterInfoError = false;
    pollAttempt = 0;
    utxoStatuses = [];
    mintedSummary = null;
    lastUpdateBalanceError = null;
    pollingStopped = false;
    pollFatalMessage = '';
    pollingPrincipal = null;
    pollingSession = null;
    pollingLineage = null;
    finalTermsLoaded = false;
    termsConfirmed = false;
    termsChanged = false;
    useAvailableBalanceOptIn = false;
    collateralAmountInput = '0.01';
    icusdAmount = 50;
    dropPct = 25;
    step = 'choose';
  }

  // ── Reconciliation on load / principal change ────────────────────────
  async function reconcileForPrincipal(key: string, capturedSession = captureSession()) {
    const now = Date.now();
    const loaded = loadIntent(localStorage, key, now, NETWORK_SCOPE);
    if (!isStillLiveSession(capturedSession, captureSession())) return;
    if (!loaded) {
      // A reconcile started before this tab created its first intent must not erase that newer
      // in-memory draft when its empty read finally returns.
      if (!intent || intent.principal !== key) intent = null;
      ingressRecoveryWarning = '';
      if (ownerPrincipal) {
        try {
          const ctx: ActionBoundContext = {
            expectedPrincipalText: key,
            assertCurrent: () => isSessionStillLive(capturedSession),
          };
          const state = await protocolService.getCollateralIngressStateBound(ctx, CANISTER_IDS.CKBTC_LEDGER);
          if (!isSessionStillLive(capturedSession) || intent?.principal === key) return;
          if (state.active_request[0]) {
            ingressRecoveryWarning = 'A collateral request is already pending for this account. Recheck the collateral request before opening another vault.';
          } else {
            const latest = state.latest_result[0];
            if (latest && 'Complete' in latest.phase && latest.result[0] && 'Open' in latest.result[0]) {
              const vaultId = Number(latest.result[0].Open.vault_id);
              const vault = (await fetchVaultLites(ownerPrincipal)).find((candidate) => candidate.vaultId === vaultId);
              if (!isSessionStillLive(capturedSession) || intent?.principal === key) return;
              if (!vault || vault.borrowedIcusd === 0n) {
                ingressRecoveryWarning = `The latest collateral request opened vault #${vaultId}, but no saved borrow intent is available. Inspect your vaults before starting another open.`;
              }
            }
          }
          const priorZeroDebtVault = (await fetchVaultLites(ownerPrincipal)).find((vault) =>
            vault.collateralPrincipal === CKBTC_PRINCIPAL && vault.borrowedIcusd === 0n
          );
          if (!isSessionStillLive(capturedSession) || intent?.principal === key) return;
          if (priorZeroDebtVault) ingressRecoveryWarning = `Vault #${priorZeroDebtVault.vaultId} has no debt and may match a prior open whose local request was lost. Inspect it before opening another.`;
        } catch {
          ingressRecoveryWarning = 'Collateral request history could not be checked. Opening is paused until the account journal can be read safely.';
        }
      }
      return;
    }
    ingressRecoveryWarning = '';
    intent = loaded;
    collateralAmountInput = String(loaded.collateralAmountBitcoin);
    icusdAmount = loaded.icusdAmount;

    if (loaded.borrowConfirmed && loaded.vaultId !== null) {
      outcome = { kind: 'success', vaultId: loaded.vaultId, message: 'Vault opened and icUSD borrowed.' };
      step = 'done';
      return;
    }

    // Any unresolved attempt (a known vault id to re-verify, OR an unknown vault id with a
    // still-open open_and_borrow attempt) needs a fresh on-chain check — classifyRecheckOutcome
    // handles both shapes and can only ever confirm success or stay ambiguous_pending, never
    // manufacture partial_zero_debt/failed from a query alone.
    const needsReconcile = loaded.vaultId !== null || loaded.pendingAction === 'open_and_borrow';
    if (needsReconcile && ownerPrincipal) {
      try {
        const vaults = await fetchVaultLites(ownerPrincipal);
        if (!isStillLiveSession(capturedSession, captureSession()) || !isStillSamePrincipal(key, principalKey(ownerPrincipal))) return;
        if (intent && intent.createdAt !== loaded.createdAt) return;
        let classified = loaded.pendingAction === 'finish_borrow' && loaded.borrowDispatchStarted !== false
          ? { kind: 'ambiguous_pending' as const, vaultId: loaded.vaultId, message: 'Vault debt alone cannot attribute a borrow whose reply was lost. Continue to recheck; do not retry.' }
          : loaded.openRequestId &&
          (loaded.pendingAction === 'open_and_borrow' ||
            (loaded.pendingAction === 'finish_borrow' && loaded.borrowDispatchStarted === false))
          ? classifyOpenVaultV2Status({
              status: await protocolService.getCollateralIngressBound({
                expectedPrincipalText: key,
                assertCurrent: () => isLiveIntentSession(capturedSession, loaded.createdAt),
              }, CANISTER_IDS.CKBTC_LEDGER, BigInt(loaded.openRequestId)),
              ownerText: key,
              ledgerText: CANISTER_IDS.CKBTC_LEDGER,
              expectedRequestId: BigInt(loaded.openRequestId),
              collateralPrincipal: CKBTC_PRINCIPAL,
              expectedCollateralRaw: expectedWireFromIntent(loaded).collateralAmountRaw,
              vaults,
            })
          : classifyRecheckOutcome({
              knownVaultId: loaded.vaultId,
              vaults,
              beforeIds: new Set(loaded.preActionVaultIds ?? []),
              ckbtcPrincipal: CKBTC_PRINCIPAL,
              expected: expectedWireFromIntent(loaded),
            });
        // A previously persisted partial marker is not enough to authorize a new mint attempt.
        // The current backend may retain a pending mint after returning an error, so a recheck
        // that cannot confirm the borrow must remain ambiguous across reloads.
        outcome = classified;
        const resolved: BitcoinBorrowIntentRecord = {
          ...loaded,
          openRequestId: classified.kind === 'failed' ? null : loaded.openRequestId,
          vaultId: classified.vaultId,
          borrowConfirmed: classified.kind === 'success',
          step: classified.kind === 'success' ? 'done' : 'confirm',
          pendingAction: nextPendingActionForOutcome(classified.kind, loaded.pendingAction ?? null),
          partialBorrowAcknowledged: classified.kind === 'partial_zero_debt',
          updatedAt: now,
        };
        intent = resolved;
        persistIntent();
        step = resolved.step;
        finalTermsLoaded = false;
        return;
      } catch {
        // Fall through to a manual resume below; the UI offers a read-only recheck.
      }
    }

    step = loaded.step;
  }

  async function handlePrincipalChanged(newKey: string | null) {
    // Every transition — including a disconnect-then-reconnect of the SAME principal —
    // starts a new session. A mutating action captures the generation at entry; a late
    // continuation whose generation no longer matches must not touch the live intent/outcome.
    sessionGeneration += 1;
    stopPolling();
    depositAddress = null;
    addressError = '';
    addressLoading = false;
    minterInfoSummary = null;
    minterInfoError = false;
    pollAttempt = 0;
    utxoStatuses = [];
    mintedSummary = null;
    lastUpdateBalanceError = null;
    pollingStopped = false;
    pollFatalMessage = '';
    pollingPrincipal = null;
    pollingSession = null;
    pollingLineage = null;
    finalTermsLoaded = false;
    termsConfirmed = false;
    termsChanged = false;
    outcome = null;
    confirmError = '';

    if (newKey === null) {
      intent = null;
      if (step !== 'choose') step = 'signin';
      return;
    }
    await reconcileForPrincipal(newKey, captureSession());
    // If the account changed while a confirm screen was open, the previous
    // refresh may already have left `finalTermsLoaded` false. Start an explicit
    // new-session refresh so the old pending Promise cannot suppress it.
    if (step === 'confirm' && !outcome) void refreshFinalTerms();
  }

  let lastPrincipalKey: string | null = null;
  const unsubConnected = isConnectedStore.subscribe((v) => {
    isConnected = v;
  });
  const unsubPrincipal = principalStore.subscribe((v) => {
    const key = principalKey(v);
    ownerPrincipal = v;
    ownerPrincipalText = key;
    if (key !== lastPrincipalKey) {
      lastPrincipalKey = key;
      void handlePrincipalChanged(key);
    }
  });

  // Cross-tab reconciliation: another tab writing this account's intent record (e.g. it
  // started or resolved a mutating action) must be picked up here so this tab never offers
  // a fresh "Confirm and borrow" on top of an action another tab already has in flight.
  function handleStorageEvent(e: StorageEvent) {
    if (!ownerPrincipalText || e.key !== storageKeyForPrincipal(ownerPrincipalText, NETWORK_SCOPE)) return;
    void reconcileForPrincipal(ownerPrincipalText, captureSession());
  }

  onMount(() => {
    void refreshBorrowingTerms();
    freshnessTimer = setInterval(() => {
      freshnessNow = Date.now();
      if (document.visibilityState !== 'visible' || borrowingTermsRefreshBusy || freshnessNow < nextAutomaticTermsRefreshAt) return;
      if (!calculatorReady) void refreshBorrowingTerms();
    }, 5_000);
    if (typeof window !== 'undefined') window.addEventListener('storage', handleStorageEvent);

  });

  onDestroy(() => {
    destroyed = true;
    if (freshnessTimer !== null) clearInterval(freshnessTimer);
    unsubConnected();
    unsubPrincipal();
    stopPolling();
    if (addressCopyTimer !== null) clearTimeout(addressCopyTimer);
    if (typeof window !== 'undefined') window.removeEventListener('storage', handleStorageEvent);
  });
</script>

<svelte:head><title>Borrow with BTC | Rumi Protocol</title></svelte:head>

<div class="dbw-page">
  <div class="dbw-hero">
    <div class="dbw-hero-logo">
      <span class="dbw-bitcoin-mark" aria-label="Bitcoin">₿</span>
    </div>
    <h1>Keep your Bitcoin. Borrow dollars.</h1>
    <p class="dbw-subtitle">Borrow icUSD against ckBTC, the Internet Computer’s 1:1 Bitcoin representation. icUSD is dollar-denominated; your collateral remains Bitcoin-backed.</p>
  </div>

  <div class="dbw-stepper" aria-label="Borrowing steps">
    <div class="dbw-step" class:is-active={step === 'choose'} class:is-done={step !== 'choose'}>
      <span class="dbw-step-circle">1</span><span class="dbw-step-label">Amounts</span>
    </div>
    <span class="dbw-step-line"></span>
    <div class="dbw-step" class:is-active={step === 'signin'} class:is-done={step === 'send' || step === 'confirm' || step === 'done'}>
      <span class="dbw-step-circle">2</span><span class="dbw-step-label">Sign in</span>
    </div>
    <span class="dbw-step-line"></span>
    <div class="dbw-step" class:is-active={step === 'send'} class:is-done={step === 'confirm' || step === 'done'}>
      <span class="dbw-step-circle">3</span><span class="dbw-step-label">Send BTC</span>
    </div>
    <span class="dbw-step-line"></span>
    <div class="dbw-step" class:is-active={step === 'confirm' || step === 'done'} class:is-done={step === 'done'}>
      <span class="dbw-step-circle">4</span><span class="dbw-step-label">Borrow</span>
    </div>
  </div>

  {#if isConnected && ownerPrincipalText && step !== 'choose'}
    <div class="dbw-signed-in-strip">
      Signed in as <code>{formatAddress(ownerPrincipalText, 8, 6)}</code>.
      <button type="button" class="dbw-link-btn" disabled={actionInProgress} on:click={switchWallet}>Not you? Switch wallet.</button>
    </div>
  {/if}

  <div class="dbw-panel">
    {#if step === 'choose'}
      <div class="dbw-row dbw-row--annotated">
        <label class="dbw-field-label" for="dbw-collateral">BTC you'll send</label>
        <div class="dbw-input-wrap">
          <input
            id="dbw-collateral"
            type="text"
            inputmode="decimal"
            bind:value={collateralAmountInput}
            class="dbw-input"
          />
          <span class="dbw-input-suffix">BTC</span>
        </div>
        {#if calculatorReady}
          <p class="dbw-hint">&asymp; ${formatNumber(collateralAmount * collateralPrice)} at today's BTC price</p>
        {:else if collateralConfigLoading}
          <p class="dbw-hint">Loading live BTC price&hellip;</p>
        {:else}
          <p class="dbw-hint dbw-hint--warn">
            {#if borrowingTermsRefreshBusy}
              Refreshing live collateral price and borrowing terms…
            {:else}
              {borrowingTermsRefreshError || 'Live price or borrowing terms need a refresh.'}
            {/if}
          </p>
          <button class="dbw-btn dbw-btn--secondary" type="button" on:click={() => void refreshBorrowingTerms()} disabled={borrowingTermsRefreshBusy}>
            {borrowingTermsRefreshBusy ? 'Refreshing…' : 'Refresh borrowing terms'}
          </button>
        {/if}
      </div>

      <div class="dbw-row">
        <div class="dbw-field-label-row">
          <label class="dbw-field-label" for="dbw-icusd">icUSD you'll borrow</label>
          {#if maxBorrow > 0}
            <button type="button" class="dbw-max-btn" on:click={setMaxBorrowAmount}>Max</button>
          {/if}
        </div>
        <div class="dbw-input-wrap">
          <input id="dbw-icusd" type="number" min="0" step="0.01" bind:value={icusdAmount} class="dbw-input" />
          <span class="dbw-input-suffix">icUSD</span>
        </div>
      </div>

      <p class="dbw-note">Estimates update as you type and use current backend terms when available.</p>

      <div class="dbw-result-strip">
        <div class="dbw-result-item">
          <span class="dbw-result-label">Collateral ratio</span>
          <span class="dbw-result-value dbw-band-{crBand}">
            {calculatorReady ? (risk.collateralRatioPct === Infinity ? 'no debt' : `${formatNumber(risk.collateralRatioPct)}%`) : 'Unavailable'}
          </span>
        </div>
        <div class="dbw-result-item">
          <span class="dbw-result-label">Borrow fee</span>
          <span class="dbw-result-value">{calculatorReady ? `${formatNumber(risk.borrowFeeIcusd, 4)} icUSD` : 'Unavailable'}</span>
        </div>
        <div class="dbw-result-item">
          <span class="dbw-result-label">icUSD received</span>
          <span class="dbw-result-value">{calculatorReady ? `${formatNumber(risk.icusdReceived, 4)} icUSD` : 'Unavailable'}</span>
        </div>
        <div class="dbw-result-item">
          <span class="dbw-result-label">Liquidation price</span>
          <span class="dbw-result-value dbw-band-{liqBand}">{calculatorReady ? `$${formatNumber(risk.liquidationPriceUsd, 4)}` : 'Unavailable'}</span>
        </div>
      </div>

      {#if calculatorReady}
      <div class="dbw-downside">
        <label class="dbw-field-label" for="dbw-drop">If BTC fell by {dropPct}%, your ratio would be about {formatNumber(downside.impliedCrPct)}%.</label>
        <input id="dbw-drop" type="range" min="1" max="90" bind:value={dropPct} class="dbw-slider" />
        {#if risk.safetyDeltaPct > 0}
          <p class="dbw-hint">You'd be at your liquidation price if BTC fell about {formatNumber(risk.safetyDeltaPct)}% from here.</p>
        {/if}
      </div>
      {:else}
        <p class="dbw-hint">Live collateral price and borrowing terms are unavailable or stale. Risk estimates will appear after a fresh backend read.</p>
      {/if}

      <p class="dbw-risk">ckBTC deposits need Bitcoin confirmations, and the minter may hold or reject deposits under its deposit checks. Borrowing can lead to liquidation if the collateral value falls below the required threshold.</p>

      {#if step1Error}<p class="dbw-error" role="alert">{step1Error}</p>{/if}
      {#if collateralConfigMissing}<p class="dbw-error" role="alert">ckBTC collateral is not available right now. Please try again later.</p>{/if}

      <button class="dbw-btn dbw-btn--primary dbw-btn--full" type="button" on:click={proceedToSignIn} disabled={!calculatorReady || borrowingTermsRefreshBusy}>
        {borrowingTermsRefreshBusy ? 'Refreshing terms…' : 'Continue with this loan'}
      </button>

      <p class="dbw-bitcoin-only-hint">Need to mint or redeem ckBTC? <a href="/bitcoin">Go to the Bitcoin minter</a>.</p>
    {:else if step === 'signin'}
      <button type="button" class="dbw-link-btn dbw-back-link" on:click={() => (step = 'choose')}>&larr; Back to amounts</button>
      <h2>Sign in to continue</h2>
      <p class="dbw-panel-sub">
        Sign in with the same kind of account you already use elsewhere: Google, Apple, Microsoft, or a device passkey,
        through Internet Identity. No new password, no seed phrase for this step.
      </p>

      <div class="dbw-signin-actions">
        <button class="dbw-btn dbw-btn--primary dbw-btn--full" type="button" disabled={signInBusy} on:click={() => connectWith(WALLET_TYPES.INTERNET_IDENTITY)}>
          {signInBusy ? 'Connecting…' : 'Continue with Internet Identity'}
        </button>
        <button class="dbw-btn dbw-btn--secondary dbw-btn--full" type="button" disabled={signInBusy} on:click={() => connectWith(WALLET_TYPES.OISY)}>
          {signInBusy ? 'Connecting…' : 'Prefer a crypto wallet? Use Oisy'}
        </button>
      </div>

      {#if signInError}<p class="dbw-error" role="alert">{signInError}</p>{/if}

      <details class="dbw-disclosure">
        <summary>What happens to my BTC?</summary>
        <p>
          DFINITY’s minter holds the BTC backing your ckBTC. Rumi locks that ckBTC as vault collateral. After repaying
          the debt and withdrawing the collateral, you can redeem through the <a href="/bitcoin">Bitcoin minter</a>.
        </p>
      </details>
    {:else if step === 'send'}
      <h2>Send BTC to your address</h2>
      <p class="dbw-panel-sub">Your ckBTC will arrive in your connected wallet, then you'll borrow against it.</p>

      <div class="dbw-intent-strip">
        <div><span class="dbw-intent-label">Send</span><strong>{formatNumber(collateralAmount)} BTC</strong></div>
        <div><span class="dbw-intent-label">Borrow</span><strong>{formatNumber(icusdAmount)} icUSD</strong></div>
      </div>

      <p class="dbw-risk">
        Bitcoin deposits need {minterInfoSummary?.minConfirmationsValue ?? 'unavailable'} confirmations before they mint,
        confirmation timing depends on Bitcoin network conditions. You can close this tab and come back; your
        deposit address does not change.
      </p>

      {#if !depositAddress}
        {#if addressLoading}
          <p class="dbw-panel-sub" aria-live="polite">Fetching your deposit address&hellip;</p>
        {:else if addressError}
          <p class="dbw-error" role="alert">{addressError}</p>
          <button class="dbw-btn dbw-btn--primary" type="button" on:click={requestDepositAddress}>Retry</button>
        {/if}
      {:else}
        <div class="dbw-deposit-grid">
          <div class="dbw-qr-pane">
            {#if qrDataUrl}
              <img src={qrDataUrl} alt="BTC deposit address QR code" class="dbw-qr" />
            {:else}
              <div class="dbw-qr-empty">QR</div>
            {/if}
          </div>
          <div class="dbw-address-pane">
            <span class="dbw-field-label">Your BTC deposit address</span>
            <button type="button" class="dbw-address-btn" on:click={copyDepositAddress}>
              <span>{depositAddress}</span>
              <small>{addressCopied ? 'Copied' : 'Copy'}</small>
            </button>
            <p class="dbw-bitcoin-only-send">Only send BTC to this address. Sending anything else, or from an exchange that does not support Bitcoin withdrawals, may lose funds.</p>
          </div>
        </div>

        <div class="dbw-stats-row">
          <div class="dbw-stat">
            <span class="dbw-stat-label">Minimum deposit</span>
            <span class="dbw-stat-value">{#if minterInfoSummary}{minterInfoSummary.minDepositValue}{:else if minterInfoLoading}Loading…{:else}Unavailable{/if}</span>
          </div>
          <div class="dbw-stat">
            <span class="dbw-stat-label">Required confirmations</span>
            <span class="dbw-stat-value">{#if minterInfoSummary}{minterInfoSummary.minConfirmationsValue}{:else if minterInfoLoading}Loading…{:else}Unavailable{/if}</span>
          </div>
        </div>

        {#if mintStepIndex < 2 && !isPolling}
          <button class="dbw-btn dbw-btn--primary dbw-btn--full" type="button" on:click={beginSentBitcoinFlow}>I sent the BTC</button>
        {:else}
          <div class="dbw-poll-status">
            <p><strong>{confirmationDisplay.statusLabel}</strong></p>
            {#if confirmationDisplay.meter}
              <div class="dbw-meter-track"><div class="dbw-meter-fill" style="width:{confirmationMeterPercent(confirmationDisplay.meter)}%"></div></div>
              <p class="dbw-hint">{confirmationDisplay.meter.confirmations}/{confirmationDisplay.meter.requiredConfirmations} confirmations</p>
            {/if}
            {#if confirmationDisplay.amountDetectedLabel}<p class="dbw-hint">Detected: {confirmationDisplay.amountDetectedLabel}{#if confirmationDisplay.utxoCountLabel} ({confirmationDisplay.utxoCountLabel}){/if}</p>{/if}
            {#if confirmationDisplay.nextCheckLabel}<p class="dbw-hint">{confirmationDisplay.nextCheckLabel}</p>{/if}
            {#if pollFatalMessage}<p class="dbw-error" role="alert">{pollFatalMessage}</p>{/if}
            {#if pollingStopped && !mintedSummary}
              <button class="dbw-btn dbw-btn--secondary" type="button" on:click={recheckAfterTimeout}>Check again</button>
            {/if}
          </div>
        {/if}

        {#if mintedSummary}
          <p class="dbw-success">Minted {satoshiToBitcoin(mintedSummary.satoshiAmount ?? 0n)} BTC worth of ckBTC into your wallet.</p>
        {/if}



        {#if canOfferExistingBalance}
          <div class="dbw-recovery-card">
            <p>We have not detected a new deposit from this session, but this wallet holds {formatNumber(satoshiToBitcoin(walletCkbtcBalanceSatoshi), 8)} ckBTC. Choose whether to use this existing ckBTC as collateral.</p>
            <button class="dbw-btn dbw-btn--secondary" type="button" on:click={useExistingBalance}>Use existing ckBTC balance</button>
          </div>
        {/if}

        {#if resolvedCollateralBitcoin > 0}
          <p class="dbw-hint">
            Collateral ready for borrowing: {formatNumber(resolvedCollateralBitcoin, 8)} ckBTC{collateralResolution.source === 'existing_balance_opt_in' ? ' (from your explicitly selected wallet balance)' : ''}
            {#if collateralResolution.feeReservedSatoshi > 0n}
              (reserve {formatNumber(feeReservedBitcoin, 8)} ckBTC for two ledger fees of {formatNumber(satoshiToBitcoin(ckbtcLedgerFeeSatoshi), 8)} each, from your {formatNumber(satoshiToBitcoin(sourceBalanceSatoshi), 8)} ckBTC)
            {/if}
          </p>
          <button class="dbw-btn dbw-btn--primary dbw-btn--full" type="button" on:click={proceedToConfirm}>Continue to confirm borrow</button>
        {:else if collateralAllConsumedByFees}
          <p class="dbw-error" role="alert">
            Your {formatNumber(satoshiToBitcoin(sourceBalanceSatoshi), 8)} ckBTC is too small to cover the network fees
            required to deposit it as collateral (about {formatNumber(feeReservedBitcoin, 8)} ckBTC). Send a larger
            amount of BTC to use this flow.
          </p>
        {/if}

        <p class="dbw-panel-sub">
          Changed your mind about borrowing? Your deposit address does not expire, but minting will not happen on its
          own while this tab stays closed — come back here (or the <a href="/bitcoin">Bitcoin minter</a>) and check again
          once it is confirmed. From there you can hold the ckBTC, send it back to a Bitcoin address, or finish
          borrowing later.
        </p>
      {/if}
    {:else if step === 'confirm'}
      <h2>Confirm and borrow</h2>

      {#if hasUnresolvedPendingAction}
        <p class="dbw-panel-sub" role="status">
          An action for this account may already be in progress, possibly from another tab or a prior attempt that
          did not finish loading. Check its real on-chain state before continuing.
        </p>
        <button class="dbw-btn dbw-btn--secondary dbw-btn--full" type="button" disabled={actionInProgress} on:click={recheckOutcome}>
          {actionInProgress ? 'Checking…' : 'Recheck on-chain'}
        </button>
      {:else if outcome && outcome.kind === 'partial_zero_debt'}
        <p class="dbw-error" role="alert">{outcome.message}</p>
        <p class="dbw-panel-sub">Your BTC collateral is safely locked in vault #{outcome.vaultId}. Finish the borrow below, or come back later; nothing will be created again.</p>
        <button class="dbw-btn dbw-btn--primary dbw-btn--full" type="button" disabled={actionInProgress} on:click={finishBorrowOnVault}>
          {actionInProgress ? 'Borrowing…' : `Finish borrowing ${formatNumber(confirmIcusdAmount)} icUSD`}
        </button>
      {:else if outcome && outcome.kind === 'ambiguous_pending'}
        <p class="dbw-panel-sub" role="status">{outcome.message}</p>
        <button class="dbw-btn dbw-btn--secondary dbw-btn--full" type="button" disabled={actionInProgress} on:click={recheckOutcome}>
          {actionInProgress ? 'Checking…' : 'Recheck on-chain'}
        </button>
      {:else if outcome && outcome.kind === 'failed'}
        <p class="dbw-error" role="alert">{outcome.message}</p>
        <div class="dbw-signin-actions">
          <button class="dbw-btn dbw-btn--secondary" type="button" on:click={backToAmounts}>Back to amounts</button>
          <button class="dbw-btn dbw-btn--primary" type="button" on:click={retryConfirm}>Try again</button>
        </div>
      {:else if !finalTermsLoaded}
        <p class="dbw-panel-sub" aria-live="polite">{finalTermsError || 'Refreshing live terms before you borrow…'}</p>
      {:else if finalRisk}
        <div class="dbw-intent-strip">
          <div><span class="dbw-intent-label">Collateral to lock</span><strong>{formatNumber(resolvedCollateralBitcoin, 8)} ckBTC</strong></div>
          <div><span class="dbw-intent-label">Borrow</span><strong>{formatNumber(confirmIcusdAmount)} icUSD</strong></div>
        </div>
        <p class="dbw-hint">Reserve {formatNumber(feeReservedBitcoin, 8)} ckBTC for two ledger fees ({formatNumber(satoshiToBitcoin(ckbtcLedgerFeeSatoshi), 8)} ckBTC each).</p>

        <div class="dbw-result-strip">
          <div class="dbw-result-item">
            <span class="dbw-result-label">Collateral ratio</span>
            <span class="dbw-result-value">{finalRisk.collateralRatioPct === Infinity ? 'no debt' : `${formatNumber(finalRisk.collateralRatioPct)}%`}</span>
          </div>
          <div class="dbw-result-item">
            <span class="dbw-result-label">icUSD received</span>
            <span class="dbw-result-value">{formatNumber(finalRisk.icusdReceived, 4)} icUSD</span>
          </div>
          <div class="dbw-result-item">
            <span class="dbw-result-label">Liquidation price</span>
            <span class="dbw-result-value">${formatNumber(finalRisk.liquidationPriceUsd, 4)}</span>
          </div>
        </div>

        {#if !finalRisk.isValidCr}
          <p class="dbw-error" role="alert">
            At the current live price, this collateral ratio is below the {formatNumber(minimumCr * 100)}% minimum.
            Lower the icUSD amount or go back and send more BTC.
          </p>
        {/if}

        {#if termsChanged && !termsConfirmed}
          <div class="dbw-recovery-card">
            <p>BTC moved since you started. These are the refreshed numbers above, not your original Step 1 estimate.</p>
            <button class="dbw-btn dbw-btn--secondary" type="button" on:click={acknowledgeUpdatedTerms}>I see the updated terms, continue</button>
          </div>
        {/if}

        {#if confirmError}<p class="dbw-error" role="alert">{confirmError}</p>{/if}
        {#if ingressRecoveryWarning}<p class="dbw-error" role="alert">{ingressRecoveryWarning}</p>{/if}

        <button class="dbw-btn dbw-btn--primary dbw-btn--full" type="button" disabled={!canConfirm} on:click={confirmAndBorrow}>
          {actionInProgress ? 'Opening vault…' : 'Confirm and open vault'}
        </button>
      {/if}
    {:else if step === 'done'}
      <h2>Your BTC position</h2>
      <p class="dbw-success">
        Vault #{outcome?.vaultId ?? intent?.vaultId} opened. Confirmed collateral: {formatNumber(confirmedCollateralBitcoin, 8)} ckBTC
        (about the same amount of BTC deposited). Debt on the vault is
        {formatNumber(confirmedDoneDebt)} icUSD
        {#if finalRisk}({formatNumber(finalRisk.icusdReceived, 4)} icUSD estimated wallet receipt after the borrowing fee){/if}.
      </p>

      {#if doneCkbtcVaults.length > 0}
        <div class="dbw-vault-list">
          {#each doneCkbtcVaults as vault (vault.vaultId)}
            <VaultCard {vault} icpPrice={0} bitcoinOnly={true} expandedVaultId={doneExpandedVaultId} on:updated={handleDoneVaultUpdated} on:toggle={handleDoneVaultToggle} />
          {/each}
        </div>
      {/if}

      <div class="dbw-signin-actions">
        <a class="dbw-btn dbw-btn--secondary" href="/bitcoin">Mint or redeem Bitcoin</a>
        <button class="dbw-btn dbw-btn--secondary" type="button" on:click={resetFlow}>Do it again</button>
      </div>
    {/if}
  </div>
</div>

<style>
  .dbw-page { max-width: 720px; margin: 0 auto; padding: 0 1rem 3rem; }

  .dbw-hero { text-align: center; margin-bottom: 1.5rem; }
  .dbw-hero-logo { display: flex; align-items: center; justify-content: center; margin: 0 auto 0.75rem; }
  .dbw-bitcoin-mark { width: 72px; height: 72px; display: grid; place-items: center; border-radius: 50%; background: #f7931a; color: #fff; font-size: 2.6rem; font-weight: 700; }
  .dbw-hero h1 {
    font-family: inherit;
    font-weight: 700; font-size: 1.9rem; margin: 0.25rem 0; color: var(--rumi-text-primary);
  }
  .dbw-subtitle { font-size: 1rem; color: var(--rumi-text-secondary); margin: 0; font-family: 'Inter', sans-serif; }

  .dbw-stepper { display: flex; align-items: center; justify-content: center; gap: 0.5rem; margin-bottom: 1rem; }
  .dbw-step { display: flex; flex-direction: column; align-items: center; gap: 0.25rem; opacity: 0.5; }
  .dbw-step.is-active, .dbw-step.is-done { opacity: 1; }
  .dbw-step-circle {
    width: 1.75rem; height: 1.75rem; border-radius: 50%; display: flex; align-items: center; justify-content: center;
    background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border); font-size: 0.8125rem; font-weight: 700;
    color: var(--rumi-text-secondary);
  }
  .dbw-step.is-active .dbw-step-circle { border-color: var(--rumi-action); color: var(--rumi-text-primary); }
  .dbw-step.is-done .dbw-step-circle { background: var(--rumi-action-dim); border-color: var(--rumi-action); color: var(--rumi-action); }
  .dbw-step-label { font-size: 0.6875rem; color: var(--rumi-text-secondary); }
  .dbw-step-line { width: 1.5rem; height: 1px; background: var(--rumi-border); }

  .dbw-signed-in-strip {
    text-align: center; font-size: 0.8125rem; color: var(--rumi-text-secondary); margin-bottom: 0.75rem;
  }
  .dbw-signed-in-strip code { color: var(--rumi-text-primary); }
  .dbw-link-btn {
    background: none; border: none; color: var(--rumi-action); font-size: 0.8125rem; cursor: pointer; padding: 0;
    text-decoration: underline; margin-left: 0.375rem;
  }
  .dbw-link-btn:disabled { color: var(--rumi-text-muted); cursor: not-allowed; text-decoration: none; }
  .dbw-back-link { margin: 0 0 0.25rem; display: inline-block; }

  .dbw-vault-list { display: flex; flex-direction: column; gap: 0.5rem; }

  .dbw-panel {
    background: var(--rumi-bg-surface1); border: 1px solid var(--rumi-border); border-radius: 0.75rem;
    padding: 1.5rem; display: flex; flex-direction: column; gap: 1rem;
  }
  .dbw-panel h2 { margin: 0 0 -0.5rem; font-size: 1.125rem; color: var(--rumi-text-primary); }
  .dbw-panel-sub { margin: 0; font-size: 0.8125rem; color: var(--rumi-text-secondary); line-height: 1.5; }

  .dbw-row { position: relative; display: flex; flex-direction: column; gap: 0.375rem; }
  .dbw-row--annotated { overflow: visible; }
  .dbw-field-label { font-size: 0.8125rem; font-weight: 500; color: var(--rumi-text-secondary); }
  .dbw-field-label-row { display: flex; justify-content: space-between; align-items: baseline; }
  .dbw-input-wrap { position: relative; }
  .dbw-input {
    width: 100%; padding: 0.625rem 3.5rem 0.625rem 0.75rem; border-radius: 0.5rem;
    border: 1px solid var(--rumi-border); background: var(--rumi-bg-surface2); color: var(--rumi-text-primary);
    font-family: 'Inter', sans-serif; font-variant-numeric: tabular-nums;
  }
  .dbw-input-suffix {
    position: absolute; right: 0.75rem; top: 50%; transform: translateY(-50%);
    font-size: 0.8125rem; color: var(--rumi-text-muted);
  }
  .dbw-max-btn {
    font-size: 0.6875rem; font-weight: 600; color: var(--rumi-text-muted);
    background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border); border-radius: 0.25rem;
    padding: 0.125rem 0.375rem; cursor: pointer;
  }
  .dbw-hint { margin: 0; font-size: 0.75rem; color: var(--rumi-text-secondary); }
  .dbw-hint--warn { color: var(--rumi-caution); }
  .dbw-note { margin: 0; font-size: 0.75rem; color: var(--rumi-text-secondary); font-style: italic; }

  .dbw-result-strip {
    display: grid; grid-template-columns: repeat(2, 1fr); gap: 0.75rem;
    background: var(--rumi-bg-surface2); border-radius: 0.5rem; padding: 0.875rem;
  }
  .dbw-result-item { display: flex; flex-direction: column; gap: 0.125rem; }
  .dbw-result-label { font-size: 0.6875rem; color: var(--rumi-text-secondary); }
  .dbw-result-value {
    font-family: 'Inter', sans-serif; font-weight: 700; font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
  }
  .dbw-band-safe { color: var(--rumi-safe); }
  .dbw-band-caution { color: var(--rumi-caution); }
  .dbw-band-danger { color: var(--rumi-danger); }

  .dbw-downside { display: flex; flex-direction: column; gap: 0.5rem; }
  .dbw-slider { width: 100%; accent-color: var(--rumi-action); }

  .dbw-risk { margin: 0; font-size: 0.75rem; color: var(--rumi-text-secondary); line-height: 1.5; }
  .dbw-error {
    margin: 0; padding: 0.625rem; background: rgba(224,107,159,0.1); border: 1px solid rgba(224,107,159,0.2);
    border-radius: 0.5rem; font-size: 0.8125rem; color: #e881a8;
  }
  .dbw-success {
    margin: 0; padding: 0.625rem; background: rgba(45,212,191,0.1); border: 1px solid rgba(45,212,191,0.2);
    border-radius: 0.5rem; font-size: 0.8125rem; color: #5eead4;
  }

  .dbw-btn {
    font-family: 'Inter', sans-serif; font-weight: 700; font-size: 0.875rem; border-radius: 0.5rem;
    padding: 0.75rem 1rem; cursor: pointer; border: none; text-align: center; text-decoration: none;
    display: inline-block;
  }
  .dbw-btn--full { width: 100%; }
  .dbw-btn--primary { background: var(--rumi-action); color: var(--rumi-bg-primary); }
  .dbw-btn--primary:disabled { opacity: 0.5; cursor: not-allowed; }
  .dbw-btn--secondary { background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border-hover); color: var(--rumi-text-primary); }

  .dbw-bitcoin-only-hint { margin: 0; font-size: 0.75rem; color: var(--rumi-text-secondary); text-align: center; }
  .dbw-bitcoin-only-hint a, .dbw-panel-sub a { color: var(--rumi-action); }

  .dbw-signin-actions { display: flex; flex-direction: column; gap: 0.625rem; }
  .dbw-disclosure { font-size: 0.8125rem; color: var(--rumi-text-secondary); }
  .dbw-disclosure summary { cursor: pointer; color: var(--rumi-text-primary); font-weight: 600; }
  .dbw-disclosure p { margin: 0.5rem 0 0; line-height: 1.5; }

  .dbw-intent-strip {
    display: grid; grid-template-columns: 1fr 1fr; gap: 1rem; padding: 0.875rem;
    border: 1px solid var(--rumi-border); border-radius: 0.5rem; background: rgba(255,255,255,0.02);
  }
  .dbw-intent-label { display: block; font-size: 0.6875rem; color: var(--rumi-text-secondary); text-transform: uppercase; letter-spacing: 0.04em; }
  .dbw-intent-strip strong { font-family: 'Inter', sans-serif; font-variant-numeric: tabular-nums; font-size: 1rem; }

  .dbw-deposit-grid { display: grid; grid-template-columns: 180px 1fr; gap: 1rem; }
  .dbw-qr-pane {
    display: grid; place-items: center; min-height: 180px; border: 1px solid rgba(45,212,191,0.18);
    border-radius: 0.5rem; background: var(--rumi-bg-surface2);
  }
  .dbw-qr { width: 160px; height: 160px; padding: 0.375rem; border-radius: 0.5rem; background: #fff; }
  .dbw-qr-empty { display: grid; place-items: center; width: 160px; height: 160px; border: 1px dashed var(--rumi-border-hover); color: var(--rumi-text-muted); }
  .dbw-address-pane { display: flex; flex-direction: column; gap: 0.625rem; min-width: 0; }
  .dbw-address-btn {
    display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; width: 100%;
    padding: 0.75rem; border: 1px solid var(--rumi-border-hover); border-radius: 0.5rem;
    background: var(--rumi-bg-surface2); color: var(--rumi-text-primary); font-family: 'Inter', sans-serif; font-size: 0.8125rem;
    text-align: left; cursor: pointer;
  }
  .dbw-address-btn span { min-width: 0; overflow-wrap: anywhere; }
  .dbw-address-btn small { color: var(--rumi-action); font-weight: 700; flex-shrink: 0; }
  .dbw-bitcoin-only-send { margin: 0; font-size: 0.75rem; color: var(--rumi-danger); }

  .dbw-stats-row { display: flex; gap: 1.5rem; }
  .dbw-stat { display: flex; flex-direction: column; gap: 0.125rem; }
  .dbw-stat-label { font-size: 0.6875rem; color: var(--rumi-text-secondary); }
  .dbw-stat-value { font-family: 'Inter', sans-serif; font-weight: 600; color: var(--rumi-text-primary); }

  .dbw-poll-status { display: flex; flex-direction: column; gap: 0.5rem; }
  .dbw-meter-track { height: 6px; border-radius: 3px; background: var(--rumi-bg-surface3); overflow: hidden; }
  .dbw-meter-fill { height: 100%; background: var(--rumi-action); transition: width 0.3s ease; }

  .dbw-recovery-card {
    display: flex; flex-direction: column; gap: 0.625rem; padding: 0.875rem;
    border: 1px solid rgba(167,139,250,0.3); border-radius: 0.5rem; background: rgba(167,139,250,0.08);
    font-size: 0.8125rem; color: var(--rumi-text-secondary);
  }
  .dbw-recovery-card p { margin: 0; }

  /* Decorative annotation; the main interface uses restrained system typography. */

  @media (max-width: 640px) {
    .dbw-deposit-grid { grid-template-columns: 1fr; }
    .dbw-result-strip { grid-template-columns: 1fr; }
    .dbw-intent-strip { grid-template-columns: 1fr; }
  }
</style>
