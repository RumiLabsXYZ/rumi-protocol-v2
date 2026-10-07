<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { get } from 'svelte/store';
  import { Principal } from '@dfinity/principal';
  import { developerAccess } from '../lib/stores/developer';
  import { formatNumber, formatStableTx } from '$lib/utils/format';
  import { interpolateMultiplier, computeProjectedRate } from '$lib/utils/interpolate';
  import { tweened } from 'svelte/motion';
  import { cubicOut } from 'svelte/easing';
  import { appDataStore, protocolStatus, isLoadingProtocol } from '$lib/stores/appDataStore';
  import { walletStore, isConnected, principal } from '$lib/stores/wallet';
  import { protocolService } from '$lib/services/protocol';
  import type { ActionBoundContext } from '$lib/services/protocol';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { MINIMUM_CR, LIQUIDATION_CR } from '$lib/protocol';
  import { collateralStore, activeCollateralTypes } from '$lib/stores/collateralStore';
  import { newVaultCollateralTypes, isHiddenForNewVaults } from '$lib/utils/newVaultCollateralPolicy';
  import { collateralSequenceLockName } from '$lib/utils/collateralSequenceLock';
  import { CANISTER_IDS, CONFIG } from '$lib/config';
  import { walletSessionGeneration } from '$lib/services/auth';
  import ProtocolStats from '$lib/components/dashboard/ProtocolStats.svelte';
  import MultiplierBadge from '$lib/components/points/MultiplierBadge.svelte';
  import { seasonStore, earningActive } from '$lib/stores/seasonStore';
  import XrpBorrowModal from '$lib/components/borrow/XrpBorrowModal.svelte';
  import {
    isNativeXrpCollateral,
    xrpCreditedCollateral,
    XRPL_BASE_RESERVE_XRP,
  } from '$lib/utils/nativeXrpBorrowFlow';
  import type { CollateralInfo } from '$lib/services/types';

  let collateralAmount = 1;
  let icusdAmount = 5;
  let errorMessage = '';
  let successMessage = '';
  let actionInProgress = false;
  type RootOpenStage = 'opening' | 'borrow_ready' | 'borrow_pending' | 'ambiguous' | 'done';
  type RootOpenIntent = {
    version: 1; principal: string; network: string; ledger: string; collateralPrincipal: string;
    requestId: string; collateralRaw: string; borrowRaw: string; collateralAmount: number;
    createdAt: number;
    icusdAmount: number; vaultId: number | null; stage: RootOpenStage;
  };
  const ROOT_OPEN_PREFIX = 'rumi_root_open_intent_';
  const ROOT_NETWORK = CONFIG.isLocal ? 'local' : 'mainnet';
  let rootOpenIntent: RootOpenIntent | null = null;
  let rootIngressWarning = '';
  let showDevInput = false;
  let xrpBorrowIntent: {
    collateralAmount: number;
    icusdAmount: number;
    collateralInfo: CollateralInfo;
  } | null = null;
  $: xrpBorrowFlowActive = Boolean(xrpBorrowIntent);

  // Collateral token selector
  let selectedCollateralPrincipal: string = CANISTER_IDS.ICP_LEDGER;
  let showCollateralDropdown = false;

  // Populate collateral token list from store, with ICP fallback.
  // Collaterals hidden by newVaultCollateralTypes() are display-gated on this
  // page only; they stay active for existing vaults and everywhere else.
  $: newVaultCollaterals = newVaultCollateralTypes($activeCollateralTypes);
  $: collateralTokens = newVaultCollaterals.length > 0
    ? newVaultCollaterals.map(ct => ({
        id: ct.principal,
        label: ct.symbol,
        color: ct.color,
      }))
    : [{ id: CANISTER_IDS.ICP_LEDGER, label: 'ICP', color: '#2DD4BF' }];

  // Derive per-collateral reactive values — subscribe to $collateralStore so updates
  // propagate when data loads AND when the user switches tokens
  $: selectedCollateralInfo = $collateralStore.collaterals.find(c => c.principal === selectedCollateralPrincipal);
  $: isNativeXrpSelected = isNativeXrpCollateral(selectedCollateralInfo);
  $: selectedSymbol = selectedCollateralInfo?.symbol ?? 'ICP';
  $: selectedMinCR = selectedCollateralInfo?.minimumCr ?? MINIMUM_CR;
  $: selectedLiqCR = selectedCollateralInfo?.liquidationCr ?? LIQUIDATION_CR;
  $: selectedBorrowingFee = selectedCollateralInfo?.borrowingFee ?? 0;
  $: borrowingFeeCurve = $protocolStatus?.borrowingFeeCurveResolved ?? [];

  // Price: use per-collateral price from store, fall back to ICP from protocol status
  $: icpPrice = $protocolStatus?.lastIcpRate || 0;
  $: collateralPrice = selectedCollateralInfo?.price
    || (selectedCollateralPrincipal === CANISTER_IDS.ICP_LEDGER ? icpPrice : 0);
  // ── Credited collateral (single source of truth for ALL risk math) ──────────
  // For native XRP the user names the amount they will SEND, and the XRPL base
  // reserve is deducted from it rather than added on top, so what actually backs
  // the loan is send-amount minus the reserve. Every ratio, price and limit below
  // must use this, NOT the typed amount — sizing a max borrow against the typed
  // amount would let the user request more than the credited collateral supports
  // and the backend would reject the borrow after their XRP had already landed.
  // Every other collateral credits the full amount, so this is a no-op for them.
  $: xrpReserveEstimate = isNativeXrpSelected ? XRPL_BASE_RESERVE_XRP : 0;
  $: creditedCollateralAmount = isNativeXrpSelected
    ? xrpCreditedCollateral(collateralAmount, xrpReserveEstimate)
    : collateralAmount;
  $: collateralValue = creditedCollateralAmount * collateralPrice;

  let isPriceLoading = true;
  let priceRefreshInterval: ReturnType<typeof setInterval>;
  let priceUpdateError = false;

  // Legacy alias for backward compat in template
  $: icpAmount = collateralAmount;

  $: projectedMintCr = (() => {
    if (icusdAmount <= 0 || creditedCollateralAmount <= 0 || collateralPrice <= 0) return Infinity;
    const collateralVal = creditedCollateralAmount * collateralPrice;
    return collateralVal / icusdAmount;
  })();
  $: mintFeeMultiplier = borrowingFeeCurve.length > 0
    ? interpolateMultiplier(borrowingFeeCurve, projectedMintCr)
    : 1;
  $: effectiveMintFeeRate = selectedBorrowingFee * mintFeeMultiplier;
  $: calculatedBorrowFee = icusdAmount * effectiveMintFeeRate;
  $: calculatedIcusdAmount = icusdAmount - calculatedBorrowFee;
  // ── Projected interest rate from rate curve ──
  $: rateCurve = $protocolStatus?.perCollateralRateCurves?.find(
    (c) => c.collateralType === selectedCollateralPrincipal
  );
  $: mintRecoveryMultiplier = (() => {
    const m = $protocolStatus?.mode;
    if (m && typeof m === 'object' && 'Recovery' in m) return $protocolStatus?.recoveryCrMultiplier ?? 1;
    return 1;
  })();
  $: projectedMintRate = rateCurve
    ? computeProjectedRate(rateCurve.baseRate, rateCurve.markers, projectedMintCr, mintRecoveryMultiplier)
    : (selectedCollateralInfo?.interestRateApr ?? 0);
  $: calculatedCollateralRatio = creditedCollateralAmount > 0 && icusdAmount >= 0.001
    ? ((creditedCollateralAmount * collateralPrice) / icusdAmount) * 100 : creditedCollateralAmount > 0 ? Infinity : 0;
  $: formattedCollateralRatio = calculatedCollateralRatio === Infinity
    ? '∞' : calculatedCollateralRatio > 1000000 ? '>1,000,000' : formatNumber(calculatedCollateralRatio);
  $: isValidCollateralRatio = calculatedCollateralRatio >= selectedMinCR * 100;
  // CR color: 3 states for text + marker
  $: crColorClass = calculatedCollateralRatio < selectedMinCR * 100 ? 'danger'
    : calculatedCollateralRatio < selectedMinCR * 1.234 * 100 ? 'caution' : 'safe';

  // Liquidation price
  $: liquidationPrice = creditedCollateralAmount > 0 && icusdAmount > 0
    ? (icusdAmount * selectedLiqCR) / creditedCollateralAmount : 0;
  $: liqPriceRatio = collateralPrice > 0 && liquidationPrice > 0 ? liquidationPrice / collateralPrice : 0;
  $: liqPriceSeverity = liqPriceRatio > 0.75 ? 'danger' : liqPriceRatio > 0.5 ? 'caution' : 'safe';
  $: safetyDelta = collateralPrice > 0 && liquidationPrice > 0
    ? ((collateralPrice - liquidationPrice) / collateralPrice) * 100 : 0;

  // Max borrow — 0.5% haircut so Max never overshoots the backend oracle price
  $: maxBorrow = creditedCollateralAmount > 0 && collateralPrice > 0
    ? Math.floor(((creditedCollateralAmount * collateralPrice) / selectedMinCR) * 0.995 * 100) / 100 : 0;

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
    // Defensive: hidden collaterals are never offered for new vaults.
    if (isHiddenForNewVaults(principalText)) return;
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

  function rootOpenKey(principalText: string) {
    return `${ROOT_OPEN_PREFIX}${ROOT_NETWORK}_${principalText}`;
  }

  function sameRootOpenLineage(left: Partial<RootOpenIntent> | null, right: RootOpenIntent) {
    return !!left && left.principal === right.principal && left.network === right.network &&
      left.ledger === right.ledger && left.requestId === right.requestId &&
      left.collateralRaw === right.collateralRaw && left.borrowRaw === right.borrowRaw &&
      left.collateralPrincipal === right.collateralPrincipal && left.createdAt === right.createdAt;
  }

  function persistRootOpenIntent(record: RootOpenIntent | null, ownerText = record?.principal, expectedCurrent?: RootOpenIntent) {
    if (!ownerText) return false;
    try {
      if (record) {
        const existing = localStorage.getItem(rootOpenKey(ownerText));
        if (existing) {
          const current = JSON.parse(existing) as Partial<RootOpenIntent>;
          if (!sameRootOpenLineage(current, record)) return false;
        }
        localStorage.setItem(rootOpenKey(ownerText), JSON.stringify(record));
      }
      else {
        if (expectedCurrent) {
          const existing = localStorage.getItem(rootOpenKey(ownerText));
          if (!existing || !sameRootOpenLineage(JSON.parse(existing) as Partial<RootOpenIntent>, expectedCurrent)) return false;
        }
        localStorage.removeItem(rootOpenKey(ownerText));
      }
      return true;
    } catch { return false; }
  }

  function updateRootOpenIntent(record: RootOpenIntent): boolean {
    if (!persistRootOpenIntent(record)) return false;
    if ($principal?.toText() === record.principal) rootOpenIntent = record;
    return true;
  }

  function parseRootOpenIntent(ownerText: string): RootOpenIntent | null {
    try {
      const value = JSON.parse(localStorage.getItem(rootOpenKey(ownerText)) || 'null');
      if (!value || value.version !== 1 || value.principal !== ownerText || value.network !== ROOT_NETWORK ||
          typeof value.ledger !== 'string' || typeof value.collateralPrincipal !== 'string' ||
          typeof value.requestId !== 'string' || !/^\d+$/.test(value.requestId) ||
          typeof value.collateralRaw !== 'string' || !/^\d+$/.test(value.collateralRaw) ||
          typeof value.borrowRaw !== 'string' || !/^\d+$/.test(value.borrowRaw) ||
          typeof value.createdAt !== 'number' || !Number.isFinite(value.createdAt) ||
          typeof value.collateralAmount !== 'number' || !Number.isFinite(value.collateralAmount) || value.collateralAmount <= 0 ||
          typeof value.icusdAmount !== 'number' || !Number.isFinite(value.icusdAmount) || value.icusdAmount <= 0 ||
          !(value.vaultId === null || (typeof value.vaultId === 'number' && Number.isSafeInteger(value.vaultId))) ||
          !['opening', 'borrow_ready', 'borrow_pending', 'ambiguous', 'done'].includes(value.stage)) return null;
      return value as RootOpenIntent;
    } catch { return null; }
  }

  function makeRootActionContext(ownerText: string): ActionBoundContext {
    const session = get(walletSessionGeneration);
    return {
      expectedPrincipalText: ownerText,
      assertCurrent: () => {
        if ($principal?.toText() !== ownerText || get(walletSessionGeneration) !== session) {
          throw new Error('Wallet session changed. Reconnect and recheck the saved operation.');
        }
        return true;
      },
    };
  }

  async function rootVaultSnapshot(ownerText: string): Promise<Array<{ vaultId: number; collateralPrincipal: string; collateralAmount: bigint; borrowedIcusd: bigint }>> {
    const owner = Principal.fromText(ownerText);
    const vaults = await publicActor.get_vaults([owner]);
    return vaults.map((vault: any) => ({
      vaultId: Number(vault.vault_id),
      collateralPrincipal: vault.collateral_type.toText(),
      collateralAmount: BigInt(vault.collateral_amount),
      borrowedIcusd: BigInt(vault.borrowed_icusd_amount),
    }));
  }

  function rootStatusMatches(record: RootOpenIntent, status: any): boolean {
    return !!status && status.owner.toText() === record.principal && status.ledger.toText() === record.ledger &&
      status.request_id.toString() === record.requestId && status.amount_raw.toString() === record.collateralRaw &&
      'Open' in status.operation && status.operation.Open.collateral_type.toText() === record.collateralPrincipal;
  }

  async function reconcileRootOpen(ownerText: string) {
    const ctx = makeRootActionContext(ownerText);
    let stored = parseRootOpenIntent(ownerText);
    rootIngressWarning = '';
    rootOpenIntent = stored;
    if (stored) {
      if (stored.stage === 'done') return;
      try {
        const savedCollateralInfo = collateralStore.getCollateralInfo(stored.collateralPrincipal);
        if (!savedCollateralInfo && stored.collateralPrincipal !== CANISTER_IDS.ICP_LEDGER) {
          rootIngressWarning = 'Collateral settings are still loading. The saved request remains held until its ledger can be verified.';
          return;
        }
        if (stored.ledger !== (savedCollateralInfo?.ledgerCanisterId ?? CONFIG.currentIcpLedgerId)) {
          rootIngressWarning = 'The saved collateral ledger no longer matches this asset. Open is paused for safety.';
          return;
        }
        const status = await protocolService.getCollateralIngressBound(ctx, stored.ledger, BigInt(stored.requestId));
        ctx.assertCurrent();
        if (!rootStatusMatches(stored, status)) {
          stored = { ...stored, stage: 'ambiguous' };
          rootOpenIntent = stored; persistRootOpenIntent(stored);
          rootIngressWarning = 'The saved collateral request is not present in the retained journal. Do not create another vault until you inspect your account and resolve this request.';
          return;
        }
        if (status && 'Rejected' in status.phase) {
          const message = status.result[0] && 'Rejected' in status.result[0] ? status.result[0].Rejected.message : 'The backend recorded this collateral request as rejected.';
          if (!persistRootOpenIntent(null, ownerText, stored)) {
            rootIngressWarning = 'A newer collateral intent replaced this request in another tab. The older result was not allowed to clear it.';
            rootOpenIntent = parseRootOpenIntent(ownerText);
            return;
          }
          rootOpenIntent = null;
          errorMessage = message;
          return;
        }
        const vaults = await rootVaultSnapshot(ownerText);
        ctx.assertCurrent();
        if (status && 'Complete' in status.phase && status.result[0] && 'Open' in status.result[0]) {
          const vaultId = Number(status.result[0].Open.vault_id);
          const vault = vaults.find((candidate) => candidate.vaultId === vaultId);
          if (!vault || vault.collateralAmount.toString() !== stored.collateralRaw || vault.collateralPrincipal !== stored.collateralPrincipal) {
            stored = { ...stored, vaultId, stage: 'ambiguous' };
          } else if (stored.stage === 'borrow_pending') {
            // The vault balance can show that debt exists, but it cannot attribute that debt to
            // the borrow whose reply was lost (another tab/action may have borrowed the same amount).
            // Keep the operation unresolved unless the original bound call returned its typed receipt.
            stored = { ...stored, vaultId, stage: 'ambiguous' };
          } else if (stored.stage === 'opening' || stored.stage === 'ambiguous') {
            stored = { ...stored, vaultId, stage: vault.borrowedIcusd === 0n ? 'borrow_ready' : 'ambiguous' };
          } else if (stored.stage === 'borrow_ready' && vault.borrowedIcusd !== 0n) {
            stored = { ...stored, vaultId, stage: 'ambiguous' };
          }
          if (!updateRootOpenIntent(stored)) {
            rootIngressWarning = 'A newer collateral intent replaced this request in another tab. The older result was not allowed to overwrite it.';
            rootOpenIntent = parseRootOpenIntent(ownerText);
            return;
          }
          if (stored.stage === 'borrow_ready') successMessage = `Vault #${vaultId} is open with collateral. Borrowing is a separate confirmation.`;
          else if (stored.stage === 'done') successMessage = `Vault #${vaultId} opened and ${stored.icusdAmount} icUSD borrowing is confirmed.`;
          else rootIngressWarning = `Vault #${vaultId} exists, but its borrow outcome is unresolved. Recheck before any further action.`;
        } else {
          stored = { ...stored, stage: 'ambiguous' };
          if (!updateRootOpenIntent(stored)) {
            rootIngressWarning = 'A newer collateral intent replaced this request in another tab. The older result was not allowed to overwrite it.';
            rootOpenIntent = parseRootOpenIntent(ownerText);
            return;
          }
          rootIngressWarning = status?.last_error[0] || 'Collateral is pending or held. Do not create another vault; recheck this request later.';
        }
      } catch {
        rootIngressWarning = 'The saved collateral request could not be checked safely. No new open or borrow will be submitted.';
      }
      return;
    }
    try {
      if (localStorage.getItem(rootOpenKey(ownerText)) !== null) {
        rootIngressWarning = 'A saved collateral request record is unreadable. No new open will be submitted until the record is resolved manually.';
        return;
      }
    } catch {
      rootIngressWarning = 'Local collateral request storage is unavailable. Opening is paused until it can be checked safely.';
      return;
    }

    // Storage can be lost after a successful open. The owner-scoped journal is checked before
    // offering a fresh ID; an unresolved active request or unborrowed latest open blocks new opens.
    try {
      const ledger = selectedCollateralInfo?.ledgerCanisterId ?? CONFIG.currentIcpLedgerId;
      const state = await protocolService.getCollateralIngressStateBound(ctx, ledger);
      ctx.assertCurrent();
      if (state.active_request[0]) {
        rootIngressWarning = 'A collateral request is already active for this account. Inspect or reconcile it before opening another vault.';
        return;
      }
      const latest = state.latest_result[0];
      if (latest && 'Complete' in latest.phase && latest.result[0] && 'Open' in latest.result[0]) {
        const vaultId = Number(latest.result[0].Open.vault_id);
        const vault = (await rootVaultSnapshot(ownerText)).find((candidate) => candidate.vaultId === vaultId);
        ctx.assertCurrent();
        if (!vault || vault.borrowedIcusd === 0n) {
          rootIngressWarning = `The collateral journal shows an open unborrowed vault #${vaultId}, but its local borrow intent is missing. Inspect your vaults before opening another.`;
        }
      }
      const existingZeroDebtVault = (await rootVaultSnapshot(ownerText)).find((vault) =>
        vault.collateralPrincipal === selectedCollateralPrincipal && vault.borrowedIcusd === 0n
      );
      ctx.assertCurrent();
      if (existingZeroDebtVault) {
        rootIngressWarning = `Vault #${existingZeroDebtVault.vaultId} has no debt and may be the result of an earlier open with lost local state. Inspect it before opening another.`;
      }
    } catch {
      rootIngressWarning = 'Collateral request history could not be checked. Opening is paused until the account journal can be read safely.';
    }
  }

  async function runRootOpen(ownerText: string) {
    const locks = typeof navigator !== 'undefined' ? (navigator as any).locks : null;
    if (!locks?.request) throw new Error('This browser cannot safely coordinate collateral opens across tabs. Use one tab and try again when Web Locks are available.');
    // Freeze the exact terms before waiting for the lock; all routes using this
    // owner/ledger request sequence share the same Web Lock name.
    const submittedCollateralPrincipal = selectedCollateralPrincipal;
    const submittedCollateralInfo = collateralStore.getCollateralInfo(submittedCollateralPrincipal);
    const submittedCollateralAmount = collateralAmount;
    const submittedIcusdAmount = icusdAmount;
    const ledger = submittedCollateralInfo?.ledgerCanisterId ?? CONFIG.currentIcpLedgerId;
    return locks.request(collateralSequenceLockName(ownerText, ROOT_NETWORK, ledger), async () => {
      const ctx = makeRootActionContext(ownerText);
      const decimals = submittedCollateralInfo?.decimals ?? 8;
      const collateralRaw = BigInt(Math.floor(submittedCollateralAmount * Math.pow(10, decimals)));
      const borrowRaw = BigInt(Math.floor(submittedIcusdAmount * 100_000_000));
      const statusState = await protocolService.getCollateralIngressStateBound(ctx, ledger);
      ctx.assertCurrent();
      if (statusState.active_request[0]) throw new Error('A collateral request is already active for this account. Recheck it before opening another vault.');
      const latest = statusState.latest_result[0];
      if (latest && 'Complete' in latest.phase && latest.result[0] && 'Open' in latest.result[0]) {
        const latestVaultId = Number(latest.result[0].Open.vault_id);
        const latestVault = (await rootVaultSnapshot(ownerText)).find((vault) => vault.vaultId === latestVaultId);
        ctx.assertCurrent();
        if (!latestVault || latestVault.borrowedIcusd === 0n) {
          throw new Error(`The latest request opened unborrowed vault #${latestVaultId}. Inspect your vaults before creating another.`);
        }
      }
      const existingZeroDebtVault = (await rootVaultSnapshot(ownerText)).find((vault) =>
        vault.collateralPrincipal === submittedCollateralPrincipal && vault.borrowedIcusd === 0n
      );
      ctx.assertCurrent();
      if (existingZeroDebtVault) {
        throw new Error(`Vault #${existingZeroDebtVault.vaultId} has no debt and may be an earlier open with lost local state. Inspect it before creating another.`);
      }
      const requestId = statusState.next_request_id;
      if (requestId <= 0n) throw new Error('The backend returned an invalid collateral request ID.');
      const record: RootOpenIntent = {
        version: 1, principal: ownerText, network: ROOT_NETWORK, ledger,
        collateralPrincipal: submittedCollateralPrincipal, requestId: requestId.toString(),
        collateralRaw: collateralRaw.toString(), borrowRaw: borrowRaw.toString(),
        collateralAmount: submittedCollateralAmount, icusdAmount: submittedIcusdAmount,
        createdAt: Date.now(), vaultId: null, stage: 'opening',
      };
      // This durable write is before any approval or open call; the Nat and exact inputs survive reload.
      if (!persistRootOpenIntent(record)) throw new Error('The operation could not be saved safely, so no approval or open was submitted.');
      if ($principal?.toText() === ownerText) rootOpenIntent = record;

      const result = await protocolService.openVaultV2Bound(ctx, requestId, collateralRaw, submittedCollateralPrincipal);
      let status = result.status;
      if (!status) {
        try { status = await protocolService.getCollateralIngressBound(ctx, ledger, requestId); } catch { /* leave unresolved */ }
      }
      ctx.assertCurrent();
      if (rootStatusMatches(record, status) && status && 'Rejected' in status.phase) {
        if (!persistRootOpenIntent(null, ownerText, record)) {
          rootIngressWarning = 'A newer collateral intent replaced this request in another tab. The older result was not allowed to clear it.';
          rootOpenIntent = parseRootOpenIntent(ownerText);
          return;
        }
        if ($principal?.toText() === ownerText) rootOpenIntent = null;
        const message = status.result[0] && 'Rejected' in status.result[0] ? status.result[0].Rejected.message : 'The backend recorded this collateral request as rejected.';
        throw new Error(message);
      }
      if (result.kind === 'predispatch_aborted') {
        if (!persistRootOpenIntent(null, ownerText, record)) {
          rootIngressWarning = 'A newer collateral intent replaced this request in another tab. The older result was not allowed to clear it.';
          rootOpenIntent = parseRootOpenIntent(ownerText);
          return;
        }
        if ($principal?.toText() === ownerText) rootOpenIntent = null;
        throw new Error(`${result.errorMessage || 'Collateral approval was not completed.'}${result.approvalMayHaveMutated ? ' A token approval may have changed; collateral was not opened.' : ''}`);
      }
      const vaults = await rootVaultSnapshot(ownerText);
      ctx.assertCurrent();
      if (rootStatusMatches(record, status) && status && 'Complete' in status.phase && status.result[0] && 'Open' in status.result[0]) {
        const vaultId = Number(status.result[0].Open.vault_id);
        const vault = vaults.find((candidate) => candidate.vaultId === vaultId);
        if (vault && vault.collateralAmount === collateralRaw && vault.collateralPrincipal === submittedCollateralPrincipal && vault.borrowedIcusd === 0n) {
          const ready = { ...record, vaultId, stage: 'borrow_ready' as const };
          if (!updateRootOpenIntent(ready)) throw new Error('Vault open was confirmed, but local state changed. Reconcile the saved request before borrowing.');
          successMessage = `Vault #${vaultId} is open with collateral. Review and click Borrow separately to continue.`;
          return;
        }
      }
      const unresolved = { ...record, stage: 'ambiguous' as const };
      updateRootOpenIntent(unresolved);
      rootIngressWarning = `${status?.last_error[0] || 'The open is pending or unresolved. The same saved request is held; do not start another open.'}${result.approvalMayHaveMutated ? ' A token approval may also have changed.' : ''}`;
    });
  }

  async function borrowRootVault() {
    const record = rootOpenIntent;
    if (!record || record.stage !== 'borrow_ready' || record.vaultId === null) return;
    const locks = typeof navigator !== 'undefined' ? (navigator as any).locks : null;
    if (!locks?.request) { errorMessage = 'This browser cannot safely coordinate the borrow across tabs. Use one tab and try again when Web Locks are available.'; return; }
    actionInProgress = true; errorMessage = ''; rootIngressWarning = '';
    try {
      await locks.request(collateralSequenceLockName(record.principal, ROOT_NETWORK, record.ledger), async () => {
        const current = parseRootOpenIntent(record.principal);
        if (!current || current.requestId !== record.requestId || current.stage !== 'borrow_ready') throw new Error('The saved open intent changed in another tab. Recheck before borrowing.');
        const ctx = makeRootActionContext(record.principal);
        const status = await protocolService.getCollateralIngressBound(ctx, record.ledger, BigInt(record.requestId));
        if (!rootStatusMatches(record, status) || !status || !('Complete' in status.phase) || !status.result[0] || !('Open' in status.result[0])) {
          throw new Error('The exact collateral open is not confirmed in the backend journal. Borrowing is paused.');
        }
        const before = (await rootVaultSnapshot(record.principal)).find((vault) => vault.vaultId === record.vaultId);
        ctx.assertCurrent();
        if (!before || before.collateralAmount.toString() !== record.collateralRaw || before.borrowedIcusd !== 0n) {
          throw new Error('The vault is missing, changed, or already has debt. Recheck before borrowing.');
        }
        if (collateralAmount !== record.collateralAmount || icusdAmount !== record.icusdAmount || !isValidCollateralRatio) {
          throw new Error('The saved borrow terms changed or no longer satisfy the displayed collateral ratio. Review the terms and open a new vault only after resolving this saved operation.');
        }
        const pending = { ...record, stage: 'borrow_pending' as const };
        if (!updateRootOpenIntent(pending)) throw new Error('The borrow could not be saved safely, so nothing was submitted.');
        const result = await protocolService.borrowFromVaultBound(ctx, record.vaultId!, BigInt(record.borrowRaw));
        const after = (await rootVaultSnapshot(record.principal)).find((vault) => vault.vaultId === record.vaultId);
        ctx.assertCurrent();
        if (result.kind === 'dispatched_ok' && result.submittedIcusdRaw === BigInt(record.borrowRaw) &&
            result.blockIndex !== null && after?.borrowedIcusd.toString() === record.borrowRaw) {
          updateRootOpenIntent({ ...pending, stage: 'done' });
          successMessage = `Successfully created vault #${record.vaultId} and borrowed ${record.icusdAmount} icUSD.`;
          if ($principal) await appDataStore.refreshAll($principal);
        } else if (result.kind === 'predispatch_aborted') {
          updateRootOpenIntent({ ...record, stage: 'borrow_ready' });
          errorMessage = `${result.errorMessage || 'Borrow was not submitted.'} The vault remains open; use the separate borrow action after rechecking.`;
        } else {
          updateRootOpenIntent({ ...pending, stage: 'ambiguous' });
          rootIngressWarning = 'Borrow was submitted or may have reached the backend, but exact debt is not confirmed. Do not retry until the vault state is resolved.';
        }
      });
    } catch (error) {
      errorMessage = error instanceof Error ? error.message : 'Borrow result is unresolved.';
    } finally { actionInProgress = false; }
  }

  async function recheckRootOpen() {
    const record = rootOpenIntent;
    if (!record || !['opening', 'borrow_pending', 'ambiguous'].includes(record.stage)) return;
    actionInProgress = true;
    try { await reconcileRootOpen(record.principal); }
    finally { actionInProgress = false; }
  }

  onMount(() => {
    loadProtocolData();
    refreshPrice();
    seasonStore.ensureLoaded();
    // Ensure collateral configs are loaded (provides per-asset CR, fees, etc.)
    void collateralStore.fetchSupportedCollateral().then(() => {
      const connectedPrincipal = get(principal);
      if (connectedPrincipal) void reconcileRootOpen(connectedPrincipal.toText());
    }).catch(() => {});
    const unsubscribePrincipal = principal.subscribe((value) => {
      if (value) void reconcileRootOpen(value.toText());
      else { rootOpenIntent = null; rootIngressWarning = ''; }
    });
    priceRefreshInterval = setInterval(refreshPrice, 30000);
    return () => { unsubscribePrincipal(); if (priceRefreshInterval) clearInterval(priceRefreshInterval); };
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
    if (xrpBorrowFlowActive) return;
    if (!$isConnected) { errorMessage = 'Please connect your wallet first'; return; }
    if (rootIngressWarning) { errorMessage = rootIngressWarning; return; }
    if (collateralAmount <= 0) { errorMessage = 'Please enter a valid collateral amount'; return; }
    // Native XRP: the reserve comes OUT of the sent amount, so anything at or
    // below the reserve credits zero collateral and the backend rejects it.
    if (isNativeXrpSelected && creditedCollateralAmount <= 0) {
      errorMessage = `Send more than ${xrpReserveEstimate} XRP — that much is the XRPL account reserve, which is deducted from your deposit.`;
      return;
    }
    if (icusdAmount <= 0) { errorMessage = 'Please enter a valid icUSD amount to borrow'; return; }
    if (!isValidCollateralRatio) { errorMessage = `Collateral ratio must be at least ${(selectedMinCR * 100).toFixed(0)}%`; return; }
    if (isNativeXrpSelected && selectedCollateralInfo) {
      errorMessage = '';
      successMessage = '';
      xrpBorrowIntent = {
        collateralAmount,
        icusdAmount,
        collateralInfo: selectedCollateralInfo,
      };
      return;
    }
    if (rootOpenIntent && rootOpenIntent.stage !== 'done') {
      errorMessage = rootOpenIntent.stage === 'borrow_ready'
        ? `Vault #${rootOpenIntent.vaultId} is already open. Use the separate Borrow confirmation below.`
        : 'A previous collateral request is unresolved. Recheck it before starting another open.';
      return;
    }
    if (rootOpenIntent?.stage === 'done') {
      if (!persistRootOpenIntent(null, rootOpenIntent.principal, rootOpenIntent)) {
        rootIngressWarning = 'A newer collateral intent replaced this completed record in another tab. Recheck account state before opening again.';
        return;
      }
      rootOpenIntent = null;
    }
    actionInProgress = true; errorMessage = ''; successMessage = '';
    try {
      const ownerText = $principal?.toText();
      if (!ownerText) throw new Error('Please connect your wallet first.');
      await runRootOpen(ownerText);
    } catch (error) {
      console.error('Error creating vault:', error);
      errorMessage = error instanceof Error ? error.message : 'Unknown error occurred';
    } finally { actionInProgress = false; }
  }

  async function handleXrpBorrowComplete(event: CustomEvent<{ vaultId: number; oisyResilient?: boolean }>) {
    const intent = xrpBorrowIntent;
    const vaultLabel = `vault #${event.detail.vaultId}`;
    successMessage = event.detail.oisyResilient
      ? `Submitted: created ${vaultLabel} and borrowed ${intent?.icusdAmount ?? icusdAmount} icUSD. (Wallet glitch ignored — confirmed on-chain.)`
      : `Successfully created ${vaultLabel} and borrowed ${intent?.icusdAmount ?? icusdAmount} icUSD!`;
    xrpBorrowIntent = null;
    if ($principal) await appDataStore.refreshAll($principal);
    collateralAmount = 1;
    icusdAmount = 5;
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
                  class="icp-input form-input" placeholder="0.00" disabled={actionInProgress || xrpBorrowFlowActive} />
                <button class="token-selector"
                  disabled={xrpBorrowFlowActive}
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
                <p class="form-hint">≈ ${formatNumber(collateralValue)}</p>
              {/if}
            </div>

            <!-- icUSD borrow input -->
            <div class="form-field">
              <label for="icusd-amount" class="form-label">icUSD to Borrow</label>
              <div class="input-wrap">
                <input id="icusd-amount" type="number" bind:value={icusdAmount} min="0" step="0.01"
                  class="icp-input form-input form-input-with-max" placeholder="0.00" disabled={actionInProgress || xrpBorrowFlowActive} />
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
                {#if rateCurve}
                  <div class="fee-row"><span>Interest Rate</span><span>{(projectedMintRate * 100).toFixed(2)}% APR</span></div>
                {/if}
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
            {#if rootIngressWarning}
              <div class="msg-error" role="alert">{rootIngressWarning}</div>
              {#if rootOpenIntent && ['opening', 'borrow_pending', 'ambiguous'].includes(rootOpenIntent.stage)}
                <button class="btn-primary cta-button" type="button" disabled={actionInProgress} on:click={recheckRootOpen}>
                  {actionInProgress ? 'Rechecking…' : 'Recheck saved request'}
                </button>
              {/if}
            {/if}
            {#if rootOpenIntent?.stage === 'borrow_ready'}
              <div class="msg-success">Vault #{rootOpenIntent.vaultId} is open. Borrowing requires this separate confirmation.</div>
              <button class="btn-primary cta-button" type="button" disabled={actionInProgress || !$isConnected} on:click={borrowRootVault}>
                {actionInProgress ? 'Borrowing…' : `Borrow ${rootOpenIntent.icusdAmount} icUSD from vault #${rootOpenIntent.vaultId}`}
              </button>
            {/if}

            {#if $earningActive}
              <div class="points-hint">
                <MultiplierBadge multiplier={1} variant="full" />
                <span>on the icUSD you mint, while the vault is open</span>
              </div>
            {/if}

            <button
              class="btn-primary cta-button"
              on:click={createVault}
              disabled={actionInProgress || xrpBorrowFlowActive || !$isConnected || Boolean(rootIngressWarning) || (Boolean(rootOpenIntent) && rootOpenIntent?.stage !== 'done')}
            >
              {#if !$isConnected}Connect Wallet to Continue
              {:else if actionInProgress}Opening Vault…
              {:else if xrpBorrowFlowActive}Preparing XRP vault...
              {:else}Create Vault (Borrow Separately){/if}
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

  /* Airdrop points hint above the CTA */
  .points-hint {
    display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap;
    font-size: 0.75rem; color: var(--rumi-text-muted);
  }

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

{#if xrpBorrowIntent}
  <XrpBorrowModal
    collateralAmount={xrpBorrowIntent.collateralAmount}
    icusdAmount={xrpBorrowIntent.icusdAmount}
    collateralInfo={xrpBorrowIntent.collateralInfo}
    on:close={() => { xrpBorrowIntent = null; }}
    on:complete={handleXrpBorrowComplete}
  />
{/if}
