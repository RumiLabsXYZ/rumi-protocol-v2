<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { walletStore as wallet } from "$lib/stores/wallet";
  import { protocolService } from '$lib/services/protocol';
  import { formatNumber } from '$lib/utils/format';
  import ProtocolStats from '$lib/components/dashboard/ProtocolStats.svelte';
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
  
  $: isConnected = $wallet.isConnected;
  
  let animatedPrice = tweened(0, { duration: 600, easing: cubicOut });
  $: if (icpPrice > 0) { animatedPrice.set(icpPrice); }
  
  function calculateCollateralRatio(vault: CandidVault): number {
    const icpAmount = Number(vault.icp_margin_amount || 0) / 100_000_000;
    const icusdAmount = Number(vault.borrowed_icusd_amount || 0) / 100_000_000;
    if (icusdAmount <= 0) return Infinity;
    const ratio = (icpAmount * icpPrice / icusdAmount) * 100;
    return isFinite(ratio) ? ratio : 0;
  }
  
  function calculateLiquidationProfit(vault: CandidVault): { icpAmount: number, profitUsd: number } {
    const icusdDebt = Number(vault.borrowed_icusd_amount || 0) / 100_000_000;
    const icpCollateral = Number(vault.icp_margin_amount || 0) / 100_000_000;
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
      const amountE8s = BigInt(Math.floor(icusdAmount * 100_000_000));
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
      const icusdDebt = Number(vault.borrowed_icusd_amount) / 100_000_000;
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

<div class="page-container">
  <h1 class="page-title">Market Liquidations</h1>

  <div class="page-layout">
    <!-- LEFT: Liquidatable vaults (primary) -->
    <div class="action-column">
      <!-- ICP price inline context -->
      <div class="price-inline">
        <span class="price-label">ICP Price</span>
        <span class="price-value">
          {#if icpPrice > 0}${$animatedPrice.toFixed(2)}{:else if isPriceLoading}…{:else}—{/if}
        </span>
      </div>

      <div class="action-card">
        <div class="card-header">
          <h2 class="section-title">Liquidatable Vaults</h2>
          <button class="btn-secondary btn-sm" on:click={loadLiquidatableVaults} disabled={isLoading}>
            {isLoading ? 'Loading…' : 'Refresh'}
          </button>
        </div>

        {#if !isConnected}
          <div class="msg-warn">Connect your wallet to liquidate. You'll need icUSD to pay vault debt.</div>
        {/if}
        {#if liquidationError}<div class="msg-error">{liquidationError}</div>{/if}
        {#if liquidationSuccess}<div class="msg-success">{liquidationSuccess}</div>{/if}

        {#if isLoading}
          <div class="loading-state">
            <div class="w-8 h-8 border-3 border-green-400 border-t-transparent rounded-full animate-spin"></div>
          </div>
        {:else if liquidatableVaults.length === 0}
          <div class="empty-state">
            <svg class="empty-icon" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
            </svg>
            <p class="empty-text">No liquidatable vaults. All positions are healthy.</p>
          </div>
        {:else}
          <div class="vault-table-wrap">
            <table class="vault-table">
              <thead>
                <tr>
                  <th>Vault</th>
                  <th>Debt</th>
                  <th>Collateral</th>
                  <th>Ratio</th>
                  <th>Profit</th>
                  <th class="text-right">Action</th>
                </tr>
              </thead>
              <tbody>
                {#each liquidatableVaults as vault (vault.vault_id)}
                  {@const cr = calculateCollateralRatio(vault)}
                  {@const profit = calculateLiquidationProfit(vault)}
                  <tr>
                    <td>#{vault.vault_id}</td>
                    <td>{formatNumber(vault.borrowed_icusd_amount / 1e8)} icUSD</td>
                    <td>{formatNumber(vault.icp_margin_amount / 1e8)} ICP</td>
                    <td><span class="text-danger">{formatNumber(cr)}%</span></td>
                    <td>
                      <span>{formatNumber(profit.icpAmount)} ICP</span>
                      <span class="profit-usd">≈ ${formatNumber(profit.profitUsd)}</span>
                    </td>
                    <td class="text-right">
                      <div class="action-group">
                        <div class="partial-row">
                          <input type="number" bind:value={partialLiquidationAmounts[vault.vault_id]}
                            min="0" max={vault.borrowed_icusd_amount / 1e8} step="0.01"
                            placeholder="icUSD" class="icp-input partial-input" />
                          <button class="btn-secondary btn-sm"
                            on:click={() => partialLiquidationAmounts[vault.vault_id] && partialLiquidateVault(vault.vault_id, partialLiquidationAmounts[vault.vault_id])}
                            disabled={processingVaultId !== null || !isConnected || !partialLiquidationAmounts[vault.vault_id]}>
                            Partial
                          </button>
                        </div>
                        <button class="btn-danger btn-sm"
                          on:click={() => liquidateVault(vault.vault_id)}
                          disabled={processingVaultId !== null || !isConnected}>
                          {#if processingVaultId === vault.vault_id}
                            {isApprovingAllowance ? 'Approving…' : 'Liquidating…'}
                          {:else}Full Liquidate{/if}
                        </button>
                      </div>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </div>
    </div>

    <!-- RIGHT: Protocol context -->
    <div class="context-column">
      <ProtocolStats />
    </div>
  </div>
</div>

<style>
  .page-container { max-width: 1100px; margin: 0 auto; }
  .page-layout { display: grid; grid-template-columns: 1fr 280px; gap: 1.5rem; align-items: start; }
  .action-column { min-width: 0; }
  .action-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    padding: 1.5rem;
  }
  .context-column { position: sticky; top: 5rem; }

  .price-inline { display: flex; align-items: baseline; gap: 0.5rem; margin-bottom: 0.75rem; }
  .price-label { font-size: 0.8125rem; color: var(--rumi-text-muted); }
  .price-value { font-family: 'Inter', sans-serif; font-size: 0.875rem; font-weight: 600; color: var(--rumi-text-secondary); font-variant-numeric: tabular-nums; }

  .card-header { display: flex; justify-content: space-between; align-items: center; margin-bottom: 1rem; }
  .btn-sm { padding: 0.375rem 0.75rem; font-size: 0.75rem; }

  /* Messages */
  .msg-warn { padding: 0.625rem; background: rgba(245,158,11,0.1); border: 1px solid rgba(245,158,11,0.2); border-radius: 0.5rem; font-size: 0.8125rem; color: #fcd34d; margin-bottom: 0.75rem; }
  .msg-error { padding: 0.625rem; background: rgba(239,68,68,0.1); border: 1px solid rgba(239,68,68,0.2); border-radius: 0.5rem; font-size: 0.8125rem; color: #fca5a5; margin-bottom: 0.75rem; }
  .msg-success { padding: 0.625rem; background: rgba(16,185,129,0.1); border: 1px solid rgba(16,185,129,0.2); border-radius: 0.5rem; font-size: 0.8125rem; color: #6ee7b7; margin-bottom: 0.75rem; }

  /* States */
  .loading-state { display: flex; justify-content: center; padding: 3rem 0; }
  .empty-state { text-align: center; padding: 3rem 1rem; }
  .empty-icon { width: 2.5rem; height: 2.5rem; color: var(--rumi-text-muted); margin: 0 auto 0.75rem; }
  .empty-text { font-size: 0.875rem; color: var(--rumi-text-secondary); }

  /* Table */
  .vault-table-wrap { overflow-x: auto; }
  .vault-table { width: 100%; border-collapse: collapse; }
  .vault-table th { padding: 0.5rem 0.75rem; text-align: left; font-size: 0.6875rem; font-weight: 500; text-transform: uppercase; letter-spacing: 0.05em; color: var(--rumi-text-muted); border-bottom: 1px solid var(--rumi-border); }
  .vault-table td { padding: 0.75rem; font-size: 0.8125rem; color: var(--rumi-text-primary); border-bottom: 1px solid var(--rumi-border); vertical-align: middle; }
  .vault-table tbody tr:hover { background: rgba(90,100,180,0.04); }
  .text-danger { color: var(--rumi-danger); }
  .text-right { text-align: right; }
  .profit-usd { display: block; font-size: 0.75rem; color: var(--rumi-safe); }

  /* Action cells */
  .action-group { display: flex; flex-direction: column; gap: 0.375rem; align-items: flex-end; }
  .partial-row { display: flex; gap: 0.375rem; align-items: center; }
  .partial-input { width: 5rem; padding: 0.25rem 0.5rem; font-size: 0.75rem; }

  @media (max-width: 768px) {
    .page-layout { grid-template-columns: 1fr; }
    .context-column { position: static; }
  }
</style>
