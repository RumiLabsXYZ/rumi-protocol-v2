<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { walletStore } from '../../stores/wallet';
  import { AMM_TOKENS, parseTokenAmount, formatTokenAmount, getLedgerFee, type AmmToken } from '../../services/ammService';
  import { resolveRoute, executeRoute, type SwapRoute } from '../../services/swapRouter';

  const dispatch = createEventDispatcher();

  let fromIdx = 0; // Index into AMM_TOKENS
  let toIdx = 1;
  let amount = '';
  let loading = false;
  let quoting = false;
  let error = '';
  let currentRoute: SwapRoute | null = null;
  let slippageBps = 50;
  let showSlippage = false;
  let showFromDropdown = false;
  let showToDropdown = false;

  let quoteTimer: ReturnType<typeof setTimeout> | null = null;

  $: isConnected = $walletStore.isConnected;
  $: fromToken = AMM_TOKENS[fromIdx];
  $: toToken = AMM_TOKENS[toIdx];

  // Wallet balance for "from" token
  $: walletBalance = (() => {
    if (!$walletStore.tokenBalances) return 0n;
    const key = fromToken.balanceKey;
    if (!key) return 0n;
    return $walletStore.tokenBalances[key]?.raw ?? 0n;
  })();

  $: walletBalanceFormatted = formatTokenAmount(walletBalance, fromToken.decimals);

  // Formatted output
  $: outputFormatted = currentRoute
    ? formatTokenAmount(currentRoute.estimatedOutput, toToken.decimals)
    : '';

  // Effective rate
  $: effectiveRate = (() => {
    if (!currentRoute || !amount || parseFloat(amount) <= 0) return null;
    const inputValue = parseFloat(amount);
    const outputValue = Number(currentRoute.estimatedOutput) / Math.pow(10, toToken.decimals);
    return (outputValue / inputValue).toFixed(6);
  })();

  // Price impact (approximate)
  $: priceImpact = (() => {
    if (!currentRoute || !amount || parseFloat(amount) <= 0) return null;
    const inputValue = parseFloat(amount);
    const outputValue = Number(currentRoute.estimatedOutput) / Math.pow(10, toToken.decimals);
    const feeBps = parseFloat(currentRoute.feeDisplay.replace(/[~%]/g, '')) * 100;
    const feeRate = feeBps / 10000;
    const rateAfterFeeRemoval = (outputValue / inputValue) / (1 - feeRate);
    const impact = (1 - rateAfterFeeRemoval) * 100;
    if (Math.abs(impact) < 0.005) return '0.00';
    return impact.toFixed(2);
  })();

  // Price impact warning levels: 2% yellow, 5% red
  $: impactLevel = (() => {
    if (priceImpact === null) return 'none';
    const val = Math.abs(parseFloat(priceImpact));
    if (val >= 5) return 'danger';
    if (val >= 2) return 'warn';
    return 'none';
  })();

  // Debounced quote
  $: if (amount && parseFloat(amount) > 0) {
    currentRoute = null;
    debouncedQuote();
  } else {
    currentRoute = null;
  }

  function debouncedQuote() {
    if (quoteTimer) clearTimeout(quoteTimer);
    quoteTimer = setTimeout(fetchQuote, 400);
  }

  async function fetchQuote() {
    const val = parseFloat(amount);
    if (!val || val <= 0) { currentRoute = null; return; }
    try {
      quoting = true;
      const amountRaw = parseTokenAmount(amount, fromToken.decimals);
      currentRoute = await resolveRoute(fromToken, toToken, amountRaw);
    } catch (err: any) {
      currentRoute = null;
      console.warn('Quote failed:', err.message);
    } finally {
      quoting = false;
    }
  }

  function flipTokens() {
    const tmp = fromIdx;
    fromIdx = toIdx;
    toIdx = tmp;
    amount = '';
    currentRoute = null;
    error = '';
  }

  function selectFrom(index: number) {
    if (index === toIdx) toIdx = fromIdx;
    fromIdx = index;
    showFromDropdown = false;
    amount = '';
    currentRoute = null;
    error = '';
  }

  function selectTo(index: number) {
    if (index === fromIdx) fromIdx = toIdx;
    toIdx = index;
    showToDropdown = false;
    currentRoute = null;
    error = '';
  }

  function setMax() {
    const totalFees = getLedgerFee(fromToken) * 2n;
    const adjusted = walletBalance > totalFees ? walletBalance - totalFees : 0n;
    const divisor = Math.pow(10, fromToken.decimals);
    amount = (Number(adjusted) / divisor).toFixed(fromToken.decimals);
  }

  function setSlippage(bps: number) {
    slippageBps = bps;
  }

  async function handleSwap() {
    if (!amount || parseFloat(amount) <= 0) {
      error = 'Enter a valid amount';
      return;
    }
    if (!currentRoute) {
      error = 'Waiting for quote';
      return;
    }

    // Red warning gate at 5%
    if (impactLevel === 'danger') {
      const confirmed = confirm(`Price impact is ${priceImpact}%. Are you sure you want to proceed?`);
      if (!confirmed) return;
    }

    try {
      loading = true;
      error = '';
      const amountRaw = parseTokenAmount(amount, fromToken.decimals);

      const totalFees = getLedgerFee(fromToken) * 2n;
      if (amountRaw + totalFees > walletBalance) {
        error = 'Insufficient balance (amount + fees)';
        return;
      }

      await executeRoute(currentRoute, fromToken, toToken, amountRaw, slippageBps);
      dispatch('success', { action: 'swap' });
      amount = '';
      currentRoute = null;
    } catch (err: any) {
      error = err.message || 'Swap failed';
    } finally {
      loading = false;
    }
  }

  function closeDropdowns() {
    showFromDropdown = false;
    showToDropdown = false;
  }
</script>

<svelte:window on:click={closeDropdowns} />

<div class="swap-panel">
  {#if !isConnected}
    <div class="connect-gate">
      <div class="gate-icon">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M17 1l4 4-4 4"/>
          <path d="M3 11V9a4 4 0 0 1 4-4h14"/>
          <path d="M7 23l-4-4 4-4"/>
          <path d="M21 13v2a4 4 0 0 1-4 4H3"/>
        </svg>
      </div>
      <p class="gate-text">Connect your wallet to swap tokens</p>
    </div>
  {:else}
    <!-- FROM section -->
    <div class="section-label">From</div>
    <div class="balance-row">
      <span class="balance-label">Available</span>
      <span class="balance-value">
        {walletBalanceFormatted}
        <span class="balance-symbol">{fromToken.symbol}</span>
      </span>
    </div>
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
        <button class="token-selector"
          on:click|stopPropagation={() => { showFromDropdown = !showFromDropdown; showToDropdown = false; }}>
          <span class="token-dot" style="background:{fromToken.color}"></span>
          {fromToken.symbol}
          <svg class="token-chevron" width="10" height="6" viewBox="0 0 10 6" fill="none">
            <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </button>
        <button class="max-btn" on:click={setMax} disabled={loading}>MAX</button>
      </div>

      {#if showFromDropdown}
        <div class="token-dropdown" on:click|stopPropagation>
          {#each AMM_TOKENS as token, i}
            <button class="token-option" class:token-option-active={fromIdx === i}
              on:click={() => selectFrom(i)}>
              <span class="token-dot" style="background:{token.color}"></span>
              {token.symbol}
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Flip button -->
    <div class="flip-row">
      <button class="flip-btn" on:click={flipTokens} disabled={loading} title="Swap direction">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M7 16V4M7 4L3 8M7 4l4 4"/>
          <path d="M17 8v12m0 0l4-4m-4 4l-4-4"/>
        </svg>
      </button>
    </div>

    <!-- TO section -->
    <div class="section-label">To (estimated)</div>
    <div class="input-wrapper">
      <div class="output-display" class:has-value={currentRoute !== null}>
        {#if quoting}
          <span class="quoting-text">Fetching quote…</span>
        {:else if currentRoute !== null}
          {outputFormatted}
        {:else}
          <span class="placeholder-text">0.00</span>
        {/if}
      </div>
      <div class="input-actions">
        <button class="token-selector"
          on:click|stopPropagation={() => { showToDropdown = !showToDropdown; showFromDropdown = false; }}>
          <span class="token-dot" style="background:{toToken.color}"></span>
          {toToken.symbol}
          <svg class="token-chevron" width="10" height="6" viewBox="0 0 10 6" fill="none">
            <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </button>
      </div>

      {#if showToDropdown}
        <div class="token-dropdown" on:click|stopPropagation>
          {#each AMM_TOKENS as token, i}
            <button class="token-option" class:token-option-active={toIdx === i}
              on:click={() => selectTo(i)}>
              <span class="token-dot" style="background:{token.color}"></span>
              {token.symbol}
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Rate & impact info -->
    {#if effectiveRate !== null}
      <div class="info-rows">
        <div class="info-row">
          <span class="info-label">Rate</span>
          <span class="info-value">1 {fromToken.symbol} = {effectiveRate} {toToken.symbol}</span>
        </div>
        <div class="info-row">
          <span class="info-label">Swap fee</span>
          <span class="info-value">{currentRoute?.feeDisplay ?? '—'}</span>
        </div>
        {#if priceImpact !== null}
          <div class="info-row">
            <span class="info-label">Price impact</span>
            <span class="info-value"
              class:impact-warn={impactLevel === 'warn'}
              class:impact-danger={impactLevel === 'danger'}
              class:impact-favorable={parseFloat(priceImpact) < 0}>{priceImpact}%</span>
          </div>
        {/if}
        {#if currentRoute && currentRoute.hops > 1}
          <div class="info-row">
            <span class="info-label">Route</span>
            <span class="info-value route-path">{currentRoute.pathDisplay}</span>
          </div>
        {/if}
        <div class="info-row">
          <span class="info-label">Slippage tolerance</span>
          <button class="slippage-toggle" on:click|stopPropagation={() => { showSlippage = !showSlippage; }}>
            {(slippageBps / 100).toFixed(1)}%
            <svg class="token-chevron" width="8" height="5" viewBox="0 0 10 6" fill="none">
              <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          </button>
        </div>
      </div>
    {/if}

    {#if showSlippage}
      <div class="slippage-bar">
        <button class="slip-btn" class:active={slippageBps === 10} on:click={() => setSlippage(10)}>0.1%</button>
        <button class="slip-btn" class:active={slippageBps === 50} on:click={() => setSlippage(50)}>0.5%</button>
        <button class="slip-btn" class:active={slippageBps === 100} on:click={() => setSlippage(100)}>1%</button>
      </div>
    {/if}

    <!-- Swap button -->
    <button
      class="submit-btn"
      on:click={handleSwap}
      disabled={loading || !amount || parseFloat(amount) <= 0 || currentRoute === null}
    >
      {#if loading}
        <span class="spinner"></span>
        Swapping…
      {:else}
        Swap {fromToken.symbol} → {toToken.symbol}
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
  .swap-panel {
    width: 100%;
  }

  /* ── Section labels ── */
  .section-label {
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    margin-bottom: 0.5rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
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

  /* ── Balance row ── */
  .balance-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 0.5rem;
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
    margin-bottom: 0.25rem;
  }

  .amount-input {
    width: 100%;
    padding: 0.875rem 1rem;
    padding-right: 10rem;
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

  /* ── Output display (non-editable) ── */
  .output-display {
    width: 100%;
    padding: 0.875rem 1rem;
    padding-right: 7rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    color: var(--rumi-text-muted);
    font-family: 'Inter', sans-serif;
    font-size: 1.125rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    min-height: 3.125rem;
    display: flex;
    align-items: center;
  }

  .output-display.has-value {
    color: var(--rumi-text-primary);
  }

  .quoting-text {
    color: var(--rumi-text-muted);
    font-size: 0.875rem;
    font-weight: 400;
  }

  .placeholder-text {
    color: var(--rumi-text-muted);
    font-weight: 400;
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

  /* ── Token selector ── */
  .token-selector {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    padding: 0.25rem 0.5rem;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    cursor: pointer;
    transition: border-color 0.15s;
  }

  .token-selector:hover { border-color: #2DD4BF; }

  .token-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
    display: inline-block;
  }

  .token-chevron {
    color: var(--rumi-text-secondary);
    flex-shrink: 0;
  }

  .token-dropdown {
    position: absolute;
    right: 3rem;
    top: calc(50% + 1.25rem);
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    padding: 0.25rem;
    z-index: 10;
    box-shadow: 0 4px 12px rgba(0,0,0,0.3);
    min-width: 120px;
  }

  .token-option {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    padding: 0.5rem 0.625rem;
    border: none;
    background: transparent;
    color: var(--rumi-text-secondary);
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    border-radius: 0.375rem;
    transition: background 0.1s;
  }

  .token-option:hover { background: var(--rumi-bg-surface3); }
  .token-option-active { color: var(--rumi-text-primary); font-weight: 600; }

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

  /* ── Flip button ── */
  .flip-row {
    display: flex;
    justify-content: center;
    padding: 0.25rem 0;
  }

  .flip-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 2rem;
    height: 2rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    color: var(--rumi-text-secondary);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .flip-btn:hover:not(:disabled) {
    border-color: var(--rumi-teal);
    color: var(--rumi-teal);
  }

  .flip-btn:disabled { opacity: 0.4; cursor: not-allowed; }

  /* ── Info rows ── */
  .info-rows {
    margin-top: 0.75rem;
    padding: 0.625rem 0.75rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
  }

  .info-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.25rem 0;
  }

  .info-label {
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
  }

  .info-value {
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-text-secondary);
    font-variant-numeric: tabular-nums;
  }

  .info-value.impact-warn {
    color: #fbbf24;
  }

  .info-value.impact-danger {
    color: var(--rumi-danger);
    font-weight: 600;
  }

  .info-value.impact-favorable {
    color: var(--rumi-safe);
  }

  .route-path {
    font-family: 'SF Mono', 'Fira Code', monospace;
    letter-spacing: -0.01em;
  }

  /* ── Slippage ── */
  .slippage-toggle {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    background: none;
    border: none;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-teal);
    cursor: pointer;
    padding: 0;
  }

  .slippage-bar {
    display: flex;
    gap: 0.375rem;
    margin-top: 0.5rem;
    justify-content: center;
  }

  .slip-btn {
    padding: 0.25rem 0.75rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-text-secondary);
    cursor: pointer;
    transition: all 0.15s;
  }

  .slip-btn:hover { border-color: var(--rumi-teal); color: var(--rumi-teal); }
  .slip-btn.active {
    background: var(--rumi-teal-dim);
    border-color: var(--rumi-border-teal);
    color: var(--rumi-teal);
    font-weight: 600;
  }

  /* ── Submit button ── */
  .submit-btn {
    width: 100%;
    padding: 0.875rem;
    margin-top: 1rem;
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
</style>
