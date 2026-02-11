<script lang="ts">
  import { onMount } from "svelte";
  import { walletStore as wallet } from "$lib/stores/wallet";
  import { protocolService } from '$lib/services/protocol';
  import { formatNumber } from '$lib/utils/format';
  import { tweened } from 'svelte/motion';
  import { cubicOut } from 'svelte/easing';
  import type { CandidVault } from '$lib/services/types';
  import { walletOperations } from "$lib/services/protocol/walletOperations";
  import { CONFIG } from "$lib/config";

  let liquidatableVaults: CandidVault[] = [];
  let icpPrice = 0;
  let isLoading = true;
  let isPriceLoading = true;
  let liquidationError = "";
  let liquidationSuccess = "";
  let processingVaultId: number | null = null;
  let isApprovingAllowance = false;
  let liquidationAmounts: { [vaultId: number]: string } = {};

  $: isConnected = $wallet.isConnected;

  $: walletIcusd = $wallet.tokenBalances?.ICUSD
    ? parseFloat($wallet.tokenBalances.ICUSD.formatted) : 0;

  let animatedPrice = tweened(0, { duration: 600, easing: cubicOut });
  $: if (icpPrice > 0) { animatedPrice.set(icpPrice); }

  $: sortedVaults = [...liquidatableVaults].sort((a, b) => {
    const crA = calculateCollateralRatio(a);
    const crB = calculateCollateralRatio(b);
    if (crA !== crB) return crA - crB;
    return a.vault_id - b.vault_id;
  });

  function calculateCollateralRatio(vault: CandidVault): number {
    const icpAmount = Number(vault.icp_margin_amount || 0) / 1e8;
    const icusdAmount = Number(vault.borrowed_icusd_amount || 0) / 1e8;
    if (icusdAmount <= 0) return Infinity;
    const ratio = (icpAmount * icpPrice / icusdAmount) * 100;
    return isFinite(ratio) ? ratio : 0;
  }

  function getVaultDebt(vault: CandidVault): number {
    return Number(vault.borrowed_icusd_amount || 0) / 1e8;
  }

  function getMaxLiquidation(vault: CandidVault): number {
    const debt = getVaultDebt(vault);
    const icpCollateral = Number(vault.icp_margin_amount || 0) / 1e8;
    const currentPrice = icpPrice || 0;

    // Cap: only liquidate enough to restore CR to ~150%
    // After liquidation of X icUSD, liquidator seizes X/(price*0.9) ICP
    // New CR = (collateral - seized) * price / (debt - X) >= 1.5
    // Solve for X: the minimum needed to reach 150%
    if (currentPrice > 0 && debt > 0) {
      const collateralValue = icpCollateral * currentPrice;
      const currentCr = collateralValue / debt;
      if (currentCr < 1.5) {
        // How much icUSD to liquidate to bring CR to 150%:
        // (collateralValue - (X / 0.9)) / (debt - X) = 1.5
        // collateralValue - X/0.9 = 1.5*debt - 1.5*X
        // X*(1.5 - 1/0.9) = 1.5*debt - collateralValue
        // X*(1.5 - 1.1111) = 1.5*debt - collateralValue
        // X * 0.3889 = 1.5*debt - collateralValue
        const factor = 1.5 - (1 / 0.9);
        const numerator = 1.5 * debt - collateralValue;
        if (factor > 0 && numerator > 0) {
          const restoreCap = numerator / factor;
          // Cap to this, but never more than full debt, never more than wallet balance
          return Math.min(walletIcusd, debt, restoreCap);
        }
      }
    }

    // Fallback: full debt capped to wallet balance
    return Math.min(walletIcusd, debt);
  }

  function calculateSeizure(vault: CandidVault, icusdAmount: number): { icpSeized: number, usdValue: number } {
    const currentPrice = icpPrice || 1;
    const icpCollateral = Number(vault.icp_margin_amount || 0) / 1e8;
    let icpReceived = currentPrice > 0 ? icusdAmount / currentPrice * (1 / 0.9) : 0;
    const icpSeized = Math.min(icpReceived, icpCollateral);
    const usdValue = icpSeized * currentPrice;
    return {
      icpSeized: isFinite(icpSeized) ? icpSeized : 0,
      usdValue: isFinite(usdValue) ? usdValue : 0
    };
  }

  function getInputVal(vault: CandidVault): number {
    return parseFloat(liquidationAmounts[vault.vault_id]) || 0;
  }

  function isOverMax(vault: CandidVault): boolean {
    const v = getInputVal(vault);
    if (v <= 0) return false;
    return v > getMaxLiquidation(vault);
  }

  // Reactive seizure: called from template, reads liquidationAmounts directly
  function getSeizure(vault: CandidVault): { icpSeized: number, usdValue: number } | null {
    // Reference the whole object so Svelte tracks assignment
    const _amounts = liquidationAmounts;
    const v = parseFloat(_amounts[vault.vault_id]) || 0;
    if (v <= 0) return null;
    if (v > getMaxLiquidation(vault)) return null;
    return calculateSeizure(vault, v);
  }

  function setMax(vault: CandidVault) {
    const max = getMaxLiquidation(vault);
    if (max > 0) liquidationAmounts[vault.vault_id] = max.toFixed(4);
  }

  async function loadLiquidatableVaults() {
    isLoading = true; liquidationError = "";
    try {
      const vaults = await protocolService.getLiquidatableVaults();
      liquidatableVaults = vaults.map(vault => ({
        ...vault,
        original_icp_margin_amount: vault.icp_margin_amount,
        original_borrowed_icusd_amount: vault.borrowed_icusd_amount,
        icp_margin_amount: Number(vault.icp_margin_amount || 0),
        borrowed_icusd_amount: Number(vault.borrowed_icusd_amount || 0),
        vault_id: Number(vault.vault_id || 0),
        owner: vault.owner.toString()
      }));
    } catch (error) {
      console.error("Error loading liquidatable vaults:", error);
      liquidationError = "Failed to load liquidatable vaults.";
    } finally { isLoading = false; }
  }

  async function checkAndApproveAllowance(icusdAmount: number): Promise<boolean> {
    try {
      isApprovingAllowance = true;
      const amountE8s = BigInt(Math.floor(icusdAmount * 1e8));
      const spenderCanisterId = CONFIG.currentCanisterId;
      const currentAllowance = await walletOperations.checkIcusdAllowance(spenderCanisterId);
      if (currentAllowance < amountE8s) {
        const approvalAmount = amountE8s * BigInt(150) / BigInt(100);
        const approvalResult = await walletOperations.approveIcusdTransfer(approvalAmount, spenderCanisterId);
        if (!approvalResult.success) { liquidationError = approvalResult.error || "Failed to approve icUSD transfer"; return false; }
        await new Promise(resolve => setTimeout(resolve, 1000));
      }
      return true;
    } catch (error) {
      console.error("Error checking/approving allowance:", error);
      liquidationError = "Failed to approve icUSD transfer.";
      return false;
    } finally { isApprovingAllowance = false; }
  }

  async function handleLiquidate(vault: CandidVault) {
    if (!isConnected) { liquidationError = "Please connect your wallet"; return; }
    if (processingVaultId !== null) return;
    const inputAmount = getInputVal(vault);
    if (inputAmount <= 0) { liquidationError = "Enter an icUSD amount"; return; }
    if (isOverMax(vault)) { liquidationError = "Amount exceeds maximum"; return; }

    const vaultDebt = getVaultDebt(vault);
    const isFullLiquidation = inputAmount >= vaultDebt * 0.999;

    liquidationError = ""; liquidationSuccess = ""; processingVaultId = vault.vault_id;
    try {
      const icusdBalance = await walletOperations.getIcusdBalance();
      if (icusdBalance < inputAmount) {
        liquidationError = `Insufficient icUSD. Need ${formatNumber(inputAmount)}, have ${formatNumber(icusdBalance)}.`;
        processingVaultId = null; return;
      }
      if (!await checkAndApproveAllowance(inputAmount * 1.20)) { processingVaultId = null; return; }
      await loadLiquidatableVaults();
      if (!liquidatableVaults.find(v => v.vault_id === vault.vault_id)) {
        liquidationError = "Vault no longer available"; processingVaultId = null; return;
      }
      let result;
      if (isFullLiquidation) {
        result = await protocolService.liquidateVault(vault.vault_id);
      } else {
        result = await protocolService.partialLiquidateVault(vault.vault_id, inputAmount);
      }
      if (result.success) {
        const seizure = calculateSeizure(vault, inputAmount);
        liquidationSuccess = `Liquidated vault #${vault.vault_id}. Paid ${formatNumber(inputAmount)} icUSD, received ${formatNumber(seizure.icpSeized, 4)} ICP.`;
        liquidationAmounts[vault.vault_id] = '';
        await loadLiquidatableVaults();
      } else { liquidationError = result.error || "Liquidation failed"; }
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      liquidationError = msg.includes('underflow') ? "Vault state changed. Try again." : msg;
    } finally { processingVaultId = null; }
  }

  async function refreshIcpPrice() {
    try { isPriceLoading = true; icpPrice = await protocolService.getICPPrice(); }
    catch (error) { console.error("Error fetching ICP price:", error); }
    finally { isPriceLoading = false; }
  }

  onMount(() => {
    refreshIcpPrice(); loadLiquidatableVaults();
    const pi = setInterval(refreshIcpPrice, 30000);
    const vi = setInterval(loadLiquidatableVaults, 60000);
    return () => { clearInterval(pi); clearInterval(vi); };
  });
</script>

<svelte:head><title>RUMI Protocol - Liquidations</title></svelte:head>

<div class="liq-page">
  <div class="liq-header">
    <h1 class="page-title">Market Liquidations</h1>
    <span class="price-pill">
      ICP
      {#if icpPrice > 0}
        <span class="price-pill-value">${$animatedPrice.toFixed(2)}</span>
      {:else if isPriceLoading}
        <span class="price-pill-value">…</span>
      {:else}
        <span class="price-pill-value">—</span>
      {/if}
    </span>
  </div>

  <div class="liq-summary">
    <span class="summary-stat">{sortedVaults.length} liquidatable vault{sortedVaults.length !== 1 ? 's' : ''}</span>
    <span class="summary-sep">·</span>
    <button class="summary-refresh" on:click={loadLiquidatableVaults} disabled={isLoading}>
      {isLoading ? 'Loading…' : 'Refresh'}
    </button>
  </div>

  {#if !isConnected}
    <div class="msg msg-warn">Connect wallet to liquidate. You'll need icUSD to pay vault debt.</div>
  {/if}
  {#if liquidationError}<div class="msg msg-error">{liquidationError}</div>{/if}
  {#if liquidationSuccess}<div class="msg msg-success">{liquidationSuccess}</div>{/if}

  {#if isLoading && liquidatableVaults.length === 0}
    <div class="loading-state"><div class="spinner"></div></div>
  {:else if sortedVaults.length === 0}
    <div class="empty-state">
      <p class="empty-text">No liquidatable vaults. All positions are healthy.</p>
    </div>
  {:else}
    <div class="liq-list">
      {#each sortedVaults as vault (vault.vault_id)}
        {@const cr = calculateCollateralRatio(vault)}
        {@const debt = getVaultDebt(vault)}
        {@const maxLiq = getMaxLiquidation(vault)}
        {@const isProcessingThis = processingVaultId === vault.vault_id}
        {@const crDanger = cr < 130}
        {@const crCaution = cr >= 130 && cr < 150}

        <div class="liq-card">
          <div class="card-body">
            <!-- LEFT: risk + numbers -->
            <div class="card-left">
              <div class="left-header">
                <span class="vault-id">#{vault.vault_id}</span>
                <span class="cr-badge" class:cr-danger={crDanger} class:cr-caution={crCaution}>
                  {#if crDanger}
                    <svg class="warn-icon" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M8.257 3.099c.765-1.36 2.722-1.36 3.486 0l5.58 9.92c.75 1.334-.213 2.98-1.742 2.98H4.42c-1.53 0-2.493-1.646-1.743-2.98l5.58-9.92zM11 13a1 1 0 11-2 0 1 1 0 012 0zm-1-8a1 1 0 00-1 1v3a1 1 0 002 0V6a1 1 0 00-1-1z" clip-rule="evenodd" /></svg>
                  {/if}
                  {formatNumber(cr, 1)}%
                </span>
              </div>
              <div class="left-stats">
                <span class="stat"><span class="stat-label">Debt</span> <span class="stat-value">{formatNumber(debt, 2)} icUSD</span></span>
                <span class="stat-sep">·</span>
                <span class="stat"><span class="stat-label">Collateral</span> <span class="stat-value">{formatNumber(vault.icp_margin_amount / 1e8, 4)} ICP</span></span>
              </div>
            </div>

            <!-- CENTER: outcome (appears when user types) -->
            <div class="card-center">
              {#if getSeizure(vault)}
                {@const s = getSeizure(vault)}
                <span class="outcome-label">You receive</span>
                <span class="outcome-line">{formatNumber(s.icpSeized, 4)} ICP <span class="outcome-usd">${formatNumber(s.usdValue, 2)}</span></span>
              {/if}
            </div>

            <!-- RIGHT: execution -->
            <div class="card-right">
              <div class="input-label-row">
                <span class="input-label">Amount to liquidate</span>
                {#if maxLiq > 0}
                  <button class="max-text" on:click={() => setMax(vault)}>Max: {formatNumber(maxLiq, 4)}</button>
                {:else if isConnected}
                  <span class="max-loading">Max: ····</span>
                {/if}
              </div>
              <div class="exec-row">
                <div class="input-wrap">
                  <input type="number" class="liq-input" class:input-over={isOverMax(vault)}
                    bind:value={liquidationAmounts[vault.vault_id]}
                    on:input={() => { liquidationAmounts = liquidationAmounts; }}
                    min="0" step="0.01"
                    placeholder="0.00"
                    disabled={isProcessingThis} />
                  <span class="input-suffix">icUSD</span>
                </div>
                <button class="btn-primary btn-sm btn-liquidate"
                  on:click={() => handleLiquidate(vault)}
                  disabled={!isConnected || processingVaultId !== null || !getInputVal(vault) || isOverMax(vault)}>
                  {#if isProcessingThis}
                    {isApprovingAllowance ? 'Approving…' : 'Liquidating…'}
                  {:else}
                    Liquidate
                  {/if}
                </button>
              </div>
            </div>
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .liq-page { max-width: 800px; margin: 0 auto; }
  .liq-header { display: flex; align-items: baseline; gap: 1rem; margin-bottom: 0.25rem; }

  .price-pill {
    display: inline-flex; align-items: baseline; gap: 0.375rem;
    padding: 0.1875rem 0.625rem;
    background: var(--rumi-bg-surface1); border: 1px solid var(--rumi-border);
    border-radius: 1rem; font-size: 0.75rem; color: var(--rumi-text-muted);
    font-family: 'Inter', sans-serif;
  }
  .price-pill-value { font-weight: 600; color: var(--rumi-text-secondary); font-variant-numeric: tabular-nums; }

  .liq-summary { display: flex; align-items: center; gap: 0.5rem; margin-bottom: 1rem; font-size: 0.8125rem; color: var(--rumi-text-muted); }
  .summary-stat { font-variant-numeric: tabular-nums; }
  .summary-sep { opacity: 0.4; }
  .summary-refresh {
    background: none; border: none; cursor: pointer; color: var(--rumi-text-muted);
    font-size: 0.75rem; padding: 0; text-decoration: underline; transition: color 0.15s;
  }
  .summary-refresh:hover { color: var(--rumi-text-secondary); }
  .summary-refresh:disabled { opacity: 0.5; cursor: not-allowed; text-decoration: none; }

  .msg { padding: 0.5rem 0.75rem; border-radius: 0.375rem; font-size: 0.8125rem; margin-bottom: 0.625rem; }
  .msg-warn { background: rgba(245,158,11,0.08); border: 1px solid rgba(245,158,11,0.15); color: #fcd34d; }
  .msg-error { background: rgba(239,68,68,0.08); border: 1px solid rgba(239,68,68,0.15); color: #fca5a5; }
  .msg-success { background: rgba(16,185,129,0.08); border: 1px solid rgba(16,185,129,0.15); color: #6ee7b7; }

  .loading-state { display: flex; justify-content: center; padding: 3rem 0; }
  .spinner { width: 1.25rem; height: 1.25rem; border: 2px solid var(--rumi-border-hover); border-top-color: var(--rumi-action); border-radius: 50%; animation: spin 0.8s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  .empty-state { text-align: center; padding: 3rem 1rem; }
  .empty-text { font-size: 0.875rem; color: var(--rumi-text-secondary); }

  /* ── Card list ── */
  .liq-list { display: flex; flex-direction: column; gap: 0.625rem; }

  .liq-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    box-shadow: inset 0 1px 0 0 rgba(200,210,240,0.03), 0 2px 8px -2px rgba(8,11,22,0.6), 0 1px 3px -1px rgba(14,18,40,0.4);
    transition: border-color 0.15s ease;
  }
  .liq-card:hover {
    border-color: rgba(209,118,232,0.08);
    box-shadow: inset 0 0 20px 0 rgba(209,118,232,0.03), inset 0 1px 0 0 rgba(200,210,240,0.03), 0 2px 8px -2px rgba(8,11,22,0.6);
  }

  /* ── Single-band card body: left numbers, right execution ── */
  .card-body {
    display: flex; align-items: stretch;
    padding: 0.75rem 1rem;
    gap: 1.25rem;
  }

  /* LEFT: risk + numbers */
  .card-left {
    flex: 1; min-width: 0;
    display: flex; flex-direction: column; justify-content: center;
    gap: 0.25rem;
  }

  .left-header {
    display: flex; align-items: center; gap: 0.625rem;
  }
  .vault-id {
    font-family: 'Circular Std','Inter',sans-serif;
    font-weight: 500; font-size: 0.8125rem; color: var(--rumi-text-muted);
  }
  .cr-badge {
    display: inline-flex; align-items: center; gap: 0.25rem;
    font-family: 'Inter', sans-serif; font-weight: 700; font-size: 0.9375rem;
    font-variant-numeric: tabular-nums;
    color: var(--rumi-text-primary);
  }
  .cr-danger { color: var(--rumi-danger); }
  .cr-caution { color: var(--rumi-caution); }
  .warn-icon { width: 0.875rem; height: 0.875rem; flex-shrink: 0; }

  .left-stats {
    display: flex; align-items: baseline; gap: 0.5rem;
    flex-wrap: wrap;
  }
  .stat { display: inline-flex; align-items: baseline; gap: 0.25rem; }
  .stat-label { font-size: 0.6875rem; color: var(--rumi-text-muted); }
  .stat-value {
    font-family: 'Inter', sans-serif; font-weight: 500; font-size: 0.8125rem;
    font-variant-numeric: tabular-nums; color: var(--rumi-text-secondary);
  }
  .stat-sep { color: var(--rumi-text-muted); opacity: 0.3; font-size: 0.75rem; }

  /* CENTER: outcome */
  .card-center {
    flex: 0 0 auto;
    display: flex; flex-direction: column; align-items: center; justify-content: center;
    gap: 0.1875rem;
    min-width: 7rem;
  }
  .outcome-label {
    font-size: 0.6875rem; color: var(--rumi-text-muted); white-space: nowrap;
  }
  .outcome-line {
    font-family: 'Inter', sans-serif; font-weight: 600; font-size: 0.875rem;
    font-variant-numeric: tabular-nums; color: var(--rumi-text-primary); white-space: nowrap;
  }
  .outcome-usd {
    font-weight: 400; font-size: 0.75rem; color: var(--rumi-text-muted);
  }

  /* RIGHT: execution */
  .card-right {
    flex: 0 0 16rem;
    display: flex; flex-direction: column; justify-content: center;
    gap: 0.25rem;
  }

  .input-label-row {
    display: flex; justify-content: space-between; align-items: baseline; gap: 0.5rem;
  }
  .input-label { font-size: 0.6875rem; font-weight: 500; color: var(--rumi-text-muted); }

  .max-text {
    background: none; border: none; cursor: pointer; padding: 0;
    font-size: 0.6875rem; font-weight: 500; white-space: nowrap;
    color: var(--rumi-text-muted); opacity: 0.85;
    transition: opacity 0.15s;
  }
  .max-text:hover { opacity: 1; text-decoration: underline; }

  .max-loading {
    font-size: 0.6875rem; font-weight: 500; white-space: nowrap;
    color: var(--rumi-text-muted); opacity: 0.5;
    animation: pulse-subtle 1.5s ease-in-out infinite;
  }
  @keyframes pulse-subtle { 0%, 100% { opacity: 0.35; } 50% { opacity: 0.65; } }

  .exec-row { display: flex; gap: 0.375rem; align-items: center; }

  .input-wrap { position: relative; flex: 1; }
  .liq-input {
    width: 100%; padding: 0.375rem 2.75rem 0.375rem 0.5rem;
    background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border);
    border-radius: 0.375rem; color: var(--rumi-text-primary);
    font-family: 'Inter', sans-serif; font-size: 0.8125rem;
    font-variant-numeric: tabular-nums; transition: border-color 0.15s;
  }
  .liq-input:focus { outline: none; border-color: var(--rumi-teal); box-shadow: 0 0 0 1px rgba(45,212,191,0.12); }
  .liq-input:disabled { opacity: 0.5; }
  .liq-input.input-over { color: var(--rumi-text-muted); opacity: 0.5; }
  .input-suffix {
    position: absolute; right: 0.5rem; top: 50%; transform: translateY(-50%);
    font-size: 0.6875rem; color: var(--rumi-text-muted); pointer-events: none;
  }

  .btn-liquidate {
    padding: 0.375rem 0.875rem; white-space: nowrap; flex-shrink: 0;
    font-family: 'Inter', sans-serif;
  }

  /* Number input cleanup */
  .liq-input::-webkit-outer-spin-button,
  .liq-input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }
  .liq-input[type=number] { -moz-appearance: textfield; appearance: textfield; }

  @media (max-width: 640px) {
    .liq-header { flex-wrap: wrap; }
    .card-body { flex-direction: column; gap: 0.625rem; }
    .card-right { flex: none; }
  }
</style>
