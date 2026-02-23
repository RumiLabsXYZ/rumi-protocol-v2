<script lang="ts">
  import { formatNumber } from '../../utils/format';
  import type { Vault } from '../../services/types';
  import { protocolService } from '../../services/protocol';
  import { vaultStore } from '../../stores/vaultStore';
  import { protocolManager } from '../../services/ProtocolManager';
  import { CONFIG, CANISTER_IDS } from '../../config';
  import { createEventDispatcher } from 'svelte';
  import { walletStore } from '../../stores/wallet';
  import { MINIMUM_CR, LIQUIDATION_CR, E8S, getMinimumCR, getLiquidationCR } from '$lib/protocol';
  import { collateralStore } from '../../stores/collateralStore';
  import { TokenService } from '../../services/tokenService';

  export let vault: Vault;
  export let icpPrice: number = 0;
  export let expandedVaultId: number | null = null;

  // ── Per-collateral derived values ──
  $: vaultCollateralType = vault.collateralType || CANISTER_IDS.ICP_LEDGER;
  $: vaultCollateralInfo = collateralStore.getCollateralInfo(vaultCollateralType);
  $: collateralSymbol = vault.collateralSymbol || vaultCollateralInfo?.symbol || 'ICP';
  $: collateralDecimals = vault.collateralDecimals ?? vaultCollateralInfo?.decimals ?? 8;
  $: collateralDecimalsFactor = Math.pow(10, collateralDecimals);
  $: vaultCollateralPrice = vaultCollateralInfo?.price || (vaultCollateralType === CANISTER_IDS.ICP_LEDGER ? icpPrice : 0);
  $: vaultMinCR = getMinimumCR(vaultCollateralType);
  $: vaultLiqCR = getLiquidationCR(vaultCollateralType);
  $: vaultCollateralAmount = vault.collateralAmount ?? vault.icpMargin;

  const dispatch = createEventDispatcher<{ updated: void; toggle: { vaultId: number } }>();

  $: expanded = expandedVaultId === vault.vaultId;

  function toggleExpand() {
    dispatch('toggle', { vaultId: vault.vaultId });
    clearMessages();
    if (!expanded) {
      activeAction = null;
      addCollateralAmount = ''; borrowAmount = ''; repayAmount = ''; withdrawAmount = '';
    }
  }

  // ── Derived display ──
  $: collateralValueUsd = vaultCollateralAmount * vaultCollateralPrice;
  $: collateralRatio = vault.borrowedIcusd > 0
    ? collateralValueUsd / vault.borrowedIcusd : Infinity;
  $: borrowedValueUsd = vault.borrowedIcusd;
  $: riskLevel = getRiskLevel(collateralRatio);
  $: maxBorrowable = Math.max(0, (collateralValueUsd / vaultMinCR) - vault.borrowedIcusd);

  // Token type for repayment
  let repayTokenType: 'icUSD' | 'CKUSDT' | 'CKUSDC' = 'icUSD';

  // Wallet balances for input caps
  $: walletIcp = $walletStore.tokenBalances?.ICP
    ? parseFloat($walletStore.tokenBalances.ICP.formatted) : 0;
  $: walletIcusd = $walletStore.tokenBalances?.ICUSD
    ? parseFloat($walletStore.tokenBalances.ICUSD.formatted) : 0;
  $: walletCkusdt = $walletStore.tokenBalances?.CKUSDT
    ? parseFloat($walletStore.tokenBalances.CKUSDT.formatted) : 0;
  $: walletCkusdc = $walletStore.tokenBalances?.CKUSDC
    ? parseFloat($walletStore.tokenBalances.CKUSDC.formatted) : 0;
  // Per-collateral wallet balance for "Add Collateral" cap
  let nonIcpCollateralBalance = 0;
  let _lastFetchedCt = '';
  $: if (vaultCollateralType !== CANISTER_IDS.ICP_LEDGER && $walletStore.isConnected && $walletStore.principal) {
    // Fetch balance for this vault's collateral token
    const ct = vaultCollateralType;
    const ledger = vaultCollateralInfo?.ledgerCanisterId || ct;
    if (ct !== _lastFetchedCt) {
      _lastFetchedCt = ct;
      TokenService.getTokenBalance(ledger, $walletStore.principal)
        .then(raw => { nonIcpCollateralBalance = Number(raw) / collateralDecimalsFactor; })
        .catch(() => { nonIcpCollateralBalance = 0; });
    }
  }
  $: maxAddCollateral = vaultCollateralType === CANISTER_IDS.ICP_LEDGER ? walletIcp : nonIcpCollateralBalance;
  $: activeRepayBalance = repayTokenType === 'CKUSDT' ? walletCkusdt
    : repayTokenType === 'CKUSDC' ? walletCkusdc : walletIcusd;
  $: effectiveRepayBalance = (repayTokenType === 'CKUSDT' || repayTokenType === 'CKUSDC')
    ? Math.max(0, (activeRepayBalance - 0.01) / 1.0005)
    : Math.max(0, activeRepayBalance - 0.001);
  $: maxRepayable = Math.min(effectiveRepayBalance, vault.borrowedIcusd);

  // ── Withdraw max: keeps CR at minimum for this collateral ──
  $: maxWithdrawable = (() => {
    if (vaultCollateralAmount <= 0) return 0;
    if (vault.borrowedIcusd === 0) return vaultCollateralAmount;
    if (!vaultCollateralPrice || vaultCollateralPrice <= 0) return 0;
    const minCollateral = (vault.borrowedIcusd * vaultMinCR) / vaultCollateralPrice;
    return Math.max(0, vaultCollateralAmount - minCollateral);
  })();

  // ── Credit usage ──
  $: creditCapacity = collateralValueUsd / vaultMinCR;
  $: creditUsed = vault.borrowedIcusd > 0 && creditCapacity > 0
    ? Math.min((vault.borrowedIcusd / creditCapacity) * 100, 100) : 0;
  $: creditRisk = creditUsed >= 85 ? 'danger' : creditUsed >= 65 ? 'warning' : 'normal';

  $: fmtMargin = formatNumber(vaultCollateralAmount, 4);
  $: fmtCollateralUsd = formatNumber(collateralValueUsd, 2);
  $: fmtBorrowed = formatNumber(vault.borrowedIcusd, 2);
  $: fmtBorrowedUsd = formatNumber(borrowedValueUsd, 2);
  $: fmtRatio = collateralRatio === Infinity ? '—' : `${(collateralRatio * 100).toFixed(1)}%`;
  $: riskTooltip = riskLevel === 'warning'
    ? 'Approaching minimum collateral ratio'
    : riskLevel === 'danger' ? 'At risk of liquidation. Add collateral or repay.' : '';

  // ── Liquidation price: collateral price at which CR hits liquidation ratio ──
  $: liquidationPrice = vault.borrowedIcusd > 0 && vaultCollateralAmount > 0
    ? (vault.borrowedIcusd * vaultLiqCR) / vaultCollateralAmount : 0;

  // ── Active projected CR (based on active action panel) ──
  $: activeProjectedCr = activeAction === 'add' ? projectedCrAdd
    : activeAction === 'withdraw' ? projectedCrWithdraw
    : activeAction === 'borrow' ? projectedCrBorrow
    : activeAction === 'repay' ? projectedCrRepay
    : null;
  $: activeProjectedRisk = projectedRisk(activeProjectedCr);
  $: fmtActiveProjectedCr = fmtProjectedCr(activeProjectedCr);
  $: showProjectedCr = activeProjectedCr !== null && activeProjectedCr !== collateralRatio;

  // Projected liquidation price per action
  $: projectedLiqPrice = (() => {
    if (activeAction === 'add') {
      const amt = parseFloat(addCollateralAmount);
      if (!amt || amt <= 0) return null;
      const newMargin = vaultCollateralAmount + amt;
      return vault.borrowedIcusd > 0 && newMargin > 0
        ? (vault.borrowedIcusd * vaultLiqCR) / newMargin : 0;
    }
    if (activeAction === 'withdraw') {
      const amt = parseFloat(withdrawAmount);
      if (!amt || amt <= 0) return null;
      const newMargin = vaultCollateralAmount - amt;
      return vault.borrowedIcusd > 0 && newMargin > 0
        ? (vault.borrowedIcusd * vaultLiqCR) / newMargin : 0;
    }
    if (activeAction === 'borrow') {
      const amt = parseFloat(borrowAmount);
      if (!amt || amt <= 0) return null;
      const newDebt = vault.borrowedIcusd + amt;
      return newDebt > 0 && vaultCollateralAmount > 0
        ? (newDebt * vaultLiqCR) / vaultCollateralAmount : 0;
    }
    if (activeAction === 'repay') {
      const amt = parseFloat(repayAmount);
      if (!amt || amt <= 0) return null;
      const newDebt = vault.borrowedIcusd - amt;
      return newDebt > 0 && vaultCollateralAmount > 0
        ? (newDebt * vaultLiqCR) / vaultCollateralAmount : 0;
    }
    return null;
  })();

  // ── Safety delta indicator ──
  $: safetyDelta = (() => {
    if (activeProjectedCr === null || activeProjectedCr === collateralRatio) return null;
    if (collateralRatio === Infinity && activeProjectedCr === Infinity) return null;
    if (collateralRatio === Infinity) return { direction: 'down' as const, pct: 0 };
    if (activeProjectedCr === Infinity) return { direction: 'up' as const, pct: 0 };
    const delta = ((activeProjectedCr - collateralRatio) / collateralRatio) * 100;
    if (Math.abs(delta) < 0.1) return null;
    return {
      direction: delta > 0 ? 'up' as const : 'down' as const,
      pct: Math.abs(delta)
    };
  })();

  // ── Collateral price distance to liquidation ──
  $: liqPriceDistance = liquidationPrice > 0 && vaultCollateralPrice > 0
    ? ((vaultCollateralPrice - liquidationPrice) / vaultCollateralPrice) * 100 : 0;

  function getRiskLevel(ratio: number): 'normal' | 'warning' | 'danger' {
    if (ratio === Infinity || ratio >= vaultMinCR) return 'normal';
    if (ratio > vaultLiqCR) return 'warning';
    return 'danger';
  }

  // ── Action state ──
  let activeAction: 'add' | 'withdraw' | 'borrow' | 'repay' | null = null;
  let addCollateralAmount = '';
  let withdrawAmount = '';
  let borrowAmount = '';
  let repayAmount = '';
  let isProcessing = false;
  let actionError = '';
  let actionSuccess = '';
  let showAdvanced = false;
  let isWithdrawingAndClosing = false;
  let showTokenDropdown = false;
  let hasChangedToken = false;

  function selectAction(action: 'add' | 'withdraw' | 'borrow' | 'repay') {
    if (isProcessing) return;
    clearMessages();
    addCollateralAmount = ''; withdrawAmount = ''; borrowAmount = ''; repayAmount = '';
    activeAction = activeAction === action ? null : action;
  }

  function onTokenChange() { repayAmount = ''; clearMessages(); }

  $: repayTokenLabel = repayTokenType === 'icUSD' ? 'icUSD'
    : repayTokenType === 'CKUSDT' ? 'ckUSDT' : 'ckUSDC';

  function selectToken(token: 'icUSD' | 'CKUSDT' | 'CKUSDC') {
    repayTokenType = token;
    hasChangedToken = true;
    showTokenDropdown = false;
    onTokenChange();
  }

  // ── Projected CR calculations ──
  $: projectedCrAdd = (() => {
    const amt = parseFloat(addCollateralAmount);
    if (!amt || amt <= 0 || !vaultCollateralPrice) return null;
    const newCollateral = (vaultCollateralAmount + amt) * vaultCollateralPrice;
    return vault.borrowedIcusd > 0 ? newCollateral / vault.borrowedIcusd : Infinity;
  })();

  $: projectedCrWithdraw = (() => {
    const amt = parseFloat(withdrawAmount);
    if (!amt || amt <= 0 || !vaultCollateralPrice) return null;
    const newCollateral = (vaultCollateralAmount - amt) * vaultCollateralPrice;
    return vault.borrowedIcusd > 0 && newCollateral > 0 ? newCollateral / vault.borrowedIcusd : Infinity;
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
    if (ratio === Infinity) return '—';
    return `${(ratio * 100).toFixed(1)}%`;
  }

  function projectedRisk(ratio: number | null): 'normal' | 'warning' | 'danger' {
    if (ratio === null || ratio === Infinity) return 'normal';
    return getRiskLevel(ratio);
  }

  $: borrowCrInvalid = projectedCrBorrow !== null && projectedCrBorrow !== Infinity && projectedCrBorrow < vaultMinCR;

  $: addOverMax = (() => {
    const v = parseFloat(addCollateralAmount);
    return v > 0 && maxAddCollateral > 0 && v > maxAddCollateral;
  })();
  $: withdrawOverMax = (() => {
    const v = parseFloat(withdrawAmount);
    return v > 0 && v > maxWithdrawable;
  })();
  $: borrowOverMax = (() => {
    const v = parseFloat(borrowAmount);
    return v > 0 && maxBorrowable > 0 && v > maxBorrowable;
  })();
  $: repayOverMax = (() => {
    const v = parseFloat(repayAmount);
    return v > 0 && maxRepayable > 0 && v > maxRepayable;
  })();

  $: canWithdraw = vault.borrowedIcusd === 0 && vaultCollateralAmount > 0;
  $: canClose = vault.borrowedIcusd === 0;

  function clearMessages() { actionError = ''; actionSuccess = ''; }

  function setMaxAddCollateral() {
    if (maxAddCollateral > 0) addCollateralAmount = maxAddCollateral.toFixed(4);
  }
  function setMaxWithdraw() {
    if (maxWithdrawable > 0) withdrawAmount = maxWithdrawable.toFixed(4);
  }
  function setMaxBorrow() {
    if (maxBorrowable > 0) borrowAmount = maxBorrowable.toFixed(2);
  }
  function setMaxRepay() {
    if (maxRepayable > 0) repayAmount = maxRepayable.toFixed(4);
  }

  function clampInput(field: 'add' | 'withdraw' | 'borrow' | 'repay') {
    if (field === 'add') {
      const v = parseFloat(addCollateralAmount);
      if (isNaN(v) || v < 0) addCollateralAmount = '';
    } else if (field === 'withdraw') {
      const v = parseFloat(withdrawAmount);
      if (isNaN(v) || v < 0) withdrawAmount = '';
    } else if (field === 'borrow') {
      const v = parseFloat(borrowAmount);
      if (isNaN(v) || v < 0) borrowAmount = '';
    } else if (field === 'repay') {
      const v = parseFloat(repayAmount);
      if (isNaN(v) || v < 0) repayAmount = '';
    }
  }

  // ── Stats for each action ──
  $: activeStats = (() => {
    if (activeAction === 'add') return {
      label1: 'Collateral', value1: `${fmtMargin} ${collateralSymbol}`,
      label2: 'Value', value2: `$${fmtCollateralUsd}`,
    };
    if (activeAction === 'withdraw') return {
      label1: 'Collateral', value1: `${fmtMargin} ${collateralSymbol}`,
      label2: 'Max withdraw', value2: `${formatNumber(maxWithdrawable, 4)} ${collateralSymbol}`,
    };
    if (activeAction === 'borrow') return {
      label1: 'Debt', value1: `${fmtBorrowed} icUSD`,
      label2: 'Available', value2: `${formatNumber(maxBorrowable, 2)} icUSD`,
    };
    if (activeAction === 'repay') return {
      label1: 'Debt', value1: `${fmtBorrowed} icUSD`,
      label2: 'Value', value2: `$${fmtBorrowedUsd}`,
    };
    return null;
  })();

  async function handleAddCollateral() {
    const amount = parseFloat(addCollateralAmount);
    if (!amount || amount <= 0) { actionError = `Enter a valid ${collateralSymbol} amount`; return; }
    if (addOverMax) { actionError = `Exceeds wallet balance (${formatNumber(maxAddCollateral, 4)} ${collateralSymbol})`; return; }
    clearMessages(); isProcessing = true;
    try {
      const ledgerCanisterId = vaultCollateralInfo?.ledgerCanisterId ?? CONFIG.currentIcpLedgerId;
      const amountRaw = BigInt(Math.floor(amount * collateralDecimalsFactor));
      const spenderCanisterId = CONFIG.currentCanisterId;
      const currentAllowance = await protocolService.checkCollateralAllowance(spenderCanisterId, ledgerCanisterId);
      if (currentAllowance < amountRaw) {
        const bufferAmount = amountRaw * BigInt(120) / BigInt(100);
        const approvalResult = await protocolService.approveCollateralTransfer(bufferAmount, spenderCanisterId, ledgerCanisterId);
        if (!approvalResult.success) { actionError = approvalResult.error || 'Approval failed'; return; }
        await new Promise(r => setTimeout(r, 2000));
      }
      const result = await protocolService.addMarginToVault(vault.vaultId, amount, vaultCollateralType);
      if (result.success) {
        actionSuccess = `Added ${amount} ${collateralSymbol}`; addCollateralAmount = '';
        await vaultStore.refreshVault(vault.vaultId); dispatch('updated');
      } else { actionError = result.error || 'Failed'; }
    } catch (err) { actionError = err instanceof Error ? err.message : 'Unknown error';
    } finally { isProcessing = false; }
  }

  async function handleWithdrawPartial() {
    const amount = parseFloat(withdrawAmount);
    if (!amount || amount <= 0) { actionError = `Enter a valid ${collateralSymbol} amount`; return; }
    if (withdrawOverMax) { actionError = `Max withdrawable: ${formatNumber(maxWithdrawable, 4)} ${collateralSymbol}`; return; }
    clearMessages(); isProcessing = true;
    try {
      const result = await protocolService.withdrawPartialCollateral(vault.vaultId, amount);
      if (result.success) {
        actionSuccess = `Withdrew ${amount} ${collateralSymbol}`; withdrawAmount = '';
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
    if (repayOverMax) { actionError = `Max: ${formatNumber(maxRepayable, 2)} ${repayTokenType === 'icUSD' ? 'icUSD' : repayTokenType}`; return; }
    clearMessages(); isProcessing = true;
    try {
      let result;
      if (repayTokenType === 'icUSD') {
        result = await protocolManager.repayToVault(vault.vaultId, amount);
      } else {
        result = await protocolManager.repayToVaultWithStable(vault.vaultId, amount, repayTokenType);
      }
      if (result.success) {
        actionSuccess = `Repaid ${amount} ${repayTokenType === 'icUSD' ? 'icUSD' : repayTokenType}`; repayAmount = '';
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
<div class="vault-card" class:vault-card-danger={riskLevel === 'danger'} class:vault-card-warning={riskLevel === 'warning'}
  style={showProjectedCr ? `border-left-color: var(--rumi-${activeProjectedRisk === 'danger' ? 'danger' : activeProjectedRisk === 'warning' ? 'caution' : 'success'})` : ''}>
  <button class="vault-row" on:click={toggleExpand}>
    <span class="vault-id">#{vault.vaultId}</span>
    <span class="vault-cell">
      <span class="cell-label">Collateral</span>
      <span class="cell-value">{fmtMargin} {collateralSymbol}</span>
      <span class="cell-sub">${fmtCollateralUsd}</span>
    </span>
    <span class="vault-cell">
      <span class="cell-label">Borrowed</span>
      <span class="cell-value">{fmtBorrowed} icUSD</span>
      <span class="cell-sub">${fmtBorrowedUsd}</span>
    </span>
    <span class="vault-cell vault-cell-credit">
      <span class="cell-label">Credit</span>
      <span class="cell-value credit-meter-row">
        <span class="credit-meter-track">
          <span class="credit-meter-fill" class:meter-normal={creditRisk === 'normal'}
            class:meter-warning={creditRisk === 'warning'} class:meter-danger={creditRisk === 'danger'}
            style="width: {creditUsed}%"></span>
        </span>
      </span>
      <span class="cell-sub">{creditUsed.toFixed(0)}% used</span>
    </span>
    <span class="vault-cell vault-cell-ratio">
      <span class="cell-label">CR</span>
      {#if showProjectedCr}
        <span class="cell-value ratio-text cr-projected-row">
          <span class="cr-old" class:ratio-warning={riskLevel === 'warning'} class:ratio-danger={riskLevel === 'danger'}>{fmtRatio}</span>
          <span class="cr-arrow">→</span>
          <span class="cr-new" class:ratio-warning={activeProjectedRisk === 'warning'}
            class:ratio-danger={activeProjectedRisk === 'danger'}
            class:ratio-healthy={activeProjectedRisk === 'normal'}>{fmtActiveProjectedCr}</span>
        </span>
      {:else}
        <span class="cell-value ratio-text" class:ratio-warning={riskLevel === 'warning'} class:ratio-danger={riskLevel === 'danger'} title={riskTooltip}>
          {#if riskLevel !== 'normal'}
            <svg class="warn-icon" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M8.257 3.099c.765-1.36 2.722-1.36 3.486 0l5.58 9.92c.75 1.334-.213 2.98-1.742 2.98H4.42c-1.53 0-2.493-1.646-1.743-2.98l5.58-9.92zM11 13a1 1 0 11-2 0 1 1 0 012 0zm-1-8a1 1 0 00-1 1v3a1 1 0 002 0V6a1 1 0 00-1-1z" clip-rule="evenodd" /></svg>
          {/if}
          {fmtRatio}
        </span>
      {/if}
    </span>
    <span class="vault-chevron" class:vault-chevron-open={expanded}>
      <svg viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z" clip-rule="evenodd" /></svg>
    </span>
  </button>

  <!-- ── Expanded: pill groups + two-column layout ── -->
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

      <!-- Action layout: pills top-right, stats left, input right -->
      <div class="action-layout">
        <!-- Pill groups: right column -->
        <div class="pill-groups">
          <div class="pill-group pill-group-collateral">
            <button class="pill pill-collateral" class:pill-active-collateral={activeAction === 'add'}
              on:click={() => selectAction('add')} disabled={isProcessing}>Deposit</button>
            <button class="pill pill-collateral" class:pill-active-collateral={activeAction === 'withdraw'}
              class:pill-disabled={maxWithdrawable <= 0 && vault.borrowedIcusd > 0}
              on:click={() => selectAction('withdraw')} disabled={isProcessing || (maxWithdrawable <= 0 && vault.borrowedIcusd > 0)}>Withdraw</button>
          </div>
          <div class="pill-group pill-group-debt">
            <button class="pill pill-debt" class:pill-active-debt={activeAction === 'borrow'}
              on:click={() => selectAction('borrow')} disabled={isProcessing}>Borrow</button>
            <button class="pill pill-debt" class:pill-active-debt={activeAction === 'repay'}
              on:click={() => selectAction('repay')} disabled={isProcessing}>Repay</button>
          </div>
        </div>

        {#if activeAction}
        <!-- Stats panel: left column -->
        <div class="stats-panel">
            {#if activeStats}
              <div class="stat-row">
                <span class="stat-label">{activeStats.label1}</span>
                <span class="stat-value">{activeStats.value1}</span>
              </div>
              <div class="stat-row">
                <span class="stat-label">{activeStats.label2}</span>
                <span class="stat-value">{activeStats.value2}</span>
              </div>
            {/if}
            <div class="stat-divider"></div>
            <!-- CR row -->
            <div class="stat-row">
              <span class="stat-label">CR</span>
              <span class="stat-value">
                {#if showProjectedCr}
                  <span class="stat-cr-old">{fmtRatio}</span>
                  <span class="stat-arrow">→</span>
                  <span class="stat-cr-new" class:ratio-warning={activeProjectedRisk === 'warning'}
                    class:ratio-danger={activeProjectedRisk === 'danger'}
                    class:ratio-healthy={activeProjectedRisk === 'normal'}>{fmtActiveProjectedCr}</span>
                {:else}
                  <span class:ratio-warning={riskLevel === 'warning'} class:ratio-danger={riskLevel === 'danger'}>{fmtRatio}</span>
                {/if}
              </span>
            </div>
            <!-- Liq price row -->
            {#if vault.borrowedIcusd > 0 || activeAction === 'borrow'}
              <div class="stat-row">
                <span class="stat-label">Liq. price</span>
                <span class="stat-value">
                  {#if projectedLiqPrice !== null}
                    <span class="stat-cr-old">${formatNumber(liquidationPrice, 2)}</span>
                    <span class="stat-arrow">→</span>
                    <span>${formatNumber(projectedLiqPrice, 2)}</span>
                  {:else}
                    ${formatNumber(liquidationPrice, 2)}
                  {/if}
                </span>
              </div>
              {#if liqPriceDistance > 0}
                <div class="stat-row">
                  <span class="stat-label">Distance</span>
                  <span class="stat-value stat-distance" class:stat-distance-danger={liqPriceDistance < 15}
                    class:stat-distance-warning={liqPriceDistance >= 15 && liqPriceDistance < 30}>
                    {liqPriceDistance.toFixed(1)}% below {collateralSymbol}
                  </span>
                </div>
              {/if}
            {/if}
            <!-- Safety delta -->
            {#if safetyDelta}
              <div class="safety-delta" class:safety-up={safetyDelta.direction === 'up'} class:safety-down={safetyDelta.direction === 'down'}>
                <span class="safety-arrow">{safetyDelta.direction === 'up' ? '▲' : '▼'}</span>
                <span>{safetyDelta.direction === 'up' ? 'Safer' : 'Riskier'} by {safetyDelta.pct.toFixed(1)}%</span>
              </div>
            {/if}
          </div>

          <!-- Right: input panel -->
          <div class="input-panel" class:input-panel-collateral={activeAction === 'add' || activeAction === 'withdraw'}
            class:input-panel-debt={activeAction === 'borrow' || activeAction === 'repay'}>

            {#if activeAction === 'add'}
              <div class="input-header">
                <span class="input-label">Deposit Collateral</span>
                {#if maxAddCollateral > 0}
                  <button class="max-text" on:click={setMaxAddCollateral}>Max</button>
                {/if}
              </div>
              <div class="action-input-row">
                <input type="number" class="action-input" bind:value={addCollateralAmount}
                  on:blur={() => clampInput('add')}
                  placeholder="0.00" min="0.001" step="0.01" disabled={isProcessing} />
                <span class="input-suffix">{collateralSymbol}</span>
              </div>
              <div class="input-submit-row">
                <button class="btn-submit btn-submit-collateral" on:click={handleAddCollateral}
                  disabled={isProcessing || !addCollateralAmount || addOverMax}>
                  {isProcessing ? '...' : 'Deposit'}
                </button>
              </div>

            {:else if activeAction === 'withdraw'}
              <div class="input-header">
                <span class="input-label">Withdraw Collateral</span>
                {#if maxWithdrawable > 0}
                  <button class="max-text" on:click={setMaxWithdraw}>Max</button>
                {/if}
              </div>
              {#if vault.borrowedIcusd > 0}
                <span class="input-hint">Keeps CR above {(vaultMinCR * 100).toFixed(0)}%</span>
              {/if}
              <div class="action-input-row">
                <input type="number" class="action-input" bind:value={withdrawAmount}
                  on:blur={() => clampInput('withdraw')}
                  placeholder="0.00" min="0.001" step="0.01" disabled={isProcessing} />
                <span class="input-suffix">{collateralSymbol}</span>
              </div>
              <div class="input-submit-row">
                <button class="btn-submit btn-submit-collateral" on:click={handleWithdrawPartial}
                  disabled={isProcessing || !withdrawAmount || withdrawOverMax}>
                  {isProcessing ? '...' : 'Withdraw'}
                </button>
              </div>

            {:else if activeAction === 'borrow'}
              <div class="input-header">
                <span class="input-label">Borrow icUSD</span>
                {#if maxBorrowable > 0}
                  <button class="max-text" on:click={setMaxBorrow}>Max</button>
                {/if}
              </div>
              <div class="action-input-row">
                <input type="number" class="action-input" bind:value={borrowAmount}
                  on:blur={() => clampInput('borrow')}
                  placeholder="0.00" min="0.1" step="0.1" disabled={isProcessing} />
                <span class="input-suffix">icUSD</span>
              </div>
              <div class="input-submit-row">
                <button class="btn-submit btn-submit-debt" on:click={handleBorrow}
                  disabled={isProcessing || !borrowAmount || borrowCrInvalid || borrowOverMax}>
                  {isProcessing ? '...' : 'Borrow'}
                </button>
              </div>

            {:else if activeAction === 'repay'}
              <div class="input-header">
                <span class="input-label">Repay Debt</span>
                {#if maxRepayable > 0}
                  <button class="max-text" on:click={setMaxRepay}>Max</button>
                {/if}
              </div>
              <div class="action-input-row">
                <input type="number" class="action-input action-input-repay" bind:value={repayAmount}
                  on:blur={() => clampInput('repay')}
                  placeholder="0.00" min="0" step="0.01" disabled={isProcessing} />
                <button class="token-selector" class:token-selector-pulse={!hasChangedToken}
                  on:click={() => { showTokenDropdown = !showTokenDropdown; }}
                  disabled={isProcessing}>
                  <span class="token-dot" class:token-dot-icusd={repayTokenType === 'icUSD'}
                    class:token-dot-ckusdt={repayTokenType === 'CKUSDT'}
                    class:token-dot-ckusdc={repayTokenType === 'CKUSDC'}></span>
                  {repayTokenLabel}
                  <span class="token-chevron">▾</span>
                </button>
                {#if showTokenDropdown}
                  <div class="token-dropdown">
                    <button class="token-option" class:token-option-active={repayTokenType === 'icUSD'}
                      on:click={() => selectToken('icUSD')}>
                      <span class="token-dot token-dot-icusd"></span> icUSD
                    </button>
                    <button class="token-option" class:token-option-active={repayTokenType === 'CKUSDT'}
                      on:click={() => selectToken('CKUSDT')}>
                      <span class="token-dot token-dot-ckusdt"></span> ckUSDT
                    </button>
                    <button class="token-option" class:token-option-active={repayTokenType === 'CKUSDC'}
                      on:click={() => selectToken('CKUSDC')}>
                      <span class="token-dot token-dot-ckusdc"></span> ckUSDC
                    </button>
                  </div>
                {/if}
              </div>
              {#if !hasChangedToken}
                <span class="token-hint">Tap token to pay with ckUSDT or ckUSDC</span>
              {/if}
              <div class="input-submit-row">
                <button class="btn-submit btn-submit-debt" on:click={handleRepay}
                  disabled={isProcessing || !repayAmount || repayOverMax}>
                  {isProcessing ? '...' : 'Repay'}
                </button>
              </div>
            {/if}
          </div>
        {/if}
      </div>

      {#if canWithdraw || canClose}
        <div class="advanced-section">
          <button class="advanced-toggle" on:click={() => showAdvanced = !showAdvanced}>
            {showAdvanced ? '▾' : '▸'} Advanced
          </button>
          {#if showAdvanced}
            <div class="advanced-content">
              <button class="btn-danger btn-sm" on:click={handleWithdrawAndClose} disabled={isWithdrawingAndClosing}>
                {isWithdrawingAndClosing ? 'Closing...' : 'Withdraw Collateral & Close Vault'}
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
    display: grid; grid-template-columns: 3rem auto auto auto 1fr 2rem;
    align-items: start; column-gap: 3rem; padding: 0.625rem 1rem;
    width: 100%; background: none; border: none;
    color: inherit; cursor: pointer; text-align: left; font-family: inherit;
  }
  .vault-id { font-family: 'Circular Std','Inter',sans-serif; font-weight: 500; font-size: 0.8125rem; color: var(--rumi-text-muted); align-self: center; }
  .vault-cell { display: flex; flex-direction: column; gap: 0.0625rem; }
  .cell-label { font-size: 0.6875rem; color: var(--rumi-text-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .cell-value { font-family: 'Inter',sans-serif; font-weight: 600; font-size: 0.875rem; font-variant-numeric: tabular-nums; color: var(--rumi-text-primary); }
  .cell-sub { font-size: 0.75rem; color: var(--rumi-text-muted); font-variant-numeric: tabular-nums; }
  .vault-cell-ratio { text-align: right; align-items: flex-end; justify-self: end; }
  .ratio-text { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 1.125rem; font-weight: 700; }
  .ratio-warning { color: var(--rumi-caution); }
  .ratio-danger { color: var(--rumi-danger); }
  .ratio-healthy { color: var(--rumi-action); }
  .warn-icon { width: 0.875rem; height: 0.875rem; flex-shrink: 0; }

  /* ── Credit meter ── */
  .vault-cell-credit { min-width: 5rem; }
  .credit-meter-row { display: flex; align-items: center; height: 1.25rem; }
  .credit-meter-track { width: 4.5rem; height: 0.25rem; background: var(--rumi-bg-surface2); border-radius: 9999px; overflow: hidden; }
  .credit-meter-fill { display: block; height: 100%; border-radius: 9999px; transition: width 0.3s ease; }
  .meter-normal { background: var(--rumi-success, #10b981); }
  .meter-warning { background: var(--rumi-caution); }
  .meter-danger { background: var(--rumi-danger); }

  /* ── Projected CR in header ── */
  .cr-projected-row { display: inline-flex; align-items: center; gap: 0.25rem; }
  .cr-old { text-decoration: line-through; opacity: 0.5; font-size: 0.75rem; }
  .cr-arrow { color: var(--rumi-text-muted); font-size: 0.625rem; }
  .cr-new { font-weight: 700; }

  .vault-chevron { display: flex; align-items: center; justify-content: center; align-self: center; transition: transform 0.15s ease; }
  .vault-chevron svg { width: 1rem; height: 1rem; color: var(--rumi-text-muted); }
  .vault-chevron-open { transform: rotate(90deg); }

  /* ── Expanded ── */
  .vault-actions { border-top: 1px solid var(--rumi-border); padding: 0.625rem 1rem 0.75rem; }

  /* ── Two pill groups ── */
  .pill-groups {
    grid-column: 2; grid-row: 1;
    display: flex; justify-content: space-between; gap: 0.5rem;
    margin-bottom: 0.25rem;
  }
  .pill-group {
    display: flex; border-radius: 0.375rem; overflow: hidden;
    border: 1px solid var(--rumi-border);
  }
  .pill {
    padding: 0.3125rem 0.75rem;
    background: var(--rumi-bg-surface2); border: none;
    font-family: 'Inter',sans-serif; font-size: 0.75rem; font-weight: 500;
    cursor: pointer; transition: all 0.15s ease; text-align: center;
    min-width: 4.5rem;
  }
  .pill-collateral { color: var(--rumi-text-secondary); }
  .pill-debt { color: var(--rumi-text-secondary); }
  .pill:first-child { border-right: 1px solid var(--rumi-border); }

  /* Collateral pills: teal */
  .pill-collateral:hover:not(:disabled) { color: #2DD4BF; background: rgba(45,212,191,0.06); }
  .pill-active-collateral {
    background: rgba(45,212,191,0.12); color: #2DD4BF; font-weight: 600;
  }
  .pill-group-collateral { border-color: rgba(45,212,191,0.15); }
  .pill-group-collateral:has(.pill-active-collateral) { border-color: rgba(45,212,191,0.35); }

  /* Debt pills: purple */
  .pill-debt:hover:not(:disabled) { color: #d176e8; background: rgba(209,118,232,0.06); }
  .pill-active-debt {
    background: rgba(209,118,232,0.12); color: #d176e8; font-weight: 600;
  }
  .pill-group-debt { border-color: rgba(209,118,232,0.15); }
  .pill-group-debt:has(.pill-active-debt) { border-color: rgba(209,118,232,0.35); }

  .pill-disabled { opacity: 0.35; cursor: not-allowed; }
  .pill:disabled { cursor: not-allowed; }

  /* ── Grid layout: stats left, pills+input right ── */
  .action-layout {
    display: grid; grid-template-columns: 1fr 1.2fr; gap: 0.5rem 1rem;
  }

  /* ── Stats panel (left column, spans rows 1-2) ── */
  .stats-panel {
    grid-column: 1; grid-row: 1 / 3;
    display: flex; flex-direction: column; gap: 0.375rem;
    padding: 0.625rem 0.75rem;
    background: var(--rumi-bg-surface2);
    border-radius: 0.5rem;
    border: 1px solid var(--rumi-border);
  }
  .stat-row {
    display: flex; justify-content: space-between; align-items: baseline;
    font-size: 0.75rem;
  }
  .stat-label { color: var(--rumi-text-muted); font-weight: 400; }
  .stat-value {
    color: var(--rumi-text-primary); font-weight: 600;
    font-variant-numeric: tabular-nums; font-family: 'Inter',sans-serif;
  }
  .stat-divider {
    height: 1px; background: var(--rumi-border); margin: 0.125rem 0;
  }
  .stat-cr-old { opacity: 0.45; text-decoration: line-through; font-weight: 400; }
  .stat-arrow { color: var(--rumi-text-muted); font-size: 0.625rem; margin: 0 0.125rem; }
  .stat-cr-new { font-weight: 600; }
  .stat-distance { font-weight: 500; color: var(--rumi-text-secondary); }
  .stat-distance-danger { color: var(--rumi-danger); }
  .stat-distance-warning { color: var(--rumi-caution); }

  /* Safety delta badge */
  .safety-delta {
    display: inline-flex; align-items: center; gap: 0.25rem;
    font-size: 0.6875rem; font-weight: 600;
    padding: 0.1875rem 0.5rem; border-radius: 0.25rem;
    margin-top: 0.125rem; width: fit-content;
  }
  .safety-up {
    color: #34d399; background: rgba(52,211,153,0.08);
  }
  .safety-down {
    color: #f87171; background: rgba(248,113,113,0.08);
  }
  .safety-arrow { font-size: 0.5rem; }

  /* ── Input panel (right column, row 2) ── */
  .input-panel {
    grid-column: 2; grid-row: 2;
    display: flex; flex-direction: column; gap: 0.375rem;
    padding: 0.625rem 0.75rem;
    border-radius: 0.5rem;
    border: 1px solid var(--rumi-border);
  }
  .input-panel-collateral { border-color: rgba(45,212,191,0.2); }
  .input-panel-debt { border-color: rgba(209,118,232,0.2); }

  .input-header {
    display: flex; justify-content: space-between; align-items: baseline;
  }
  .input-label { font-size: 0.75rem; font-weight: 500; color: var(--rumi-text-secondary); }
  .input-hint { font-size: 0.6875rem; color: var(--rumi-text-muted); margin-top: -0.125rem; }

  .action-input-row { position: relative; }
  .action-input {
    width: 100%; padding: 0.4375rem 3rem 0.4375rem 0.625rem;
    background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border);
    border-radius: 0.375rem; color: var(--rumi-text-primary);
    font-family: 'Inter',sans-serif; font-size: 0.875rem;
    font-variant-numeric: tabular-nums; transition: border-color 0.15s;
  }
  .input-panel-collateral .action-input:focus { outline: none; border-color: #2DD4BF; box-shadow: 0 0 0 1px rgba(45,212,191,0.12); }
  .input-panel-debt .action-input:focus { outline: none; border-color: #d176e8; box-shadow: 0 0 0 1px rgba(209,118,232,0.12); }
  .action-input:disabled { opacity: 0.5; }
  .input-suffix {
    position: absolute; right: 0.625rem; top: 50%; transform: translateY(-50%);
    font-size: 0.75rem; color: var(--rumi-text-muted); pointer-events: none;
  }

  /* ── Token selector (in-field dropdown) ── */
  .action-input-repay { padding-right: 5.5rem; }
  .token-selector {
    position: absolute; right: 0.375rem; top: 50%; transform: translateY(-50%);
    display: inline-flex; align-items: center; gap: 0.25rem;
    padding: 0.1875rem 0.375rem; border-radius: 0.25rem;
    background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border);
    color: var(--rumi-text-primary); font-family: 'Inter',sans-serif;
    font-size: 0.6875rem; font-weight: 500; cursor: pointer;
    transition: all 0.15s ease;
  }
  .token-selector:hover:not(:disabled) {
    border-color: #d176e8; background: rgba(209,118,232,0.06);
  }
  .token-selector:disabled { opacity: 0.5; cursor: not-allowed; }
  .token-chevron { font-size: 0.5rem; color: var(--rumi-text-muted); margin-left: 0.0625rem; }

  /* Subtle pulse on first render to draw attention */
  .token-selector-pulse {
    animation: token-pulse 2s ease-in-out 0.5s 2;
  }
  @keyframes token-pulse {
    0%, 100% { box-shadow: none; }
    50% { box-shadow: 0 0 0 2px rgba(209,118,232,0.25); }
  }

  /* Token color dots */
  .token-dot {
    width: 0.375rem; height: 0.375rem; border-radius: 9999px;
    display: inline-block; flex-shrink: 0;
  }
  .token-dot-icusd { background: #818cf8; }
  .token-dot-ckusdt { background: #26a17b; }
  .token-dot-ckusdc { background: #2775ca; }

  /* Dropdown */
  .token-dropdown {
    position: absolute; right: 0.375rem; top: calc(50% + 1rem);
    display: flex; flex-direction: column;
    background: var(--rumi-bg-surface1); border: 1px solid var(--rumi-border);
    border-radius: 0.375rem; overflow: hidden; z-index: 10;
    box-shadow: 0 4px 12px -2px rgba(0,0,0,0.5);
    min-width: 6rem;
  }
  .token-option {
    display: flex; align-items: center; gap: 0.375rem;
    padding: 0.375rem 0.5rem;
    background: none; border: none;
    color: var(--rumi-text-secondary); font-family: 'Inter',sans-serif;
    font-size: 0.6875rem; font-weight: 500; cursor: pointer;
    transition: background 0.1s;
  }
  .token-option:hover { background: rgba(209,118,232,0.08); color: var(--rumi-text-primary); }
  .token-option-active { color: #d176e8; font-weight: 600; }

  /* Hint text */
  .token-hint {
    font-size: 0.625rem; color: var(--rumi-text-muted);
    opacity: 0.7; margin-top: -0.125rem;
  }

  /* Max button */
  .max-text {
    background: none; border: none; cursor: pointer; padding: 0;
    font-size: 0.6875rem; font-weight: 600; white-space: nowrap;
    color: var(--rumi-text-muted); opacity: 0.85;
    transition: opacity 0.15s;
  }
  .max-text:hover { opacity: 1; text-decoration: underline; }

  /* Submit button */
  .input-submit-row { display: flex; justify-content: flex-end; margin-top: 0.125rem; }
  .btn-submit {
    padding: 0.375rem 1rem; font-size: 0.75rem; font-weight: 600;
    border-radius: 0.375rem; border: none; cursor: pointer;
    font-family: 'Inter',sans-serif; transition: all 0.15s ease;
  }
  .btn-submit:disabled { opacity: 0.4; cursor: not-allowed; }
  .btn-submit-collateral {
    background: rgba(45,212,191,0.15); color: #2DD4BF;
    border: 1px solid rgba(45,212,191,0.3);
  }
  .btn-submit-collateral:hover:not(:disabled) { background: rgba(45,212,191,0.25); }
  .btn-submit-debt {
    background: rgba(209,118,232,0.15); color: #d176e8;
    border: 1px solid rgba(209,118,232,0.3);
  }
  .btn-submit-debt:hover:not(:disabled) { background: rgba(209,118,232,0.25); }

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
    .vault-cell-credit { display: none; }
    .action-layout { grid-template-columns: 1fr; }
    .pill-groups { grid-column: 1; flex-direction: column; gap: 0.5rem; }
    .pill-group { flex: 1; }
    .pill { flex: 1; }
    .stats-panel { grid-column: 1; grid-row: auto; order: 2; }
    .input-panel { grid-column: 1; grid-row: auto; order: 1; }
  }
</style>
