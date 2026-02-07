<script lang="ts">
  import { onMount, onDestroy } from "svelte";
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
  let partialLiquidationAmounts: { [vaultId: number]: number } = {};
  let hoveredVaultId: number | null = null;
  
  $: isConnected = $wallet.isConnected;
  
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
  
  function calculateLiquidationProfit(vault: CandidVault): { icpAmount: number, profitUsd: number } {
    const icusdDebt = Number(vault.borrowed_icusd_amount || 0) / 1e8;
    const icpCollateral = Number(vault.icp_margin_amount || 0) / 1e8;
    const currentPrice = icpPrice || 1;
    let icpReceived = currentPrice > 0 ? icusdDebt / currentPrice * (1 / 0.9) : 0;
    const icpToReceive = Math.min(icpReceived, icpCollateral);
    const profitUsd = (icpToReceive * currentPrice) - icusdDebt;
    return { icpAmount: isFinite(icpToReceive) ? icpToReceive : 0, profitUsd: isFinite(profitUsd) ? profitUsd : 0 };
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

  async function checkAndApproveAllowance(vaultId: number, icusdAmount: number): Promise<boolean> {
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

  async function liquidateVault(vaultId: number) {
    if (!isConnected) { liquidationError = "Please connect your wallet"; return; }
    if (processingVaultId !== null) return;
    const vault = liquidatableVaults.find(v => v.vault_id === vaultId);
    if (!vault) { liquidationError = "Vault not found"; return; }
    liquidationError = ""; liquidationSuccess = ""; processingVaultId = vaultId;
    try {
      const icusdDebt = Number(vault.borrowed_icusd_amount) / 1e8;
      const icusdBalance = await walletOperations.getIcusdBalance();
      if (icusdBalance < icusdDebt) { liquidationError = `Insufficient icUSD. Need ${formatNumber(icusdDebt)}, have ${formatNumber(icusdBalance)}.`; processingVaultId = null; return; }
      if (!await checkAndApproveAllowance(vaultId, icusdDebt * 1.20)) { processingVaultId = null; return; }
      await loadLiquidatableVaults();
      if (!liquidatableVaults.find(v => v.vault_id === vaultId)) { liquidationError = "Vault no longer available"; processingVaultId = null; return; }
      const result = await protocolService.liquidateVault(vaultId);
      if (result.success) {
        liquidationSuccess = `Liquidated vault #${vaultId}. Paid ${formatNumber(icusdDebt)} icUSD, received ≈${formatNumber(calculateLiquidationProfit(vault).icpAmount)} ICP.`;
        await loadLiquidatableVaults();
      } else { liquidationError = result.error || "Liquidation failed"; }
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      liquidationError = msg.includes('underflow') ? "Vault state changed. Try again." : msg;
    } finally { processingVaultId = null; }
  }

  async function partialLiquidateVault(vaultId: number, liquidateAmount: number) {
    if (!isConnected) { liquidationError = "Please connect your wallet"; return; }
    if (processingVaultId !== null) return;
    const vault = liquidatableVaults.find(v => v.vault_id === vaultId);
    if (!vault) { liquidationError = "Vault not found"; return; }
    liquidationError = ""; liquidationSuccess = ""; processingVaultId = vaultId;
    try {
      const icusdBalance = await walletOperations.getIcusdBalance();
      if (icusdBalance < liquidateAmount) { liquidationError = `Insufficient icUSD. Need ${formatNumber(liquidateAmount)}, have ${formatNumber(icusdBalance)}.`; processingVaultId = null; return; }
      if (!await checkAndApproveAllowance(vaultId, liquidateAmount * 1.20)) { processingVaultId = null; return; }
      await loadLiquidatableVaults();
      if (!liquidatableVaults.find(v => v.vault_id === vaultId)) { liquidationError = "Vault no longer available"; processingVaultId = null; return; }
      const result = await protocolService.partialLiquidateVault(vaultId, liquidateAmount);
      if (result.success) {
        const expectedIcp = (liquidateAmount / 0.9) / icpPrice;
        liquidationSuccess = `Partially liquidated vault #${vaultId}. Paid ${formatNumber(liquidateAmount)} icUSD, received ≈${formatNumber(expectedIcp)} ICP.`;
        await loadLiquidatableVaults();
      } else { liquidationError = result.error || "Partial liquidation failed"; }
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
    <div class="table-wrap">
      <table class="liq-table">
        <thead>
          <tr>
            <th class="th-id">#</th>
            <th class="th-profit">Profit</th>
            <th class="th-ratio">CR</th>
            <th class="th-debt">Debt</th>
            <th class="th-collateral">Collateral</th>
            <th class="th-action">Action</th>
          </tr>
        </thead>
        <tbody>
          {#each sortedVaults as vault (vault.vault_id)}
            {@const cr = calculateCollateralRatio(vault)}
            {@const profit = calculateLiquidationProfit(vault)}
            {@const isHovered = hoveredVaultId === vault.vault_id}
            {@const isProcessingThis = processingVaultId === vault.vault_id}
            <tr class="liq-row"
              on:mouseenter={() => hoveredVaultId = vault.vault_id}
              on:mouseleave={() => hoveredVaultId = null}>
              <td class="cell-id">{vault.vault_id}</td>
              <td class="cell-profit">
                <span class="profit-icp">{formatNumber(profit.icpAmount, 4)} ICP</span>
                <span class="profit-usd">≈${formatNumber(profit.profitUsd, 2)}</span>
              </td>
              <td class="cell-ratio">
                <span class="ratio-value" class:ratio-danger={cr < 130} class:ratio-caution={cr >= 130 && cr < 150}>
                  {formatNumber(cr, 1)}%
                </span>
              </td>
              <td class="cell-debt">
                <span class="debt-value">{formatNumber(vault.borrowed_icusd_amount / 1e8, 2)}</span>
                <span class="debt-unit">icUSD</span>
              </td>
              <td class="cell-collateral">
                <span class="coll-value">{formatNumber(vault.icp_margin_amount / 1e8, 4)}</span>
                <span class="coll-unit">ICP</span>
              </td>
              <td class="cell-action">
                {#if isProcessingThis}
                  <span class="action-processing">{isApprovingAllowance ? 'Approving…' : 'Liquidating…'}</span>
                {:else}
                  <div class="action-stack">
                    <button class="btn-liquidate"
                      on:click={() => liquidateVault(vault.vault_id)}
                      disabled={!isConnected || processingVaultId !== null}>
                      Liquidate
                    </button>
                    <div class="partial-row" class:partial-visible={isHovered || partialLiquidationAmounts[vault.vault_id]}>
                      <input type="number" class="partial-input"
                        bind:value={partialLiquidationAmounts[vault.vault_id]}
                        min="0" max={vault.borrowed_icusd_amount / 1e8} step="0.01"
                        placeholder="Partial icUSD" />
                      <button class="btn-partial"
                        on:click={() => partialLiquidationAmounts[vault.vault_id] && partialLiquidateVault(vault.vault_id, partialLiquidationAmounts[vault.vault_id])}
                        disabled={!isConnected || processingVaultId !== null || !partialLiquidationAmounts[vault.vault_id]}>
                        Go
                      </button>
                    </div>
                  </div>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<style>
  .liq-page { max-width: 900px; margin: 0 auto; }
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

  .table-wrap { overflow-x: auto; }
  .liq-table { width: 100%; border-collapse: collapse; font-family: 'Inter', sans-serif; }
  .liq-table thead th {
    padding: 0.375rem 0.625rem; text-align: left;
    font-size: 0.6875rem; font-weight: 600; text-transform: uppercase;
    letter-spacing: 0.06em; color: var(--rumi-text-secondary);
    border-bottom: 1px solid var(--rumi-border-hover); white-space: nowrap; user-select: none;
  }
  .th-action { text-align: right; }

  .liq-row td {
    padding: 0.5rem 0.625rem; font-size: 0.8125rem; color: var(--rumi-text-primary);
    border-bottom: 1px solid var(--rumi-border); vertical-align: middle;
    white-space: nowrap; transition: background 0.1s ease;
  }
  .liq-row:hover td { background: rgba(90,100,180,0.05); }
  .liq-row:last-child td { border-bottom: none; }

  .cell-id { font-size: 0.75rem; color: var(--rumi-text-muted); font-variant-numeric: tabular-nums; }
  .cell-profit { min-width: 8rem; }
  .profit-icp { display: block; font-weight: 600; font-size: 0.875rem; font-variant-numeric: tabular-nums; color: var(--rumi-text-primary); }
  .profit-usd { display: block; font-size: 0.6875rem; color: var(--rumi-text-muted); font-variant-numeric: tabular-nums; }

  .cell-ratio { font-variant-numeric: tabular-nums; }
  .ratio-value { font-weight: 500; font-size: 0.8125rem; color: var(--rumi-text-primary); }
  .ratio-danger { color: var(--rumi-danger); }
  .ratio-caution { color: var(--rumi-caution); }

  .cell-debt, .cell-collateral { font-variant-numeric: tabular-nums; }
  .debt-value, .coll-value { font-weight: 500; }
  .debt-unit, .coll-unit { font-size: 0.6875rem; color: var(--rumi-text-muted); margin-left: 0.25rem; }

  .cell-action { text-align: right; min-width: 9rem; }
  .action-processing { font-size: 0.75rem; color: var(--rumi-text-muted); font-style: italic; }
  .action-stack { display: flex; flex-direction: column; align-items: flex-end; gap: 0.25rem; }

  .btn-liquidate {
    padding: 0.25rem 0.625rem;
    background: rgba(239, 68, 68, 0.12); border: 1px solid rgba(239, 68, 68, 0.25);
    border-radius: 0.25rem; color: #f87171;
    font-size: 0.75rem; font-weight: 500; cursor: pointer;
    transition: background 0.15s, border-color 0.15s; font-family: 'Inter', sans-serif;
  }
  .btn-liquidate:hover { background: rgba(239, 68, 68, 0.20); border-color: rgba(239, 68, 68, 0.4); }
  .btn-liquidate:disabled { opacity: 0.4; cursor: not-allowed; }

  .partial-row {
    display: flex; gap: 0.25rem; align-items: center;
    opacity: 0; height: 0; overflow: hidden;
    transition: opacity 0.15s ease, height 0.15s ease;
  }
  .partial-visible { opacity: 1; height: auto; overflow: visible; }

  .partial-input {
    width: 5.5rem; padding: 0.1875rem 0.375rem;
    background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border);
    border-radius: 0.25rem; color: var(--rumi-text-primary);
    font-size: 0.6875rem; font-family: 'Inter', sans-serif;
    font-variant-numeric: tabular-nums; transition: border-color 0.15s;
  }
  .partial-input:focus { outline: none; border-color: var(--rumi-teal); }
  .partial-input::placeholder { color: var(--rumi-text-muted); font-size: 0.625rem; }

  .btn-partial {
    padding: 0.1875rem 0.5rem;
    background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border-hover);
    border-radius: 0.25rem; color: var(--rumi-text-secondary);
    font-size: 0.6875rem; font-weight: 500; cursor: pointer;
    transition: border-color 0.15s, color 0.15s; font-family: 'Inter', sans-serif;
  }
  .btn-partial:hover { border-color: var(--rumi-action); color: var(--rumi-action); }
  .btn-partial:disabled { opacity: 0.4; cursor: not-allowed; }

  .partial-input::-webkit-outer-spin-button,
  .partial-input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }
  .partial-input[type=number] { -moz-appearance: textfield; appearance: textfield; }

  @media (max-width: 640px) {
    .liq-header { flex-wrap: wrap; }
    .th-collateral, .cell-collateral { display: none; }
    .cell-action { min-width: 7rem; }
  }
</style>
