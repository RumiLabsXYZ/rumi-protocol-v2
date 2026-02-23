<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { developerAccess } from '../lib/stores/developer';
  import { formatNumber } from '$lib/utils/format';
  import { tweened } from 'svelte/motion';
  import { cubicOut } from 'svelte/easing';
  import { appDataStore, protocolStatus, isLoadingProtocol } from '$lib/stores/appDataStore';
  import { walletStore, isConnected, principal } from '$lib/stores/wallet';
  import { protocolService } from '$lib/services/protocol';
  import { MINIMUM_CR, LIQUIDATION_CR, getMinimumCR, getLiquidationCR, getBorrowingFee } from '$lib/protocol';
  import { collateralStore, activeCollateralTypes } from '$lib/stores/collateralStore';
  import { CANISTER_IDS } from '$lib/config';
  import ProtocolStats from '$lib/components/dashboard/ProtocolStats.svelte';

  let collateralAmount = 1;
  let icusdAmount = 5;
  let errorMessage = '';
  let successMessage = '';
  let actionInProgress = false;
  let showDevInput = false;

  // Collateral token selector
  let selectedCollateralPrincipal = CANISTER_IDS.ICP_LEDGER;
  let showCollateralDropdown = false;

  // Populate collateral token list from store, with ICP fallback
  $: collateralTokens = $activeCollateralTypes.length > 0
    ? $activeCollateralTypes.map(ct => ({
        id: ct.principal,
        label: ct.symbol,
        color: ct.color,
      }))
    : [{ id: CANISTER_IDS.ICP_LEDGER, label: 'ICP', color: '#2DD4BF' }];

  // Derive per-collateral reactive values
  $: selectedCollateralInfo = collateralStore.getCollateralInfo(selectedCollateralPrincipal);
  $: selectedSymbol = selectedCollateralInfo?.symbol ?? 'ICP';
  $: selectedMinCR = getMinimumCR(selectedCollateralPrincipal);
  $: selectedLiqCR = getLiquidationCR(selectedCollateralPrincipal);
  $: selectedBorrowingFee = getBorrowingFee(selectedCollateralPrincipal);

  // Price: use per-collateral price from store, fall back to ICP from protocol status
  $: icpPrice = $protocolStatus?.lastIcpRate || 0;
  $: collateralPrice = selectedCollateralInfo?.price
    || (selectedCollateralPrincipal === CANISTER_IDS.ICP_LEDGER ? icpPrice : 0);
  $: collateralValue = collateralAmount * collateralPrice;

  let isPriceLoading = true;
  let priceRefreshInterval: ReturnType<typeof setInterval>;
  let priceUpdateError = false;

  // Legacy alias for backward compat in template
  $: icpAmount = collateralAmount;

  $: calculatedBorrowFee = icusdAmount * selectedBorrowingFee;
  $: calculatedIcusdAmount = icusdAmount - calculatedBorrowFee;
  $: calculatedCollateralRatio = collateralAmount > 0 && icusdAmount >= 0.001
    ? ((collateralAmount * collateralPrice) / icusdAmount) * 100 : collateralAmount > 0 ? Infinity : 0;
  $: formattedCollateralRatio = calculatedCollateralRatio === Infinity
    ? '∞' : calculatedCollateralRatio > 1000000 ? '>1,000,000' : formatNumber(calculatedCollateralRatio);
  $: isValidCollateralRatio = calculatedCollateralRatio >= selectedMinCR * 100;

  // Liquidation price
  $: liquidationPrice = collateralAmount > 0 && icusdAmount > 0
    ? (icusdAmount * selectedLiqCR) / collateralAmount : 0;
  $: liqPriceRatio = collateralPrice > 0 && liquidationPrice > 0 ? liquidationPrice / collateralPrice : 0;
  $: liqPriceSeverity = liqPriceRatio > 0.75 ? 'danger' : liqPriceRatio > 0.5 ? 'caution' : 'safe';
  $: safetyDelta = collateralPrice > 0 && liquidationPrice > 0
    ? ((collateralPrice - liquidationPrice) / collateralPrice) * 100 : 0;

  // Max borrow
  $: maxBorrow = collateralAmount > 0 && collateralPrice > 0
    ? Math.floor(((collateralAmount * collateralPrice) / selectedMinCR) * 100) / 100 : 0;

  // CR gauge zones (percentage positions on a 0–400% scale)
  $: gaugePosition = calculatedCollateralRatio === Infinity
    ? 100 : Math.min(calculatedCollateralRatio / 4, 100);
  // Zone boundaries mapped to 0-100 scale
  $: liqZone = (selectedLiqCR * 100) / 4;
  const cautionZone = 200 / 4;                    // 50%

  function selectCollateral(principalText: string) {
    selectedCollateralPrincipal = principalText;
    showCollateralDropdown = false;
  }

  function handleWindowClick(e: MouseEvent) {
    if (showCollateralDropdown) {
      const target = e.target as HTMLElement;
      if (!target.closest('.token-selector') && !target.closest('.token-dropdown')) {
        showCollateralDropdown = false;
      }
    }
  }

  function setMaxBorrow() {
    if (maxBorrow > 0) icusdAmount = maxBorrow;
  }

  onMount(() => {
    loadProtocolData();
    refreshPrice();
    priceRefreshInterval = setInterval(refreshPrice, 30000);
    return () => { if (priceRefreshInterval) clearInterval(priceRefreshInterval); };
  });

  onDestroy(() => { if (priceRefreshInterval) clearInterval(priceRefreshInterval); });

  async function loadProtocolData() {
    try { await appDataStore.fetchProtocolStatus(); }
    catch (error) { console.error('Error loading protocol data:', error); errorMessage = 'Failed to load protocol data'; }
  }

  async function refreshPrice() {
    try {
      isPriceLoading = true; priceUpdateError = false;
      await appDataStore.fetchProtocolStatus(true);
    } catch (error) { console.error('Failed to refresh price:', error); priceUpdateError = true; }
    finally { isPriceLoading = false; }
  }

  async function createVault() {
    if (!$isConnected) { errorMessage = 'Please connect your wallet first'; return; }
    if (collateralAmount <= 0) { errorMessage = 'Please enter a valid collateral amount'; return; }
    if (icusdAmount <= 0) { errorMessage = 'Please enter a valid icUSD amount to borrow'; return; }
    if (!isValidCollateralRatio) { errorMessage = `Collateral ratio must be at least ${(selectedMinCR * 100).toFixed(0)}%`; return; }
    actionInProgress = true; errorMessage = ''; successMessage = '';
    try {
      const openResult = await protocolService.openVault(collateralAmount, selectedCollateralPrincipal);
      if (!openResult.success) { errorMessage = openResult.error || 'Failed to open vault'; return; }
      const borrowResult = await protocolService.borrowFromVault(openResult.vaultId!, icusdAmount);
      if (borrowResult.success) {
        successMessage = `Successfully created vault #${openResult.vaultId} and borrowed ${icusdAmount} icUSD!`;
        if ($principal) await appDataStore.refreshAll($principal);
        collateralAmount = 1; icusdAmount = 5;
      } else { errorMessage = borrowResult.error || 'Failed to borrow from vault'; }
    } catch (error) {
      console.error('Error creating vault:', error);
      errorMessage = error instanceof Error ? error.message : 'Unknown error occurred';
    } finally { actionInProgress = false; }
  }
</script>

<svelte:head><title>RUMI Protocol - Borrow icUSD</title></svelte:head>

<svelte:window on:click={handleWindowClick} />

<div class="page-container">
  <h1 class="page-title">Borrow icUSD</h1>

  <div class="page-layout">
    <!-- LEFT: Protocol stats -->
    <div class="stats-column">
      <ProtocolStats protocolStatus={$protocolStatus ?? undefined} />
    </div>

    <!-- RIGHT: Action card -->
    <div class="action-column">
      <div class="action-card">
        {#if $developerAccess}
          <div class="form-stack">
            <!-- Collateral input -->
            <div class="form-field">
              <label for="collateral-amount" class="form-label">Collateral</label>
              <div class="input-wrap">
                <input id="collateral-amount" type="number" bind:value={collateralAmount} min="0" step="0.01"
                  class="icp-input form-input" placeholder="0.00" disabled={actionInProgress} />
                <button class="token-selector"
                  on:click|stopPropagation={() => { showCollateralDropdown = !showCollateralDropdown; }}>
                  <span class="token-dot" style="background:{collateralTokens.find(t => t.id === selectedCollateralPrincipal)?.color || '#2DD4BF'}"></span>
                  {selectedSymbol}
                  <svg class="token-chevron" width="10" height="6" viewBox="0 0 10 6" fill="none"><path d="M1 1l4 4 4-4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>
                </button>
                {#if showCollateralDropdown}
                  <div class="token-dropdown" on:click|stopPropagation>
                    {#each collateralTokens as token}
                      <button class="token-option" class:token-option-active={selectedCollateralPrincipal === token.id}
                        on:click={() => selectCollateral(token.id)}>
                        <span class="token-dot" style="background:{token.color}"></span>
                        {token.label}
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
              {#if collateralAmount > 0 && collateralPrice > 0}
                <p class="form-hint">≈ ${formatNumber(collateralAmount * collateralPrice)}</p>
              {/if}
            </div>

            <!-- icUSD borrow input -->
            <div class="form-field">
              <label for="icusd-amount" class="form-label">icUSD to Borrow</label>
              <div class="input-wrap">
                <input id="icusd-amount" type="number" bind:value={icusdAmount} min="0" step="0.01"
                  class="icp-input form-input form-input-with-max" placeholder="0.00" disabled={actionInProgress} />
                <div class="input-suffix-group">
                  {#if maxBorrow > 0}
                    <button class="max-btn" on:click={setMaxBorrow}>Max</button>
                  {/if}
                  <span class="input-suffix-text">icUSD</span>
                </div>
              </div>
              {#if icusdAmount > 0}
                <div class="fee-row"><span>Fee ({(selectedBorrowingFee * 100).toFixed(1)}%)</span><span>{formatNumber(calculatedBorrowFee)} icUSD</span></div>
                <div class="fee-row"><span>You receive</span><span>{formatNumber(calculatedIcusdAmount)} icUSD</span></div>
              {/if}
            </div>

            <!-- CR gauge + liquidation price -->
            {#if collateralAmount > 0 && icusdAmount > 0}
              <div class="gauge-section">
                <div class="gauge-header">
                  <span>Collateral Ratio</span>
                  <span class:ratio-safe={isValidCollateralRatio} class:ratio-danger={!isValidCollateralRatio}>
                    {formattedCollateralRatio}%
                  </span>
                </div>
                <div class="gauge-track">
                  <div class="gauge-zone gauge-zone-red" style="width:{liqZone}%"></div>
                  <div class="gauge-zone gauge-zone-yellow" style="width:{cautionZone - liqZone}%; left:{liqZone}%"></div>
                  <div class="gauge-zone gauge-zone-green" style="width:{100 - cautionZone}%; left:{cautionZone}%"></div>
                  <div class="gauge-marker" class:marker-safe={isValidCollateralRatio} class:marker-danger={!isValidCollateralRatio}
                    style="left:{gaugePosition}%"></div>
                </div>
                <div class="gauge-labels">
                  <span>{(selectedLiqCR * 100).toFixed(0)}%</span>
                  <span>200%</span>
                  <span>400%+</span>
                </div>

                <!-- Liquidation price -->
                {#if liquidationPrice > 0}
                  <div class="liq-price">
                    <span class="liq-price-label">Liquidation Price</span>
                    <span class="liq-price-value liq-{liqPriceSeverity}">
                      ${formatNumber(liquidationPrice)}
                    </span>
                  </div>
                  {#if safetyDelta > 0}
                    <p class="form-hint">{formatNumber(safetyDelta)}% below current price</p>
                  {/if}
                {/if}
              </div>
            {/if}

            {#if errorMessage}<div class="msg-error">{errorMessage}</div>{/if}
            {#if successMessage}<div class="msg-success">{successMessage}</div>{/if}

            <button class="btn-primary cta-button" on:click={createVault} disabled={actionInProgress || !$isConnected}>
              {#if !$isConnected}Connect Wallet to Continue
              {:else if actionInProgress}Creating Vault…
              {:else}Create Vault & Borrow icUSD{/if}
            </button>
          </div>
        {:else}
          <div class="dev-gate">
            <p class="dev-gate-text">Vault creation requires developer access during beta.</p>
            <button class="btn-primary" on:click={() => showDevInput = true}>Enable Developer Mode</button>
          </div>
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
  .page-container { max-width: 820px; margin: 0 auto; }
  .page-layout { display: grid; grid-template-columns: 280px 1fr; gap: 1.5rem; align-items: start; }

  /* Stats column — left */
  .stats-column { position: sticky; top: 5rem; }

  /* Action card — right */
  .action-column { min-width: 0; display: flex; justify-content: center; }
  .action-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
    width: 100%;
    max-width: 420px;
  }

  /* Form */
  .form-stack { display: flex; flex-direction: column; gap: 1.25rem; }
  .form-field { display: flex; flex-direction: column; gap: 0.25rem; }
  .form-label { font-size: 0.8125rem; font-weight: 500; color: var(--rumi-text-secondary); }
  .input-wrap { position: relative; }
  .form-input { width: 100%; padding-right: 5.5rem; }
  .form-input-with-max { padding-right: 7rem; }
  .form-hint { font-size: 0.75rem; color: var(--rumi-text-muted); margin-top: 0.125rem; }
  .fee-row { display: flex; justify-content: space-between; font-size: 0.75rem; color: var(--rumi-text-muted); }

  /* Input suffix group (Max + token label) */
  .input-suffix-group {
    position: absolute; right: 0.75rem; top: 50%; transform: translateY(-50%);
    display: flex; align-items: center; gap: 0.375rem;
  }
  .input-suffix-text { font-size: 0.8125rem; color: var(--rumi-text-muted); }
  .max-btn {
    font-size: 0.6875rem; font-weight: 600; color: var(--rumi-text-muted);
    background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border);
    border-radius: 0.25rem; padding: 0.125rem 0.375rem; cursor: pointer;
    transition: color 0.15s, border-color 0.15s;
  }
  .max-btn:hover { color: var(--rumi-text-primary); border-color: var(--rumi-text-muted); }

  /* Token selector (collateral dropdown) */
  .token-selector {
    position: absolute; right: 0.5rem; top: 50%; transform: translateY(-50%);
    display: flex; align-items: center; gap: 0.375rem;
    background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border);
    border-radius: 0.375rem; padding: 0.25rem 0.5rem;
    font-size: 0.8125rem; font-weight: 600; color: var(--rumi-text-primary);
    cursor: pointer; transition: border-color 0.15s;
  }
  .token-selector:hover { border-color: #2DD4BF; }
  .token-chevron { color: var(--rumi-text-secondary); flex-shrink: 0; }
  .token-dot {
    width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0;
  }
  .token-dot-icp { background: #2DD4BF; }

  .token-dropdown {
    position: absolute; right: 0.5rem; top: calc(50% + 1.25rem);
    background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border);
    border-radius: 0.5rem; padding: 0.25rem; z-index: 10;
    box-shadow: 0 4px 12px rgba(0,0,0,0.3);
    min-width: 120px;
  }
  .token-option {
    display: flex; align-items: center; gap: 0.5rem;
    width: 100%; padding: 0.5rem 0.625rem; border: none;
    background: transparent; color: var(--rumi-text-secondary);
    font-size: 0.8125rem; font-weight: 500; cursor: pointer;
    border-radius: 0.375rem; transition: background 0.1s;
  }
  .token-option:hover { background: var(--rumi-bg-surface3); }
  .token-option-active { color: var(--rumi-text-primary); font-weight: 600; }

  /* CR gauge */
  .gauge-section {
    padding: 0.75rem; background: var(--rumi-bg-surface2); border-radius: 0.5rem;
  }
  .gauge-header {
    display: flex; justify-content: space-between;
    font-size: 0.8125rem; color: var(--rumi-text-secondary); margin-bottom: 0.5rem;
  }
  .gauge-track {
    position: relative; height: 8px; border-radius: 4px; overflow: hidden;
    background: var(--rumi-bg-surface3);
  }
  .gauge-zone {
    position: absolute; top: 0; height: 100%;
  }
  .gauge-zone-red { background: rgba(239, 68, 68, 0.35); left: 0; border-radius: 4px 0 0 4px; }
  .gauge-zone-yellow { background: rgba(245, 158, 11, 0.3); }
  .gauge-zone-green { background: rgba(16, 185, 129, 0.3); border-radius: 0 4px 4px 0; }
  .gauge-marker {
    position: absolute; top: -3px; width: 3px; height: 14px;
    border-radius: 1.5px; transform: translateX(-50%);
    transition: left 0.3s ease;
  }
  .marker-safe { background: var(--rumi-safe); box-shadow: 0 0 4px rgba(16, 185, 129, 0.5); }
  .marker-danger { background: var(--rumi-danger); box-shadow: 0 0 4px rgba(239, 68, 68, 0.5); }
  .gauge-labels {
    display: flex; justify-content: space-between;
    font-size: 0.625rem; color: var(--rumi-text-muted); margin-top: 0.25rem;
    padding: 0 0.125rem;
  }

  /* Liquidation price */
  .liq-price {
    display: flex; justify-content: space-between; align-items: baseline;
    margin-top: 0.625rem; font-size: 0.8125rem;
  }
  .liq-price-label { color: var(--rumi-text-secondary); }
  .liq-price-value { font-family: 'Inter', sans-serif; font-weight: 600; font-variant-numeric: tabular-nums; }
  .liq-safe { color: var(--rumi-safe); }
  .liq-caution { color: var(--rumi-caution); }
  .liq-danger { color: var(--rumi-danger); }

  .ratio-safe { color: var(--rumi-safe); }
  .ratio-danger { color: var(--rumi-danger); }

  /* Messages */
  .msg-error { padding: 0.625rem; background: rgba(239,68,68,0.1); border: 1px solid rgba(239,68,68,0.2); border-radius: 0.5rem; font-size: 0.8125rem; color: #fca5a5; }
  .msg-success { padding: 0.625rem; background: rgba(16,185,129,0.1); border: 1px solid rgba(16,185,129,0.2); border-radius: 0.5rem; font-size: 0.8125rem; color: #6ee7b7; }

  /* CTA */
  .cta-button { width: 100%; padding: 0.75rem; }

  /* Dev gate */
  .dev-gate { text-align: center; padding: 2rem 1rem; }
  .dev-gate-text { font-size: 0.875rem; color: var(--rumi-text-secondary); margin-bottom: 1rem; }

  /* Number input cleanup */
  input::-webkit-outer-spin-button, input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }
  input[type=number] { -moz-appearance: textfield; appearance: textfield; }

  @media (max-width: 768px) {
    .page-layout { grid-template-columns: 1fr; }
    .stats-column { position: static; order: 2; }
    .action-column { order: 1; }
    .action-card { max-width: none; }
  }
</style>
