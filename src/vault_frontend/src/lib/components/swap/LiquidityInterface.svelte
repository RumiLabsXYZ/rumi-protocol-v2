<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { walletStore } from '../../stores/wallet';
  import {
    threePoolService,
    POOL_TOKENS,
    parseTokenAmount,
    formatTokenAmount,
    getLedgerFee,
  } from '../../services/threePoolService';
  import { formatStableTokenDisplay } from '../../utils/format';

  const dispatch = createEventDispatcher();

  // ── Tab state ──
  type Tab = 'add' | 'remove';
  let activeTab: Tab = 'add';

  // ── Add liquidity state ──
  let addAmounts = ['', '', ''];
  let addLoading = false;
  let addQuoting = false;
  let addError = '';
  let addLpEstimate: bigint | null = null;
  let addSlippageBps = 50;
  let showAddSlippage = false;

  // ── Remove liquidity state ──
  type RemoveMode = 'proportional' | 'single';
  let removeMode: RemoveMode = 'proportional';
  let removeLpAmount = '';
  let removeLoading = false;
  let removeQuoting = false;
  let removeError = '';
  let removeEstimates: bigint[] | null = null;
  let removeSingleEstimate: bigint | null = null;
  let removeSingleIndex = 0;
  let removeSlippageBps = 50;
  let showRemoveSlippage = false;
  let showSingleDropdown = false;

  // ── Debounce timers ──
  let addQuoteTimer: ReturnType<typeof setTimeout> | null = null;
  let removeQuoteTimer: ReturnType<typeof setTimeout> | null = null;

  $: isConnected = $walletStore.isConnected;

  // ── Wallet balances ──
  const LEDGER_TO_KEY: Record<string, string> = {
    [POOL_TOKENS[0].ledgerId]: 'ICUSD',
    [POOL_TOKENS[1].ledgerId]: 'CKUSDT',
    [POOL_TOKENS[2].ledgerId]: 'CKUSDC',
  };

  function getBalance(index: number): bigint {
    if (!$walletStore.tokenBalances) return 0n;
    const key = LEDGER_TO_KEY[POOL_TOKENS[index].ledgerId];
    if (!key) return 0n;
    return $walletStore.tokenBalances[key]?.raw ?? 0n;
  }

  // ── LP balance ──
  let lpBalance: bigint = 0n;

  $: if (isConnected && $walletStore.principal) {
    loadLpBalance();
  }

  async function loadLpBalance() {
    try {
      if ($walletStore.principal) {
        lpBalance = await threePoolService.getLpBalance($walletStore.principal);
      }
    } catch (e) {
      console.warn('Failed to load LP balance:', e);
    }
  }

  // LP in human-readable format (18 decimals)
  $: lpBalanceFormatted = formatLpDisplay(lpBalance);

  function formatLpDisplay(raw: bigint): string {
    const value = Number(raw) / 1e18;
    if (value === 0) return '0.00';
    if (value < 0.01) return value.toFixed(6);
    return value.toFixed(4).replace(/0+$/, '').replace(/\.$/, '.00');
  }

  // ── Balance validation ──
  $: addExceedsBalance = addAmounts.some((a, i) => {
    const val = parseFloat(a);
    if (!val || val <= 0) return false;
    const raw = parseTokenAmount(a, POOL_TOKENS[i].decimals);
    const fees = getLedgerFee(POOL_TOKENS[i].decimals) * 2n;
    return raw + fees > getBalance(i);
  });

  $: removeExceedsBalance = (() => {
    const val = parseFloat(removeLpAmount || '0');
    if (!val || val <= 0) return false;
    return BigInt(Math.floor(val * 1e18)) > lpBalance;
  })();

  // ── Add: debounced quote ──
  $: {
    const hasAmount = addAmounts.some(a => a && parseFloat(a) > 0);
    if (hasAmount) {
      debouncedAddQuote();
    } else {
      addLpEstimate = null;
    }
  }

  function debouncedAddQuote() {
    if (addQuoteTimer) clearTimeout(addQuoteTimer);
    addQuoteTimer = setTimeout(fetchAddQuote, 400);
  }

  async function fetchAddQuote() {
    try {
      addQuoting = true;
      const amounts = addAmounts.map((a, i) => {
        const val = parseFloat(a);
        if (!val || val <= 0) return 0n;
        return parseTokenAmount(a, POOL_TOKENS[i].decimals);
      });
      if (amounts.every(a => a === 0n)) { addLpEstimate = null; return; }
      addLpEstimate = await threePoolService.calcAddLiquidity(amounts);
    } catch {
      addLpEstimate = null;
    } finally {
      addQuoting = false;
    }
  }

  // ── Remove: debounced quote ──
  $: if (removeLpAmount && parseFloat(removeLpAmount) > 0) {
    debouncedRemoveQuote();
  } else {
    removeEstimates = null;
    removeSingleEstimate = null;
  }

  function debouncedRemoveQuote() {
    if (removeQuoteTimer) clearTimeout(removeQuoteTimer);
    removeQuoteTimer = setTimeout(fetchRemoveQuote, 400);
  }

  async function fetchRemoveQuote() {
    const val = parseFloat(removeLpAmount);
    if (!val || val <= 0) { removeEstimates = null; removeSingleEstimate = null; return; }
    try {
      removeQuoting = true;
      const lpRaw = BigInt(Math.floor(val * 1e18));
      if (removeMode === 'proportional') {
        removeEstimates = await threePoolService.calcRemoveLiquidity(lpRaw);
        removeSingleEstimate = null;
      } else {
        removeSingleEstimate = await threePoolService.calcRemoveOneCoin(lpRaw, removeSingleIndex);
        removeEstimates = null;
      }
    } catch {
      removeEstimates = null;
      removeSingleEstimate = null;
    } finally {
      removeQuoting = false;
    }
  }

  // ── Re-quote on mode/index change ──
  function setRemoveMode(mode: RemoveMode) {
    removeMode = mode;
    removeEstimates = null;
    removeSingleEstimate = null;
    if (removeLpAmount && parseFloat(removeLpAmount) > 0) debouncedRemoveQuote();
  }

  function selectSingleToken(index: number) {
    removeSingleIndex = index;
    showSingleDropdown = false;
    removeSingleEstimate = null;
    if (removeLpAmount && parseFloat(removeLpAmount) > 0) debouncedRemoveQuote();
  }

  // ── Max buttons ──
  function setAddMax(index: number) {
    const balance = getBalance(index);
    const totalFees = getLedgerFee(POOL_TOKENS[index].decimals) * 2n;
    const adjusted = balance > totalFees ? balance - totalFees : 0n;
    const divisor = Math.pow(10, POOL_TOKENS[index].decimals);
    addAmounts[index] = (Number(adjusted) / divisor).toFixed(POOL_TOKENS[index].decimals);
    addAmounts = [...addAmounts]; // trigger reactivity
  }

  function setRemoveMax() {
    const val = Number(lpBalance) / 1e18;
    removeLpAmount = val.toFixed(18).replace(/0+$/, '').replace(/\.$/, '');
  }

  // ── Submit: Add Liquidity ──
  async function handleAdd() {
    const amounts = addAmounts.map((a, i) => {
      const val = parseFloat(a);
      if (!val || val <= 0) return 0n;
      return parseTokenAmount(a, POOL_TOKENS[i].decimals);
    });

    if (amounts.every(a => a === 0n)) {
      addError = 'Enter at least one amount';
      return;
    }
    if (addLpEstimate === null) {
      addError = 'Waiting for quote';
      return;
    }

    // Check each balance
    for (let k = 0; k < 3; k++) {
      if (amounts[k] > 0n) {
        const balance = getBalance(k);
        const fees = getLedgerFee(POOL_TOKENS[k].decimals) * 2n;
        if (amounts[k] + fees > balance) {
          addError = `Insufficient ${POOL_TOKENS[k].symbol} balance`;
          return;
        }
      }
    }

    try {
      addLoading = true;
      addError = '';
      const minLp = addLpEstimate * BigInt(10000 - addSlippageBps) / 10000n;
      await threePoolService.addLiquidity(amounts, minLp);
      dispatch('success', { action: 'add_liquidity' });
      addAmounts = ['', '', ''];
      addLpEstimate = null;
      loadLpBalance();
    } catch (err: any) {
      addError = err.message || 'Add liquidity failed';
    } finally {
      addLoading = false;
    }
  }

  // ── Submit: Remove Liquidity ──
  async function handleRemove() {
    const val = parseFloat(removeLpAmount);
    if (!val || val <= 0) {
      removeError = 'Enter LP amount';
      return;
    }
    const lpRaw = BigInt(Math.floor(val * 1e18));
    if (lpRaw > lpBalance) {
      removeError = 'Insufficient LP balance';
      return;
    }

    try {
      removeLoading = true;
      removeError = '';

      if (removeMode === 'proportional') {
        if (!removeEstimates) { removeError = 'Waiting for quote'; return; }
        const minAmounts = removeEstimates.map(a =>
          a * BigInt(10000 - removeSlippageBps) / 10000n
        );
        await threePoolService.removeLiquidity(lpRaw, minAmounts);
      } else {
        if (removeSingleEstimate === null) { removeError = 'Waiting for quote'; return; }
        const minAmount = removeSingleEstimate * BigInt(10000 - removeSlippageBps) / 10000n;
        await threePoolService.removeOneCoin(lpRaw, removeSingleIndex, minAmount);
      }

      dispatch('success', { action: 'remove_liquidity' });
      removeLpAmount = '';
      removeEstimates = null;
      removeSingleEstimate = null;
      loadLpBalance();
    } catch (err: any) {
      removeError = err.message || 'Remove liquidity failed';
    } finally {
      removeLoading = false;
    }
  }

  function closeDropdowns() {
    showSingleDropdown = false;
  }
</script>

<svelte:window on:click={closeDropdowns} />

<div class="liquidity-panel">
  <!-- Sub-tabs: Add | Remove -->
  <div class="sub-tabs">
    <button class="sub-tab" class:active={activeTab === 'add'} on:click={() => { activeTab = 'add'; }}>
      Add
    </button>
    <button class="sub-tab" class:active={activeTab === 'remove'} on:click={() => { activeTab = 'remove'; }}>
      Remove
    </button>
  </div>

  {#if !isConnected}
    <div class="connect-gate">
      <div class="gate-icon">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="10"/>
          <path d="M12 8v4l2 2"/>
        </svg>
      </div>
      <p class="gate-text">Connect your wallet to manage liquidity</p>
    </div>
  {:else if activeTab === 'add'}
    <!-- ─── ADD LIQUIDITY ─── -->
    {#each POOL_TOKENS as token, i}
      <div class="token-input-group">
        <div class="token-input-header">
          <div class="token-label">
            <span class="token-dot" style="background:{token.color}"></span>
            {token.symbol}
          </div>
          <div class="balance-info">
            <span class="balance-label">Bal:</span>
            <span class="balance-value">{formatStableTokenDisplay(getBalance(i), token.decimals)}</span>
            <button class="max-btn" on:click={() => setAddMax(i)} disabled={addLoading}>MAX</button>
          </div>
        </div>
        <input
          type="number"
          step="any"
          min="0"
          placeholder="0.00"
          bind:value={addAmounts[i]}
          disabled={addLoading}
          class="amount-input"
          class:has-value={addAmounts[i] && parseFloat(addAmounts[i]) > 0}
          class:exceeds-balance={(() => {
            const val = parseFloat(addAmounts[i]);
            if (!val || val <= 0) return false;
            const raw = parseTokenAmount(addAmounts[i], token.decimals);
            const fees = getLedgerFee(token.decimals) * 2n;
            return raw + fees > getBalance(i);
          })()}
        />
      </div>
    {/each}

    <!-- Estimated LP output -->
    <div class="estimate-section">
      <span class="estimate-label">Estimated LP tokens</span>
      <span class="estimate-value">
        {#if addQuoting}
          Calculating…
        {:else if addLpEstimate !== null}
          {formatLpDisplay(addLpEstimate)}
        {:else}
          —
        {/if}
      </span>
    </div>

    <!-- Slippage -->
    <div class="info-rows">
      <div class="info-row">
        <span class="info-label">Slippage tolerance</span>
        <button class="slippage-toggle" on:click|stopPropagation={() => { showAddSlippage = !showAddSlippage; }}>
          {(addSlippageBps / 100).toFixed(1)}%
          <svg class="token-chevron" width="8" height="5" viewBox="0 0 10 6" fill="none">
            <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </button>
      </div>
    </div>

    {#if showAddSlippage}
      <div class="slippage-bar">
        <button class="slip-btn" class:active={addSlippageBps === 10} on:click={() => { addSlippageBps = 10; }}>0.1%</button>
        <button class="slip-btn" class:active={addSlippageBps === 50} on:click={() => { addSlippageBps = 50; }}>0.5%</button>
        <button class="slip-btn" class:active={addSlippageBps === 100} on:click={() => { addSlippageBps = 100; }}>1%</button>
      </div>
    {/if}

    <button
      class="submit-btn"
      on:click={handleAdd}
      disabled={addLoading || addAmounts.every(a => !a || parseFloat(a) <= 0) || addLpEstimate === null || addExceedsBalance}
    >
      {#if addLoading}
        <span class="spinner"></span>
        Adding Liquidity…
      {:else}
        Add Liquidity
      {/if}
    </button>

    {#if addError}
      <div class="error-bar">
        <svg viewBox="0 0 16 16" fill="currentColor" width="14" height="14">
          <path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zm0 10.5a.75.75 0 1 1 0-1.5.75.75 0 0 1 0 1.5zM8.75 8a.75.75 0 0 1-1.5 0V5a.75.75 0 0 1 1.5 0v3z"/>
        </svg>
        {addError}
      </div>
    {/if}

  {:else}
    <!-- ─── REMOVE LIQUIDITY ─── -->
    <div class="token-input-group">
      <div class="token-input-header">
        <div class="token-label">LP Tokens</div>
        <div class="balance-info">
          <span class="balance-label">Bal:</span>
          <span class="balance-value">{lpBalanceFormatted}</span>
          <button class="max-btn" on:click={setRemoveMax} disabled={removeLoading}>MAX</button>
        </div>
      </div>
      <input
        type="number"
        step="any"
        min="0"
        placeholder="0.00"
        bind:value={removeLpAmount}
        disabled={removeLoading}
        class="amount-input"
        class:has-value={removeLpAmount && parseFloat(removeLpAmount) > 0}
        class:exceeds-balance={removeExceedsBalance}
      />
    </div>

    <!-- Mode toggle: Proportional / Single Token -->
    <div class="mode-toggle">
      <button class="mode-btn" class:active={removeMode === 'proportional'} on:click={() => setRemoveMode('proportional')}>
        Proportional
      </button>
      <button class="mode-btn" class:active={removeMode === 'single'} on:click={() => setRemoveMode('single')}>
        Single Token
      </button>
    </div>

    <!-- Estimated outputs -->
    {#if removeMode === 'proportional'}
      <div class="remove-estimates">
        {#each POOL_TOKENS as token, i}
          <div class="estimate-row">
            <div class="token-label-small">
              <span class="token-dot" style="background:{token.color}"></span>
              {token.symbol}
            </div>
            <span class="estimate-value-small">
              {#if removeQuoting}
                …
              {:else if removeEstimates}
                {formatTokenAmount(removeEstimates[i], token.decimals)}
              {:else}
                —
              {/if}
            </span>
          </div>
        {/each}
      </div>
    {:else}
      <div class="single-token-select">
        <span class="info-label">Receive as</span>
        <div class="select-wrapper">
          <button class="token-selector" on:click|stopPropagation={() => { showSingleDropdown = !showSingleDropdown; }}>
            <span class="token-dot" style="background:{POOL_TOKENS[removeSingleIndex].color}"></span>
            {POOL_TOKENS[removeSingleIndex].symbol}
            <svg class="token-chevron" width="10" height="6" viewBox="0 0 10 6" fill="none">
              <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>
          </button>
          {#if showSingleDropdown}
            <div class="token-dropdown" on:click|stopPropagation>
              {#each POOL_TOKENS as token, i}
                <button class="token-option" class:token-option-active={removeSingleIndex === i}
                  on:click={() => selectSingleToken(i)}>
                  <span class="token-dot" style="background:{token.color}"></span>
                  {token.symbol}
                </button>
              {/each}
            </div>
          {/if}
        </div>
      </div>
      <div class="estimate-section">
        <span class="estimate-label">Estimated output</span>
        <span class="estimate-value">
          {#if removeQuoting}
            Calculating…
          {:else if removeSingleEstimate !== null}
            {formatTokenAmount(removeSingleEstimate, POOL_TOKENS[removeSingleIndex].decimals)} {POOL_TOKENS[removeSingleIndex].symbol}
          {:else}
            —
          {/if}
        </span>
      </div>
    {/if}

    <!-- Slippage -->
    <div class="info-rows">
      <div class="info-row">
        <span class="info-label">Slippage tolerance</span>
        <button class="slippage-toggle" on:click|stopPropagation={() => { showRemoveSlippage = !showRemoveSlippage; }}>
          {(removeSlippageBps / 100).toFixed(1)}%
          <svg class="token-chevron" width="8" height="5" viewBox="0 0 10 6" fill="none">
            <path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </button>
      </div>
    </div>

    {#if showRemoveSlippage}
      <div class="slippage-bar">
        <button class="slip-btn" class:active={removeSlippageBps === 10} on:click={() => { removeSlippageBps = 10; }}>0.1%</button>
        <button class="slip-btn" class:active={removeSlippageBps === 50} on:click={() => { removeSlippageBps = 50; }}>0.5%</button>
        <button class="slip-btn" class:active={removeSlippageBps === 100} on:click={() => { removeSlippageBps = 100; }}>1%</button>
      </div>
    {/if}

    <button
      class="submit-btn"
      on:click={handleRemove}
      disabled={removeLoading || !removeLpAmount || parseFloat(removeLpAmount) <= 0 || removeExceedsBalance || (removeMode === 'proportional' ? !removeEstimates : removeSingleEstimate === null)}
    >
      {#if removeLoading}
        <span class="spinner"></span>
        Removing Liquidity…
      {:else}
        Remove Liquidity
      {/if}
    </button>

    {#if removeError}
      <div class="error-bar">
        <svg viewBox="0 0 16 16" fill="currentColor" width="14" height="14">
          <path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zm0 10.5a.75.75 0 1 1 0-1.5.75.75 0 0 1 0 1.5zM8.75 8a.75.75 0 0 1-1.5 0V5a.75.75 0 0 1 1.5 0v3z"/>
        </svg>
        {removeError}
      </div>
    {/if}
  {/if}
</div>

<style>
  .liquidity-panel {
    width: 100%;
  }

  /* ── Sub-tabs ── */
  .sub-tabs {
    display: flex;
    gap: 0;
    margin-bottom: 1.25rem;
    background: var(--rumi-bg-surface2);
    border-radius: 0.5rem;
    padding: 0.1875rem;
    border: 1px solid var(--rumi-border);
  }

  .sub-tab {
    flex: 1;
    padding: 0.5rem 0;
    background: transparent;
    border: none;
    border-radius: 0.375rem;
    font-size: 0.8125rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .sub-tab.active {
    background: var(--rumi-bg-surface1);
    color: var(--rumi-text-primary);
    font-weight: 600;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.15);
  }

  .sub-tab:hover:not(.active) {
    color: var(--rumi-text-secondary);
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

  /* ── Token input groups ── */
  .token-input-group {
    margin-bottom: 0.75rem;
  }

  .token-input-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 0.375rem;
  }

  .token-label {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
  }

  .balance-info {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    font-size: 0.75rem;
  }

  .balance-label {
    color: var(--rumi-text-muted);
  }

  .balance-value {
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
  }

  /* ── Input ── */
  .amount-input {
    width: 100%;
    padding: 0.75rem 1rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    color: var(--rumi-text-primary);
    font-family: 'Inter', sans-serif;
    font-size: 1rem;
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

  .amount-input.exceeds-balance {
    border-color: #ef4444;
    box-shadow: 0 0 0 2px rgba(239, 68, 68, 0.15);
  }

  /* ── Max button ── */
  .max-btn {
    padding: 0.125rem 0.375rem;
    background: var(--rumi-teal-dim);
    border: 1px solid var(--rumi-border-teal);
    border-radius: 0.25rem;
    color: var(--rumi-teal);
    font-size: 0.625rem;
    font-weight: 700;
    letter-spacing: 0.04em;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .max-btn:hover:not(:disabled) { background: rgba(45, 212, 191, 0.15); }
  .max-btn:disabled { opacity: 0.4; cursor: not-allowed; }

  /* ── Token dot ── */
  .token-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
    display: inline-block;
  }

  /* ── Estimate section ── */
  .estimate-section {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.625rem 0.75rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    margin-bottom: 0.75rem;
  }

  .estimate-label {
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
  }

  .estimate-value {
    font-size: 0.875rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    font-variant-numeric: tabular-nums;
  }

  /* ── Mode toggle ── */
  .mode-toggle {
    display: flex;
    gap: 0;
    margin-bottom: 0.75rem;
    background: var(--rumi-bg-surface2);
    border-radius: 0.375rem;
    padding: 0.125rem;
    border: 1px solid var(--rumi-border);
  }

  .mode-btn {
    flex: 1;
    padding: 0.375rem 0;
    background: transparent;
    border: none;
    border-radius: 0.25rem;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .mode-btn.active {
    background: var(--rumi-bg-surface1);
    color: var(--rumi-text-primary);
    font-weight: 600;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.12);
  }

  /* ── Remove estimates ── */
  .remove-estimates {
    padding: 0.5rem 0.75rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    margin-bottom: 0.75rem;
  }

  .estimate-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.3125rem 0;
  }

  .token-label-small {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-text-secondary);
  }

  .estimate-value-small {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    font-variant-numeric: tabular-nums;
  }

  /* ── Single token select ── */
  .single-token-select {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.75rem;
  }

  .select-wrapper {
    position: relative;
  }

  .token-selector {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    padding: 0.375rem 0.625rem;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
    cursor: pointer;
    transition: border-color 0.15s;
  }

  .token-selector:hover { border-color: #2DD4BF; }

  .token-chevron {
    color: var(--rumi-text-secondary);
    flex-shrink: 0;
  }

  .token-dropdown {
    position: absolute;
    right: 0;
    top: calc(100% + 0.25rem);
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

  /* ── Info rows & slippage ── */
  .info-rows {
    padding: 0.5rem 0.75rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    margin-bottom: 0.5rem;
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
    margin-bottom: 0.5rem;
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
    margin-top: 0.75rem;
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

  /* ── Error bar ── */
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
