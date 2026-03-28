<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  import { walletStore } from '../../stores/wallet';
  import { ammService, AMM_TOKENS, parseTokenAmount, formatTokenAmount, getLedgerFee, approvalAmount } from '../../services/ammService';
  import type { PoolInfo } from '../../services/ammService';
  import { CANISTER_IDS } from '../../config';
  import { priceService } from '../../services/priceService';
  import { threePoolService } from '../../services/threePoolService';

  const dispatch = createEventDispatcher();

  type Tab = 'add' | 'remove';
  let activeTab: Tab = 'add';

  // Pool data
  let pool: PoolInfo | null = null;
  let userLpShares = 0n;
  let loading = false;
  let error = '';
  let poolLoading = true;

  // Add liquidity state
  let addAmountA = ''; // 3USD
  let addAmountB = ''; // ICP
  let addLoading = false;
  let slippageBps = 50;

  // Price data for auto-pairing (only needed for empty pool / initial deposit)
  let icpPriceUsd: number | null = null;
  let threeUsdPriceUsd: number | null = null;
  let priceLoading = false;
  let lastEdited: 'A' | 'B' | null = null;

  // Remove liquidity state
  let removePercent = 0;
  let removeLoading = false;

  $: isConnected = $walletStore.isConnected;

  // Token references
  const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
  const icpToken = AMM_TOKENS.find(t => t.symbol === 'ICP')!;

  // Whether token_a in the pool is 3USD (vs ICP). Determined at load time.
  let tokenAIs3USD = true;

  // Balances
  $: threeUsdBalance = $walletStore.tokenBalances?.THREEUSD?.raw ?? 0n;
  $: icpBalance = $walletStore.tokenBalances?.ICP?.raw ?? 0n;

  // Pool reserves mapped to the correct token regardless of pool ordering
  $: threeUsdReserve = pool ? (tokenAIs3USD ? pool.reserve_a : pool.reserve_b) : 0n;
  $: icpReserve = pool ? (tokenAIs3USD ? pool.reserve_b : pool.reserve_a) : 0n;

  $: isEmptyPool = !pool || (pool.reserve_a === 0n && pool.reserve_b === 0n);

  // Estimated removal amounts
  $: removeEstimate3USD = (() => {
    if (!pool || pool.total_lp_shares === 0n || userLpShares === 0n || removePercent === 0) return 0n;
    const sharesToBurn = userLpShares * BigInt(removePercent) / 100n;
    return threeUsdReserve * sharesToBurn / pool.total_lp_shares;
  })();

  $: removeEstimateICP = (() => {
    if (!pool || pool.total_lp_shares === 0n || userLpShares === 0n || removePercent === 0) return 0n;
    const sharesToBurn = userLpShares * BigInt(removePercent) / 100n;
    return icpReserve * sharesToBurn / pool.total_lp_shares;
  })();

  onMount(loadPool);

  async function loadPool() {
    poolLoading = true;
    try {
      const pools = await ammService.getPools();
      const threePoolId = CANISTER_IDS.THREEPOOL;
      const icpLedgerId = CANISTER_IDS.ICP_LEDGER;
      pool = pools.find(p => {
        const a = p.token_a.toText();
        const b = p.token_b.toText();
        return (a === threePoolId && b === icpLedgerId) || (a === icpLedgerId && b === threePoolId);
      }) ?? null;
      if (pool) {
        tokenAIs3USD = pool.token_a.toText() === threePoolId;
      }
      if (pool && isConnected && $walletStore.principal) {
        userLpShares = await ammService.getLpBalance(pool.pool_id, $walletStore.principal);
      }
      // Fetch external prices if pool is empty (needed for initial deposit pairing)
      if (!pool || (pool.reserve_a === 0n && pool.reserve_b === 0n)) {
        priceLoading = true;
        try {
          const [icpP, tpStatus] = await Promise.all([
            priceService.getCurrentIcpPrice(),
            threePoolService.getPoolStatus(),
          ]);
          icpPriceUsd = icpP;
          // virtual_price is scaled by 1e18: divide to get USD value of 1 3USD LP token
          threeUsdPriceUsd = Number(tpStatus.virtual_price) / 1e18;
        } catch (e) {
          console.error('Failed to load prices for auto-pairing:', e);
        } finally {
          priceLoading = false;
        }
      }
    } catch (e) {
      console.error('Failed to load AMM pool:', e);
      error = 'Failed to load pool data. Please try refreshing.';
    } finally {
      poolLoading = false;
    }
  }

  function onAmountAInput(e: Event) {
    const val = (e.target as HTMLInputElement).value;
    addAmountA = val;
    lastEdited = 'A';
    autoPairFromA(val);
  }

  function onAmountBInput(e: Event) {
    const val = (e.target as HTMLInputElement).value;
    addAmountB = val;
    lastEdited = 'B';
    autoPairFromB(val);
  }

  function autoPairFromA(val: string) {
    if (!val || val === '.' || parseFloat(val) === 0) {
      addAmountB = '';
      return;
    }
    try {
      const amtA = parseFloat(val);
      if (isNaN(amtA)) return;

      if (!isEmptyPool && pool) {
        // Use pool reserve ratio
        const reserveA = Number(threeUsdReserve) / 1e8; // 3USD has 8 decimals
        const reserveB = Number(icpReserve) / 1e8;       // ICP has 8 decimals
        if (reserveA > 0) {
          addAmountB = (amtA * reserveB / reserveA).toFixed(8).replace(/\.?0+$/, '');
        }
      } else if (icpPriceUsd && threeUsdPriceUsd) {
        // Empty pool: use external prices
        // amtA is in 3USD. Value = amtA * threeUsdPriceUsd. Equivalent ICP = value / icpPriceUsd
        const icpAmount = amtA * threeUsdPriceUsd / icpPriceUsd;
        addAmountB = icpAmount.toFixed(8).replace(/\.?0+$/, '');
      }
    } catch {
      // Parse error — don't update the other field
    }
  }

  function autoPairFromB(val: string) {
    if (!val || val === '.' || parseFloat(val) === 0) {
      addAmountA = '';
      return;
    }
    try {
      const amtB = parseFloat(val);
      if (isNaN(amtB)) return;

      if (!isEmptyPool && pool) {
        const reserveA = Number(threeUsdReserve) / 1e8;
        const reserveB = Number(icpReserve) / 1e8;
        if (reserveB > 0) {
          addAmountA = (amtB * reserveA / reserveB).toFixed(8).replace(/\.?0+$/, '');
        }
      } else if (icpPriceUsd && threeUsdPriceUsd) {
        // amtB is in ICP. Value = amtB * icpPriceUsd. Equivalent 3USD = value / threeUsdPriceUsd
        const threeUsdAmount = amtB * icpPriceUsd / threeUsdPriceUsd;
        addAmountA = threeUsdAmount.toFixed(8).replace(/\.?0+$/, '');
      }
    } catch {
      // Parse error — don't update the other field
    }
  }

  async function handleAdd() {
    if (!pool) return;
    const amtA = addAmountA ? parseTokenAmount(addAmountA, threeUsdToken.decimals) : 0n;
    const amtB = addAmountB ? parseTokenAmount(addAmountB, icpToken.decimals) : 0n;
    if (amtA === 0n && amtB === 0n) {
      error = 'Enter at least one amount';
      return;
    }

    try {
      addLoading = true;
      error = '';
      // Map UI amounts (3USD, ICP) to pool ordering (amountA, amountB)
      const poolAmountA = tokenAIs3USD ? amtA : amtB;
      const poolAmountB = tokenAIs3USD ? amtB : amtA;
      const poolTokenA = tokenAIs3USD ? threeUsdToken : icpToken;
      const poolTokenB = tokenAIs3USD ? icpToken : threeUsdToken;

      // Estimate LP shares and apply slippage protection
      // For initial deposit (empty pool), minLp=0 is fine since there's nothing to front-run
      let minLp = 0n;
      if (pool.total_lp_shares > 0n) {
        // Proportional estimate: min(amtA * total / reserveA, amtB * total / reserveB)
        const estA = poolAmountA > 0n ? poolAmountA * pool.total_lp_shares / pool.reserve_a : BigInt(Number.MAX_SAFE_INTEGER);
        const estB = poolAmountB > 0n ? poolAmountB * pool.total_lp_shares / pool.reserve_b : BigInt(Number.MAX_SAFE_INTEGER);
        const lpEstimate = estA < estB ? estA : estB;
        minLp = lpEstimate * BigInt(10000 - slippageBps) / 10000n;
      }
      await ammService.addLiquidity(pool.pool_id, poolAmountA, poolAmountB, minLp, poolTokenA, poolTokenB);
      dispatch('success', { action: 'add_liquidity' });
      addAmountA = '';
      addAmountB = '';
      await loadPool();
    } catch (err: any) {
      error = err.message || 'Add liquidity failed';
    } finally {
      addLoading = false;
    }
  }

  async function handleRemove() {
    if (!pool || removePercent === 0 || userLpShares === 0n) return;

    try {
      removeLoading = true;
      error = '';
      const sharesToBurn = userLpShares * BigInt(removePercent) / 100n;
      // Apply slippage to estimates — map back to pool ordering for minAmountA/minAmountB
      const min3USD = removeEstimate3USD * BigInt(10000 - slippageBps) / 10000n;
      const minICP = removeEstimateICP * BigInt(10000 - slippageBps) / 10000n;
      const minA = tokenAIs3USD ? min3USD : minICP;
      const minB = tokenAIs3USD ? minICP : min3USD;
      await ammService.removeLiquidity(pool.pool_id, sharesToBurn, minA, minB);
      dispatch('success', { action: 'remove_liquidity' });
      removePercent = 0;
      await loadPool();
    } catch (err: any) {
      error = err.message || 'Remove liquidity failed';
    } finally {
      removeLoading = false;
    }
  }

  function goBack() {
    dispatch('back');
  }
</script>

{#if poolLoading}
  <div class="loading-text">Loading pool...</div>
{:else if !pool}
  <div class="empty-text">3USD/ICP pool not yet created.</div>
{:else}
  <button class="back-btn" on:click={goBack}>← All pools</button>

  <!-- Pool overview -->
  <div class="pool-overview">
    <div class="overview-pair">
      <span class="pool-dot" style="background:#34d399"></span>
      <span class="pool-dot" style="background:#29abe2"></span>
      <span class="overview-name">3USD / ICP</span>
    </div>
    <div class="overview-stats">
      <span>3USD: {formatTokenAmount(threeUsdReserve, 8)}</span>
      <span>ICP: {formatTokenAmount(icpReserve, 8)}</span>
    </div>
    {#if isConnected && userLpShares > 0n}
      <div class="user-position">
        Your LP: {formatTokenAmount(userLpShares, 8)} shares
      </div>
    {/if}
  </div>

  <!-- Tabs -->
  <div class="sub-tabs">
    <button class="sub-tab" class:active={activeTab === 'add'} on:click={() => { activeTab = 'add'; error = ''; }}>Add</button>
    <button class="sub-tab" class:active={activeTab === 'remove'} on:click={() => { activeTab = 'remove'; error = ''; }}>Remove</button>
  </div>

  {#if activeTab === 'add'}
    {#if !isConnected}
      <p class="connect-text">Connect your wallet to add liquidity</p>
    {:else}
      <div class="input-group">
        <label class="input-label">3USD</label>
        <input type="number" step="any" min="0" placeholder="0.00"
               value={addAmountA} on:input={onAmountAInput}
               disabled={addLoading} class="token-input" />
        <span class="input-balance">Bal: {formatTokenAmount(threeUsdBalance, 8)}</span>
      </div>
      <div class="input-group">
        <label class="input-label">ICP</label>
        <input type="number" step="any" min="0" placeholder="0.00"
               value={addAmountB} on:input={onAmountBInput}
               disabled={addLoading} class="token-input" />
        <span class="input-balance">Bal: {formatTokenAmount(icpBalance, 8)}</span>
      </div>
      {#if pool && pool.reserve_a > 0n && pool.reserve_b > 0n}
        {@const reserveA_f = Number(threeUsdReserve) / 1e8}
        {@const reserveB_f = Number(icpReserve) / 1e8}
        {@const rate = reserveA_f > 0 ? (reserveB_f / reserveA_f) : 0}
        <div class="price-info">
          1 3USD = {rate.toFixed(4)} ICP <span class="price-source">(pool ratio)</span>
        </div>
      {:else if icpPriceUsd && threeUsdPriceUsd}
        {@const rate = threeUsdPriceUsd / icpPriceUsd}
        <div class="price-info">
          1 3USD = {rate.toFixed(4)} ICP <span class="price-source">(price feeds)</span>
        </div>
      {:else if priceLoading}
        <div class="price-info">Loading prices...</div>
      {/if}
      <button class="submit-btn" on:click={handleAdd} disabled={addLoading}>
        {#if addLoading}
          <span class="spinner"></span> Adding...
        {:else}
          Add Liquidity
        {/if}
      </button>
    {/if}

  {:else}
    {#if !isConnected}
      <p class="connect-text">Connect your wallet to remove liquidity</p>
    {:else if userLpShares === 0n}
      <p class="connect-text">You have no LP shares in this pool</p>
    {:else}
      <!-- Percentage slider -->
      <div class="slider-section">
        <div class="slider-header">
          <span class="slider-label">Amount to remove</span>
          <span class="slider-value">{removePercent}%</span>
        </div>
        <input type="range" min="0" max="100" step="1" bind:value={removePercent} class="slider" />
        <div class="slider-presets">
          <button class="preset-btn" class:active={removePercent === 25} on:click={() => { removePercent = 25; }}>25%</button>
          <button class="preset-btn" class:active={removePercent === 50} on:click={() => { removePercent = 50; }}>50%</button>
          <button class="preset-btn" class:active={removePercent === 75} on:click={() => { removePercent = 75; }}>75%</button>
          <button class="preset-btn" class:active={removePercent === 100} on:click={() => { removePercent = 100; }}>100%</button>
        </div>
      </div>

      {#if removePercent > 0}
        <div class="remove-estimates">
          <div class="estimate-row">
            <span>3USD</span>
            <span>{formatTokenAmount(removeEstimate3USD, 8)}</span>
          </div>
          <div class="estimate-row">
            <span>ICP</span>
            <span>{formatTokenAmount(removeEstimateICP, 8)}</span>
          </div>
        </div>
      {/if}

      <button class="submit-btn remove-btn" on:click={handleRemove} disabled={removeLoading || removePercent === 0}>
        {#if removeLoading}
          <span class="spinner"></span> Removing...
        {:else}
          Remove {removePercent}% Liquidity
        {/if}
      </button>
    {/if}
  {/if}

  {#if error}
    <div class="error-bar">
      <svg viewBox="0 0 16 16" fill="currentColor" width="14" height="14">
        <path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zm0 10.5a.75.75 0 1 1 0-1.5.75.75 0 0 1 0 1.5zM8.75 8a.75.75 0 0 1-1.5 0V5a.75.75 0 0 1 1.5 0v3z"/>
      </svg>
      {error}
    </div>
  {/if}
{/if}

<style>
  .loading-text, .empty-text, .connect-text {
    text-align: center;
    padding: 1.5rem;
    color: var(--rumi-text-muted);
    font-size: 0.8125rem;
  }

  .back-btn {
    background: none;
    border: none;
    color: var(--rumi-teal);
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    padding: 0;
    margin-bottom: 1rem;
  }

  .back-btn:hover { text-decoration: underline; }

  .pool-overview {
    padding: 0.75rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    margin-bottom: 1rem;
  }

  .overview-pair {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    margin-bottom: 0.5rem;
  }

  .pool-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    display: inline-block;
  }

  .overview-name {
    font-size: 0.9375rem;
    font-weight: 600;
    color: var(--rumi-text-primary);
  }

  .overview-stats {
    display: flex;
    gap: 1rem;
    font-size: 0.75rem;
    color: var(--rumi-text-secondary);
    font-variant-numeric: tabular-nums;
  }

  .user-position {
    margin-top: 0.5rem;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--rumi-teal);
  }

  /* Sub tabs */
  .sub-tabs {
    display: flex;
    gap: 0.25rem;
    margin-bottom: 1rem;
  }

  .sub-tab {
    flex: 1;
    padding: 0.375rem 0;
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    background: transparent;
    color: var(--rumi-text-muted);
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
  }

  .sub-tab.active {
    background: var(--rumi-bg-surface2);
    color: var(--rumi-text-primary);
    border-color: var(--rumi-teal);
    font-weight: 600;
  }

  /* Input groups */
  .input-group {
    margin-bottom: 0.75rem;
  }

  .input-label {
    display: block;
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    margin-bottom: 0.25rem;
  }

  .token-input {
    width: 100%;
    padding: 0.75rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    color: var(--rumi-text-primary);
    font-size: 1rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    -moz-appearance: textfield;
    appearance: textfield;
  }

  .token-input::-webkit-inner-spin-button,
  .token-input::-webkit-outer-spin-button {
    -webkit-appearance: none;
  }

  .token-input:focus {
    outline: none;
    border-color: var(--rumi-teal);
  }

  .input-balance {
    display: block;
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
    margin-top: 0.25rem;
    text-align: right;
  }

  /* Slider */
  .slider-section {
    margin-bottom: 1rem;
  }

  .slider-header {
    display: flex;
    justify-content: space-between;
    margin-bottom: 0.5rem;
  }

  .slider-label {
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
  }

  .slider-value {
    font-size: 1.25rem;
    font-weight: 700;
    color: var(--rumi-text-primary);
  }

  .slider {
    width: 100%;
    -webkit-appearance: none;
    height: 4px;
    background: var(--rumi-border);
    border-radius: 2px;
    outline: none;
  }

  .slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--rumi-teal);
    cursor: pointer;
  }

  .slider-presets {
    display: flex;
    gap: 0.375rem;
    margin-top: 0.5rem;
    justify-content: center;
  }

  .preset-btn {
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

  .preset-btn:hover { border-color: var(--rumi-teal); color: var(--rumi-teal); }
  .preset-btn.active {
    background: var(--rumi-teal-dim);
    border-color: var(--rumi-border-teal);
    color: var(--rumi-teal);
    font-weight: 600;
  }

  .remove-estimates {
    padding: 0.75rem;
    background: var(--rumi-bg-surface2);
    border: 1px solid var(--rumi-border);
    border-radius: 0.375rem;
    margin-bottom: 1rem;
  }

  .estimate-row {
    display: flex;
    justify-content: space-between;
    font-size: 0.8125rem;
    font-variant-numeric: tabular-nums;
    padding: 0.25rem 0;
  }

  .estimate-row span:first-child { color: var(--rumi-text-muted); }
  .estimate-row span:last-child { color: var(--rumi-text-primary); font-weight: 600; }

  /* Submit button */
  .submit-btn {
    width: 100%;
    padding: 0.875rem;
    margin-top: 0.5rem;
    background: var(--rumi-action);
    color: var(--rumi-bg-primary);
    border: none;
    border-radius: 0.5rem;
    font-size: 0.9375rem;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.2s;
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

  .remove-btn { background: var(--rumi-danger, #e06b9f); }
  .remove-btn:hover:not(:disabled) { background: #c85a8a; box-shadow: none; }

  .spinner {
    width: 1rem;
    height: 1rem;
    border: 2px solid transparent;
    border-top-color: currentColor;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin { to { transform: rotate(360deg); } }

  .price-info {
    text-align: center;
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
    padding: 0.375rem 0;
    margin-bottom: 0.25rem;
  }

  .price-source {
    opacity: 0.6;
    font-style: italic;
  }

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
