<script lang="ts">
  import Modal from '../common/Modal.svelte';
  import { isValidPrincipal, transferICP, transferICUSD, toE8s, ICP_TRANSFER_FEE, ICUSD_TRANSFER_FEE } from '../../services/transferService';

  export let onClose: () => void;
  export let onSuccess: () => void;
  export let onToast: (message: string, type: 'success' | 'error' | 'info') => void;
  export let icpBalance: string = '0';
  export let icusdBalance: string = '0';

  type Token = 'ICP' | 'icUSD';
  let selectedToken: Token = 'ICP';
  let recipient = '';
  let amount = '';
  let sending = false;
  let errorMsg = '';

  $: currentBalance = selectedToken === 'ICP' ? icpBalance : icusdBalance;
  $: fee = selectedToken === 'ICP' ? Number(ICP_TRANSFER_FEE) / 1e8 : Number(ICUSD_TRANSFER_FEE) / 1e8;
  $: amountNum = parseFloat(amount) || 0;
  $: balanceNum = parseFloat(currentBalance) || 0;
  $: maxSendable = Math.max(0, balanceNum - fee);
  $: isValid = amountNum > 0 && amountNum <= maxSendable && recipient.trim().length > 0;

  function selectToken(token: Token) {
    selectedToken = token;
    amount = '';
    errorMsg = '';
  }

  function setMax() {
    if (maxSendable > 0) {
      amount = maxSendable.toFixed(8).replace(/\.?0+$/, '');
    }
  }

  async function handleSend() {
    errorMsg = '';

    if (!recipient.trim()) {
      errorMsg = 'Enter a recipient principal';
      return;
    }
    if (!isValidPrincipal(recipient.trim())) {
      errorMsg = 'Invalid principal ID';
      return;
    }
    if (amountNum <= 0) {
      errorMsg = 'Enter an amount greater than 0';
      return;
    }
    if (amountNum > maxSendable) {
      errorMsg = `Insufficient balance (max ${maxSendable.toFixed(4)} after fee)`;
      return;
    }

    sending = true;
    try {
      const e8s = toE8s(amountNum);
      const result = selectedToken === 'ICP'
        ? await transferICP(recipient.trim(), e8s)
        : await transferICUSD(recipient.trim(), e8s);

      if (result.success) {
        onToast(`Sent ${amount} ${selectedToken} successfully`, 'success');
        onSuccess();
      } else {
        errorMsg = result.error || 'Transfer failed';
        onToast(errorMsg, 'error');
      }
    } catch (err) {
      errorMsg = err instanceof Error ? err.message : 'Transfer failed';
      onToast(errorMsg, 'error');
    } finally {
      sending = false;
    }
  }
</script>

<Modal title="Send {selectedToken}" {onClose} maxWidth="26rem">
  <div class="send-content">
    <!-- Token Toggle -->
    <div class="token-toggle">
      <button
        class="token-btn"
        class:active={selectedToken === 'ICP'}
        on:click={() => selectToken('ICP')}
      >
        <img src="/icp_logo.png" alt="ICP" class="token-icon" />
        ICP
      </button>
      <button
        class="token-btn"
        class:active={selectedToken === 'icUSD'}
        on:click={() => selectToken('icUSD')}
      >
        <img src="/icusd-logo_v3.svg" alt="icUSD" class="token-icon" />
        icUSD
      </button>
    </div>

    <!-- Recipient -->
    <div class="field">
      <label class="field-label" for="recipient">Recipient Principal</label>
      <input
        id="recipient"
        class="field-input"
        type="text"
        placeholder="e.g. rrkah-fqaaa-aaaaa-..."
        bind:value={recipient}
        disabled={sending}
      />
    </div>

    <!-- Amount -->
    <div class="field">
      <label class="field-label" for="amount">Amount</label>
      <div class="amount-row">
        <input
          id="amount"
          class="field-input amount-input"
          type="number"
          step="any"
          min="0"
          placeholder="0.00"
          bind:value={amount}
          disabled={sending}
        />
        <button class="max-btn" on:click={setMax} disabled={sending}>MAX</button>
      </div>
      <div class="field-meta">
        <span>Balance: {currentBalance} {selectedToken}</span>
        <span>Fee: {fee} {selectedToken}</span>
      </div>
    </div>

    {#if errorMsg}
      <div class="error-box">{errorMsg}</div>
    {/if}

    <!-- Send Button -->
    <button
      class="send-btn"
      on:click={handleSend}
      disabled={sending || !isValid}
    >
      {#if sending}
        <div class="spinner"></div>
        Sending...
      {:else}
        Send {selectedToken}
      {/if}
    </button>
  </div>
</Modal>

<style>
  .send-content {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .token-toggle {
    display: flex;
    gap: 0.5rem;
    background: rgba(0, 0, 0, 0.2);
    border-radius: 0.5rem;
    padding: 0.25rem;
  }

  .token-btn {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    padding: 0.6rem;
    border: 1px solid transparent;
    border-radius: 0.375rem;
    background: transparent;
    color: rgba(255, 255, 255, 0.5);
    font-size: 0.875rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .token-btn:hover {
    color: rgba(255, 255, 255, 0.8);
  }

  .token-btn.active {
    background: rgba(139, 92, 246, 0.3);
    border-color: rgba(139, 92, 246, 0.4);
    color: white;
  }

  .token-icon {
    width: 1.25rem;
    height: 1.25rem;
    border-radius: 50%;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
  }

  .field-label {
    color: rgba(255, 255, 255, 0.7);
    font-size: 0.8rem;
    font-weight: 500;
  }

  .field-input {
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.15);
    border-radius: 0.5rem;
    padding: 0.7rem 0.75rem;
    color: white;
    font-size: 0.875rem;
    width: 100%;
    outline: none;
    transition: border-color 0.15s ease;
  }

  .field-input:focus {
    border-color: rgba(139, 92, 246, 0.5);
  }

  .field-input::placeholder {
    color: rgba(255, 255, 255, 0.25);
  }

  .field-input:disabled {
    opacity: 0.5;
  }

  .amount-row {
    display: flex;
    gap: 0.5rem;
  }

  .amount-input {
    flex: 1;
  }

  /* Hide number input spinners */
  .amount-input::-webkit-outer-spin-button,
  .amount-input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }
  .amount-input[type=number] {
    -moz-appearance: textfield;
  }

  .max-btn {
    padding: 0.7rem 0.75rem;
    background: rgba(255, 255, 255, 0.08);
    border: 1px solid rgba(255, 255, 255, 0.15);
    border-radius: 0.5rem;
    color: rgba(139, 92, 246, 0.9);
    font-size: 0.75rem;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .max-btn:hover:not(:disabled) {
    background: rgba(139, 92, 246, 0.15);
  }

  .field-meta {
    display: flex;
    justify-content: space-between;
    color: rgba(255, 255, 255, 0.4);
    font-size: 0.7rem;
  }

  .error-box {
    background: rgba(224, 107, 159, 0.15);
    border: 1px solid rgba(224, 107, 159, 0.3);
    border-radius: 0.5rem;
    padding: 0.625rem 0.75rem;
    color: #e881a8;
    font-size: 0.8rem;
  }

  .send-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    width: 100%;
    padding: 0.8rem;
    background: linear-gradient(135deg, #7c3aed, #6d28d9);
    border: none;
    border-radius: 0.5rem;
    color: white;
    font-size: 0.9rem;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .send-btn:hover:not(:disabled) {
    background: linear-gradient(135deg, #8b5cf6, #7c3aed);
  }

  .send-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .spinner {
    width: 1rem;
    height: 1rem;
    border: 2px solid rgba(255, 255, 255, 0.3);
    border-top-color: white;
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }
</style>
