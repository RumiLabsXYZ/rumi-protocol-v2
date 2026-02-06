<script lang="ts">
  import { onMount } from 'svelte';
  import Modal from '../common/Modal.svelte';
  import { copyToClipboard } from '../../utils/principalHelpers';
  import QRCode from 'qrcode';

  export let principal: string;
  export let onClose: () => void;
  export let onToast: (message: string, type: 'success' | 'error' | 'info') => void;

  let copied = false;
  let qrDataUrl = '';

  onMount(async () => {
    try {
      qrDataUrl = await QRCode.toDataURL(principal, {
        width: 200,
        margin: 2,
        color: { dark: '#ffffffdd', light: '#00000000' },
        errorCorrectionLevel: 'M'
      });
    } catch (err) {
      console.error('QR generation failed:', err);
    }
  });

  async function handleCopy() {
    const success = await copyToClipboard(principal);
    if (success) {
      copied = true;
      onToast('Principal copied to clipboard', 'success');
      setTimeout(() => { copied = false; }, 2000);
    } else {
      onToast('Failed to copy', 'error');
    }
  }
</script>

<Modal title="Receive" {onClose} maxWidth="24rem">
  <div class="receive-content">
    <!-- QR Code -->
    {#if qrDataUrl}
      <div class="qr-container">
        <img src={qrDataUrl} alt="QR code for principal" class="qr-image" />
      </div>
    {:else}
      <div class="qr-placeholder">
        <div class="qr-spinner"></div>
      </div>
    {/if}

    <!-- Principal -->
    <div class="principal-box">
      <code class="principal-text">{principal}</code>
    </div>

    <!-- Helper text -->
    <p class="receive-hint">Use this address to receive ICP or icUSD.</p>

    <!-- Copy button -->
    <button class="copy-btn" on:click={handleCopy}>
      {#if copied}
        <svg class="btn-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M20 6L9 17l-5-5" />
        </svg>
        Copied!
      {:else}
        <svg class="btn-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <rect x="9" y="9" width="13" height="13" rx="2" />
          <path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1" />
        </svg>
        Copy Principal
      {/if}
    </button>
  </div>
</Modal>

<style>
  .receive-content {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.75rem;
  }

  .qr-container {
    padding: 1rem;
    background: rgba(255, 255, 255, 0.06);
    border-radius: 0.75rem;
    border: 1px solid rgba(255, 255, 255, 0.08);
  }

  .qr-image {
    width: 180px;
    height: 180px;
    display: block;
    image-rendering: pixelated;
  }

  .qr-placeholder {
    width: 180px;
    height: 180px;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .qr-spinner {
    width: 1.5rem;
    height: 1.5rem;
    border: 2px solid rgba(255, 255, 255, 0.15);
    border-top-color: rgba(139, 92, 246, 0.6);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  .principal-box {
    width: 100%;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 0.5rem;
    padding: 0.75rem;
    word-break: break-all;
    text-align: center;
  }

  .principal-text {
    color: rgba(255, 255, 255, 0.75);
    font-size: 0.75rem;
    line-height: 1.5;
    letter-spacing: 0.01em;
  }

  .receive-hint {
    color: rgba(255, 255, 255, 0.4);
    font-size: 0.75rem;
    margin: 0;
  }

  .copy-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    width: 100%;
    padding: 0.7rem;
    background: rgba(139, 92, 246, 0.25);
    border: 1px solid rgba(139, 92, 246, 0.3);
    border-radius: 0.5rem;
    color: white;
    font-size: 0.85rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .copy-btn:hover {
    background: rgba(139, 92, 246, 0.4);
  }

  .btn-icon {
    width: 1rem;
    height: 1rem;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }
</style>
