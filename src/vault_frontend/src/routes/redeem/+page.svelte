<script lang="ts">
  function redemptionSubmissionPaused(): boolean {
    return true;
  }
  import { onMount } from 'svelte';
  import { walletStore as wallet } from '$lib/stores/wallet';
  import { protocolService } from '$lib/services/protocol';
  import { currentWalletType, walletSessionGeneration } from '$lib/services/auth';
  import { ApiClient } from '$lib/services/protocol/apiClient';
  import { getOisySignerAgent } from '$lib/services/oisySigner';
  import { CONFIG } from '$lib/config';
  import { formatNumber } from '$lib/utils/format';
  import ProtocolStats from '$lib/components/dashboard/ProtocolStats.svelte';
  import { resolveRoute } from '$lib/services/swapRouter';
  import { AMM_TOKENS } from '$lib/services/ammService';
  import { getVaultCrTextColor } from '$lib/utils/vaultHealth';
  import {
    ICUSD_E8S,
    acceptPreparedRedemptionOffer,
    acceptedRedemptionOfferIsCurrent,
    formatUsd,
    quoteIsFresh,
    quoteMatchesAmount,
    queueCandidatePricesFresh,
    maxRedeemableInput,
    redemptionPreflightIsFresh,
    toHumanIcusd,
    toHumanRawAmount,
    toQueueEntryViews,
    type RedemptionQueue,
    type RedemptionQuote,
    type RedemptionPreflight,
    type RedemptionOfferContext,
    type AcceptedRedemptionOffer,
  } from '$lib/utils/redemptionPreview';

  let isConnected = false;
  let icusdBalance = 0;
  let redemptionPreflight: RedemptionPreflight | null = null;
  let preflightLoading = false;
  let preflightError = '';
  let preflightRevision = 0;
  let walletPrincipal: string | null = null;
  let walletSnapshotReady = false;
  let icusdAmount = 0;
  let isLoading = true;
  let actionInProgress = false;
  let errorMessage = '';
  let successMessage = '';
  let ambiguousMessage = '';
  let ambiguousNeedsRefresh = false;

  let advisoryQueue: RedemptionQueue | null = null;
  let advisoryQuote: RedemptionQuote | null = null;
  let preparedQueue: RedemptionQueue | null = null;
  let preparedQuote: RedemptionQuote | null = null;
  let preparedContext: RedemptionOfferContext | null = null;
  let offerDeclined = false;
  let acceptedOffer: AcceptedRedemptionOffer | null = null;
  let liveOfferLoading = false;
  let liveOfferError = '';
  let liveOfferNotice = '';
  let queueLoading = false;
  let queueError = '';
  let quoteLoading = false;
  let quoteError = '';
  let quoteRevision = 0;
  let offerRevision = 0;
  let lastObservedAmountE8s = -1n;
  let quoteDebounceTimer: ReturnType<typeof setTimeout> | null = null;
  let preserveSuccessMessageOnInputReset = false;
  let freshnessTimer: ReturnType<typeof setInterval> | null = null;
  let freshnessNowMs = Date.now();
  let oisySignerPrincipal: any = null;
  let oisySignerReadyKey: string | null = null;
  let oisySignerWarmingKey: string | null = null;
  let oisySignerFailedKey: string | null = null;
  let oisySignerError = '';

  $: amountE8s = toE8s(icusdAmount);
  $: queue = preparedQueue ?? advisoryQueue;
  $: quote = preparedQuote ?? advisoryQuote;
  $: quoteSource = preparedQuote ? 'live-offer' : advisoryQuote ? 'estimate' : null;
  $: offerContext = {
    principalText: walletPrincipal ?? '',
    ledgerId: CONFIG.currentIcusdLedgerId,
    walletType: $currentWalletType,
    sessionGeneration: $walletSessionGeneration,
    networkKey: CONFIG.host,
  };
  $: queueRows = queue ? toQueueEntryViews(queue.entries) : [];
  $: firstRun = queueRows[0] ?? null;
  $: quoteFresh = quoteIsFresh(quote, queue, BigInt(freshnessNowMs) * 1_000_000n);
  $: quoteAmountMatches = quoteMatchesAmount(quote, amountE8s);
  $: liveOfferContextMatches = preparedContext !== null && offerContextsMatch(preparedContext, offerContext);
  $: liveOfferUsable = quoteSource === 'live-offer' && !!quote && quoteFresh && quoteAmountMatches && liveOfferContextMatches;
  $: preflightFresh = redemptionPreflightIsFresh(
    redemptionPreflight,
    walletPrincipal,
    CONFIG.currentIcusdLedgerId,
    $currentWalletType,
    $walletSessionGeneration,
    freshnessNowMs,
  );
  $: quoteNetAmount = quote ? toHumanRawAmount(quote.net_collateral_raw, quote.decimals) : '';
  $: quoteNetValueUsd = quote && quote.price_usd > 0
    ? Number(quote.net_collateral_raw) / 10 ** quote.decimals * quote.price_usd
    : 0;
  $: maxWalletAmount = Number(maxRedeemableInput(null, redemptionPreflight)) / Number(ICUSD_E8S);
  $: maxFirstRunAmount = firstRun
    ? Number(maxRedeemableInput(firstRun.maxInputIcusdE8s, redemptionPreflight)) / Number(ICUSD_E8S)
    : 0;
  $: exceedsFreshBalance = redemptionPreflight
    ? amountE8s > redemptionPreflight.balanceRaw
    : icusdAmount > icusdBalance;
  $: currentOisySignerKey = isConnected && $currentWalletType === 'oisy' && walletPrincipal
    ? `${walletPrincipal}|${$walletSessionGeneration}|${CONFIG.host}|${CONFIG.currentCanisterId}|${CONFIG.currentIcusdLedgerId}`
    : null;
  $: oisySignerReadyForCurrentSession = $currentWalletType !== 'oisy'
    || (!!currentOisySignerKey && oisySignerReadyKey === currentOisySignerKey);
  $: canAcceptOffer = liveOfferUsable && !offerDeclined && preflightFresh && !preflightLoading
    && !ambiguousNeedsRefresh && oisySignerReadyForCurrentSession;
  $: if (currentOisySignerKey && oisySignerReadyKey !== currentOisySignerKey
      && oisySignerWarmingKey !== currentOisySignerKey
      && oisySignerFailedKey !== currentOisySignerKey) {
    void warmOisySigner(currentOisySignerKey);
  }

  let unsubscribeWallet: (() => void) | null = null;
  unsubscribeWallet = wallet.subscribe(state => {
    const nextPrincipal = state.principal?.toText?.() ?? state.principal?.toString?.() ?? null;
    const principalChanged = walletSnapshotReady && nextPrincipal !== walletPrincipal;
    isConnected = state.isConnected;
    oisySignerPrincipal = state.principal ?? null;
    if (!state.isConnected || !nextPrincipal) {
      oisySignerReadyKey = null;
      oisySignerWarmingKey = null;
      oisySignerFailedKey = null;
      oisySignerError = '';
    }
    icusdBalance = state.tokenBalances?.ICUSD ? Number(state.tokenBalances.ICUSD.formatted) : 0;
    walletPrincipal = nextPrincipal;
    walletSnapshotReady = true;
    if (principalChanged) {
      redemptionPreflight = null;
      acceptedOffer = null;
      liveOfferNotice = 'Wallet changed. Check a live offer again before accepting it.';
      void refreshRedemptionPreflight();
    }
  });

  function currentOfferContext(): RedemptionOfferContext {
    return {
      principalText: walletPrincipal ?? '',
      ledgerId: CONFIG.currentIcusdLedgerId,
      walletType: $currentWalletType,
      sessionGeneration: $walletSessionGeneration,
      networkKey: CONFIG.host,
    };
  }

  function offerContextsMatch(a: RedemptionOfferContext, b: RedemptionOfferContext): boolean {
    return a.principalText === b.principalText && a.ledgerId === b.ledgerId
      && a.walletType === b.walletType && a.sessionGeneration === b.sessionGeneration
      && a.networkKey === b.networkKey;
  }

  async function warmOisySigner(key: string) {
    const principal = oisySignerPrincipal;
    if (!principal || key !== currentOisySignerKey) return;
    oisySignerWarmingKey = key;
    oisySignerError = '';
    try {
      await getOisySignerAgent(principal);
      if (key === currentOisySignerKey) {
        oisySignerReadyKey = key;
        oisySignerFailedKey = null;
      }
    } catch (error) {
      if (key === currentOisySignerKey) {
        oisySignerReadyKey = null;
        oisySignerFailedKey = key;
        oisySignerError = error instanceof Error ? error.message : 'Could not prepare the Oisy signer.';
      }
    } finally {
      if (oisySignerWarmingKey === key) oisySignerWarmingKey = null;
    }
  }

  function retryOisySignerWarmup() {
    if (!currentOisySignerKey) return;
    oisySignerFailedKey = null;
    void warmOisySigner(currentOisySignerKey);
  }

  function snapshotAgeLabel(timestampNs: bigint | undefined): string {
    if (timestampNs === undefined || timestampNs < 0n) return 'time unavailable';
    const ageNs = BigInt(Date.now()) * 1_000_000n - timestampNs;
    if (ageNs < 0n) return 'timestamp is ahead of this device clock';
    const seconds = Number(ageNs / 1_000_000_000n);
    if (seconds < 60) return `${seconds}s ago`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
    return `${Math.floor(seconds / 3600)}h ago`;
  }

  function offerExpiryLabel(quote: RedemptionQuote): string {
    const expiresNs = quote.quoted_at_ns + quote.quote_validity_window_ns;
    const remaining = expiresNs - BigInt(Date.now()) * 1_000_000n;
    if (remaining <= 0n) return 'expired';
    return `expires in ${Math.max(1, Math.ceil(Number(remaining) / 1_000_000_000))}s`;
  }

  function toE8s(amount: number): bigint {
    if (!Number.isFinite(amount) || amount <= 0) return 0n;
    return BigInt(Math.floor(amount * Number(ICUSD_E8S) + 1e-7));
  }

  function invalidatePreparedOffer() {
    offerRevision += 1;
    acceptedOffer = null;
    liveOfferLoading = false;
    preparedQueue = null;
    preparedQuote = null;
    preparedContext = null;
    offerDeclined = false;
  }

  function invalidateQuote(message = '') {
    quoteRevision += 1;
    advisoryQuote = null;
    invalidatePreparedOffer();
    quoteError = message;
    liveOfferError = '';
    liveOfferNotice = '';
    if (quoteDebounceTimer) clearTimeout(quoteDebounceTimer);
    quoteLoading = false;
  }

  function schedulePreview(amount: bigint) {
    invalidateQuote();
    queueError = '';
    if (preserveSuccessMessageOnInputReset) preserveSuccessMessageOnInputReset = false;
    else successMessage = '';
    if (amount <= 0n) return;
    quoteLoading = true;
    quoteDebounceTimer = setTimeout(() => {
      void refreshPreview(amount, false);
    }, 350);
  }

  $: if (amountE8s !== lastObservedAmountE8s) {
    lastObservedAmountE8s = amountE8s;
    schedulePreview(amountE8s);
  }

  function backendError(error: unknown): string {
    if (error && typeof error === 'object') {
      const variant = error as Record<string, any>;
      if ('RedemptionCapacityExceeded' in variant) {
        const maximum = variant.RedemptionCapacityExceeded?.max_input_icusd_e8s;
        return `This amount exceeds the first run's current capacity${maximum !== undefined ? ` (${toHumanIcusd(BigInt(maximum))} icUSD max)` : ''}. Reduce the amount or refresh the order after a separate redemption.`;
      }
      if ('RedemptionQuoteUnavailable' in variant) {
        return String(variant.RedemptionQuoteUnavailable || 'The backend cannot prepare a quote for this run right now.');
      }
      if ('RedemptionPriorityChanged' in variant) {
        return 'The redemption order changed before the quote could be prepared. Refresh to review the new first run.';
      }
      if ('RedemptionMinimumNotMet' in variant) {
        return 'The current net payout no longer meets the quoted minimum. Refresh the quote before redeeming.';
      }
      if ('Protocol' in variant) {
        // Redemption endpoints wrap the legacy shared ProtocolError so this
        // path can grow structured redemption-only variants without changing
        // every older protocol endpoint's stable error shape.
        return ApiClient.formatProtocolError(variant.Protocol);
      }
    }
    try {
      return ApiClient.formatProtocolError(error);
    } catch {
      return typeof error === 'string' ? error : 'The backend could not prepare this redemption quote.';
    }
  }

  function isCapacityError(message: string): boolean {
    return /capacity|exceeds.*run|run.*exceed/i.test(message);
  }

  function offerRefreshError(error: unknown): string {
    if (error && typeof error === 'object') {
      const variants = error as Record<string, any>;
      const [name] = Object.keys(variants);
      const details = variants[name];
      const retryAfterNs = details?.retry_after_ns;
      if (typeof retryAfterNs === 'bigint' && retryAfterNs > 0n) {
        const seconds = Math.max(1, Math.ceil(Number(retryAfterNs) / 1_000_000_000));
        return `A live offer is being prepared or the refresh limit was reached. Try again in about ${seconds} seconds.`;
      }
      if (typeof details?.message === 'string') return details.message;
      if (name === 'CandidateLimitExceeded') {
        return `The live order has more than ${details?.max_candidates ?? 'the allowed number of'} collateral types to refresh safely. The estimate is unchanged.`;
      }
      if (name === 'Protocol') return ApiClient.formatProtocolError(details);
    }
    try {
      return ApiClient.formatProtocolError(error);
    } catch {
      return error instanceof Error ? error.message : 'The backend could not prepare a live offer. Your estimate is unchanged.';
    }
  }

  async function checkLiveOffer(amount = amountE8s) {
    if (amount <= 0n) {
      liveOfferError = 'Enter an icUSD amount before checking a live offer.';
      return;
    }
    invalidatePreparedOffer();
    const revision = ++offerRevision;
    const requestContext = currentOfferContext();
    liveOfferLoading = true;
    liveOfferError = '';
    liveOfferNotice = '';
    try {
      const response = await protocolService.prepareRedemptionOffer(amount);
      if (revision !== offerRevision || amount !== toE8s(icusdAmount)
        || !offerContextsMatch(requestContext, currentOfferContext())) return;
      if ('Err' in response) {
        liveOfferError = offerRefreshError(response.Err);
        return;
      }

      const refreshed = response.Ok;
      preparedQueue = refreshed.queue;
      advisoryQueue = refreshed.queue;
      if ('Err' in refreshed.quote) {
        liveOfferError = backendError(refreshed.quote.Err);
        return;
      }

      const candidate = refreshed.quote.Ok;
      const nowNs = BigInt(Date.now()) * 1_000_000n;
      if (candidate.amount_e8s !== amount || !quoteIsFresh(candidate, refreshed.queue, nowNs)) {
        liveOfferError = 'The live offer was incomplete or expired before it could be reviewed. Check for a new offer.';
        return;
      }
      preparedQuote = candidate;
      preparedContext = requestContext;
      freshnessNowMs = Date.now();
    } catch (error) {
      if (revision !== offerRevision) return;
      liveOfferError = offerRefreshError(error);
    } finally {
      if (revision === offerRevision) liveOfferLoading = false;
    }
  }

  function declineLiveOffer() {
    acceptedOffer = null;
    offerDeclined = true;
    liveOfferNotice = 'Offer declined. This snapshot remains visible for reference; check a new live offer before accepting another one.';
  }

  async function refreshPreview(amount = amountE8s, userRequested = true) {
    if (quoteDebounceTimer) clearTimeout(quoteDebounceTimer);
    const revision = ++quoteRevision;
    quoteError = '';
    queueError = '';
    liveOfferError = '';
    invalidatePreparedOffer();
    queueLoading = true;
    quoteLoading = amount > 0n;
    isLoading = true;
    const requestedContext = currentOfferContext();

    try {
      const response = await protocolService.getRedemptionPreview(amount);
      if (revision !== quoteRevision || amount !== toE8s(icusdAmount)
        || !offerContextsMatch(requestedContext, currentOfferContext())) return;
      advisoryQueue = response.queue;
      advisoryQuote = amount > 0n && 'Ok' in response.estimate ? response.estimate.Ok : null;
      if ('Err' in response.estimate && amount > 0n) quoteError = backendError(response.estimate.Err);
      freshnessNowMs = Date.now();
      if (ambiguousNeedsRefresh && userRequested) {
        ambiguousNeedsRefresh = false;
        ambiguousMessage = 'The estimate and wallet checks have refreshed. Check a live offer before deciding whether to redeem.';
      }
    } catch (error) {
      if (revision !== quoteRevision) return;
      queueError = error instanceof Error ? error.message : 'Could not refresh the cached redemption estimate.';
      if (amount > 0n) quoteError = 'The cached estimate could not be refreshed. You can still check for a live offer.';
    } finally {
      if (revision === quoteRevision) {
        queueLoading = false;
        quoteLoading = false;
        isLoading = false;
      }
    }
  }

  function useMaximumForFirstRun() {
    if (maxFirstRunAmount > 0) icusdAmount = maxFirstRunAmount;
  }

  function useWalletMaximum() {
    if (maxWalletAmount > 0) icusdAmount = maxWalletAmount;
  }

  async function refreshRedemptionPreflight() {
    const revision = ++preflightRevision;
    const requestedPrincipal = walletPrincipal;
    if (!isConnected || !requestedPrincipal) {
      redemptionPreflight = null;
      preflightError = '';
      preflightLoading = false;
      return;
    }
    preflightLoading = true;
    preflightError = '';
    try {
      const snapshot = await protocolService.getRedemptionPreflight();
      if (revision !== preflightRevision || requestedPrincipal !== walletPrincipal) return;
      if (snapshot.principalText !== requestedPrincipal || snapshot.ledgerId !== CONFIG.currentIcusdLedgerId) {
        redemptionPreflight = null;
        preflightError = 'The fee and allowance snapshot belongs to a different wallet or ledger. Refresh before redeeming.';
        return;
      }
      redemptionPreflight = snapshot;
      // The component's last periodic tick may predate this async response.
      freshnessNowMs = Date.now();
    } catch (error) {
      if (revision !== preflightRevision) return;
      redemptionPreflight = null;
      preflightError = error instanceof Error ? error.message : 'Could not refresh icUSD balance, allowance, and ledger fee.';
    } finally {
      if (revision === preflightRevision) preflightLoading = false;
    }
  }

  async function refreshAfterAmbiguousResult() {
    try {
      await Promise.all([
        wallet.refreshBalance({ skipCache: true }),
        refreshRedemptionPreflight(),
        refreshPreview(amountE8s, true),
      ]);
      ambiguousNeedsRefresh = false;
    } catch (error) {
      queueError = error instanceof Error ? error.message : 'Could not refresh wallet balances and redemption quote.';
    }
  }

  // Optional AMM comparison is informational. Redemption always uses the backend quote above.
  const icusdToken = AMM_TOKENS.find(token => token.symbol === 'icUSD')!;
  const swapTargets = [
    { token: AMM_TOKENS.find(token => token.symbol === 'ckUSDT')!, symbol: 'ckUSDT', decimals: 6 },
    { token: AMM_TOKENS.find(token => token.symbol === 'ckUSDC')!, symbol: 'ckUSDC', decimals: 6 },
    { token: AMM_TOKENS.find(token => token.symbol === 'ICP')!, symbol: 'ICP', decimals: 8 },
  ];
  interface SwapQuote { symbol: string; outputHuman: number; valueUsd: number }
  let bestSwapQuote: SwapQuote | null = null;
  let swapQuoteTimer: ReturnType<typeof setTimeout> | null = null;
  let swapQuoteRevision = 0;
  $: if (icusdAmount > 0.01) scheduleSwapQuote(icusdAmount);
  $: if (icusdAmount <= 0.01) bestSwapQuote = null;

  function scheduleSwapQuote(amount: number) {
    if (swapQuoteTimer) clearTimeout(swapQuoteTimer);
    swapQuoteTimer = setTimeout(() => { void fetchSwapQuote(amount); }, 500);
  }

  async function fetchSwapQuote(amount: number) {
    const revision = ++swapQuoteRevision;
    const amountRaw = toE8s(amount);
    const candidates = await Promise.allSettled(swapTargets.map(async ({ token, symbol, decimals }) => {
      const route = await resolveRoute(icusdToken, token, amountRaw);
      const outputHuman = Number(route.estimatedOutput) / 10 ** decimals;
      const tokenUsd = symbol === 'ICP' ? (quote?.symbol === 'ICP' ? quote.price_usd : 0) : 1;
      return { symbol, outputHuman, valueUsd: outputHuman * tokenUsd };
    }));
    if (revision !== swapQuoteRevision || amount !== icusdAmount) return;
    const successes = candidates.flatMap(result => result.status === 'fulfilled' && result.value.valueUsd > 0 ? [result.value] : []);
    bestSwapQuote = successes.reduce<SwapQuote | null>((best, candidate) => !best || candidate.valueUsd > best.valueUsd ? candidate : best, null);
  }

  $: swapAdvantageUsd = bestSwapQuote && quoteNetValueUsd > 0
    ? bestSwapQuote.valueUsd - quoteNetValueUsd
    : 0;
  $: swapIsBetter = swapAdvantageUsd > 0;

  async function acceptAndRedeem() {
    if (!oisySignerReadyForCurrentSession) {
      errorMessage = 'Oisy is still preparing for this wallet session. Wait for signer preparation or retry it before approving.';
      return;
    }
    if (redemptionSubmissionPaused()) {
      errorMessage = 'Redemption submissions are paused until transfer recovery is available. No icUSD was approved or submitted.';
      return;
    }
    const accepted = acceptPreparedRedemptionOffer(
      preparedQuote,
      preparedQueue,
      amountE8s,
      currentOfferContext(),
      BigInt(Date.now()) * 1_000_000n,
    );
    if (!accepted || !preflightFresh || !redemptionPreflight) {
      acceptedOffer = null;
      liveOfferNotice = !accepted
        ? 'This live offer has expired or no longer matches the form. Check a new live offer.'
        : 'Wallet checks expired. Refresh them before accepting this offer.';
      return;
    }
    acceptedOffer = accepted;
    await handleRedeem(accepted);
  }

  async function handleRedeem(accepted: AcceptedRedemptionOffer) {
    errorMessage = '';
    successMessage = '';
    if (!isConnected) { errorMessage = 'Connect a wallet before redeeming.'; return; }
    if (amountE8s <= 0n) { errorMessage = 'Enter a valid icUSD amount.'; return; }
    if (exceedsFreshBalance) { errorMessage = 'Your live icUSD balance is below this amount plus the required ledger fee reserve.'; return; }
    if (!preparedQuote || !preparedQueue
      || !acceptedRedemptionOfferIsCurrent(accepted, preparedQuote, preparedQueue, amountE8s, currentOfferContext(), BigInt(Date.now()) * 1_000_000n)) {
      acceptedOffer = null;
      errorMessage = 'This offer is no longer current. Check a new live offer before accepting it.';
      return;
    }
    if (!preflightFresh || !redemptionPreflight) { errorMessage = 'Refresh the icUSD balance, allowance, and ledger fee before redeeming.'; return; }
    if (ambiguousNeedsRefresh) { errorMessage = 'Refresh balances and the quote before choosing whether to retry.'; return; }

    actionInProgress = true;
    try {
      const result = await protocolService.redeemQuoted({
        amount_e8s: accepted.amountE8s,
        expected_collateral_type: preparedQuote.collateral_type,
        min_net_collateral_raw: accepted.minimumNetCollateralRaw,
      }, redemptionPreflight, accepted);
      if (result.ambiguous) {
        ambiguousNeedsRefresh = true;
        ambiguousMessage = result.ambiguityStage === 'approval'
          ? 'The approval response was lost. The redemption call was not sent. Refresh balances and the quote before choosing whether to retry approval.'
          : result.error || 'The redemption response was lost after submission. The payout is unconfirmed. Refresh balances and redemption quote before choosing what to do next.';
        advisoryQueue = preparedQueue ?? advisoryQueue;
        invalidatePreparedOffer();
        return;
      }
      if (!result.success) {
        acceptedOffer = null;
        if (result.ambiguityStage === 'approval') {
          errorMessage = result.error || 'The icUSD approval was rejected. The redemption was not submitted.';
          return;
        }
        errorMessage = result.error || 'The quoted redemption was not accepted. Refresh the quote and try again.';
        advisoryQueue = preparedQueue ?? advisoryQueue;
        invalidatePreparedOffer();
        liveOfferNotice = 'The offer was not accepted by the backend. Check a new live offer before trying again.';
        return;
      }

      const receipt = result.redemption;
      if (receipt) {
        const payoutStatus = typeof receipt.payoutStatus === 'string'
          ? receipt.payoutStatus
          : Object.keys(receipt.payoutStatus ?? {})[0] ?? 'Pending';
        const block = result.blockIndex !== undefined ? ` icUSD block ${result.blockIndex}.` : '';
        successMessage = `${payoutStatus}: ${toHumanRawAmount(receipt.netCollateralRaw, receipt.decimals)} ${receipt.symbol} queued for delivery. The payout has not been credited yet.${block}${result.message ? ` ${result.message}` : ''}`;
      } else {
        successMessage = result.message || 'Redemption accepted. Refresh your wallet to check the pending payout.';
      }
      if (!result.sessionChangedAfterSubmission) {
        preserveSuccessMessageOnInputReset = true;
        icusdAmount = 0;
        advisoryQueue = preparedQueue ?? advisoryQueue;
        advisoryQuote = preparedQuote ?? advisoryQuote;
        invalidatePreparedOffer();
      } else {
        // Keep the amount and stale quote visible: a typed reply belongs to the
        // old wallet session and must not be mistaken for the newly active one.
        advisoryQueue = preparedQueue ?? advisoryQueue;
        invalidatePreparedOffer();
        liveOfferNotice = result.message || 'This redemption belongs to the prior wallet session. Verify that wallet’s queue before continuing.';
      }
      if (!result.sessionChangedAfterSubmission) {
        await wallet.refreshBalance({ skipCache: true });
        await refreshRedemptionPreflight();
        void refreshPreview(0n);
      }
    } catch (error) {
      errorMessage = error instanceof Error ? error.message : 'An unexpected error occurred while submitting the redemption.';
      advisoryQueue = preparedQueue ?? advisoryQueue;
      invalidatePreparedOffer();
      liveOfferNotice = 'The submission result is uncertain. Refresh balances and check a new live offer before another redemption.';
    } finally {
      actionInProgress = false;
    }
  }

  onMount(() => {
    void refreshPreview(0n, false);
    void refreshRedemptionPreflight();
    freshnessTimer = setInterval(() => { freshnessNowMs = Date.now(); }, 1000);
    return () => {
      if (quoteDebounceTimer) clearTimeout(quoteDebounceTimer);
      if (swapQuoteTimer) clearTimeout(swapQuoteTimer);
      if (freshnessTimer) clearInterval(freshnessTimer);
      preflightRevision += 1;
      unsubscribeWallet?.();
    };
  });
</script>

<svelte:head>
  <title>Redeem | Rumi Protocol</title>
</svelte:head>

<div class="page-container">
  <h1 class="page-title">Redeem icUSD</h1>

  <div class="page-layout">
    <!-- LEFT: Protocol stats sidebar -->
    <div class="stats-column">
      <ProtocolStats />
    </div>

    <!-- RIGHT: Action card -->
    <div class="action-column">
      <!-- Main redeem card -->
      <div class="action-card">
        <div class="card-body">
          <!-- Amount input -->
          <div>
            <label for="icusd-amount" class="input-label">icUSD Amount</label>
            <div class="input-wrap">
              <input
                id="icusd-amount"
                type="number"
                bind:value={icusdAmount}
                min="0"
                step="0.01"
                class="amount-input"
                placeholder="0.00"
                disabled={actionInProgress}
              />
              <div class="input-suffix">
                <span>icUSD</span>
              </div>
            </div>
            {#if isConnected && icusdBalance > 0}
              <div class="max-btn-row">
                <button
                  class="max-btn"
                  on:click={useWalletMaximum}
                  disabled={actionInProgress || preflightLoading || !preflightFresh || maxWalletAmount <= 0}
                  title="Uses the fresh balance, allowance, and ledger fee snapshot."
                >
                  Wallet max (fee-aware): {formatNumber(maxWalletAmount, 4)}
                </button>
                {#if firstRun}
                  <button class="max-btn" on:click={useMaximumForFirstRun} disabled={actionInProgress || preflightLoading || !preflightFresh || maxFirstRunAmount <= 0}>
                    Max for first run: {formatNumber(maxFirstRunAmount, 4)}
                  </button>
                {/if}
              </div>
              <p class="fee-buffer-note">Maximum amounts leave one current icUSD ledger fee when your allowance covers the amount, or two fees when approval is needed. The service rechecks the live balance, allowance, and fee before signing.</p>
            {/if}
          </div>

          {#if quote}
            <div class="fee-breakdown">
              <div class="offer-kind-row">
                {#if quoteSource === 'live-offer' && liveOfferUsable}
                  <strong>Live offer · not yet accepted</strong>
                  <span>Updated {snapshotAgeLabel(quote.quoted_at_ns)} · {offerExpiryLabel(quote)}</span>
                {:else if quoteSource === 'live-offer' && !liveOfferContextMatches}
                  <strong>Offer from a previous wallet or network session</strong>
                  <span>Check a new live offer before accepting it.</span>
                {:else if quoteSource === 'live-offer'}
                  <strong>Expired live offer</strong>
                  <span>Its terms cannot be accepted. Check for a new live offer.</span>
                {:else}
                  <strong>Indicative estimate</strong>
                  <span>Price data from {snapshotAgeLabel(quote.price_timestamp_ns)} · not an accepted offer</span>
                {/if}
              </div>
              <div class="fee-row muted">
                <span>RMR ({(quote.rmr * 100).toFixed(0)}%):</span>
                <span>{toHumanIcusd(quote.effective_icusd_e8s)} icUSD value</span>
              </div>
              <div class="fee-row muted">
                <span>Redemption fee ({(Number(quote.fee_e8s) / Number(quote.amount_e8s) * 100).toFixed(2)}%):</span>
                <span>{toHumanIcusd(quote.fee_e8s)} icUSD</span>
              </div>
              <div class="fee-row">
                <span>{quoteSource === 'live-offer' && liveOfferUsable ? 'Net payout in this offer:' : 'Indicative net payout:'}</span>
                <span class="value-highlight">{quoteNetAmount} {quote.symbol}</span>
              </div>
              <div class="fee-row">
                <span>Estimated value at quoted price:</span>
                <span class="value-highlight">{formatUsd(quoteNetValueUsd)}</span>
              </div>
              {#if quote.ledger_fee_raw > 0n}
                <div class="fee-row muted">
                  <span>Collateral ledger fee:</span>
                  <span>{toHumanRawAmount(quote.ledger_fee_raw, quote.decimals)} {quote.symbol}</span>
                </div>
              {/if}
            </div>
          {/if}

          {#if quoteLoading}
            <div class="msg msg-info" role="status">Refreshing the estimate…</div>
          {/if}
          {#if liveOfferLoading}
            <div class="msg msg-info" role="status">Checking current prices and preparing a live offer. This does not approve or move icUSD.</div>
          {/if}
          {#if quoteError}
            <div class="msg msg-error" role="alert">
              <span>{quoteError}</span>
              {#if isCapacityError(quoteError) && firstRun}
                <button class="inline-action" on:click={useMaximumForFirstRun} disabled={maxFirstRunAmount <= 0}>
                  Use max for first run ({formatNumber(maxFirstRunAmount, 4)} icUSD)
                </button>
              {/if}
            </div>
          {/if}
          {#if preflightLoading}
            <div class="msg msg-info" role="status">Refreshing the icUSD balance, approval allowance, and live ledger fee…</div>
          {:else if preflightError}
            <div class="msg msg-error" role="alert">{preflightError}<button class="inline-action" on:click={refreshRedemptionPreflight}>Refresh wallet checks</button></div>
          {:else if isConnected && !preflightFresh}
            <div class="msg msg-info" role="status">Wallet checks expired. Refresh them before redeeming.<button class="inline-action" on:click={refreshRedemptionPreflight}>Refresh wallet checks</button></div>
          {/if}
          {#if currentOisySignerKey && !oisySignerReadyForCurrentSession}
            <div class="msg msg-info" role="status">
              {#if oisySignerWarmingKey === currentOisySignerKey}
                Preparing the Oisy signer for this wallet session. Redemption approval stays disabled until this finishes.
              {:else}
                Oisy signer preparation failed{oisySignerError ? `: ${oisySignerError}` : ''}. Retry before approving.
                <button class="inline-action" on:click={retryOisySignerWarmup}>Retry Oisy signer preparation</button>
              {/if}
            </div>
          {/if}
          {#if quote && !quoteFresh}
            <div class="msg msg-info" role="status">This snapshot is only an estimate. Check a live offer before accepting or redeeming.</div>
          {/if}
          {#if liveOfferError}
            <div class="msg msg-error" role="alert">
              <span>{liveOfferError}</span>
              {#if isCapacityError(liveOfferError) && firstRun}
                <button class="inline-action" on:click={useMaximumForFirstRun} disabled={maxFirstRunAmount <= 0}>
                  Use max for this run ({formatNumber(maxFirstRunAmount, 4)} icUSD)
                </button>
              {/if}
            </div>
          {/if}
          {#if liveOfferNotice}
            <div class="msg msg-info" role="status">{liveOfferNotice}</div>
          {/if}
          {#if redemptionSubmissionPaused()}
            <div class="msg msg-info" role="status">Redemption submissions are paused while transfer recovery is added. Checking offers does not approve or move icUSD.</div>
          {/if}
          {#if ambiguousMessage}
            <div class="msg msg-info" role="status">
              {ambiguousMessage}
              {#if ambiguousNeedsRefresh}
                <button class="inline-action" on:click={refreshAfterAmbiguousResult}>
                  Refresh balances and quote
                </button>
              {/if}
            </div>
          {/if}

          <!-- Swap comparison banner (only shown when swapping gives more) -->
          {#if quoteSource === 'estimate' && quoteFresh && swapIsBetter && bestSwapQuote}
            <div class="swap-banner">
              <div class="swap-banner-header">
                <span class="swap-banner-icon">&#x2191;</span>
                <span class="swap-banner-title">You could get more by swapping</span>
              </div>
              <div class="swap-banner-body">
                <div class="swap-compare-row">
                  <span>Redeeming:</span>
                  <span class="swap-compare-val">{formatUsd(quoteNetValueUsd)} ({quoteNetAmount} {quote?.symbol})</span>
                </div>
                <div class="swap-compare-row highlight">
                  <span>Swapping to {bestSwapQuote.symbol}:</span>
                  <span class="swap-compare-val">~${formatNumber(bestSwapQuote.valueUsd, 2)}
                    {#if bestSwapQuote.symbol === 'ICP'}
                      ({formatNumber(bestSwapQuote.outputHuman, 4)} ICP)
                    {:else}
                      ({formatNumber(bestSwapQuote.outputHuman, 2)} {bestSwapQuote.symbol})
                    {/if}
                  </span>
                </div>
                <div class="swap-compare-row advantage">
                  <span>Advantage:</span>
                  <span class="swap-compare-val">+${formatNumber(swapAdvantageUsd, 2)}</span>
                </div>
              </div>
              <a href="/swap" class="swap-banner-link">Go to Swap &rarr;</a>
            </div>
          {/if}

          <!-- Messages -->
          {#if errorMessage}
            <div class="msg msg-error">{errorMessage}</div>
          {/if}
          {#if successMessage}
            <div class="msg msg-success">{successMessage}</div>
          {/if}

          {#if liveOfferUsable && preparedQuote}
            {#if offerDeclined}
              <button id="check-live-offer" class="submit-btn" on:click={() => checkLiveOffer(amountE8s)} disabled={liveOfferLoading || actionInProgress || amountE8s <= 0n}>
                Check a new live offer
              </button>
            {:else}
              <button
                class="submit-btn"
                on:click={acceptAndRedeem}
                disabled={redemptionSubmissionPaused() || actionInProgress || !isConnected || amountE8s <= 0n || exceedsFreshBalance || !canAcceptOffer}
              >
                {#if actionInProgress}
                  Processing accepted offer…
                {:else if redemptionSubmissionPaused()}
                  Redemptions paused
                {:else if !isConnected}
                  Connect Wallet to Accept
                {:else if preflightLoading || !preflightFresh}
                  Refresh Wallet Checks to Continue
                {:else}
                  Accept and redeem
                {/if}
              </button>
              <button class="decline-offer" on:click={declineLiveOffer} disabled={actionInProgress}>Decline</button>
              <p class="offer-acceptance-note">Accepting authorizes this exact amount, collateral, and minimum payout. Your wallet may ask you to approve icUSD before the redemption is submitted.</p>
            {/if}
          {:else}
            <button
              id="check-live-offer"
              class="submit-btn"
              on:click={() => checkLiveOffer(amountE8s)}
              disabled={liveOfferLoading || actionInProgress || amountE8s <= 0n || ambiguousNeedsRefresh}
            >
              {#if liveOfferLoading}
                Checking live offer…
              {:else}
                Check live offer
              {/if}
            </button>
          {/if}
        </div>
      </div>

      <!-- How it works (collapsed under the action card) -->
      <details class="how-it-works">
        <summary class="how-heading">How Redemption Works</summary>
        <div class="how-body">
          <ol class="how-steps">
            <li>
              <strong>Burn icUSD</strong>
              <p>The backend burns the quoted icUSD amount after checking the selected collateral run and minimum net payout.</p>
            </li>
            <li>
              <strong>Choose one queued run</strong>
              <p>Each redemption uses one consecutive run of vaults backed by the same collateral. It does not continue into the next row automatically.</p>
            </li>
            <li>
              <strong>Queue the collateral payout</strong>
              <p>Your quote names the selected token and estimated net amount after RMR, redemption fee, and collateral-ledger fee. After a successful submission, delivery is queued separately; the quote is not proof that tokens have reached your wallet.</p>
            </li>
          </ol>
          <div class="how-note">
            <p>Rows below show the current eligible order and debt-backed capacity for separate calls. The order can change as vault health or collateral prices change.</p>
          </div>
        </div>
      </details>

      <section class="redemption-queue" aria-labelledby="redemption-queue-heading">
        <div class="queue-heading-row">
          <div>
          <h2 id="redemption-queue-heading">Eligible collateral on deck</h2>
            <p class="queue-subtitle">Eligible debt-backed supported ICRC collateral, grouped into consecutive same-collateral runs in weakest-health-first order. Native XRP is not included.</p>
            {#if queue}<p class="queue-subtitle">Snapshot updated {snapshotAgeLabel(queue.observed_at_ns)}. The order may change before a live offer is prepared.</p>{/if}
          </div>
          <button class="queue-refresh" on:click={() => refreshPreview(amountE8s, true)} disabled={queueLoading || quoteLoading || liveOfferLoading}>
            Refresh estimate
          </button>
        </div>

        <div class="health-legend">
          <span class="legend-swatch" aria-hidden="true"></span>
          <span>Pink means closer to liquidation after applying that asset's own thresholds.</span>
        </div>

        {#if queueLoading && !queue}
          <div class="queue-state" role="status">Loading current redemption order…</div>
        {:else if queueError}
          <div class="queue-state queue-state-error" role="alert">{queueError} Refresh to try again.</div>
        {:else if queueLoading}
          <div class="queue-state" role="status">Refreshing the order. Rows cannot be submitted until refresh finishes.</div>
        {:else if queue && queue.entries.length === 0}
          <div class="queue-state">No debt-backed vault collateral is currently eligible for redemption.</div>
        {:else if queue}
          {#if !queueCandidatePricesFresh(queue, BigInt(freshnessNowMs) * 1_000_000n)}
            <div class="queue-state queue-state-error" role="alert">
              {#if !queue.ranking_fresh}
                The backend cannot provide a complete collateral ranking. These rows are estimates only; check a live offer to refresh the prices before accepting anything.
              {:else}
                At least one price used to rank this order is older than the allowed 10-minute price age. Check a live offer to refresh it before relying on the order.
              {/if}
            </div>
          {/if}
          <ol class="queue-list">
            {#each queueRows as row (row.runIndex)}
              <li class="queue-entry" class:first-run={row.runIndex === queueRows[0]?.runIndex} class:quoted-run={liveOfferUsable && quote?.run_index === row.runIndex}>
                <div class="queue-entry-top">
                  <div class="queue-token">
                    <span class="queue-rank">{row.runIndex + 1}</span>
                    <strong>{row.symbol}</strong>
                    {#if queueRows.slice(0, row.runIndex).some(previous => previous.symbol === row.symbol)}
                      <span class="queue-repeat">later run</span>
                    {/if}
                    {#if liveOfferUsable && quote?.run_index === row.runIndex}<span class="queue-selected">live offer</span>{/if}
                  </div>
                  <div class="queue-health">
                    <span class="health-dot" style="background:{getVaultCrTextColor(row.weakestVaultCr, row.minCr, row.liquidationCr)}"></span>
                    <span style="color:{getVaultCrTextColor(row.weakestVaultCr, row.minCr, row.liquidationCr)}">{(row.weakestVaultCr * 100).toFixed(1)}%</span>
                    <span class="queue-vault-count">{row.vaultCount} vault{row.vaultCount === 1 ? '' : 's'}</span>
                  </div>
                </div>
                <div class="queue-capacities">
                  <div>
                    <span>Max icUSD input for this call</span>
                    <strong>{toHumanIcusd(row.maxInputIcusdE8s)} icUSD</strong>
                  </div>
                  <div>
                    <span>Simulated net payout at that max</span>
                    <strong>{toHumanRawAmount(row.maxNetCollateralRaw, row.decimals)} {row.symbol}</strong>
                  </div>
                  <div>
                    <span>Collateral locked in these eligible vaults</span>
                    <strong>{toHumanRawAmount(row.lockedCollateralRaw, row.decimals)} {row.symbol}</strong>
                  </div>
                </div>
                <div class="queue-footnote">
                  {#if row.priceFresh}
                    Price {formatUsd(row.priceUsd)} · current snapshot
                  {:else}
                    Price is stale; this run cannot be quoted right now.
                  {/if}
                  · {row.runIndex === 0 ? 'This is the first run in the displayed snapshot.' : 'A later row requires its own call after the queue is refreshed.'}
                </div>
              </li>
            {/each}
          </ol>
          <p class="queue-disclaimer">Capacities are per run, not cumulative. “Collateral locked” includes vault balances that may not be redeemable; the net payout estimate is calculated from eligible debt and the current redemption rules. The queue is recalculated after each redemption.</p>
        {/if}
      </section>
    </div>
  </div>
</div>

<style>
  /* ── Page layout ───────────────────────────────────────────────── */
  .page-container {
    max-width: 820px;
    margin: 0 auto;
    padding: 0 1rem;
  }
  .page-title {
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 2rem;
    font-weight: 700;
    color: var(--rumi-purple-accent);
    letter-spacing: -0.02em;
    margin-bottom: 0.5rem;
  }
  .page-layout {
    display: grid;
    grid-template-columns: 280px 1fr;
    gap: 1.5rem;
    align-items: start;
  }
  .stats-column {
    position: sticky;
    top: 5rem;
  }
  .action-column {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  /* ── Reserve bar ───────────────────────────────────────────────── */
  .reserve-bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.625rem 1rem;
    border-radius: 0.5rem;
    background: rgba(45, 212, 191, 0.06);
    border: 1px solid rgba(45, 212, 191, 0.15);
  }
  .reserve-label {
    font-size: 0.75rem;
    font-weight: 500;
    color: #5eead4;
  }
  .reserve-amounts {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    font-size: 0.75rem;
  }
  .reserve-token {
    font-variant-numeric: tabular-nums;
    color: #d1d5db;
  }
  .reserve-sep {
    color: #4b5563;
  }
  .reserve-total {
    font-weight: 600;
    color: #5eead4;
  }

  /* ── Action card ───────────────────────────────────────────────── */
  .action-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
  }
  .card-body {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  /* ── Inputs ────────────────────────────────────────────────────── */
  .input-label {
    display: block;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-text-secondary);
    margin-bottom: 0.375rem;
  }
  .input-wrap {
    position: relative;
  }
  .amount-input {
    width: 100%;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    padding: 0.625rem 3.5rem 0.625rem 0.75rem;
    font-size: 0.9375rem;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
    outline: none;
    transition: border-color 0.15s ease;
  }
  .amount-input:focus {
    border-color: rgba(139, 92, 246, 0.5);
  }
  .amount-input:disabled {
    opacity: 0.5;
  }
  .input-suffix {
    position: absolute;
    inset: 0 0 0 auto;
    display: flex;
    align-items: center;
    padding-right: 0.75rem;
    pointer-events: none;
    font-size: 0.8125rem;
    color: var(--rumi-text-muted);
  }
  .max-btn-row {
    display: flex;
    justify-content: flex-end;
    flex-wrap: wrap;
    gap: 0.75rem;
    margin-top: 0.25rem;
  }
  .max-btn {
    font-size: 0.6875rem;
    color: #60a5fa;
    cursor: pointer;
    background: none;
    border: none;
    padding: 0;
  }
  .max-btn:hover {
    color: #93bbfd;
  }
  .fee-buffer-note {
    margin: 0.3rem 0 0;
    color: var(--rumi-text-muted);
    font-size: 0.625rem;
    line-height: 1.45;
    text-align: right;
  }

  /* Hide number input spinners */
  .amount-input::-webkit-outer-spin-button,
  .amount-input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }
  .amount-input {
    -moz-appearance: textfield;
  }

  /* ── Token selector ────────────────────────────────────────────── */
  .token-selector {
    display: flex;
    gap: 0.5rem;
  }
  .token-btn {
    flex: 1;
    padding: 0.4375rem 0.75rem;
    border-radius: 0.5rem;
    border: 1px solid rgba(107, 114, 128, 0.3);
    background: rgba(31, 41, 55, 0.4);
    color: #9ca3af;
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }
  .token-btn:hover {
    border-color: rgba(139, 92, 246, 0.4);
  }
  .token-btn.selected {
    border-color: rgba(139, 92, 246, 0.6);
    background: rgba(139, 92, 246, 0.12);
    color: #e5e7eb;
  }

  /* ── Fee breakdown ─────────────────────────────────────────────── */
  .fee-breakdown {
    padding: 0.625rem 0.75rem;
    background: var(--rumi-bg-surface2);
    border-radius: 0.5rem;
    border: 1px solid var(--rumi-border);
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .fee-row {
    display: flex;
    justify-content: space-between;
    font-size: 0.75rem;
    color: var(--rumi-text-secondary);
  }
  .fee-row.muted {
    color: var(--rumi-text-muted);
  }
  .fee-row.spillover {
    color: #fbbf24;
    margin-top: 0.25rem;
  }
  .offer-kind-row {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    margin-bottom: 0.2rem;
    font-size: 0.75rem;
  }
  .offer-kind-row strong { color: var(--rumi-text-primary); }
  .offer-kind-row span { color: var(--rumi-text-muted); font-size: 0.6875rem; }
  .offer-acceptance-note {
    margin: 0;
    color: var(--rumi-text-muted);
    font-size: 0.6875rem;
    line-height: 1.45;
  }
  .value-highlight {
    font-weight: 600;
    color: var(--rumi-text-primary);
  }

  /* ── Swap comparison banner ─────────────────────────────────────── */
  .swap-banner {
    padding: 0.75rem;
    background: rgba(45, 212, 191, 0.06);
    border: 1px solid rgba(45, 212, 191, 0.2);
    border-radius: 0.5rem;
  }
  .swap-banner-header {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    margin-bottom: 0.5rem;
  }
  .swap-banner-icon {
    font-size: 0.875rem;
    color: #5eead4;
  }
  .swap-banner-title {
    font-size: 0.8125rem;
    font-weight: 600;
    color: #5eead4;
  }
  .swap-banner-body {
    display: flex;
    flex-direction: column;
    gap: 0.1875rem;
    margin-bottom: 0.5rem;
  }
  .swap-compare-row {
    display: flex;
    justify-content: space-between;
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
  }
  .swap-compare-row.highlight {
    color: var(--rumi-text-primary);
  }
  .swap-compare-row.advantage {
    color: #5eead4;
    font-weight: 600;
  }
  .swap-compare-val {
    font-variant-numeric: tabular-nums;
  }
  .swap-banner-link {
    display: inline-block;
    font-size: 0.75rem;
    font-weight: 600;
    color: #5eead4;
    text-decoration: none;
    transition: opacity 0.15s ease;
  }
  .swap-banner-link:hover {
    opacity: 0.8;
  }

  /* ── Messages ──────────────────────────────────────────────────── */
  .msg {
    padding: 0.625rem 0.75rem;
    border-radius: 0.5rem;
    font-size: 0.8125rem;
  }
  .msg-error {
    background: rgba(224, 107, 159, 0.1);
    border: 1px solid rgba(224, 107, 159, 0.25);
    color: #e881a8;
  }
  .msg-success {
    background: rgba(45, 212, 191, 0.1);
    border: 1px solid rgba(45, 212, 191, 0.25);
    color: #5eead4;
  }
  .msg-info {
    background: rgba(139, 92, 246, 0.08);
    border: 1px solid rgba(139, 92, 246, 0.22);
    color: var(--rumi-text-secondary);
  }
  .inline-action {
    display: block;
    margin-top: 0.45rem;
    padding: 0;
    border: 0;
    background: none;
    color: #c4b5fd;
    font-weight: 600;
    text-align: left;
    cursor: pointer;
  }
  .inline-action:disabled { opacity: 0.45; cursor: not-allowed; }

  /* ── Submit button ─────────────────────────────────────────────── */
  .submit-btn {
    width: 100%;
    padding: 0.625rem 1rem;
    border-radius: 0.5rem;
    font-size: 0.875rem;
    font-weight: 600;
    color: #fff;
    background: var(--rumi-accent, #8b5cf6);
    border: none;
    cursor: pointer;
    transition: opacity 0.15s ease;
  }
  .submit-btn:hover:not(:disabled) {
    opacity: 0.9;
  }
  .submit-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .decline-offer {
    width: 100%;
    padding: 0.5rem 1rem;
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    background: var(--rumi-bg-surface2);
    color: var(--rumi-text-secondary);
    cursor: pointer;
  }
  .decline-offer:disabled { opacity: 0.45; cursor: not-allowed; }

  /* ── How it works ──────────────────────────────────────────────── */
  .how-it-works {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    overflow: hidden;
  }
  .how-heading {
    padding: 0.75rem 1rem;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-secondary);
    cursor: pointer;
    list-style: none;
  }
  .how-heading::-webkit-details-marker { display: none; }
  .how-heading::before {
    content: '▸ ';
    font-size: 0.6875rem;
  }
  .how-it-works[open] .how-heading::before { content: '▾ '; }
  .how-body {
    padding: 0 1rem 1rem;
  }
  .how-steps {
    list-style: none;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    counter-reset: step;
  }
  .how-steps li {
    counter-increment: step;
    padding-left: 2rem;
    position: relative;
  }
  .how-steps li::before {
    content: counter(step);
    position: absolute;
    left: 0;
    top: 0;
    width: 1.375rem;
    height: 1.375rem;
    border-radius: 50%;
    background: rgba(139, 92, 246, 0.2);
    color: #c4b5fd;
    font-size: 0.6875rem;
    font-weight: 600;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .how-steps strong {
    font-size: 0.8125rem;
    color: var(--rumi-text-primary);
  }
  .how-steps p {
    font-size: 0.75rem;
    color: var(--rumi-text-secondary);
    margin: 0.125rem 0 0;
    line-height: 1.4;
  }
  .how-note {
    margin-top: 0.75rem;
    padding: 0.625rem 0.75rem;
    background: var(--rumi-bg-surface2);
    border-radius: 0.5rem;
  }
  .how-note p {
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
    line-height: 1.5;
    margin: 0;
  }

  /* ── Current redemption order ──────────────────────────────────── */
  .redemption-queue {
    padding: 1rem;
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
  }
  .queue-heading-row {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
  }
  .queue-heading-row h2 {
    margin: 0;
    color: var(--rumi-text-primary);
    font-size: 0.9375rem;
    font-weight: 650;
  }
  .queue-subtitle {
    margin: 0.2rem 0 0;
    color: var(--rumi-text-muted);
    font-size: 0.6875rem;
    line-height: 1.45;
  }
  .queue-refresh {
    flex: none;
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    padding: 0.35rem 0.55rem;
    background: var(--rumi-bg-surface2);
    color: var(--rumi-text-secondary);
    font-size: 0.6875rem;
    cursor: pointer;
  }
  .queue-refresh:disabled { opacity: 0.45; cursor: wait; }
  .health-legend {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin: 0.75rem 0;
    color: var(--rumi-text-muted);
    font-size: 0.6875rem;
  }
  .legend-swatch {
    width: 0.6rem;
    height: 0.6rem;
    border-radius: 50%;
    background: #e06b9f;
  }
  .queue-state {
    padding: 0.75rem;
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    color: var(--rumi-text-secondary);
    font-size: 0.75rem;
  }
  .queue-state-error {
    color: #e881a8;
    border-color: rgba(224, 107, 159, 0.25);
  }
  .queue-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .queue-entry {
    padding: 0.75rem;
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    background: var(--rumi-bg-surface2);
  }
  .queue-entry.first-run {
    border-color: rgba(167, 139, 250, 0.38);
  }
  .queue-entry.quoted-run {
    box-shadow: inset 2px 0 0 #a78bfa;
  }
  .queue-entry-top, .queue-token, .queue-health {
    display: flex;
    align-items: center;
  }
  .queue-entry-top { justify-content: space-between; gap: 0.75rem; }
  .queue-token { min-width: 0; gap: 0.45rem; color: var(--rumi-text-primary); font-size: 0.8125rem; }
  .queue-rank {
    display: inline-flex;
    width: 1.25rem;
    height: 1.25rem;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    background: rgba(139, 92, 246, 0.2);
    color: #c4b5fd;
    font-size: 0.6875rem;
    font-variant-numeric: tabular-nums;
  }
  .queue-repeat, .queue-selected {
    padding: 0.1rem 0.35rem;
    border-radius: 0.25rem;
    background: rgba(107, 114, 128, 0.16);
    color: var(--rumi-text-muted);
    font-size: 0.625rem;
    font-weight: 500;
  }
  .queue-selected { background: rgba(139, 92, 246, 0.16); color: #c4b5fd; }
  .queue-health { flex: none; gap: 0.35rem; font-size: 0.75rem; font-variant-numeric: tabular-nums; }
  .health-dot { width: 0.5rem; height: 0.5rem; border-radius: 50%; }
  .queue-vault-count { color: var(--rumi-text-muted); font-size: 0.6875rem; }
  .queue-capacities {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 0.5rem;
    margin-top: 0.65rem;
  }
  .queue-capacities div { display: flex; flex-direction: column; gap: 0.18rem; min-width: 0; }
  .queue-capacities span { color: var(--rumi-text-muted); font-size: 0.625rem; line-height: 1.35; }
  .queue-capacities strong { color: var(--rumi-text-primary); font-size: 0.6875rem; font-weight: 600; font-variant-numeric: tabular-nums; overflow-wrap: anywhere; }
  .queue-footnote, .queue-disclaimer {
    margin: 0.55rem 0 0;
    color: var(--rumi-text-muted);
    font-size: 0.625rem;
    line-height: 1.45;
  }
  .queue-disclaimer { margin-top: 0.75rem; }

  /* ── Responsive ────────────────────────────────────────────────── */
  @media (max-width: 768px) {
    .page-layout {
      grid-template-columns: 1fr;
    }
    .stats-column {
      position: static;
      order: 2;
    }
    .action-column {
      order: 1;
    }
    .queue-capacities { grid-template-columns: 1fr; gap: 0.35rem; }
    .queue-entry-top { align-items: flex-start; }
    .queue-health { flex-wrap: wrap; justify-content: flex-end; }
  }
</style>
