<script lang="ts">
  import { formatNumber } from '../../utils/format';
  import type { Vault } from '../../services/types';
  import { protocolService } from '../../services/protocol';
  import { vaultStore } from '../../stores/vaultStore';
  import { protocolManager } from '../../services/ProtocolManager';
  import { CONFIG } from '../../config';
  import { createEventDispatcher } from 'svelte';
  import { walletStore } from '../../stores/wallet';

  export let vault: Vault;
  export let icpPrice: number = 0;
  export let expandedVaultId: number | null = null;

  const dispatch = createEventDispatcher<{ updated: void; toggle: { vaultId: number } }>();
  const E8S = 100_000_000;
  const MINT_MINIMUM = 1.5;

  $: expanded = expandedVaultId === vault.vaultId;

  function toggleExpand() {
    dispatch('toggle', { vaultId: vault.vaultId });
    clearMessages();
    if (!expanded) {
      addCollateralAmount = ''; borrowAmount = ''; repayAmount = '';
    }
  }

  // ── Derived display ──
  $: collateralValueUsd = vault.icpMargin * icpPrice;
  $: collateralRatio = vault.borrowedIcusd > 0
    ? collateralValueUsd / vault.borrowedIcusd : Infinity;
  $: borrowedValueUsd = vault.borrowedIcusd;
  $: riskLevel = getRiskLevel(collateralRatio);
  $: maxBorrowable = Math.max(0, (collateralValueUsd / MINT_MINIMUM) - vault.borrowedIcusd);

  // Wallet balances for input caps
  $: walletIcp = $walletStore.tokenBalances?.ICP
    ? parseFloat($walletStore.tokenBalances.ICP.formatted) : 0;
  $: walletIcusd = $walletStore.tokenBalances?.ICUSD
    ? parseFloat($walletStore.tokenBalances.ICUSD.formatted) : 0;
  $: maxAddCollateral = walletIcp;
  $: maxRepayable = Math.min(walletIcusd, vault.borrowedIcusd);

  $: fmtMargin = formatNumber(vault.icpMargin, 4);
  $: fmtCollateralUsd = formatNumber(collateralValueUsd, 2);
  $: fmtBorrowed = formatNumber(vault.borrowedIcusd, 2);
  $: fmtBorrowedUsd = formatNumber(borrowedValueUsd, 2);
  $: fmtRatio = collateralRatio === Infinity ? '—' : `${(collateralRatio * 100).toFixed(1)}%`;
  $: riskTooltip = riskLevel === 'warning'
    ? 'Approaching minimum collateral ratio'
    : riskLevel === 'danger' ? 'At risk of liquidation. Add collateral or repay.' : '';

  function getRiskLevel(ratio: number): 'normal' | 'warning' | 'danger' {
    if (ratio === Infinity || ratio >= 1.5) return 'normal';
    if (ratio >= 1.4) return 'warning';  // 140–149.9% = amber
    return 'danger';                      // <140% = red
  }

  // ── Action state ──
  let addCollateralAmount = '';
  let borrowAmount = '';
  let repayAmount = '';
  let isProcessing = false;
  let actionError = '';
  let actionSuccess = '';
  let showAdvanced = false;
  let isWithdrawingAndClosing = false;

  // Single active intent: track which field is active
  $: activeIntent = addCollateralAmount ? 'add'
    : borrowAmount ? 'borrow'
    : repayAmount ? 'repay'
    : null;

  function onAddInput() { borrowAmount = ''; repayAmount = ''; }
  function onBorrowInput() { addCollateralAmount = ''; repayAmount = ''; }
  function onRepayInput() { addCollateralAmount = ''; borrowAmount = ''; }

  // ── Projected CR calculations ──
  $: projectedCrAdd = (() => {
    const amt = parseFloat(addCollateralAmount);
    if (!amt || amt <= 0 || !icpPrice) return null;
    const newCollateral = (vault.icpMargin + amt) * icpPrice;
    return vault.borrowedIcusd > 0 ? newCollateral / vault.borrowedIcusd : Infinity;
  })();

  $: projectedCrBorrow = (() => {
    const amt = parseFloat(borrowAmount);
    if (!amt || amt <= 0) return null;
    const newDebt = vault.borrowedIcusd + amt;
    return newDebt > 0 ? collateralValueUsd / newDebt : Infinity;
  })();

  $: projectedCrRepay = (() => {
    const amt = parseFloat(repayAmount);
    if (!amt || amt <= 0) return null;
    const newDebt = vault.borrowedIcusd - amt;
    return newDebt > 0 ? collateralValueUsd / newDebt : Infinity;
  })();

  function fmtProjectedCr(ratio: number | null): string {
    if (ratio === null) return '';
    if (ratio === Infinity) return '∞';
    return `${(ratio * 100).toFixed(1)}%`;
  }

  function projectedRisk(ratio: number | null): 'normal' | 'warning' | 'danger' {
    if (ratio === null || ratio === Infinity) return 'normal';
    return getRiskLevel(ratio);
  }

  // Whether projected CR is invalid (below minimum 150%) — disables action button
  $: borrowCrInvalid = projectedCrBorrow !== null && projectedCrBorrow !== Infinity && projectedCrBorrow < MINT_MINIMUM;

  // Whether input exceeds max — disables action button
  $: addOverMax = (() => {
    const v = parseFloat(addCollateralAmount);
    return v > 0 && maxAddCollateral > 0 && v > maxAddCollateral;
  })();
  $: borrowOverMax = (() => {
    const v = parseFloat(borrowAmount);
    return v > 0 && maxBorrowable > 0 && v > maxBorrowable;
  })();
  $: repayOverMax = (() => {
    const v = parseFloat(repayAmount);
    return v > 0 && maxRepayable > 0 && v > maxRepayable;
  })();

  $: canWithdraw = vault.borrowedIcusd === 0 && vault.icpMargin > 0;
  $: canClose = vault.borrowedIcusd === 0;

  function clearMessages() { actionError = ''; actionSuccess = ''; }

  function setMaxAddCollateral() {
    if (maxAddCollateral > 0) {
      borrowAmount = ''; repayAmount = '';
      addCollateralAmount = maxAddCollateral.toFixed(4);
    }
  }
  function setMaxBorrow() {
    if (maxBorrowable > 0) {
      addCollateralAmount = ''; repayAmount = '';
      borrowAmount = maxBorrowable.toFixed(2);
    }
  }
  function setMaxRepay() {
    if (maxRepayable > 0) {
      addCollateralAmount = ''; borrowAmount = '';
      repayAmount = maxRepayable.toFixed(4);
    }
  }

  // Clamp on blur — only clamp to zero if empty/negative, do NOT auto-reduce over-max
  function clampAddCollateral() {
    const v = parseFloat(addCollateralAmount);
    if (isNaN(v) || v < 0) { addCollateralAmount = ''; return; }
  }
  function clampBorrow() {
    const v = parseFloat(borrowAmount);
    if (isNaN(v) || v < 0) { borrowAmount = ''; return; }
  }
  function clampRepay() {
    const v = parseFloat(repayAmount);
    if (isNaN(v) || v < 0) { repayAmount = ''; return; }
  }

  async function handleAddCollateral() {
    const amount = parseFloat(addCollateralAmount);
    if (!amount || amount <= 0) { actionError = 'Enter a valid ICP amount'; return; }
    if (addOverMax) { actionError = `Exceeds wallet balance (${formatNumber(maxAddCollateral, 4)} ICP)`; return; }
    clearMessages(); isProcessing = true;
    try {
      const amountE8s = BigInt(Math.floor(amount * E8S));
      const spenderCanisterId = CONFIG.currentCanisterId;
      const currentAllowance = await protocolService.checkIcpAllowance(spenderCanisterId);
      if (currentAllowance < amountE8s) {
        const bufferAmount = amountE8s * BigInt(120) / BigInt(100);
        const approvalResult = await protocolService.approveIcpTransfer(bufferAmount, spenderCanisterId);
        if (!approvalResult.success) { actionError = approvalResult.error || 'Approval failed'; return; }
        await new Promise(r => setTimeout(r, 2000));
      }
      const result = await protocolService.addMarginToVault(vault.vaultId, amount);
      if (result.success) {
        actionSuccess = `Added ${amount} ICP`; addCollateralAmount = '';
        await vaultStore.refreshVault(vault.vaultId); dispatch('updated');
      } else { actionError = result.error || 'Failed'; }
    } catch (err) { actionError = err instanceof Error ? err.message : 'Unknown error';
    } finally { isProcessing = false; }
  }

  async function handleBorrow() {
    const amount = parseFloat(borrowAmount);
    if (!amount || amount <= 0) { actionError = 'Enter a valid icUSD amount'; return; }
    if (borrowOverMax || borrowCrInvalid) { actionError = `Max: ${formatNumber(maxBorrowable, 2)} icUSD`; return; }
    clearMessages(); isProcessing = true;
    try {
      const result = await protocolService.borrowFromVault(vault.vaultId, amount);
      if (result.success) {
        actionSuccess = `Borrowed ${amount} icUSD`; borrowAmount = '';
        await vaultStore.refreshVault(vault.vaultId); dispatch('updated');
      } else { actionError = result.error || 'Failed'; }
    } catch (err) { actionError = err instanceof Error ? err.message : 'Unknown error';
    } finally { isProcessing = false; }
  }

  async function handleRepay() {
    const amount = parseFloat(repayAmount);
    if (!amount || amount <= 0) { actionError = 'Enter a valid amount'; return; }
    if (repayOverMax) { actionError = `Max: ${formatNumber(maxRepayable, 2)} icUSD`; return; }
    clearMessages(); isProcessing = true;
    try {
      const result = await protocolManager.repayToVault(vault.vaultId, amount);
      if (result.success) {
        actionSuccess = `Repaid ${amount} icUSD`; repayAmount = '';
        await new Promise(r => setTimeout(r, 1000));
        await vaultStore.refreshVault(vault.vaultId); dispatch('updated');
      } else { actionError = result.error || 'Failed'; }
    } catch (err) { actionError = err instanceof Error ? err.message : 'Unknown error';
    } finally { isProcessing = false; }
  }

  async function handleWithdrawAndClose() {
    if (!canWithdraw) { actionError = 'Repay all debt first'; return; }
    clearMessages(); isWithdrawingAndClosing = true;
    try {
      const result = await protocolService.withdrawCollateralAndCloseVault(vault.vaultId);
      if (result.success) {
        actionSuccess = 'Vault closed. Collateral returned.';
        await vaultStore.refreshVaults(); dispatch('updated');
      } else { actionError = result.error || 'Failed'; }
    } catch (err) { actionError = err instanceof Error ? err.message : 'Unknown error';
    } finally { isWithdrawingAndClosing = false; }
  }
</script>

<!-- ── Collapsed row ── -->
<div class="vault-card" class:vault-card-danger={riskLevel === 'danger'} class:vault-card-warning={riskLevel === 'warning'}>
  <button class="vault-row" on:click={toggleExpand}>
    <span class="vault-id">#{vault.vaultId}</span>
    <span class="vault-cell">
      <span class="cell-label">Collateral</span>
      <span class="cell-value">{fmtMargin} ICP</span>
      <span class="cell-sub">${fmtCollateralUsd}</span>
    </span>
    <span class="vault-cell">
      <span class="cell-label">Borrowed</span>
      <span class="cell-value">{fmtBorrowed} icUSD</span>
      <span class="cell-sub">${fmtBorrowedUsd}</span>
    </span>
    <span class="vault-cell vault-cell-ratio">
      <span class="cell-label">CR</span>
      <span class="cell-value ratio-text" class:ratio-warning={riskLevel === 'warning'} class:ratio-danger={riskLevel === 'danger'} title={riskTooltip}>
        {#if riskLevel !== 'normal'}
          <svg class="warn-icon" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M8.257 3.099c.765-1.36 2.722-1.36 3.486 0l5.58 9.92c.75 1.334-.213 2.98-1.742 2.98H4.42c-1.53 0-2.493-1.646-1.743-2.98l5.58-9.92zM11 13a1 1 0 11-2 0 1 1 0 012 0zm-1-8a1 1 0 00-1 1v3a1 1 0 002 0V6a1 1 0 00-1-1z" clip-rule="evenodd" /></svg>
        {/if}
        {fmtRatio}
      </span>
    </span>
    <span class="vault-chevron" class:vault-chevron-open={expanded}>
      <svg viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z" clip-rule="evenodd" /></svg>
    </span>
  </button>

  <!-- ── Expanded: action panels ── -->
  {#if expanded}
    <div class="vault-actions">
      {#if actionError}
        <div class="msg-bar msg-error">{actionError}
          <button class="msg-dismiss" on:click={() => actionError = ''}>×</button>
        </div>
      {/if}
      {#if actionSuccess}
        <div class="msg-bar msg-success">{actionSuccess}
          <button class="msg-dismiss" on:click={() => actionSuccess = ''}>×</button>
        </div>
      {/if}

      <div class="action-grid">
        <!-- Add Collateral -->
        <div class="action-panel" class:panel-inactive={activeIntent && activeIntent !== 'add'}>
          <span class="action-label-row">
            <span class="action-label">Add Collateral</span>
            {#if maxAddCollateral > 0}
              <button class="max-text" on:click={setMaxAddCollateral}>Max: {formatNumber(maxAddCollateral, 4)} ICP</button>
            {/if}
          </span>
          <div class="action-input-row">
            <input type="number" class="action-input" bind:value={addCollateralAmount}
              on:input={onAddInput} on:blur={clampAddCollateral}
              placeholder="0.00" min="0.001" step="0.01" disabled={isProcessing} />
            <span class="input-suffix">ICP</span>
          </div>
          <div class="action-btn-row">
            <button class="btn-primary btn-sm btn-action" on:click={handleAddCollateral}
              disabled={isProcessing || !addCollateralAmount || addOverMax}>
              {isProcessing && activeIntent === 'add' ? '…' : 'Add'}
            </button>
            {#if projectedCrAdd !== null}
              <span class="cr-preview" class:cr-warning={projectedRisk(projectedCrAdd) === 'warning'}
                class:cr-danger={projectedRisk(projectedCrAdd) === 'danger'}>→ CR {fmtProjectedCr(projectedCrAdd)}</span>
            {/if}
          </div>
        </div>

        <!-- Borrow -->
        <div class="action-panel" class:panel-inactive={activeIntent && activeIntent !== 'borrow'}>
          <span class="action-label-row">
            <span class="action-label">Borrow</span>
            {#if maxBorrowable > 0}
              <button class="max-text" on:click={setMaxBorrow}>Max: {formatNumber(maxBorrowable, 2)} icUSD</button>
            {/if}
          </span>
          <div class="action-input-row">
            <input type="number" class="action-input" bind:value={borrowAmount}
              on:input={onBorrowInput} on:blur={clampBorrow}
              placeholder="0.00" min="0.1" step="0.1" disabled={isProcessing} />
            <span class="input-suffix">icUSD</span>
          </div>
          <div class="action-btn-row">
            <button class="btn-primary btn-sm btn-action" on:click={handleBorrow}
              disabled={isProcessing || !borrowAmount || borrowCrInvalid || borrowOverMax}>
              {isProcessing && activeIntent === 'borrow' ? '…' : 'Borrow'}
            </button>
            {#if projectedCrBorrow !== null}
              <span class="cr-preview" class:cr-warning={projectedRisk(projectedCrBorrow) === 'warning'}
                class:cr-danger={projectedRisk(projectedCrBorrow) === 'danger'}>→ CR {fmtProjectedCr(projectedCrBorrow)}</span>
            {/if}
          </div>
        </div>

        <!-- Repay -->
        <div class="action-panel" class:panel-inactive={activeIntent && activeIntent !== 'repay'}>
          <span class="action-label-row">
            <span class="action-label">Repay</span>
            {#if maxRepayable > 0}
              <button class="max-text" on:click={setMaxRepay}>Max: {formatNumber(maxRepayable, 4)} icUSD</button>
            {/if}
          </span>
          <div class="action-input-row">
            <input type="number" class="action-input" bind:value={repayAmount}
              on:input={onRepayInput} on:blur={clampRepay}
              placeholder="0.00" min="0" step="0.01"
              disabled={isProcessing || vault.borrowedIcusd === 0} />
            <span class="input-suffix">icUSD</span>
          </div>
          <div class="action-btn-row">
            <button class="btn-primary btn-sm btn-action" on:click={handleRepay}
              disabled={isProcessing || !repayAmount || vault.borrowedIcusd === 0 || repayOverMax}>
              {isProcessing && activeIntent === 'repay' ? '…' : 'Repay'}
            </button>
            {#if projectedCrRepay !== null}
              <span class="cr-preview" class:cr-warning={projectedRisk(projectedCrRepay) === 'warning'}
                class:cr-danger={projectedRisk(projectedCrRepay) === 'danger'}>→ CR {fmtProjectedCr(projectedCrRepay)}</span>
            {/if}
          </div>
        </div>
      </div>

      {#if canWithdraw || canClose}
        <div class="advanced-section">
          <button class="advanced-toggle" on:click={() => showAdvanced = !showAdvanced}>
            {showAdvanced ? '▾' : '▸'} Advanced
          </button>
          {#if showAdvanced}
            <div class="advanced-content">
              <button class="btn-danger btn-sm" on:click={handleWithdrawAndClose} disabled={isWithdrawingAndClosing}>
                {isWithdrawingAndClosing ? 'Closing…' : 'Withdraw Collateral & Close Vault'}
              </button>
            </div>
          {/if}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .vault-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    transition: border-color 0.2s ease, box-shadow 0.2s ease;
    box-shadow: inset 0 1px 0 0 rgba(200,210,240,0.03), 0 2px 8px -2px rgba(8,11,22,0.6), 0 1px 3px -1px rgba(14,18,40,0.4);
  }
  .vault-card:hover {
    border-color: rgba(209,118,232,0.08);
    box-shadow: inset 0 0 20px 0 rgba(209,118,232,0.04), inset 0 1px 0 0 rgba(200,210,240,0.03), 0 2px 8px -2px rgba(8,11,22,0.6);
  }
  .vault-card-danger { border-left: 2px solid var(--rumi-danger); }
  .vault-card-warning { border-left: 2px solid var(--rumi-caution); }

  .vault-row {
    display: grid; grid-template-columns: 3rem 1fr 1fr auto 2rem;
    align-items: center; gap: 1rem; padding: 0.625rem 1rem;
    width: 100%; background: none; border: none;
    color: inherit; cursor: pointer; text-align: left; font-family: inherit;
  }
  .vault-id { font-family: 'Circular Std','Inter',sans-serif; font-weight: 500; font-size: 0.8125rem; color: var(--rumi-text-muted); }
  .vault-cell { display: flex; flex-direction: column; gap: 0.0625rem; }
  .cell-label { font-size: 0.6875rem; color: var(--rumi-text-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .cell-value { font-family: 'Inter',sans-serif; font-weight: 600; font-size: 0.875rem; font-variant-numeric: tabular-nums; color: var(--rumi-text-primary); }
  .cell-sub { font-size: 0.75rem; color: var(--rumi-text-muted); font-variant-numeric: tabular-nums; }
  .vault-cell-ratio { text-align: right; align-items: flex-end; }
  .ratio-text { display: inline-flex; align-items: center; gap: 0.25rem; }
  .ratio-warning { color: var(--rumi-caution); }
  .ratio-danger { color: var(--rumi-danger); }
  .warn-icon { width: 0.875rem; height: 0.875rem; flex-shrink: 0; }

  .vault-chevron { display: flex; align-items: center; justify-content: center; transition: transform 0.15s ease; }
  .vault-chevron svg { width: 1rem; height: 1rem; color: var(--rumi-text-muted); }
  .vault-chevron-open { transform: rotate(90deg); }

  /* ── Expanded ── */
  .vault-actions { border-top: 1px solid var(--rumi-border); padding: 0.625rem 1rem 0.75rem; }
  .action-grid { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 0.75rem; }
  .action-panel { display: flex; flex-direction: column; gap: 0.3125rem; transition: opacity 0.15s ease; }
  .panel-inactive { opacity: 0.4; }

  .action-label { font-size: 0.75rem; font-weight: 500; color: var(--rumi-text-secondary); }
  .action-label-row { display: flex; justify-content: space-between; align-items: baseline; gap: 0.5rem; min-height: 1.125rem; }

  .action-input-row { position: relative; }
  .action-input {
    width: 100%; padding: 0.375rem 2.5rem 0.375rem 0.5rem;
    background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border);
    border-radius: 0.375rem; color: var(--rumi-text-primary);
    font-family: 'Inter',sans-serif; font-size: 0.8125rem;
    font-variant-numeric: tabular-nums; transition: border-color 0.15s;
  }
  .action-input:focus { outline: none; border-color: var(--rumi-teal); box-shadow: 0 0 0 1px rgba(45,212,191,0.12); }
  .action-input:disabled { opacity: 0.5; }
  .input-suffix {
    position: absolute; right: 0.5rem; top: 50%; transform: translateY(-50%);
    font-size: 0.6875rem; color: var(--rumi-text-muted); pointer-events: none;
  }

  /* Max: inline utility text, NOT a button — neutral color per spec */
  .max-text {
    background: none; border: none; cursor: pointer; padding: 0;
    font-size: 0.6875rem; font-weight: 500; white-space: nowrap;
    color: var(--rumi-text-muted); opacity: 0.85;
    transition: opacity 0.15s;
  }
  .max-text:hover { opacity: 1; text-decoration: underline; }

  /* Button + projected CR inline */
  .action-btn-row { display: flex; align-items: center; gap: 0.5rem; }
  .btn-action { width: 70%; }
  .btn-sm { padding: 0.3125rem 0.75rem; font-size: 0.75rem; border-radius: 0.375rem; }

  /* Projected CR preview */
  .cr-preview {
    font-family: 'Inter',sans-serif; font-size: 0.6875rem;
    font-variant-numeric: tabular-nums; font-weight: 500;
    color: var(--rumi-text-muted); white-space: nowrap;
  }
  .cr-warning { color: var(--rumi-caution); }
  .cr-danger { color: var(--rumi-danger); }

  /* Messages */
  .msg-bar {
    padding: 0.375rem 0.625rem; border-radius: 0.375rem;
    font-size: 0.75rem; display: flex; justify-content: space-between;
    align-items: center; margin-bottom: 0.5rem;
  }
  .msg-error { background: rgba(239,68,68,0.08); border: 1px solid rgba(239,68,68,0.2); color: #fca5a5; }
  .msg-success { background: rgba(16,185,129,0.08); border: 1px solid rgba(16,185,129,0.2); color: #6ee7b7; }
  .msg-dismiss { background: none; border: none; color: inherit; cursor: pointer; font-size: 0.875rem; padding: 0 0.25rem; opacity: 0.6; }
  .msg-dismiss:hover { opacity: 1; }

  /* Advanced */
  .advanced-section { margin-top: 0.625rem; padding-top: 0.375rem; border-top: 1px solid var(--rumi-border); }
  .advanced-toggle { background: none; border: none; color: var(--rumi-text-muted); font-size: 0.6875rem; cursor: pointer; padding: 0; }
  .advanced-toggle:hover { color: var(--rumi-text-secondary); }
  .advanced-content { margin-top: 0.375rem; }

  /* Number input cleanup */
  .action-input::-webkit-outer-spin-button,
  .action-input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }
  .action-input[type=number] { -moz-appearance: textfield; appearance: textfield; }

  @media (max-width: 640px) {
    .vault-row { grid-template-columns: 3rem 1fr 1fr 2rem; gap: 0.5rem; }
    .vault-cell-ratio { display: none; }
    .action-grid { grid-template-columns: 1fr; }
  }
</style>
