<script lang="ts">
  import { onMount } from "svelte";
  import { walletStore as wallet } from "$lib/stores/wallet";
  import { protocolService } from '$lib/services/protocol';
  import { formatNumber, formatStableDisplay, formatStableTx, formatTokenBalance } from '$lib/utils/format';
  import { tweened } from 'svelte/motion';
  import { cubicOut } from 'svelte/easing';
  import type { CandidVault } from '$lib/services/types';
  import { walletOperations, isOisyWallet } from "$lib/services/protocol/walletOperations";
  import { CONFIG, CANISTER_IDS } from "$lib/config";
  import { collateralStore } from '$lib/stores/collateralStore';
  import { getLiquidationCR, getMinimumCR } from '$lib/protocol';

  const ANON_PRINCIPAL = '2vxsx-fae';

  function resolveCollateralPrincipal(vault: CandidVault): string {
    const raw = vault.collateral_type;
    if (!raw) return CANISTER_IDS.ICP_LEDGER;
    const text = typeof raw === 'string' ? raw : raw.toText?.() || CANISTER_IDS.ICP_LEDGER;
    return text === ANON_PRINCIPAL ? CANISTER_IDS.ICP_LEDGER : text;
  }

  // ── Color interpolation for health gauge (from VaultCard) ──
  const DANGER_HEX = '#e06b9f';
  const CAUTION_HEX = '#a78bfa';
  const SAFE_HEX = '#2DD4BF';
  const WHITE_HEX = '#e2e8f0';

  function lerpColor(c1: string, c2: string, t: number): string {
    const r1 = parseInt(c1.slice(1, 3), 16), g1 = parseInt(c1.slice(3, 5), 16), b1 = parseInt(c1.slice(5, 7), 16);
    const r2 = parseInt(c2.slice(1, 3), 16), g2 = parseInt(c2.slice(3, 5), 16), b2 = parseInt(c2.slice(5, 7), 16);
    const r = Math.round(r1 + (r2 - r1) * t), g = Math.round(g1 + (g2 - g1) * t), b = Math.round(b1 + (b2 - b1) * t);
    return `#${r.toString(16).padStart(2,'0')}${g.toString(16).padStart(2,'0')}${b.toString(16).padStart(2,'0')}`;
  }

  function computeVaultGauge(vault: CandidVault) {
    const ci = getVaultCollateralInfo(vault);
    const debt = getVaultDebt(vault);
    const collateralValueUsd = ci.collateralAmount * ci.price;
    const cr = debt > 0 ? collateralValueUsd / debt : Infinity;
    const crPct = cr === Infinity ? 300 : cr * 100;
    const vaultLiqCR = getLiquidationCR(ci.ctPrincipal);
    const vaultMinCR = getMinimumCR(ci.ctPrincipal);
    const gaugePct = Math.min(Math.max((crPct - 100) / 2, 0), 100);
    const liqZonePct = Math.max(((vaultLiqCR * 100) - 100) / 2, 0);
    const borrowZonePct = Math.max(((vaultMinCR * 100) - 100) / 2, 0);
    const comfortZonePct = Math.max(((vaultMinCR * 1.234 * 100) - 100) / 2, 0);
    const halfSpan = (comfortZonePct - borrowZonePct) / 2;
    const fadeStartPct = comfortZonePct + halfSpan;
    const fadeEndPct = comfortZonePct - halfSpan;

    let gaugeColor: string;
    if (gaugePct >= fadeStartPct) gaugeColor = SAFE_HEX;
    else if (gaugePct >= fadeEndPct) { const t = (fadeStartPct - gaugePct) / (fadeStartPct - fadeEndPct); gaugeColor = lerpColor(SAFE_HEX, CAUTION_HEX, t); }
    else if (gaugePct <= liqZonePct) gaugeColor = DANGER_HEX;
    else { const t = (fadeEndPct - gaugePct) / (fadeEndPct - liqZonePct); gaugeColor = lerpColor(CAUTION_HEX, DANGER_HEX, t); }

    let crColor: string;
    if (gaugePct >= fadeStartPct) crColor = WHITE_HEX;
    else if (gaugePct <= liqZonePct) crColor = DANGER_HEX;
    else { const t = (fadeStartPct - gaugePct) / (fadeStartPct - liqZonePct); crColor = lerpColor(WHITE_HEX, DANGER_HEX, t); }

    let railStyle = '';
    if (gaugePct < fadeStartPct) {
      if (gaugePct >= fadeEndPct) {
        const opacity = (fadeStartPct - gaugePct) / (fadeStartPct - fadeEndPct);
        const rr = parseInt(crColor.slice(1, 3), 16), gg = parseInt(crColor.slice(3, 5), 16), bb = parseInt(crColor.slice(5, 7), 16);
        railStyle = `border-left: 2px solid rgba(${rr},${gg},${bb},${opacity.toFixed(2)})`;
      } else {
        railStyle = `border-left: 2px solid ${crColor}`;
      }
    }

    const comfortCR = vaultMinCR * 1.234;
    let riskLevel: string;
    if (cr === Infinity || cr >= comfortCR) riskLevel = 'safe';
    else if (cr >= vaultMinCR) riskLevel = 'caution';
    else if (cr > vaultLiqCR) riskLevel = 'warning';
    else riskLevel = 'danger';

    const fmtRatio = cr === Infinity ? '—' : `${(cr * 100).toFixed(1)}%`;
    const fmtMargin = formatTokenBalance(ci.collateralAmount);
    const fmtCollateralUsd = formatNumber(collateralValueUsd, 2);
    const fmtBorrowed = formatStableDisplay(debt);
    const fmtBorrowedUsd = formatStableDisplay(debt);
    const collateralColor = collateralStore.getCollateralColor(ci.ctPrincipal);

    return { gaugePct, liqZonePct, borrowZonePct, fadeStartPct, fadeEndPct, gaugeColor, crColor, railStyle, riskLevel, fmtRatio, fmtMargin, fmtCollateralUsd, fmtBorrowed, fmtBorrowedUsd, collateralColor, symbol: ci.symbol };
  }

  let liquidatableVaults: CandidVault[] = [];
  let allVaults: CandidVault[] = [];
  let icpPrice = 0;
  let liquidationBonus = 1.15;
  let recoveryTargetCr = 1.55;
  let isLoading = true;
  let isPriceLoading = true;
  let liquidationError = "";
  let liquidationSuccess = "";
  let processingVaultId: number | null = null;
  let isApprovingAllowance = false;
  let liquidationAmounts: { [vaultId: number]: string } = {};
  let liquidationTokens: { [vaultId: number]: 'icUSD' | 'CKUSDT' | 'CKUSDC' } = {};
  let otherVaultsPage = 0;
  let otherVaultsPageSize = 25;

  let collateralVersion = 0;

  function getLiqToken(vaultId: number): 'icUSD' | 'CKUSDT' | 'CKUSDC' {
    return liquidationTokens[vaultId] || 'icUSD';
  }

  $: isConnected = $wallet.isConnected;

  $: walletIcusd = $wallet.tokenBalances?.ICUSD
    ? parseFloat($wallet.tokenBalances.ICUSD.formatted) : 0;
  $: walletCkusdt = $wallet.tokenBalances?.CKUSDT
    ? parseFloat($wallet.tokenBalances.CKUSDT.formatted) : 0;
  $: walletCkusdc = $wallet.tokenBalances?.CKUSDC
    ? parseFloat($wallet.tokenBalances.CKUSDC.formatted) : 0;

  function getActiveBalance(vaultId: number): number {
    const token = getLiqToken(vaultId);
    if (token === 'CKUSDT') return Math.max(0, walletCkusdt - 0.01);
    if (token === 'CKUSDC') return Math.max(0, walletCkusdc - 0.01);
    return Math.max(0, walletIcusd - 0.001);
  }

  let animatedPrice = tweened(0, { duration: 600, easing: cubicOut });
  $: if (icpPrice > 0) { animatedPrice.set(icpPrice); }

  $: nonLiquidatableVaults = (() => {
    void collateralVersion;
    return allVaults
      .filter(v => !liquidatableVaults.some(lv => lv.vault_id === v.vault_id))
      .sort((a, b) => calculateHealthScore(a) - calculateHealthScore(b));
  })();

  $: sortedVaults = (() => {
    void collateralVersion;
    return [...liquidatableVaults].sort((a, b) => {
      const hsA = calculateHealthScore(a);
      const hsB = calculateHealthScore(b);
      if (hsA !== hsB) return hsA - hsB;
      return a.vault_id - b.vault_id;
    });
  })();

  function calculateCollateralRatio(vault: CandidVault): number {
    const ctPrincipal = resolveCollateralPrincipal(vault);
    const ctInfo = collateralStore.getCollateralInfo(ctPrincipal);
    const decimals = ctInfo?.decimals ?? 8;
    const price = ctInfo?.price || (ctPrincipal === CANISTER_IDS.ICP_LEDGER ? icpPrice : 0);
    const collateralAmount = Number(vault.collateral_amount || vault.icp_margin_amount || 0) / Math.pow(10, decimals);
    const icusdAmount = Number(vault.borrowed_icusd_amount || 0) / 1e8;
    if (icusdAmount <= 0) return Infinity;
    const ratio = (collateralAmount * price / icusdAmount) * 100;
    return isFinite(ratio) ? ratio : 0;
  }

  function calculateHealthScore(vault: CandidVault): number {
    const cr = calculateCollateralRatio(vault);
    if (cr === Infinity) return Infinity;
    const ci = getVaultCollateralInfo(vault);
    const liqCR = getLiquidationCR(ci.ctPrincipal) * 100;
    if (liqCR <= 0) return Infinity;
    return cr / liqCR;
  }

  function getVaultDebt(vault: CandidVault): number {
    return Number(vault.borrowed_icusd_amount || 0) / 1e8;
  }

  function getVaultCollateralInfo(vault: CandidVault) {
    const ctPrincipal = resolveCollateralPrincipal(vault);
    const ctInfo = collateralStore.getCollateralInfo(ctPrincipal);
    const decimals = ctInfo?.decimals ?? 8;
    const price = ctInfo?.price || (ctPrincipal === CANISTER_IDS.ICP_LEDGER ? icpPrice : 0);
    const symbol = ctInfo?.symbol ?? 'ICP';
    const collateralAmount = Number(vault.collateral_amount || vault.icp_margin_amount || 0) / Math.pow(10, decimals);
    const ledgerFee = ctInfo?.ledgerFee ? ctInfo.ledgerFee / Math.pow(10, decimals) : 0.0001;
    return { ctPrincipal, decimals, price, symbol, collateralAmount, ledgerFee };
  }

  function getMaxLiquidation(vault: CandidVault): number {
    const debt = getVaultDebt(vault);
    const bal = getActiveBalance(vault.vault_id);
    const { collateralAmount, price } = getVaultCollateralInfo(vault);
    const currentPrice = price || 0;

    if (currentPrice > 0 && debt > 0) {
      const collateralValue = collateralAmount * currentPrice;
      const factor = recoveryTargetCr - liquidationBonus;
      const numerator = recoveryTargetCr * debt - collateralValue;
      if (factor > 0 && numerator > 0) {
        const restoreCap = numerator / factor;
        return Math.min(bal, debt, restoreCap);
      }
    }

    return Math.min(bal, debt);
  }

  function calculateSeizure(vault: CandidVault, icusdAmount: number): { collateralSeized: number, usdValue: number, symbol: string } {
    const { collateralAmount, price, symbol, ledgerFee } = getVaultCollateralInfo(vault);
    const currentPrice = price || 1;
    let collateralReceived = currentPrice > 0 ? icusdAmount / currentPrice * liquidationBonus : 0;
    const collateralSeized = Math.max(0, Math.min(collateralReceived, collateralAmount) - ledgerFee);
    const usdValue = collateralSeized * currentPrice;
    return {
      collateralSeized: isFinite(collateralSeized) ? collateralSeized : 0,
      usdValue: isFinite(usdValue) ? usdValue : 0,
      symbol
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

  function getSeizure(vault: CandidVault): { collateralSeized: number, usdValue: number, symbol: string } | null {
    const _amounts = liquidationAmounts;
    const v = parseFloat(_amounts[vault.vault_id]) || 0;
    if (v <= 0) return null;
    if (v > getMaxLiquidation(vault)) return null;
    return calculateSeizure(vault, v);
  }

  function setMax(vault: CandidVault) {
    const max = getMaxLiquidation(vault);
    if (max > 0) liquidationAmounts[vault.vault_id] = formatStableTx(max);
  }

  function normalizeVault(vault: CandidVault): CandidVault {
    return {
      ...vault,
      original_icp_margin_amount: vault.icp_margin_amount,
      original_borrowed_icusd_amount: vault.borrowed_icusd_amount,
      icp_margin_amount: Number(vault.icp_margin_amount || 0),
      collateral_amount: Number(vault.collateral_amount || vault.icp_margin_amount || 0),
      borrowed_icusd_amount: Number(vault.borrowed_icusd_amount || 0),
      vault_id: Number(vault.vault_id || 0),
      owner: vault.owner.toString()
    };
  }

  async function loadLiquidatableVaults() {
    isLoading = true; liquidationError = "";
    try {
      const vaults = await protocolService.getLiquidatableVaults();
      liquidatableVaults = vaults.map(normalizeVault);
    } catch (error) {
      console.error("Error loading liquidatable vaults:", error);
      liquidationError = "Failed to load liquidatable vaults.";
    } finally { isLoading = false; }
  }

  async function loadAllVaults() {
    try {
      const vaults = await protocolService.getAllVaults();
      allVaults = vaults.map(normalizeVault);
    } catch (error) {
      console.error("Error loading all vaults:", error);
    }
  }

  async function checkAndApproveAllowance(icusdAmount: number): Promise<boolean> {
    try {
      isApprovingAllowance = true;
      const amountE8s = BigInt(Math.floor(icusdAmount * 1e8));
      const spenderCanisterId = CONFIG.currentCanisterId;
      const currentAllowance = await walletOperations.checkIcusdAllowance(spenderCanisterId);
      if (currentAllowance < amountE8s) {
        const LARGE_APPROVAL = BigInt(100_000_000_000_000_000);
        const approvalResult = await walletOperations.approveIcusdTransfer(LARGE_APPROVAL, spenderCanisterId);
        if (!approvalResult.success) { liquidationError = approvalResult.error || "Failed to approve icUSD transfer"; return false; }

        if (isOisyWallet()) {
          liquidationSuccess = "Approved! Click Liquidate again to complete.";
          return false;
        }
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
    if (inputAmount <= 0) { liquidationError = "Enter an amount"; return; }
    if (isOverMax(vault)) { liquidationError = "Amount exceeds maximum"; return; }

    const token = getLiqToken(vault.vault_id);
    const vaultDebt = getVaultDebt(vault);
    const isFullLiquidation = token === 'icUSD' && inputAmount >= vaultDebt * 0.999;

    liquidationError = ""; liquidationSuccess = ""; processingVaultId = vault.vault_id;
    try {
      const bal = getActiveBalance(vault.vault_id);
      if (bal < inputAmount) {
        liquidationError = `Insufficient ${token}. Need ${formatStableTx(inputAmount)}, have ${formatStableTx(bal)}.`;
        processingVaultId = null; return;
      }

      if (token === 'icUSD' && !isOisyWallet()) {
        if (!await checkAndApproveAllowance(inputAmount * 1.20)) { processingVaultId = null; return; }
      }

      await loadLiquidatableVaults();
      if (!liquidatableVaults.find(v => v.vault_id === vault.vault_id)) {
        liquidationError = "Vault no longer available"; processingVaultId = null; return;
      }

      let result;
      if (token === 'icUSD') {
        if (isFullLiquidation) {
          result = await protocolService.liquidateVault(vault.vault_id);
        } else {
          result = await protocolService.partialLiquidateVault(vault.vault_id, inputAmount);
        }
      } else {
        result = await protocolService.partialLiquidateVaultWithStable(vault.vault_id, inputAmount, token);
      }

      if (result.success) {
        const seizure = calculateSeizure(vault, inputAmount);
        liquidationSuccess = `Liquidated vault #${vault.vault_id}. Paid ${formatStableTx(inputAmount)} ${token}, received ${formatNumber(seizure.collateralSeized, 4)} ${seizure.symbol}.`;
        liquidationAmounts[vault.vault_id] = '';
        await loadLiquidatableVaults();
      } else {
        const msg = result.error || "Liquidation failed";
        if (msg.includes('Click Liquidate again')) {
          liquidationSuccess = 'Approved! Click Liquidate again to complete.';
        } else {
          liquidationError = msg;
        }
      }
    } catch (error) {
      const msg = error instanceof Error ? error.message : String(error);
      liquidationError = msg.includes('underflow') ? "Vault state changed. Try again." : msg;
    } finally { processingVaultId = null; }
  }

  async function refreshIcpPrice() {
    try {
      isPriceLoading = true;
      const status = await protocolService.getProtocolStatus();
      icpPrice = status.lastIcpRate;
      if (status.liquidationBonus > 0) liquidationBonus = status.liquidationBonus;
      if (status.recoveryTargetCr > 0) recoveryTargetCr = status.recoveryTargetCr;
    }
    catch (error) { console.error("Error fetching protocol status:", error); }
    finally { isPriceLoading = false; }
  }

  onMount(() => {
    collateralStore.fetchSupportedCollateral().then(() => {
      collateralVersion++;
      loadLiquidatableVaults(); loadAllVaults();
    });
    refreshIcpPrice();
    if ($wallet.isConnected) wallet.refreshBalance().catch(() => {});
    const pi = setInterval(refreshIcpPrice, 30000);
    const vi = setInterval(() => {
      collateralStore.fetchSupportedCollateral(true).then(() => { collateralVersion++; });
      loadLiquidatableVaults(); loadAllVaults();
    }, 60000);
    return () => { clearInterval(pi); clearInterval(vi); };
  });

  $: if ($wallet.isConnected && !walletIcusd && !walletCkusdt && !walletCkusdc) {
    wallet.refreshBalance().catch(() => {});
  }
</script>

<div class="liq-page">
  <div class="liq-summary">
    <span class="summary-stat">{sortedVaults.length} liquidatable vault{sortedVaults.length !== 1 ? 's' : ''} · {allVaults.length} total</span>
    <span class="summary-sep">·</span>
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
    <span class="summary-sep">·</span>
    <button class="summary-refresh" on:click={() => { loadLiquidatableVaults(); loadAllVaults(); }} disabled={isLoading}>
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
        {@const inputVal = parseFloat(liquidationAmounts[vault.vault_id] || '') || 0}
        {@const overMax = inputVal > 0 && maxLiq > 0 && inputVal > maxLiq}
        {@const s = inputVal > 0 && !overMax ? calculateSeizure(vault, inputVal) : null}
        {@const ci = getVaultCollateralInfo(vault)}

        <div class="liq-card">
          <div class="card-body">
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
                <span class="stat"><span class="stat-label">Debt</span> <span class="stat-value">{formatStableDisplay(debt)} icUSD</span></span>
                <span class="stat-sep">·</span>
                <span class="stat"><span class="stat-label">Collateral</span> <span class="stat-value">{formatNumber(ci.collateralAmount, 4)} {ci.symbol}</span></span>
              </div>
            </div>

            <div class="card-center">
              {#if s}
                <span class="outcome-label">You receive</span>
                <span class="outcome-line">{formatNumber(s.collateralSeized, 4)} {s.symbol} <span class="outcome-usd">${formatNumber(s.usdValue, 2)}</span></span>
              {/if}
            </div>

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
                  <input type="number" class="liq-input liq-input-with-select" class:input-over={overMax}
                    bind:value={liquidationAmounts[vault.vault_id]}
                    on:input={() => { liquidationAmounts = liquidationAmounts; }}
                    min="0" step="0.01"
                    placeholder="0.00"
                    disabled={isProcessingThis} />
                  <select class="token-select"
                    bind:value={liquidationTokens[vault.vault_id]}
                    on:change={() => { liquidationAmounts[vault.vault_id] = ''; liquidationTokens = liquidationTokens; }}
                    disabled={isProcessingThis}>
                    <option value="icUSD">icUSD</option>
                    <option value="CKUSDT">ckUSDT</option>
                    <option value="CKUSDC">ckUSDC</option>
                  </select>
                </div>
                <button class="btn-primary btn-sm btn-liquidate"
                  on:click={() => handleLiquidate(vault)}
                  disabled={!isConnected || processingVaultId !== null || inputVal <= 0}>
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

  {#if !isLoading && nonLiquidatableVaults.length > 0}
    {@const totalPages = Math.ceil(nonLiquidatableVaults.length / otherVaultsPageSize)}
    {@const pagedVaults = nonLiquidatableVaults.slice(otherVaultsPage * otherVaultsPageSize, (otherVaultsPage + 1) * otherVaultsPageSize)}
    <div class="liq-list other-vaults-section">
      <div class="section-divider"></div>
      <div class="section-header-row">
        <span class="section-header">Other Vaults</span>
        <span class="section-count">{nonLiquidatableVaults.length} vault{nonLiquidatableVaults.length !== 1 ? 's' : ''}</span>
      </div>
      {#each pagedVaults as vault (vault.vault_id)}
        {@const g = computeVaultGauge(vault)}

        <div class="ov-card" style={g.railStyle}>
          <div class="ov-row">
            <span class="ov-id"><span class="ov-dot" style="background:{g.collateralColor}"></span>#{vault.vault_id}</span>
            <span class="ov-cell">
              <span class="ov-label">Collateral</span>
              <span class="ov-value">{g.fmtMargin} {g.symbol}</span>
              <span class="ov-sub">${g.fmtCollateralUsd}</span>
            </span>
            <span class="ov-cell">
              <span class="ov-label">Borrowed</span>
              <span class="ov-value">{g.fmtBorrowed} icUSD</span>
              <span class="ov-sub">${g.fmtBorrowedUsd}</span>
            </span>
            <span class="ov-cell ov-cell-bar">
              <span class="ov-gauge-track">
                <span class="ov-gz ov-gz-pink" style="width:{g.liqZonePct}%"></span>
                <span class="ov-gz ov-gz-pink-purple" style="width:{g.fadeEndPct - g.liqZonePct}%; left:{g.liqZonePct}%"></span>
                <span class="ov-gz ov-gz-purple-green" style="width:{g.fadeStartPct - g.fadeEndPct}%; left:{g.fadeEndPct}%"></span>
                <span class="ov-gz ov-gz-teal" style="width:{100 - g.fadeStartPct}%; left:{g.fadeStartPct}%"></span>
                <span class="ov-gauge-tick" style="left:{g.borrowZonePct}%"></span>
                <span class="ov-gauge-marker" style="left:{g.gaugePct}%; background:{g.gaugeColor}; box-shadow: 0 0 4px {g.gaugeColor}80"></span>
              </span>
              <span class="ov-gauge-labels">
                <span class="ov-gauge-lbl" style="left:{g.liqZonePct}%">liq</span>
                <span class="ov-gauge-lbl ov-gauge-lbl-end">300%+</span>
              </span>
            </span>
            <span class="ov-cell ov-cell-ratio">
              <span class="ov-label">CR</span>
              <span class="ov-value ov-ratio" style="color:{g.crColor}">
                {#if g.riskLevel === 'warning' || g.riskLevel === 'danger'}
                  <svg class="warn-icon" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M8.257 3.099c.765-1.36 2.722-1.36 3.486 0l5.58 9.92c.75 1.334-.213 2.98-1.742 2.98H4.42c-1.53 0-2.493-1.646-1.743-2.98l5.58-9.92zM11 13a1 1 0 11-2 0 1 1 0 012 0zm-1-8a1 1 0 00-1 1v3a1 1 0 002 0V6a1 1 0 00-1-1z" clip-rule="evenodd" /></svg>
                {/if}
                {g.fmtRatio}
              </span>
            </span>
          </div>
        </div>
      {/each}

      {#if totalPages > 1 || otherVaultsPageSize !== 25}
        <div class="pagination-row">
          <div class="page-size-select">
            <span class="page-size-label">Show</span>
            <select class="page-size-dropdown" bind:value={otherVaultsPageSize} on:change={() => { otherVaultsPage = 0; }}>
              <option value={25}>25</option>
              <option value={50}>50</option>
              <option value={100}>100</option>
            </select>
          </div>
          {#if totalPages > 1}
            <div class="page-nav">
              <button class="page-btn" disabled={otherVaultsPage === 0} on:click={() => otherVaultsPage--}>&lsaquo;</button>
              <span class="page-info">{otherVaultsPage + 1} / {totalPages}</span>
              <button class="page-btn" disabled={otherVaultsPage >= totalPages - 1} on:click={() => otherVaultsPage++}>&rsaquo;</button>
            </div>
          {/if}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .liq-page { max-width: 800px; margin: 0 auto; }

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
  .msg-warn { background: rgba(167,139,250,0.08); border: 1px solid rgba(167,139,250,0.15); color: #c4b5fd; }
  .msg-error { background: rgba(224,107,159,0.08); border: 1px solid rgba(224,107,159,0.15); color: #e881a8; }
  .msg-success { background: rgba(45,212,191,0.08); border: 1px solid rgba(45,212,191,0.15); color: #5eead4; }

  .loading-state { display: flex; justify-content: center; padding: 3rem 0; }
  .spinner { width: 1.25rem; height: 1.25rem; border: 2px solid var(--rumi-border-hover); border-top-color: var(--rumi-action); border-radius: 50%; animation: spin 0.8s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  .empty-state { text-align: center; padding: 3rem 1rem; }
  .empty-text { font-size: 0.875rem; color: var(--rumi-text-secondary); }

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

  .card-body {
    display: flex; align-items: stretch;
    padding: 0.75rem 1rem;
    gap: 1.25rem;
  }

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
  .liq-input-with-select { padding-right: 4.5rem; }
  .token-select {
    position: absolute; right: 0.25rem; top: 50%; transform: translateY(-50%);
    background: transparent; border: none; color: var(--rumi-text-muted);
    font-size: 0.6875rem; font-family: 'Inter', sans-serif;
    cursor: pointer; padding: 0.125rem;
  }
  .token-select:focus { outline: none; }
  .token-select option { background: var(--rumi-bg-surface2); color: var(--rumi-text-primary); }

  .btn-liquidate {
    padding: 0.375rem 0.875rem; white-space: nowrap; flex-shrink: 0;
    font-family: 'Inter', sans-serif;
  }

  .liq-input::-webkit-outer-spin-button,
  .liq-input::-webkit-inner-spin-button { -webkit-appearance: none; margin: 0; }
  .liq-input[type=number] { -moz-appearance: textfield; appearance: textfield; }

  .other-vaults-section { margin-top: 0.5rem; }

  .ov-card {
    background: var(--rumi-bg-surface1);
    border: 1px solid var(--rumi-border);
    border-radius: 0.75rem;
    transition: border-color 0.2s ease, box-shadow 0.2s ease;
    box-shadow: inset 0 1px 0 0 rgba(200,210,240,0.03), 0 2px 8px -2px rgba(8,11,22,0.6), 0 1px 3px -1px rgba(14,18,40,0.4);
  }
  .ov-card:hover {
    border-color: rgba(209,118,232,0.08);
    box-shadow: inset 0 0 20px 0 rgba(209,118,232,0.04), inset 0 1px 0 0 rgba(200,210,240,0.03), 0 2px 8px -2px rgba(8,11,22,0.6);
  }
  .ov-row {
    display: grid; grid-template-columns: 3rem 8.5rem 7rem 1fr 5.5rem;
    align-items: start; column-gap: 1.25rem; padding: 0.625rem 1rem;
    width: 100%; text-align: left;
  }
  .ov-id { font-family: 'Circular Std','Inter',sans-serif; font-weight: 500; font-size: 0.8125rem; color: var(--rumi-text-muted); align-self: center; display: inline-flex; align-items: center; gap: 0.375rem; }
  .ov-dot { width: 0.375rem; height: 0.375rem; border-radius: 9999px; flex-shrink: 0; }
  .ov-cell { display: flex; flex-direction: column; gap: 0.0625rem; }
  .ov-label { font-size: 0.6875rem; color: var(--rumi-text-muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .ov-value { font-family: 'Inter',sans-serif; font-weight: 600; font-size: 0.875rem; font-variant-numeric: tabular-nums; color: var(--rumi-text-primary); }
  .ov-sub { font-size: 0.75rem; color: var(--rumi-text-muted); font-variant-numeric: tabular-nums; }
  .ov-cell-ratio { text-align: right; align-items: flex-end; justify-self: end; }
  .ov-ratio { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 1.125rem; font-weight: 700; }

  .ov-cell-bar {
    justify-self: center; align-self: center;
    display: flex; flex-direction: column; min-width: 12rem; width: 100%;
    gap: 0.125rem;
  }
  .ov-gauge-track {
    position: relative; width: 100%; height: 6px; border-radius: 3px;
    overflow: visible; background: var(--rumi-bg-surface3);
  }
  .ov-gz { position: absolute; top: 0; height: 100%; overflow: hidden; }
  .ov-gz-pink { background: linear-gradient(to right, rgba(224, 107, 159, 0.75), rgba(224, 107, 159, 0.65)); left: 0; border-radius: 3px 0 0 3px; }
  .ov-gz-pink-purple { background: linear-gradient(to right, rgba(224, 107, 159, 0.55), rgba(167, 139, 250, 0.5)); }
  .ov-gz-purple-green { background: linear-gradient(to right, rgba(167, 139, 250, 0.45), rgba(45, 212, 191, 0.45)); }
  .ov-gz-teal { background: rgba(45, 212, 191, 0.5); border-radius: 0 3px 3px 0; }
  .ov-gauge-tick {
    position: absolute; top: 0; width: 1px; height: 100%;
    background: rgba(255,255,255,0.25); transform: translateX(-50%);
    pointer-events: none;
  }
  .ov-gauge-marker {
    position: absolute; top: -5px; width: 3px; height: 16px;
    border-radius: 1.5px; transform: translateX(-50%);
    transition: left 0.3s ease; z-index: 1;
  }
  .ov-gauge-labels {
    position: relative; height: 0.75rem;
    font-size: 0.5625rem; color: var(--rumi-text-muted); opacity: 0.85;
  }
  .ov-gauge-lbl { position: absolute; transform: translateX(-50%); }
  .ov-gauge-lbl-end { right: 0; transform: none; }
  .section-divider {
    height: 1px;
    background: var(--rumi-border);
    margin: 0.75rem 0;
    opacity: 0.5;
  }
  .section-header-row {
    display: flex; align-items: baseline; gap: 0.5rem;
    margin-bottom: 0.375rem;
  }
  .section-header {
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--rumi-text-muted);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .section-count {
    font-size: 0.6875rem;
    color: var(--rumi-text-muted);
    opacity: 0.6;
  }

  .pagination-row {
    display: flex; align-items: center; justify-content: space-between;
    padding: 0.5rem 0; margin-top: 0.25rem;
  }
  .page-size-select {
    display: flex; align-items: center; gap: 0.375rem;
  }
  .page-size-label {
    font-size: 0.6875rem; color: var(--rumi-text-muted);
  }
  .page-size-dropdown {
    background: var(--rumi-bg-surface2); border: 1px solid var(--rumi-border);
    border-radius: 0.25rem; color: var(--rumi-text-secondary);
    font-size: 0.6875rem; padding: 0.125rem 0.25rem; cursor: pointer;
  }
  .page-size-dropdown:focus { outline: none; border-color: var(--rumi-teal); }
  .page-size-dropdown option { background: var(--rumi-bg-surface2); color: var(--rumi-text-primary); }
  .page-nav {
    display: flex; align-items: center; gap: 0.5rem;
  }
  .page-btn {
    background: none; border: 1px solid var(--rumi-border); border-radius: 0.25rem;
    color: var(--rumi-text-secondary); cursor: pointer;
    font-size: 0.8125rem; padding: 0.125rem 0.375rem; line-height: 1;
    transition: border-color 0.15s, color 0.15s;
  }
  .page-btn:hover:not(:disabled) { border-color: var(--rumi-text-muted); color: var(--rumi-text-primary); }
  .page-btn:disabled { opacity: 0.3; cursor: not-allowed; }
  .page-info {
    font-size: 0.6875rem; color: var(--rumi-text-muted);
    font-variant-numeric: tabular-nums;
  }

  @media (max-width: 640px) {
    .card-body { flex-direction: column; gap: 0.625rem; }
    .card-right { flex: none; }
    .ov-row { grid-template-columns: 3rem 1fr 1fr; }
    .ov-cell-bar, .ov-cell-ratio { display: none; }
  }
</style>
