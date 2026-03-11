<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { developerAccess } from '../lib/stores/developer';
  import { formatNumber, formatStableTx } from '$lib/utils/format';
  import { interpolateMultiplier } from '$lib/utils/interpolate';
  import { tweened } from 'svelte/motion';
  import { cubicOut } from 'svelte/easing';
  import { appDataStore, protocolStatus, isLoadingProtocol } from '$lib/stores/appDataStore';
  import { walletStore, isConnected, principal } from '$lib/stores/wallet';
  import { protocolService } from '$lib/services/protocol';
  import { MINIMUM_CR, LIQUIDATION_CR } from '$lib/protocol';
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

  // Derive per-collateral reactive values — subscribe to $collateralStore so updates
  // propagate when data loads AND when the user switches tokens
  $: selectedCollateralInfo = $collateralStore.collaterals.find(c => c.principal === selectedCollateralPrincipal);
  $: selectedSymbol = selectedCollateralInfo?.symbol ?? 'ICP';
  $: selectedMinCR = selectedCollateralInfo?.minimumCr ?? MINIMUM_CR;
  $: selectedLiqCR = selectedCollateralInfo?.liquidationCr ?? LIQUIDATION_CR;
  $: selectedBorrowingFee = selectedCollateralInfo?.borrowingFee ?? 0;
  $: borrowingFeeCurve = $protocolStatus?.borrowingFeeCurveResolved ?? [];

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

  $: projectedMintCr = (() => {
    if (icusdAmount <= 0 || collateralAmount <= 0 || collateralPrice <= 0) return Infinity;
    const collateralVal = collateralAmount * collateralPrice;
    return collateralVal / icusdAmount;
  })();
  $: mintFeeMultiplier = borrowingFeeCurve.length > 0
    ? interpolateMultiplier(borrowingFeeCurve, projectedMintCr)
    : 1;
  $: effectiveMintFeeRate = selectedBorrowingFee * mintFeeMultiplier;
  $: calculatedBorrowFee = icusdAmount * effectiveMintFeeRate;
  $: calculatedIcusdAmount = icusdAmount - calculatedBorrowFee;
  $: calculatedCollateralRatio = collateralAmount > 0 && icusdAmount >= 0.001
    ? ((collateralAmount * collateralPrice) / icusdAmount) * 100 : collateralAmount > 0 ? Infinity : 0;
  $: formattedCollateralRatio = calculatedCollateralRatio === Infinity
    ? '∞' : calculatedCollateralRatio > 1000000 ? '>1,000,000' : formatNumber(calculatedCollateralRatio);
  $: isValidCollateralRatio = calculatedCollateralRatio >= selectedMinCR * 100;
  // CR color: 3 states for text + marker
  $: crColorClass = calculatedCollateralRatio < selectedMinCR * 100 ? 'danger'
    : calculatedCollateralRatio < selectedMinCR * 1.234 * 100 ? 'caution' : 'safe';

  // Liquidation price
  $: liquidationPrice = collateralAmount > 0 && icusdAmount > 0
    ? (icusdAmount * selectedLiqCR) / collateralAmount : 0;
  $: liqPriceRatio = collateralPrice > 0 && liquidationPrice > 0 ? liquidationPrice / collateralPrice : 0;
  $: liqPriceSeverity = liqPriceRatio > 0.75 ? 'danger' : liqPriceRatio > 0.5 ? 'caution' : 'safe';
  $: safetyDelta = collateralPrice > 0 && liquidationPrice > 0
    ? ((collateralPrice - liquidationPrice) / collateralPrice) * 100 : 0;

  // Max borrow — 0.5% haircut so Max never overshoots the backend oracle price
  $: maxBorrow = collateralAmount > 0 && collateralPrice > 0
    ? Math.floor(((collateralAmount * collateralPrice) / selectedMinCR) * 0.995 * 100) / 100 : 0;

  // Max collateral from wallet balance (minus token ledger fee from metadata)
  $: maxCollateral = (() => {
    if (!$isConnected) return 0;
    const info = selectedCollateralInfo;
    const decimals = info?.decimals ?? 8;
    const ledgerFeeHuman = info ? info.ledgerFee / Math.pow(10, decimals) : 0.0001;
    if (selectedCollateralPrincipal === CANISTER_IDS.ICP_LEDGER) {
      const bal = $walletStore.tokenBalances?.ICP;
      return bal ? Math.max(0, parseFloat(bal.formatted) - ledgerFeeHuman) : 0;
    }
    if (info?.symbol) {
      const bal = $walletStore.tokenBalances?.[info.symbol];
      return bal ? Math.max(0, parseFloat(bal.formatted) - ledgerFeeHuman) : 0;
    }
    return 0;
  })();

  function setMaxCollateral() {
    if (maxCollateral > 0) collateralAmount = Math.floor(maxCollateral * 10000) / 10000;
  }

  // CR gauge zones (100–300% CR scale → 0–100% gauge)
  $: gaugePosition = calculatedCollateralRatio === Infinity
    ? 100 : Math.min(Math.max((calculatedCollateralRatio - 100) / 2, 0), 100);
  // Zone boundaries (all per-collateral)
  $: liqZone = Math.max(((selectedLiqCR * 100) - 100) / 2, 0);               // e.g. 16.5% for 133% liq CR
  $: borrowZone = Math.max(((selectedMinCR * 100) - 100) / 2, 0);             // e.g. 25% for 150% borrow CR
  $: comfortZone = Math.max(((selectedMinCR * 1.234 * 100) - 100) / 2, 0);    // e.g. 42.6% for ~185% comfort

  // Dual-channel color: meter = green→purple→pink, text = white→pink
  function lerpColor(c1: string, c2: string, t: number): string {
    const r1 = parseInt(c1.slice(1, 3), 16), g1 = parseInt(c1.slice(3, 5), 16), b1 = parseInt(c1.slice(5, 7), 16);
    const r2 = parseInt(c2.slice(1, 3), 16), g2 = parseInt(c2.slice(3, 5), 16), b2 = parseInt(c2.slice(5, 7), 16);
    const r = Math.round(r1 + (r2 - r1) * t), g = Math.round(g1 + (g2 - g1) * t), b = Math.round(b1 + (b2 - b1) * t);
    return `#${r.toString(16).padStart(2,'0')}${g.toString(16).padStart(2,'0')}${b.toString(16).padStart(2,'0')}`;
  }
  $: halfSpan = (comfortZone - borrowZone) / 2;
  $: fadeStartPct = comfortZone + halfSpan;
  $: fadeEndPct = comfortZone - halfSpan;
  // Meter marker color: green → purple → pink
  $: borrowGaugeColor = (() => {
    if (gaugePosition >= fadeStartPct) return '#2DD4BF';
    if (gaugePosition >= fadeEndPct) {
      const t = (fadeStartPct - gaugePosition) / (fadeStartPct - fadeEndPct);
      return lerpColor('#2DD4BF', '#a78bfa', t);
    }
    if (gaugePosition <= liqZone) return '#e06b9f';
    const t = (fadeEndPct - gaugePosition) / (fadeEndPct - liqZone);
    return lerpColor('#a78bfa', '#e06b9f', t);
  })();

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
    // Ensure collateral configs are loaded (provides per-asset CR, fees, etc.)
    collateralStore.fetchSupportedCollateral();
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
      // Compound: open vault + borrow in one backend call.
      // For Oisy this batches approve + open_vault_and_borrow into a single popup.
      const result = await protocolService.openVaultAndBorrow(collateralAmount, icusdAmount, selectedCollateralPrincipal);

      if (result.success) {
        successMessage = `Successfully created vault #${result.vaultId} and borrowed ${icusdAmount} icUSD!`;
        if ($principal) await appDataStore.refreshAll($principal);
        collateralAmount = 1; icusdAmount = 5;
      } else { errorMessage = result.error || 'Failed to create vault'; }
    } catch (error) {
      console.error('Error creating vault:', error);
      errorMessage = error instanceof Error ? error.message : 'Unknown error occurred';
    } finally { actionInProgress = false; }
  }
</script>

<svelte:head><title>Borrow | Rumi Protocol</title></svelte:head>

<svelte:window on:click={handleWindowClick} />

<div class="page-container">
  <h1 class="page-title">Borrow icUSD</h1>

  <div class="page-layout">
    <!-- LEFT: Protocol stats -->
    <div class="stats-column">
      <ProtocolStats protocolStatus={$protocolStatus ?? undefined} selectedCollateral={selectedCollateralInfo} />
    </div>

    <!-- RIGHT: Action card -->
    <div class="action-column">
      <div class="action-card">
        {#if $developerAccess}
          <div class="form-stack">
            <!-- Collateral input -->
            <div class="form-field">
              <div class="form-label-row">
                <label for="collateral-amount" class="form-label">Collateral</label>
                {#if maxCollateral > 0}
                  <button class="max-btn" on:click={setMaxCollateral}>Max: {formatNumber(maxCollateral, 4)}</button>
                {/if}
              </div>
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
                <div class="fee-row"><span>Fee ({(effectiveMintFeeRate * 100).toFixed(2)}%)</span><span>{formatStableTx(calculatedBorrowFee)} icUSD</span></div>
                <div class="fee-row"><span>You receive</span><span>{formatStableTx(calculatedIcusdAmount)} icUSD</span></div>
              {/if}
            </div>

            <!-- CR gauge + liquidation price -->
            {#if collateralAmount > 0 && icusdAmount > 0}
              <div class="gauge-section">
                <div class="gauge-header">
                  <span>Collateral Ratio</span>
                  <span class:ratio-safe={crColorClass === 'safe'} class:ratio-caution={crColorClass === 'caution'} class:ratio-danger={crColorClass === 'danger'}>
                    {formattedCollateralRatio}%
                  </span>
                </div>
                <div class="gauge-track">
                  <div class="gauge-zone gauge-zone-pink" style="width:{liqZone}%"></div>
                  <div class="gauge-zone gauge-zone-pink-purple" style="width:{fadeEndPct - liqZone}%; left:{liqZone}%"></div>
                  <div class="gauge-zone gauge-zone-purple-green" style="width:{fadeStartPct - fadeEndPct}%; left:{fadeEndPct}%"></div>
                  <div class="gauge-zone gauge-zone-teal" style="width:{100 - fadeStartPct}%; left:{fadeStartPct}%"></div>
                  <div class="gauge-tick" style="left:{borrowZone}%"></div>
                  <div class="gauge-marker"
                    style="left:{gaugePosition}%; background:{borrowGaugeColor}; box-shadow: 0 0 4px {borrowGaugeColor}80"></div>
                </div>
                <div class="gauge-labels">
                  <span class="gauge-label-abs" style="left:{liqZone}%">liq</span>
                  <span class="gauge-label-abs" style="right:0">300%+</span>
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

            <button
              class="btn-primary cta-button"
              on:click={createVault}
              disabled={actionInProgress || !$isConnected}
            >
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
  .form-label-row { display: flex; justify-content: space-between; align-items: baseline; }
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
    position: relative; height: 8px; border-radius: 4px; overflow: visible;
    background: var(--rumi-bg-surface3);
  }
  .gauge-zone { position: absolute; top: 0; height: 100%; overflow: hidden; }
  .gauge-zone-pink { background: linear-gradient(to right, rgba(224, 107, 159, 0.75), rgba(224, 107, 159, 0.65)); left: 0; border-radius: 4px 0 0 4px; }
  .gauge-zone-pink-purple { background: linear-gradient(to right, rgba(224, 107, 159, 0.55), rgba(167, 139, 250, 0.5)); }
  .gauge-zone-purple-green { background: linear-gradient(to right, rgba(167, 139, 250, 0.45), rgba(45, 212, 191, 0.45)); }
  .gauge-zone-teal { background: rgba(45, 212, 191, 0.5); border-radius: 0 4px 4px 0; }
  .gauge-tick {
    position: absolute; top: 0; width: 1px; height: 100%;
    background: rgba(255,255,255,0.25); transform: translateX(-50%);
    pointer-events: none;
  }
  .gauge-marker {
    position: absolute; top: -5px; width: 3px; height: 18px;
    border-radius: 1.5px; transform: translateX(-50%);
    transition: left 0.3s ease; z-index: 1;
  }
  .gauge-labels {
    position: relative; height: 0.875rem;
    font-size: 0.6875rem; color: var(--rumi-text-muted); margin-top: 0.25rem;
  }
  .gauge-label-abs { position: absolute; transform: translateX(-50%); }
  .gauge-label-abs:last-child { transform: none; }

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
  .ratio-caution { color: var(--rumi-caution); }
  .ratio-danger { color: var(--rumi-danger); }

  /* Messages */
  .msg-error { padding: 0.625rem; background: rgba(224,107,159,0.1); border: 1px solid rgba(224,107,159,0.2); border-radius: 0.5rem; font-size: 0.8125rem; color: #e881a8; }
  .msg-success { padding: 0.625rem; background: rgba(45,212,191,0.1); border: 1px solid rgba(45,212,191,0.2); border-radius: 0.5rem; font-size: 0.8125rem; color: #5eead4; }

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
