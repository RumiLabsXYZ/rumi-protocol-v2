<script lang="ts">
  import { createEventDispatcher, onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import QRCode from 'qrcode';
  import type { CollateralInfo } from '$lib/services/types';
  import { protocolService } from '$lib/services/protocol';
  import {
    XrpVaultService,
    type XrpVaultOpenView,
  } from '$lib/services/xrpVaultService';
  import {
    buildXrpPaymentUri,
    nativeXrpBorrowErrorCopy,
    nativeXrpBorrowLaterCopy,
    nativeXrpBorrowSeparateApprovalCopy,
    nativeXrpDepositCopy,
    nativeXrpModalOpeningCopy,
    nativeXrpModalPrimaryActionLabel,
    nativeXrpModalShouldRender,
    nativeXrpModalStatusLabel,
    nativeXrpModalTitle,
    type NativeXrpBorrowPhase,
  } from '$lib/utils/nativeXrpBorrowFlow';
  import { formatAddress } from '$lib/utils/format';
  import { WALLET_TYPES, currentWalletType } from '$lib/services/auth';
  import { walletStore } from '$lib/stores/wallet';
  import XrplWalletConnect from '$components/wallet/XrplWalletConnect.svelte';
  import { getXrplWalletAdapter, type WalletSigningPrompt } from '$lib/services/xrplWalletService';
  import { formatDropsAsXrp, xrpNumberToDrops } from '$lib/utils/nativeXrpBorrowFlow';
  import {
    createDefaultXrpDepositController,
    type XrpDepositController,
    type XrpDepositState,
  } from './xrpDepositFlow';

  export let collateralAmount: number;
  export let icusdAmount: number;
  export let collateralInfo: CollateralInfo | undefined;

  const dispatch = createEventDispatcher<{
    close: void;
    complete: { vaultId: number; oisyResilient?: boolean };
  }>();

  let phase: NativeXrpBorrowPhase = 'opening';
  let opened: XrpVaultOpenView | null = null;
  let qrDataUrl = '';
  let errorMessage = '';
  let copied = false;
  let started = false;
  let confirmWasResilient = false;

  $: copy = nativeXrpDepositCopy({
    collateralAmount,
    icusdAmount,
    collateralInfo,
    reserveBaseDrops: opened?.reserveBaseDrops ?? 0n,
  });
  $: paymentUri = opened ? buildXrpPaymentUri(opened.custodyAddress, copy.sendDrops ?? copy.sendAmount) : '';
  $: hasDepositAddress = Boolean(opened?.custodyAddress);
  $: shouldRenderModal = nativeXrpModalShouldRender(phase, hasDepositAddress);
  $: modalTitle = nativeXrpModalTitle(phase, hasDepositAddress);
  $: modalStatusLabel = nativeXrpModalStatusLabel(phase);
  $: modalPrimaryActionLabel = nativeXrpModalPrimaryActionLabel(phase, hasDepositAddress, copy.borrowAmountLabel);
  $: isBusy = phase === 'opening' || phase === 'confirming' || phase === 'borrowing';
  $: awaitingBorrowApproval = phase === 'ready_to_borrow' || phase === 'borrow_failed';

  // Oisy opens its signer popup only from inside a browser user-gesture. The
  // confirm call's XRPL verification round-trip burns that gesture, so chaining
  // the borrow onto it always trips "Signer window should not be opened outside
  // of click handler". Wallets that sign in-page (Internet Identity) have no
  // such constraint, so they keep the single-click flow.
  $: borrowNeedsOwnClick = $currentWalletType === WALLET_TYPES.OISY;

  // Wallet click-to-deposit controller. It owns the single open_xrp_vault call,
  // reload recovery, the frozen payment snapshot, readiness polling and the
  // Oisy-vs-delegated confirm rule. The manual address/QR path stays available.
  let controller: XrpDepositController | null = null;
  let dep: XrpDepositState | null = null;
  let unsubscribeController: (() => void) | null = null;
  let unsubscribePrompt: (() => void) | null = null;
  let signingPrompt: WalletSigningPrompt | null = null;
  let promptQr = '';
  let advanced = false;

  $: walletBusy = dep?.phase === 'signing' || dep?.phase === 'confirming' || dep?.phase === 'polling';
  $: walletDepositDone = dep?.phase === 'confirmed';
  $: canWalletDeposit = !!dep && ['awaiting_payment', 'invalidated', 'rejected'].includes(dep.phase);

  onMount(() => {
    void openVault();
  });

  onDestroy(() => {
    unsubscribePrompt?.();
    unsubscribeController?.();
    controller?.dispose();
  });

  function ownerKey(): string {
    try {
      return get(walletStore).principal?.toText?.() ?? 'anonymous';
    } catch {
      return 'anonymous';
    }
  }

  function onDepState(s: XrpDepositState) {
    dep = s;
    opened = s.vault;
    const borrowPhases = ['borrowing', 'borrow_failed', 'ready_to_borrow'];
    if (borrowPhases.includes(phase)) return;
    if (s.vault && (phase === 'opening' || phase === 'confirming') && s.phase !== 'confirming') {
      phase = 'awaiting';
    }
    if (s.phase === 'confirming') phase = 'confirming';
    if (s.phase === 'needs_open_confirmation' && !s.vault) {
      // Pending state unknown (signer wallet, empty cache): nothing opened until the user says so.
      phase = 'error';
      errorMessage = s.error ?? '';
      return;
    }
    if (s.phase === 'error' && !s.vault) {
      phase = 'error';
      errorMessage = s.error ?? 'Could not reserve an XRP custody address.';
      return;
    }
    errorMessage = s.error ?? '';
    if (s.phase === 'confirmed' && !advanced) {
      advanced = true;
      confirmWasResilient = s.oisyResilient;
      if (borrowNeedsOwnClick) {
        phase = 'ready_to_borrow';
      } else {
        void borrowFromConfirmedVault(confirmWasResilient);
      }
    }
  }

  async function openVault() {
    if (started) return;
    started = true;
    phase = 'opening';
    errorMessage = '';
    try {
      const sendDrops = copy.sendDrops;
      if (sendDrops === null || sendDrops <= 0n) {
        phase = 'error';
        errorMessage = 'Enter an XRP amount with at most 6 decimal places.';
        return;
      }
      const minDeposit = collateralInfo ? xrpNumberToDrops(collateralInfo.minCollateralDeposit) : null;
      controller = createDefaultXrpDepositController(ownerKey(), minDeposit && minDeposit > 0n ? minDeposit : 1n);
      unsubscribeController = controller.subscribe(onDepState);
      unsubscribePrompt = getXrplWalletAdapter().onPrompt((p) => {
        signingPrompt = p;
        promptQr = p.qrPng ?? '';
      });
      await controller.prepare(sendDrops);
      if (controller.getState().vault) {
        const v = controller.getState().vault!;
        await generateQr(buildXrpPaymentUri(v.custodyAddress, controller.getState().sentDrops ?? sendDrops));
      }
    } catch (err) {
      phase = 'error';
      errorMessage = err instanceof Error ? err.message : 'Could not reserve an XRP custody address.';
    }
  }

  /** The pending-deposit check failed (nothing was opened): ask the backend again. */
  async function retryPrepare() {
    if (!controller) return;
    const sendDrops = copy.sendDrops;
    if (sendDrops === null || sendDrops <= 0n) return;
    phase = 'opening';
    errorMessage = '';
    try {
      await controller.prepare(sendDrops);
      const v = controller.getState().vault;
      if (v) await generateQr(buildXrpPaymentUri(v.custodyAddress, controller.getState().sentDrops ?? sendDrops));
    } catch (err) {
      phase = 'error';
      errorMessage = err instanceof Error ? err.message : 'Could not reserve an XRP custody address.';
    }
  }

  /** Explicit confirmation that no unfinished XRP deposit exists: only then open a new vault. */
  async function acknowledgeAndOpen() {
    if (!controller) return;
    const sendDrops = copy.sendDrops;
    if (sendDrops === null || sendDrops <= 0n) return;
    phase = 'opening';
    errorMessage = '';
    try {
      await controller.acknowledgeNoPendingAndOpen();
      const v = controller.getState().vault;
      if (v) await generateQr(buildXrpPaymentUri(v.custodyAddress, controller.getState().sentDrops ?? sendDrops));
    } catch (err) {
      phase = 'error';
      errorMessage = err instanceof Error ? err.message : 'Could not reserve an XRP custody address.';
    }
  }

  async function depositFromWallet() {
    if (!controller) return;
    controller.propose();
    await controller.pay();
  }

  async function resumeWalletDeposit() {
    await controller?.resume();
  }

  // Explicit click only. For Oisy this is the sole place a signer-backed
  // confirm is ever started; timers never call it.
  async function confirmCollateralClick() {
    await controller?.confirmNow();
  }

  async function generateQr(uri = paymentUri) {
    if (!uri) return;
    try {
      qrDataUrl = await QRCode.toDataURL(uri, {
        width: 196,
        margin: 2,
        color: { dark: '#020617', light: '#ffffff' },
        errorCorrectionLevel: 'M',
      });
    } catch (err) {
      console.error('XRP QR generation failed:', err);
      qrDataUrl = '';
    }
  }

  async function copyAddress() {
    if (!opened) return;
    try {
      await navigator.clipboard?.writeText(opened.custodyAddress);
      copied = true;
      setTimeout(() => { copied = false; }, 1600);
    } catch {
      copied = false;
    }
  }

  async function confirmDepositAndBorrow() {
    if (!opened || !controller) return;
    errorMessage = '';
    // confirmNow() settles the controller state; onDepState() advances to the borrow step.
    await controller.confirmNow();
    const s = controller.getState();
    if (s.phase !== 'confirmed') {
      phase = 'awaiting';
      errorMessage = s.error ?? 'Deposit not detected yet.';
    }
  }

  async function borrowFromConfirmedVault(confirmResilient = confirmWasResilient) {
    if (!opened) return;
    phase = 'borrowing';
    errorMessage = '';
    try {
      const borrowed = await protocolService.borrowFromVault(opened.vaultId, icusdAmount);
      if (!borrowed.success) {
        phase = 'borrow_failed';
        errorMessage = nativeXrpBorrowErrorCopy(borrowed.error, copy.borrowAmountLabel);
        return;
      }
      dispatch('complete', {
        vaultId: opened.vaultId,
        oisyResilient: confirmResilient || borrowed.oisyResilient,
      });
    } catch (err) {
      // The collateral is already credited, so a thrown signer error must land
      // in the recoverable borrow_failed state, never back at 'awaiting'.
      phase = 'borrow_failed';
      errorMessage = nativeXrpBorrowErrorCopy(
        err instanceof Error ? err.message : undefined,
        copy.borrowAmountLabel
      );
    }
  }

  function close() {
    if (!isBusy) dispatch('close');
  }
</script>

{#if shouldRenderModal}
<div class="xrp-modal-shell" role="presentation">
  <button class="modal-backdrop" type="button" aria-label="Close XRP deposit flow" disabled={isBusy} on:click={close}></button>
  <div class="xrp-modal" role="dialog" aria-modal="true" aria-labelledby="xrp-borrow-title" tabindex="-1">
    <button class="modal-close" type="button" aria-label="Close XRP deposit flow" disabled={isBusy} on:click={close}>
      &times;
    </button>

    <div class="modal-head">
      <div>
        <p class="eyebrow">Native {copy.assetName} collateral</p>
        <h2 id="xrp-borrow-title">{modalTitle}</h2>
      </div>
      <div class="status-pill" class:status-busy={isBusy} role="status" aria-live="polite">
        {modalStatusLabel}
      </div>
    </div>

    {#if hasDepositAddress}
      <div class="intent-strip">
        <div class="intent-item">
          <span class="intent-label">Send</span>
          <strong>{copy.sendAmountLabel}</strong>
        </div>
        <div class="intent-divider"></div>
        <div class="intent-item">
          <span class="intent-label">Borrow</span>
          <strong>{copy.borrowAmountLabel}</strong>
        </div>
      </div>

      <p class="reserve-note">{copy.reserveExplanation}</p>
    {/if}

    {#if phase === 'opening'}
      <div class="loading-pane">
        <span class="spinner"></span>
        <p>{nativeXrpModalOpeningCopy()}</p>
      </div>
    {:else if opened}
      <div class="deposit-grid">
        <div class="qr-pane">
          {#if qrDataUrl}
            <img src={qrDataUrl} alt="XRP payment QR code" class="qr-code" />
          {:else}
            <div class="qr-empty">QR</div>
          {/if}
        </div>

        <div class="address-pane">
          <span class="field-label">XRP deposit address</span>
          <button class="address-button" type="button" on:click={copyAddress}>
            <span>{formatAddress(opened.custodyAddress, 12, 8)}</span>
            <small>{copied ? 'Copied' : 'Copy'}</small>
          </button>
          <p class="detail-line">Vault #{opened.vaultId} will credit {copy.collateralAmountLabel} as collateral and mint {copy.borrowAmountLabel} after the deposit is confirmed.</p>
        </div>
      </div>
    {/if}

    {#if opened && dep && !awaitingBorrowApproval && phase !== 'borrowing'}
      <div class="wallet-step">
        <span class="field-label">Pay with an XRP wallet</span>
        <XrplWalletConnect />
        <dl class="breakdown">
          <div><dt>You send</dt><dd>{formatDropsAsXrp(dep.proposal?.sendDrops ?? dep.sentDrops ?? copy.sendDrops ?? 0n)} XRP</dd></div>
          <div><dt>Account reserve (stays locked on XRPL)</dt><dd>{dep.reservedDrops !== null ? formatDropsAsXrp(dep.reservedDrops) : '0'} XRP</dd></div>
          <div><dt>Expected collateral</dt><dd>{dep.expectedCreditedDrops !== null ? formatDropsAsXrp(dep.expectedCreditedDrops) : '0'} XRP</dd></div>
          <div><dt>Credited by Rumi</dt><dd>{dep.creditedDrops !== null ? `${formatDropsAsXrp(dep.creditedDrops)} XRP` : dep.phase === 'confirmed' ? 'Confirmed' : 'Not yet confirmed'}</dd></div>
        </dl>

        <div role="status" aria-live="polite">
        {#if (dep.phase === 'signing' || dep.phase === 'wallet_pending') && signingPrompt}
          <p class="detail-line">Approve in Xaman.
            <a href={signingPrompt.deepLink} target="_blank" rel="noopener noreferrer">Open Xaman</a>
          </p>
          {#if promptQr}<img class="qr-code" src={promptQr} alt="Xaman signing QR code" />{/if}
          <p class="detail-line">Waiting for the wallet to submit the payment. Do not pay again; resume the same request.</p>
        {:else if dep.phase === 'wallet_pending'}
          <p class="detail-line">Waiting for the wallet to submit the payment. Do not pay again; resume the same request.</p>
        {:else if dep.phase === 'polling'}
          <p class="detail-line">Payment sent{dep.txHash ? ` (${formatAddress(dep.txHash, 8, 6)})` : ''}. Waiting for XRPL to validate it. The wallet hash alone does not credit collateral.</p>
        {:else if dep.phase === 'ready_to_confirm'}
          <p class="detail-line">XRPL shows funds at your vault address. Rumi must confirm the collateral.</p>
        {:else if dep.phase === 'timed_out'}
          <p class="detail-line">Still waiting for XRPL. Resume checking, or confirm in Rumi if the payment has settled.</p>
        {:else if dep.phase === 'invalidated'}
          <p class="detail-line">Your wallet account or network changed, so the prepared payment was cancelled. Nothing was sent.</p>
        {:else if dep.phase === 'rejected'}
          <p class="detail-line">The wallet request was rejected or expired. Nothing was sent.</p>
        {:else if walletDepositDone}
          <p class="detail-line">{dep.alreadyConfirmed ? 'This vault already has its deposit confirmed. No further payment is needed.' : 'Rumi confirmed the deposit.'}</p>
        {/if}
        </div>

        <div class="wallet-actions">
          {#if canWalletDeposit}
            <button class="primary-action" type="button" disabled={walletBusy} on:click={depositFromWallet}>
              Deposit {copy.sendAmountLabel} from wallet
            </button>
          {/if}
          {#if dep.phase === 'wallet_pending' || dep.phase === 'timed_out'}
            <button class="secondary-action" type="button" on:click={resumeWalletDeposit}>Resume</button>
          {/if}
          {#if dep.phase === 'ready_to_confirm' || dep.phase === 'timed_out'}
            <button class="primary-action" type="button" disabled={walletBusy} on:click={confirmCollateralClick}>
              Confirm collateral in Rumi
            </button>
          {/if}
        </div>
      </div>
    {/if}

    {#if awaitingBorrowApproval && opened}
      <div class="borrow-step">
        <p class="borrow-step-lead">{nativeXrpBorrowSeparateApprovalCopy(copy.borrowAmountLabel)}</p>
        <p class="borrow-step-note">{nativeXrpBorrowLaterCopy(opened.vaultId)}</p>
      </div>
    {/if}

    {#if errorMessage}
      <div class="modal-error" role="alert" aria-live="assertive">{errorMessage}</div>
    {/if}

    <div class="modal-actions">
      <button class="secondary-action" type="button" disabled={isBusy} on:click={close}>
        {awaitingBorrowApproval ? 'Finish later' : 'Cancel'}
      </button>

      {#if dep?.phase === 'needs_open_confirmation' && !opened}
        <button class="primary-action" type="button" on:click={acknowledgeAndOpen}>
          I have no unfinished XRP deposit, open a new one
        </button>
      {/if}

      {#if dep?.pendingReadFailed && !opened}
        <button class="primary-action" type="button" on:click={retryPrepare}>Try again</button>
      {/if}

      {#if modalPrimaryActionLabel}
        <button
          class="primary-action"
          type="button"
          disabled={!opened || isBusy || phase === 'error'}
          on:click={awaitingBorrowApproval ? () => borrowFromConfirmedVault() : confirmDepositAndBorrow}
        >
          {modalPrimaryActionLabel}
        </button>
      {/if}
    </div>
  </div>
</div>
{/if}

<style>
  .xrp-modal-shell {
    position: fixed;
    inset: 0;
    z-index: 220;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 1.25rem;
    background: rgba(3, 6, 18, 0.78);
    backdrop-filter: blur(18px);
  }

  .modal-backdrop {
    position: absolute;
    inset: 0;
    border: 0;
    background: transparent;
  }

  .modal-backdrop:disabled {
    cursor: default;
  }

  .xrp-modal {
    position: relative;
    z-index: 1;
    width: min(640px, 100%);
    border: 1px solid rgba(45, 212, 191, 0.2);
    border-radius: 8px;
    background:
      linear-gradient(145deg, rgba(20, 26, 46, 0.98), rgba(8, 11, 22, 0.98)),
      var(--rumi-bg-surface1);
    box-shadow: 0 28px 80px rgba(0, 0, 0, 0.44), inset 0 1px 0 rgba(255, 255, 255, 0.04);
    padding: 1.25rem;
    color: var(--rumi-text-primary);
  }

  .modal-close {
    position: absolute;
    top: 0.75rem;
    right: 0.75rem;
    width: 2rem;
    height: 2rem;
    border: 1px solid var(--rumi-border);
    border-radius: 6px;
    background: var(--rumi-bg-surface2);
    color: var(--rumi-text-secondary);
    font-size: 1.25rem;
    line-height: 1;
  }

  .modal-close:disabled {
    opacity: 0.45;
  }

  .modal-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    padding-right: 2.25rem;
  }

  .eyebrow,
  .intent-label,
  .field-label {
    margin: 0;
    color: var(--rumi-text-muted);
    font-size: 0.6875rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  h2 {
    margin: 0.25rem 0 0;
    font-size: clamp(1.45rem, 4vw, 2rem);
    line-height: 1.05;
    letter-spacing: 0;
  }

  .status-pill {
    flex: 0 0 auto;
    padding: 0.375rem 0.625rem;
    border: 1px solid var(--rumi-border-teal);
    border-radius: 999px;
    background: var(--rumi-teal-dim);
    color: var(--rumi-teal);
    font-size: 0.75rem;
    font-weight: 700;
  }

  .status-busy {
    border-color: rgba(167, 139, 250, 0.35);
    background: rgba(167, 139, 250, 0.1);
    color: var(--rumi-caution);
  }

  .intent-strip {
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    gap: 1rem;
    margin: 1.25rem 0;
    padding: 0.875rem;
    border: 1px solid var(--rumi-border);
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.025);
  }

  .intent-item {
    display: grid;
    gap: 0.25rem;
  }

  .intent-item strong {
    font-size: 1.1rem;
    font-variant-numeric: tabular-nums;
  }

  .intent-divider {
    width: 1px;
    height: 2.25rem;
    background: var(--rumi-border);
  }

  .reserve-note {
    margin: -0.5rem 0 1rem;
    padding: 0.75rem 0.875rem;
    border: 1px solid rgba(45, 212, 191, 0.16);
    border-radius: 8px;
    background: rgba(45, 212, 191, 0.06);
    color: var(--rumi-text-secondary);
    font-size: 0.875rem;
    line-height: 1.45;
  }

  .loading-pane {
    display: grid;
    justify-items: center;
    gap: 0.75rem;
    padding: 2rem 1rem;
    color: var(--rumi-text-secondary);
  }

  .spinner {
    width: 1.75rem;
    height: 1.75rem;
    border: 2px solid rgba(255, 255, 255, 0.14);
    border-top-color: var(--rumi-teal);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  .deposit-grid {
    display: grid;
    grid-template-columns: 210px 1fr;
    gap: 1rem;
    align-items: stretch;
  }

  .qr-pane {
    display: grid;
    place-items: center;
    min-height: 210px;
    border: 1px solid rgba(45, 212, 191, 0.18);
    border-radius: 8px;
    background: linear-gradient(180deg, rgba(45, 212, 191, 0.08), rgba(209, 118, 232, 0.05));
  }

  .qr-code {
    width: 184px;
    height: 184px;
    padding: 0.375rem;
    border-radius: 8px;
    background: #ffffff;
    image-rendering: pixelated;
  }

  .qr-empty {
    display: grid;
    place-items: center;
    width: 184px;
    height: 184px;
    border: 1px dashed var(--rumi-border-hover);
    color: var(--rumi-text-muted);
    font-weight: 700;
  }

  .address-pane {
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 0.75rem;
    min-width: 0;
  }

  .address-button {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    width: 100%;
    min-width: 0;
    padding: 0.875rem;
    border: 1px solid var(--rumi-border-hover);
    border-radius: 8px;
    background: var(--rumi-bg-surface2);
    color: var(--rumi-text-primary);
    font-family: 'Inter', sans-serif;
    font-size: 0.95rem;
    text-align: left;
  }

  .address-button span {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .address-button small {
    color: var(--rumi-teal);
    font-weight: 700;
  }

  .detail-line {
    margin: 0;
    color: var(--rumi-text-secondary);
    font-size: 0.875rem;
    line-height: 1.45;
  }

  .borrow-step {
    margin-top: 1rem;
    padding: 0.875rem;
    border: 1px solid rgba(45, 212, 191, 0.24);
    border-radius: 8px;
    background: rgba(45, 212, 191, 0.08);
  }

  .borrow-step-lead {
    margin: 0;
    color: var(--rumi-text-primary);
    font-size: 0.9375rem;
    font-weight: 600;
    line-height: 1.45;
  }

  .borrow-step-note {
    margin: 0.5rem 0 0;
    color: var(--rumi-text-secondary);
    font-size: 0.8125rem;
    line-height: 1.45;
  }

  .wallet-step {
    margin-top: 1rem;
    padding: 0.875rem;
    border: 1px solid var(--rumi-border);
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.025);
  }

  .breakdown {
    display: grid;
    gap: 0.375rem;
    margin: 0.5rem 0;
  }

  .breakdown div {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    font-size: 0.8125rem;
  }

  .breakdown dt { color: var(--rumi-text-secondary); }
  .breakdown dd { margin: 0; font-variant-numeric: tabular-nums; }

  .wallet-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }

  .modal-error {
    margin-top: 1rem;
    padding: 0.75rem 0.875rem;
    border: 1px solid rgba(224, 107, 159, 0.28);
    border-radius: 8px;
    background: rgba(224, 107, 159, 0.11);
    color: var(--rumi-danger);
    font-size: 0.875rem;
    line-height: 1.45;
  }

  .modal-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.75rem;
    margin-top: 1.25rem;
  }

  .secondary-action,
  .primary-action {
    min-height: 2.75rem;
    padding: 0 1rem;
    border-radius: 8px;
    font-weight: 800;
  }

  .secondary-action {
    border: 1px solid var(--rumi-border-hover);
    background: var(--rumi-bg-surface2);
    color: var(--rumi-text-secondary);
  }

  .primary-action {
    border: none;
    background: var(--rumi-action);
    color: var(--rumi-bg-primary);
  }

  .primary-action:disabled,
  .secondary-action:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  @media (max-width: 640px) {
    .xrp-modal-shell {
      align-items: flex-end;
      padding: 0.75rem;
    }

    .xrp-modal {
      padding: 1rem;
    }

    .modal-head,
    .modal-actions {
      flex-direction: column;
      align-items: stretch;
    }

    .intent-strip {
      grid-template-columns: 1fr;
    }

    .intent-divider {
      width: 100%;
      height: 1px;
    }

    .deposit-grid {
      grid-template-columns: 1fr;
    }

    .qr-pane {
      min-height: 190px;
    }
  }
</style>
