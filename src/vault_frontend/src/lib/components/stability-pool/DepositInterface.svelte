<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import { walletStore } from '../../stores/wallet';
  import { stabilityPoolService, formatTokenAmount, parseTokenAmount, symbolForLedger } from '../../services/stabilityPoolService';
  import type { PoolStatus, StablecoinConfig, UserPosition } from '../../services/stabilityPoolService';
  import { CANISTER_IDS } from '../../config';

  export let poolStatus: PoolStatus | null = null;
  export let userPosition: UserPosition | null = null;

  const dispatch = createEventDispatcher();

  let activeTab: 'deposit' | 'withdraw' = 'deposit';
  let amount = '';
  let loading = false;
  let error = '';
  let selectedTokenIndex = 0;

  $: isConnected = $walletStore.isConnected;
  $: activeStablecoins = poolStatus?.stablecoin_registry?.filter(s => s.is_active) ?? [];
  $: selectedToken = activeStablecoins[selectedTokenIndex] ?? null;

  // Map wallet balance keys to ledger IDs
  const LEDGER_TO_WALLET_KEY: Record<string, string> = {
    [CANISTER_IDS.ICUSD_LEDGER]: 'ICUSD',
    [CANISTER_IDS.CKUSDT_LEDGER]: 'CKUSDT',
    [CANISTER_IDS.CKUSDC_LEDGER]: 'CKUSDC',
  };

  $: walletBalance = (() => {
    if (!selectedToken || !$walletStore.tokenBalances) return 0n;
    const key = LEDGER_TO_WALLET_KEY[selectedToken.ledger_id.toText()];
    if (!key) return 0n;
    return $walletStore.tokenBalances[key]?.raw ?? 0n;
  })();

  $: walletBalanceFormatted = selectedToken
    ? formatTokenAmount(walletBalance, selectedToken.decimals)
    : '0';

  // User's deposited balance for the selected token
  $: depositedBalance = (() => {
    if (!selectedToken || !userPosition) return 0n;
    const entry = userPosition.stablecoin_balances.find(
      ([ledger]) => ledger.toText() === selectedToken.ledger_id.toText()
    );
    return entry ? entry[1] : 0n;
  })();

  $: depositedFormatted = selectedToken
    ? formatTokenAmount(depositedBalance, selectedToken.decimals)
    : '0';

  function selectToken(index: number) {
    selectedTokenIndex = index;
    amount = '';
    error = '';
  }

  function setMax() {
    if (!selectedToken) return;
    if (activeTab === 'deposit') {
      // Deduct approximate ledger fee
      const fee = selectedToken.decimals === 8 ? 100_000n : 10n; // e8s fee vs 6-dec fee
      const adjusted = walletBalance > fee ? walletBalance - fee : 0n;
      amount = formatTokenAmount(adjusted, selectedToken.decimals, selectedToken.decimals);
    } else {
      amount = formatTokenAmount(depositedBalance, selectedToken.decimals, selectedToken.decimals);
    }
  }

  async function handleSubmit() {
    if (!selectedToken || !amount || parseFloat(amount) <= 0) {
      error = 'Enter a valid amount';
      return;
    }

    try {
      loading = true;
      error = '';
      const rawAmount = parseTokenAmount(amount, selectedToken.decimals);

      if (activeTab === 'deposit') {
        if (rawAmount > walletBalance) {
          error = 'Insufficient wallet balance';
          return;
        }
        await stabilityPoolService.deposit(selectedToken.ledger_id, rawAmount);
        dispatch('success', { action: 'deposit' });
      } else {
        if (rawAmount > depositedBalance) {
          error = 'Exceeds deposited amount';
          return;
        }
        await stabilityPoolService.withdraw(selectedToken.ledger_id, rawAmount);
        dispatch('success', { action: 'withdraw' });
      }
      amount = '';
    } catch (err: any) {
      error = err.message || `Failed to ${activeTab}`;
    } finally {
      loading = false;
    }
  }
</script>

<div class="deposit-panel">
  <!-- Tab switcher -->
  <div class="tab-bar">
    <button
      class="tab" class:active={activeTab === 'deposit'}
      on:click={() => { activeTab = 'deposit'; error = ''; }}
    >Deposit</button>
    <button
      class="tab" class:active={activeTab === 'withdraw'}
      on:click={() => { activeTab = 'withdraw'; error = ''; }}
      disabled={!userPosition}
    >Withdraw</button>
    <div class="tab-indicator" class:right={activeTab === 'withdraw'}></div>
  </div>

  {#if !isConnected}
    <div class="connect-gate">
      <div class="gate-icon">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <rect x="3" y="11" width="18" height="11" rx="2" ry="2"/>
          <path d="M7 11V7a5 5 0 0 1 10 0v4"/>
        </svg>
      </div>
      <p class="gate-text">Connect your wallet to deposit stablecoins and earn liquidation rewards</p>
    </div>
  {:else}
    <!-- Token selector -->
    {#if activeStablecoins.length > 1}
      <div class="token-selector">
        <div class="selector-label">Token</div>
        <div class="token-options">
          {#each activeStablecoins as token, i}
            <button
              class="token-option"
              class:selected={selectedTokenIndex === i}
              on:click={() => selectToken(i)}
            >
              <span class="token-symbol">{token.symbol}</span>
              <span class="token-decimals">{token.decimals}d</span>
            </button>
          {/each}
        </div>
      </div>
    {/if}

    <!-- Balance display -->
    <div class="balance-row">
      <span class="balance-label">
        {activeTab === 'deposit' ? 'Available' : 'Deposited'}
      </span>
      <span class="balance-value">
        {activeTab === 'deposit' ? walletBalanceFormatted : depositedFormatted}
        <span class="balance-symbol">{selectedToken?.symbol ?? ''}</span>
      </span>
    </div>

    <!-- Input -->
    <div class="input-wrapper">
      <input
        type="number"
        step="any"
        min="0"
        placeholder="0.00"
        bind:value={amount}
        disabled={loading}
        class="amount-input"
        class:has-value={amount && parseFloat(amount) > 0}
      />
      <div class="input-actions">
        <span class="input-symbol">{selectedToken?.symbol ?? ''}</span>
        <button class="max-btn" on:click={setMax} disabled={loading}>MAX</button>
      </div>
    </div>

    <!-- Submit -->
    <button
      class="submit-btn" class:withdraw={activeTab === 'withdraw'}
      on:click={handleSubmit}
      disabled={loading || !amount || parseFloat(amount) <= 0}
    >
      {#if loading}
        <span class="spinner"></span>
        {activeTab === 'deposit' ? 'Depositing…' : 'Withdrawing…'}
      {:else}
        {activeTab === 'deposit' ? 'Deposit' : 'Withdraw'}
        {selectedToken?.symbol ?? ''}
      {/if}
    </button>

    {#if error}
      <div class="error-bar">
        <svg viewBox="0 0 16 16" fill="currentColor" width="14" height="14">
          <path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zm0 10.5a.75.75 0 1 1 0-1.5.75.75 0 0 1 0 1.5zM8.75 8a.75.75 0 0 1-1.5 0V5a.75.75 0 0 1 1.5 0v3z"/>
        </svg>
        {error}
      </div>
    {/if}
  {/if}
</div>

<style>
  .deposit-panel {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
    box-shadow:
      inset 0 1px 0 0 rgba(200, 210, 240, 0.03),
      0 2px 8px -2px rgba(8, 11, 22, 0.6);
  }

  /* ── Tab bar ── */
  .tab-bar {
    position: relative;
    display: flex;
    background: var(--rumi-bg-surface2);
    border-radius: 0.5rem;
    padding: 0.1875rem;
    margin-bottom: 1.5rem;
  }

  .tab {
    flex: 1;
    padding: 0.5rem 1rem;
    background: none;
    border: none;
    border-radius: 0.375rem;
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    cursor: pointer;
    transition: color 0.2s ease;
    position: relative;
    z-index: 1;
  }

  .tab:disabled { opacity: 0.35; cursor: not-allowed; }
  .tab.active { color: var(--rumi-text-primary); }

  .tab-indicator {
    position: absolute;
    top: 0.1875rem;
    left: 0.1875rem;
    width: calc(50% - 0.1875rem);
    height: calc(100% - 0.375rem);
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border-hover);
    border-radius: 0.375rem;
    transition: transform 0.25s cubic-bezier(0.4, 0, 0.2, 1);
    z-index: 0;
  }

  .tab-indicator.right {
    transform: translateX(100%);
  }

  /* ── Connect gate ── */
  .connect-gate {
    text-align: center;
    padding: 2.5rem 1rem;
  }

  .gate-icon {
    width: 2.5rem;
    height: 2.5rem;
    color: var(--rumi-text-muted);
    margin: 0 auto 1rem;
  }

  .gate-text {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
    line-height: 1.5;
    max-width: 280px;
    margin: 0 auto;
  }

  /* ── Token selector ── */
  .token-selector {
    margin-bottom: 1.25rem;
  }

  .selector-label {
    font-size: 0.6875rem;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--rumi-text-muted);
    margin-bottom: 0.5rem;
  }

  .token-options {
    display: flex;
    gap: 0.5rem;
  }

  .token-option {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.375rem;
    padding: 0.5rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .token-option:hover { border-color: var(--rumi-border-hover); }

  .token-option.selected {
    border-color: var(--rumi-teal);
    background: rgba(45, 212, 191, 0.06);
  }

  .token-symbol {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
  }

  .token-decimals {
    font-size: 0.625rem;
    color: var(--rumi-text-muted);
    padding: 0.0625rem 0.25rem;
    background: var(--rumi-bg-surface3);
    border-radius: 0.25rem;
  }

  /* ── Balance row ── */
  .balance-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 0.75rem;
  }

  .balance-label {
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
  }

  .balance-value {
    font-size: 0.8125rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
  }

  .balance-symbol {
    color: var(--rumi-text-secondary);
    font-weight: 400;
  }

  /* ── Input ── */
  .input-wrapper {
    position: relative;
    margin-bottom: 1.25rem;
  }

  .amount-input {
    width: 100%;
    padding: 0.875rem 1rem;
    padding-right: 7.5rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    color: var(--rumi-text-primary);
    font-family: 'Inter', sans-serif;
    font-size: 1.125rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    transition: border-color 0.2s ease, box-shadow 0.2s ease;
    -moz-appearance: textfield;
    appearance: textfield;
  }

  .amount-input::-webkit-inner-spin-button,
  .amount-input::-webkit-outer-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }

  .amount-input::placeholder {
    color: var(--rumi-text-muted);
    font-weight: 400;
  }

  .amount-input:focus {
    outline: none;
    border-color: var(--rumi-teal);
    box-shadow: 0 0 0 2px rgba(45, 212, 191, 0.1);
  }

  .amount-input.has-value {
    border-color: var(--rumi-border-hover);
  }

  .input-actions {
    position: absolute;
    right: 0.75rem;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .input-symbol {
    font-size: 0.8125rem;
    color: var(--rumi-text-secondary);
  }

  .max-btn {
    padding: 0.25rem 0.5rem;
    background: var(--rumi-teal-dim);
    border: 1px solid var(--rumi-border-teal);
    border-radius: 0.25rem;
    color: var(--rumi-teal);
    font-size: 0.6875rem;
    font-weight: 700;
    letter-spacing: 0.04em;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .max-btn:hover:not(:disabled) { background: rgba(45, 212, 191, 0.15); }
  .max-btn:disabled { opacity: 0.4; cursor: not-allowed; }

  /* ── Submit button ── */
  .submit-btn {
    width: 100%;
    padding: 0.875rem;
    background: var(--rumi-action);
    color: var(--rumi-bg-primary);
    border: none;
    border-radius: 0.5rem;
    font-family: 'Circular Std', 'Inter', sans-serif;
    font-size: 0.9375rem;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.2s ease;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
  }

  .submit-btn:hover:not(:disabled) {
    background: var(--rumi-action-bright);
    box-shadow: 0 0 20px rgba(52, 211, 153, 0.15);
  }

  .submit-btn:disabled { opacity: 0.4; cursor: not-allowed; }

  .submit-btn.withdraw {
    background: var(--rumi-purple-light);
    color: white;
  }

  .submit-btn.withdraw:hover:not(:disabled) {
    box-shadow: 0 0 20px rgba(124, 58, 237, 0.2);
  }

  /* ── Spinner ── */
  .spinner {
    width: 1rem;
    height: 1rem;
    border: 2px solid transparent;
    border-top-color: currentColor;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin { to { transform: rotate(360deg); } }

  /* ── Error ── */
  .error-bar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 0.75rem;
    padding: 0.625rem 0.75rem;
    background: rgba(224, 107, 159, 0.08);
    border: 1px solid rgba(224, 107, 159, 0.2);
    border-radius: 0.375rem;
    color: var(--rumi-danger);
    font-size: 0.8125rem;
  }

  @media (max-width: 520px) {
    .token-options { flex-wrap: wrap; }
  }
</style>
