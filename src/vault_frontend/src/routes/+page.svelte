<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { developerAccess } from '../lib/stores/developer';
  import { formatNumber } from '$lib/utils/format';
  import { tweened } from 'svelte/motion';
  import { cubicOut } from 'svelte/easing';
  import { appDataStore, protocolStatus, isLoadingProtocol } from '$lib/stores/appDataStore';
  import { walletStore, isConnected, principal } from '$lib/stores/wallet';
  import { protocolService } from '$lib/services/protocol';
  import { MINIMUM_COLLATERAL_RATIO } from '$lib/services/protocol/apiClient';
  import ProtocolStats from '$lib/components/dashboard/ProtocolStats.svelte';

  let icpAmount = 1;
  let icusdAmount = 5;
  let errorMessage = '';
  let successMessage = '';
  let actionInProgress = false;
  let showDevInput = false;

  let isPriceLoading = true;
  let animatedPrice = tweened(0, { duration: 600, easing: cubicOut });
  let previousPrice = 0;
  let priceRefreshInterval: ReturnType<typeof setInterval>;
  let priceUpdateError = false;

  $: icpPrice = $protocolStatus?.lastIcpRate || 0;
  $: collateralValue = icpAmount * icpPrice;
  $: collateralRatio = collateralValue > 0 ? collateralValue / icusdAmount : 0;
  $: isValidCollateralRatio = collateralRatio >= MINIMUM_COLLATERAL_RATIO;

  const borrowingFee = 0.005;
  $: calculatedBorrowFee = icusdAmount * borrowingFee;
  $: calculatedIcusdAmount = icusdAmount - calculatedBorrowFee;
  $: calculatedCollateralRatio = icpAmount > 0 && icusdAmount >= 0.001 
    ? ((icpAmount * icpPrice) / icusdAmount) * 100 : icpAmount > 0 ? Infinity : 0;
  $: formattedCollateralRatio = calculatedCollateralRatio === Infinity 
    ? '∞' : calculatedCollateralRatio > 1000000 ? '>1,000,000' : formatNumber(calculatedCollateralRatio);

  $: if (icpPrice > 0) {
    if (previousPrice === 0) { animatedPrice.set(icpPrice, { duration: 0 }); }
    else { animatedPrice.set(icpPrice); }
  }

  onMount(() => {
    loadProtocolData();
    refreshIcpPrice();
    priceRefreshInterval = setInterval(refreshIcpPrice, 30000);
    return () => { if (priceRefreshInterval) clearInterval(priceRefreshInterval); };
  });

  onDestroy(() => { if (priceRefreshInterval) clearInterval(priceRefreshInterval); });

  async function loadProtocolData() {
    try { await appDataStore.fetchProtocolStatus(); }
    catch (error) { console.error('Error loading protocol data:', error); errorMessage = 'Failed to load protocol data'; }
  }

  async function refreshIcpPrice() {
    try {
      isPriceLoading = true; priceUpdateError = false;
      if (icpPrice > 0) previousPrice = icpPrice;
      await appDataStore.fetchProtocolStatus(true);
    } catch (error) { console.error('Failed to refresh ICP price:', error); priceUpdateError = true; }
    finally { isPriceLoading = false; }
  }

  async function createVault() {
    if (!$isConnected) { errorMessage = 'Please connect your wallet first'; return; }
    if (icpAmount <= 0) { errorMessage = 'Please enter a valid ICP amount'; return; }
    if (icusdAmount <= 0) { errorMessage = 'Please enter a valid icUSD amount to borrow'; return; }
    if (!isValidCollateralRatio) { errorMessage = 'Collateral ratio must be at least 130%'; return; }
    actionInProgress = true; errorMessage = ''; successMessage = '';
    try {
      const openResult = await protocolService.openVault(icpAmount);
      if (!openResult.success) { errorMessage = openResult.error || 'Failed to open vault'; return; }
      const borrowResult = await protocolService.borrowFromVault(openResult.vaultId!, icusdAmount);
      if (borrowResult.success) {
        successMessage = `Successfully created vault #${openResult.vaultId} and borrowed ${icusdAmount} icUSD!`;
        if ($principal) await appDataStore.refreshAll($principal);
        icpAmount = 1; icusdAmount = 5;
      } else { errorMessage = borrowResult.error || 'Failed to borrow from vault'; }
    } catch (error) {
      console.error('Error creating vault:', error);
      errorMessage = error instanceof Error ? error.message : 'Unknown error occurred';
    } finally { actionInProgress = false; }
  }
</script>

<svelte:head><title>RUMI Protocol - Borrow icUSD</title></svelte:head>

<div class="page-container">
  <h1 class="page-title">Borrow icUSD with your ICP</h1>

  <div class="page-layout">
    <!-- LEFT: Action card (primary) -->
    <div class="action-column">
      <div class="action-card">
        <!-- ICP Price inline -->
        <div class="price-inline">
          <span class="price-label">ICP Price</span>
          <span class="price-value">
            {#if icpPrice > 0}${$animatedPrice.toFixed(2)}{:else if isPriceLoading}…{:else}—{/if}
          </span>
          {#if priceUpdateError}<span class="price-error">stale</span>{/if}
        </div>

        {#if $developerAccess}
          <div class="form-stack">
            <div class="form-field">
              <label for="icp-amount" class="form-label">ICP Collateral</label>
              <div class="input-wrap">
                <input id="icp-amount" type="number" bind:value={icpAmount} min="0" step="0.01"
                  class="icp-input form-input" placeholder="0.00" disabled={actionInProgress} />
                <span class="input-suffix">ICP</span>
              </div>
              {#if icpAmount > 0}
                <p class="form-hint">≈ ${formatNumber(icpAmount * icpPrice)}</p>
              {/if}
            </div>

            <div class="form-field">
              <label for="icusd-amount" class="form-label">icUSD to Borrow</label>
              <div class="input-wrap">
                <input id="icusd-amount" type="number" bind:value={icusdAmount} min="0" step="0.01"
                  class="icp-input form-input" placeholder="0.00" disabled={actionInProgress} />
                <span class="input-suffix">icUSD</span>
              </div>
              {#if icusdAmount > 0}
                <div class="fee-row"><span>Fee ({(borrowingFee * 100).toFixed(1)}%)</span><span>{formatNumber(calculatedBorrowFee)} icUSD</span></div>
                <div class="fee-row"><span>You receive</span><span>{formatNumber(calculatedIcusdAmount)} icUSD</span></div>
              {/if}
            </div>

            {#if icpAmount > 0 && icusdAmount > 0}
              <div class="ratio-bar">
                <div class="ratio-header">
                  <span>Collateral Ratio</span>
                  <span class:ratio-safe={isValidCollateralRatio} class:ratio-danger={!isValidCollateralRatio}>
                    {formattedCollateralRatio}%
                  </span>
                </div>
                <div class="ratio-track">
                  <div class="ratio-fill" class:ratio-safe={isValidCollateralRatio} class:ratio-danger={!isValidCollateralRatio}
                    style="width:{Math.min(calculatedCollateralRatio / 3, 100)}%"></div>
                </div>
                <p class="form-hint">
                  {isValidCollateralRatio ? 'Healthy. Higher ratio = lower liquidation risk.' : 'Below 150% minimum.'}
                </p>
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

    <!-- RIGHT: Protocol context (secondary) -->
    <div class="context-column">
      <ProtocolStats />
    </div>
  </div>
</div>

<style>
  .page-container { max-width: 1100px; margin: 0 auto; }
  .page-layout { display: grid; grid-template-columns: 1fr 280px; gap: 1.5rem; align-items: start; }

  /* Action card — primary surface */
  .action-column { min-width: 0; }
  .action-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
  }

  /* Context column — subordinate */
  .context-column { position: sticky; top: 5rem; }

  /* ICP price inline */
  .price-inline { display: flex; align-items: baseline; gap: 0.5rem; margin-bottom: 1.25rem; }
  .price-label { font-size: 0.8125rem; color: var(--rumi-text-muted); }
  .price-value { font-family: 'Inter', sans-serif; font-size: 0.875rem; font-weight: 600; color: var(--rumi-text-secondary); font-variant-numeric: tabular-nums; }
  .price-error { font-size: 0.6875rem; color: var(--rumi-caution); }

  /* Form */
  .form-stack { display: flex; flex-direction: column; gap: 1.25rem; }
  .form-field { display: flex; flex-direction: column; gap: 0.25rem; }
  .form-label { font-size: 0.8125rem; font-weight: 500; color: var(--rumi-text-secondary); }
  .input-wrap { position: relative; }
  .form-input { width: 100%; padding-right: 3.5rem; }
  .input-suffix { position: absolute; right: 1rem; top: 50%; transform: translateY(-50%); font-size: 0.8125rem; color: var(--rumi-text-muted); pointer-events: none; }
  .form-hint { font-size: 0.75rem; color: var(--rumi-text-muted); margin-top: 0.125rem; }
  .fee-row { display: flex; justify-content: space-between; font-size: 0.75rem; color: var(--rumi-text-muted); }

  /* Collateral ratio */
  .ratio-bar { padding: 0.75rem; background: var(--rumi-bg-surface2); border-radius: 0.5rem; }
  .ratio-header { display: flex; justify-content: space-between; font-size: 0.8125rem; color: var(--rumi-text-secondary); margin-bottom: 0.375rem; }
  .ratio-track { height: 4px; background: var(--rumi-bg-surface3); border-radius: 2px; overflow: hidden; }
  .ratio-fill { height: 100%; border-radius: 2px; transition: width 0.3s ease; }
  .ratio-safe { color: var(--rumi-safe); }
  .ratio-danger { color: var(--rumi-danger); }
  .ratio-fill.ratio-safe { background: var(--rumi-safe); }
  .ratio-fill.ratio-danger { background: var(--rumi-danger); }

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
    .context-column { position: static; }
  }
</style>
