<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  import QRCode from 'qrcode';
  import type { CollateralInfo } from '$lib/services/types';
  import { protocolService } from '$lib/services/protocol';
  import {
    XrpVaultService,
    type XrpVaultOpenView,
  } from '$lib/services/xrpVaultService';
  import {
    buildXrpPaymentUri,
    nativeXrpDepositCopy,
    nativeXrpModalOpeningCopy,
    nativeXrpModalPrimaryActionLabel,
    nativeXrpModalShouldRender,
    nativeXrpModalStatusLabel,
    nativeXrpModalTitle,
    type NativeXrpBorrowPhase,
  } from '$lib/utils/nativeXrpBorrowFlow';
  import { formatAddress } from '$lib/utils/format';

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

  $: copy = nativeXrpDepositCopy({
    collateralAmount,
    icusdAmount,
    collateralInfo,
    reserveBaseDrops: opened?.reserveBaseDrops ?? 0n,
  });
  $: paymentUri = opened ? buildXrpPaymentUri(opened.custodyAddress, copy.sendAmount) : '';
  $: hasDepositAddress = Boolean(opened?.custodyAddress);
  $: shouldRenderModal = nativeXrpModalShouldRender(phase, hasDepositAddress);
  $: modalTitle = nativeXrpModalTitle(phase, hasDepositAddress);
  $: modalStatusLabel = nativeXrpModalStatusLabel(phase);
  $: modalPrimaryActionLabel = nativeXrpModalPrimaryActionLabel(phase, hasDepositAddress);
  $: isBusy = phase === 'opening' || phase === 'confirming' || phase === 'borrowing';

  onMount(() => {
    void openVault();
  });

  async function openVault() {
    if (started) return;
    started = true;
    phase = 'opening';
    errorMessage = '';
    try {
      const result = await XrpVaultService.openXrpVault();
      if (!result.success || !result.data) {
        phase = 'error';
        errorMessage = result.error ?? 'Could not reserve an XRP custody address.';
        return;
      }

      opened = result.data;
      phase = 'awaiting';
      const openedCopy = nativeXrpDepositCopy({
        collateralAmount,
        icusdAmount,
        collateralInfo,
        reserveBaseDrops: result.data.reserveBaseDrops,
      });
      await generateQr(buildXrpPaymentUri(result.data.custodyAddress, openedCopy.sendAmount));
    } catch (err) {
      phase = 'error';
      errorMessage = err instanceof Error ? err.message : 'Could not reserve an XRP custody address.';
    }
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
    if (!opened) return;
    phase = 'confirming';
    errorMessage = '';

    try {
      const confirmed = await XrpVaultService.confirmXrpDeposit(opened.vaultId);
      if (!confirmed.success) {
        phase = 'awaiting';
        errorMessage = confirmed.error ?? 'Deposit not detected yet.';
        return;
      }

      await borrowFromConfirmedVault(confirmed.oisyResilient);
    } catch (err) {
      phase = 'awaiting';
      errorMessage = err instanceof Error ? err.message : 'Deposit confirmation failed.';
    }
  }

  async function borrowFromConfirmedVault(confirmResilient = false) {
    if (!opened) return;
    phase = 'borrowing';
    errorMessage = '';
    const borrowed = await protocolService.borrowFromVault(opened.vaultId, icusdAmount);
    if (!borrowed.success) {
      phase = 'borrow_failed';
      errorMessage = borrowed.error ?? 'Deposit confirmed, but borrowing failed.';
      return;
    }
    dispatch('complete', {
      vaultId: opened.vaultId,
      oisyResilient: confirmResilient || borrowed.oisyResilient,
    });
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
      <div class="status-pill" class:status-busy={isBusy}>
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

    {#if errorMessage}
      <div class="modal-error">{errorMessage}</div>
    {/if}

    <div class="modal-actions">
      {#if phase === 'borrow_failed'}
        <button class="secondary-action" type="button" on:click={() => borrowFromConfirmedVault()}>
          Retry borrow
        </button>
      {:else}
        <button class="secondary-action" type="button" disabled={isBusy} on:click={close}>
          Cancel
        </button>
      {/if}

      {#if modalPrimaryActionLabel}
        <button
          class="primary-action"
          type="button"
          disabled={!opened || isBusy || phase === 'borrow_failed' || phase === 'error'}
          on:click={confirmDepositAndBorrow}
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
