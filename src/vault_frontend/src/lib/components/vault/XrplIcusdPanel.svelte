<script lang="ts">
  /**
   * XRPL icUSD panel (Task 6): enable the icUSD trust line, borrow into an XRPL
   * wallet, repay with XRPL icUSD, move existing icUSD to XRPL, and follow every
   * asynchronous operation through reload.
   *
   * All state transitions and authority rules live in xrplIcusdPanelController.ts
   * (unit-tested). This component only renders the controller's state and turns
   * clicks into controller calls. Backend operation views are the source of truth.
   */
  import { onDestroy, onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { walletStore } from '$lib/stores/wallet';
  import { vaultStore } from '$lib/stores/vaultStore';
  import { collateralStore } from '$lib/stores/collateralStore';
  import { currentWalletType, WALLET_TYPES } from '$lib/services/auth';
  import { CONFIG } from '$lib/config';
  import { walletOperations } from '$lib/services/protocol/walletOperations';
  import { fetchLedgerFeeStrict } from '$lib/services/ledgerFeeService';
  import { getXrplWalletAdapter } from '$lib/services/xrplWalletService';
  import { xrplRpcEndpoints } from '$lib/components/borrow/xrpDepositFlow';
  import XrplWalletConnect from '$lib/components/wallet/XrplWalletConnect.svelte';
  import { formatAddress } from '$lib/utils/format';
  import {
    createDefaultXrplIcusdApi,
    estimateBorrow,
    fetchVaultDebtE8s,
    formatE8s,
    parseE8s,
    repaymentSummary,
    type ClaimActionName,
    type OperationUi,
  } from '$lib/services/xrplIcusdService';
  import {
    createXrplIcusdController,
    importDisclosure,
    type XrplIcusdController,
    type XrplIcusdPanelState,
  } from './xrplIcusdPanelController';

  /** Native-XRP vaults to borrow against / repay. Defaults to the user's native-XRP vaults. */
  export let vaults: Array<{ vaultId: number; borrowedIcusd: number; collateralType: string }> | null = null;

  /** When set (e.g. from a vault card's "Repay from XRPL wallet"), selects that vault. */
  export let focusVaultId: number | null = null;
  let lastFocus: number | null = null;

  const POLL_MS = 15_000;

  let controller: XrplIcusdController | null = null;
  let unsubscribe: (() => void) | null = null;
  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let ready = false;
  let s: XrplIcusdPanelState | null = null;

  let selectedVaultId: number | null = null;
  let borrowText = '';
  let repayText = '';
  let fullClose = false;
  let importText = '';
  let topUpText: Record<string, string> = {};
  let ledgerFeeE8s: bigint | null = null;

  $: principalText = $walletStore.principal?.toText?.() ?? '';
  $: oisy = $currentWalletType === WALLET_TYPES.OISY;

  $: nativeVaults = (vaults ?? $vaultStore.vaults.filter(
    (v) => collateralStore.getCollateralInfo(v.collateralType)?.custodyKind === 'NativeXrp'
  )) as Array<{ vaultId: number; borrowedIcusd: number; collateralType: string }>;
  $: if (nativeVaults.length > 0 && (selectedVaultId === null || !nativeVaults.some((v) => v.vaultId === selectedVaultId))) {
    selectedVaultId = nativeVaults[0].vaultId;
  }
  $: if (focusVaultId !== null && focusVaultId !== lastFocus && nativeVaults.some((v) => v.vaultId === focusVaultId)) {
    selectedVaultId = focusVaultId;
    lastFocus = focusVaultId;
  }
  $: selectedVault = nativeVaults.find((v) => v.vaultId === selectedVaultId) ?? null;
  // Exact debt (bigint e8s) from the backend vault query. The float on the vault
  // card is never used for an amount that is sent or compared.
  let exactDebtE8s: bigint | null = null;
  let debtReadFailed = false;
  let debtReq = 0;
  async function loadDebt(vaultId: number | null) {
    const req = ++debtReq;
    if (vaultId === null) { exactDebtE8s = null; debtReadFailed = false; return; }
    const v = await fetchVaultDebtE8s(vaultId);
    if (req !== debtReq) return;
    exactDebtE8s = v;
    debtReadFailed = v === null;
  }
  $: void loadDebt(selectedVaultId);
  // The debt changes after a payment is applied: read it again.
  $: if (s?.repay.phase === 'observed') void loadDebt(selectedVaultId);
  $: vaultDebtE8s = exactDebtE8s;
  $: repayOp = s && s.repay.operationId !== null ? s.operations.find((o) => o.id === s!.repay.operationId) ?? null : null;

  $: active = s?.gate?.state === 'active';
  $: conn = s?.connection ?? null;
  $: networkOk = !!(conn && s?.config && conn.network === s.config.network);
  $: grossE8s = parseE8s(borrowText);
  $: feeRate = selectedVault ? collateralStore.getCollateralInfo(selectedVault.collateralType)?.borrowingFee ?? 0 : 0;
  $: estimate = grossE8s !== null && grossE8s > 0n ? estimateBorrow(grossE8s, feeRate) : null;
  $: repayE8s = fullClose ? vaultDebtE8s : parseE8s(repayText);
  $: importE8s = parseE8s(importText);
  $: importInfo = importE8s !== null && importE8s > 0n && ledgerFeeE8s !== null ? importDisclosure(importE8s, ledgerFeeE8s) : null;
  $: hasActive = !!s?.operations.some((o) => !o.terminal && o.state !== 'held' && o.state !== 'refund_pending');
  $: reserveXrp = s ? formatDrops(s.reserve.drops) : '0.2';

  function formatDrops(drops: bigint): string {
    const whole = drops / 1_000_000n;
    const frac = (drops % 1_000_000n).toString().padStart(6, '0').replace(/0+$/, '');
    return frac ? `${whole}.${frac}` : `${whole}`;
  }

  onMount(async () => {
    try {
      const api = await createDefaultXrplIcusdApi();
      controller = createXrplIcusdController({
        api,
        wallet: getXrplWalletAdapter(),
        isOisy: () => get(currentWalletType) === WALLET_TYPES.OISY,
        ownerKey: principalText || 'anonymous',
        endpoints: (network) => xrplRpcEndpoints(network),
        approve: (amount) => walletOperations.approveIcusdTransfer(amount, CONFIG.currentCanisterId),
        ledgerFee: () => fetchLedgerFeeStrict({ ledgerId: CONFIG.currentIcusdLedgerId, decimals: 8, symbol: 'icUSD' }),
      });
      unsubscribe = controller.subscribe((next) => { s = next; });
      ready = true;
      await controller.init();
      fetchLedgerFeeStrict({ ledgerId: CONFIG.currentIcusdLedgerId, decimals: 8, symbol: 'icUSD' })
        .then((f) => { ledgerFeeE8s = f; })
        .catch(() => {});
      // Timer ticks are queries only (never an authenticated update).
      pollTimer = setInterval(() => {
        if (hasActive || s?.repay.phase === 'wallet_pending' || s?.repay.phase === 'submitted') void controller?.pollTick();
      }, POLL_MS);
    } catch (e) {
      console.warn('XRPL icUSD panel failed to start:', e);
    }
  });

  onDestroy(() => {
    if (pollTimer) clearInterval(pollTimer);
    unsubscribe?.();
    controller?.dispose();
  });

  function amountOrNull(v: bigint | null): v is bigint {
    return v !== null && v > 0n;
  }

  function doBorrow() {
    if (!controller || selectedVaultId === null || !amountOrNull(grossE8s)) return;
    void controller.borrow({ vaultId: BigInt(selectedVaultId), grossE8s });
  }
  async function doPrepareRepay() {
    if (!controller || selectedVaultId === null) return;
    let max = repayE8s;
    if (fullClose) {
      // Re-read the exact debt right now: the quote must cover what is owed at this moment.
      max = await fetchVaultDebtE8s(selectedVaultId);
      if (max === null) { debtReadFailed = true; return; }
      exactDebtE8s = max;
    }
    if (!amountOrNull(max)) return;
    void controller.prepareRepayment({ vaultId: BigInt(selectedVaultId), maxE8s: max, fullClose });
  }
  function cutoffText(ns: bigint): string {
    try { return new Date(Number(ns / 1_000_000n)).toLocaleString(); } catch { return ''; }
  }
  function doAddPayment(op: OperationUi) {
    if (!controller) return;
    const raw = topUpText[op.id.toString()] ?? '';
    const amount = parseE8s(raw);
    const vaultId = op.vaultId ?? (selectedVaultId !== null ? BigInt(selectedVaultId) : null);
    if (!amountOrNull(amount) || vaultId === null) return;
    void controller.addPayment(op.id, { vaultId, maxE8s: amount });
  }

  const CLAIM_LABEL: Record<ClaimActionName, string> = {
    ReceiveCanonical: 'Receive as ICP icUSD',
    ReissueXrpl: 'Reissue on XRPL',
    CancelUnexecutedRepayment: 'Cancel this repayment',
  };

  function opTitle(o: OperationUi): string {
    return o.kind === 'Borrow' ? 'Borrow to XRPL' : o.kind === 'Repayment' ? 'Repayment' : o.kind === 'Import' ? 'Move icUSD to XRPL' : 'Refund';
  }
  function summary(o: OperationUi) {
    return repaymentSummary(o, o.vaultId !== null && o.vaultId === (selectedVaultId !== null ? BigInt(selectedVaultId) : null) ? vaultDebtE8s : null);
  }
  const REPAY_STEPS: Array<[string, string]> = [
    ['repay_payment_pending', 'Payment pending'],
    ['repay_xrpl_received', 'XRPL received'],
    ['repay_debt_updated', 'Debt updated'],
  ];
  function stepIndex(state: string): number {
    if (state === 'complete') return 3;
    return REPAY_STEPS.findIndex(([k]) => k === state);
  }
</script>

<section class="xi-panel" data-testid="xrpl-icusd-panel">
  <header class="xi-head">
    <h3>icUSD on XRPL</h3>
    <button class="xi-secondary" disabled={!ready || s?.busy === 'refresh'} on:click={() => controller?.refresh()}>
      {s?.busy === 'refresh' ? 'Refreshing…' : 'Refresh'}
    </button>
  </header>

  {#if !s}
    <p class="xi-muted">Loading…</p>
  {:else}
    {#if s.configError && !s.config}
      <p class="xi-muted">XRPL icUSD status could not be loaded: {s.configError}</p>
    {/if}
    {#if s.gate && s.gate.state !== 'active'}
      <p class="xi-note" data-testid="xrpl-icusd-gate">{s.gate.message}</p>
    {/if}

    <XrplWalletConnect />

    {#if s.notice}<p class="xi-note" role="status" aria-live="polite">{s.notice}</p>{/if}

    {#if active}
      {#if conn && !networkOk}
        <p class="xi-warn" role="alert">Your wallet is on {conn.network}; XRPL icUSD runs on {s.config?.network}. Switch network in your wallet and reconnect.</p>
      {/if}

      <!-- Trust line -->
      <div class="xi-group" data-testid="xrpl-icusd-trustline">
        <div class="xi-group-title">1. Enable icUSD in your wallet</div>
        {#if !conn}
          <p class="xi-muted">Connect an XRPL wallet to continue.</p>
        {:else if s.trustLine.status === 'present'}
          <p class="xi-ok" role="status">icUSD is enabled for {formatAddress(conn.address, 8, 6)}{s.trustLine.limit ? ` (limit ${s.trustLine.limit})` : ''}.</p>
        {:else}
          <p class="xi-muted">
            One-time setup: a trust line to Rumi's icUSD issuer. XRPL holds
            <strong>{reserveXrp} XRP</strong> as an owner reserve while the line exists{s.reserve.live ? '' : ' (standard amount; live value unavailable)'}.
            {#if s.trustLine.status === 'checking'}Checking your wallet…{:else if s.trustLine.status === 'error'}Could not check the XRP Ledger right now.{:else if s.trustLine.status === 'missing'}No icUSD trust line found yet.{/if}
          </p>
          {#if s.trustSet.phase === 'proposed'}
            <p class="xi-muted">Review in your wallet: {s.config?.issuerAddress ? formatAddress(s.config.issuerAddress, 8, 6) : ''} icUSD, limit {formatE8s(s.config?.outstandingCapE8s ?? 0n)}.</p>
            <button class="xi-primary" on:click={() => controller?.signTrustSet()}>Approve in wallet</button>
          {:else if s.trustSet.phase === 'awaiting_wallet' || s.trustSet.phase === 'wallet_pending'}
            <p class="xi-muted" role="status" aria-live="polite">Waiting for your wallet…</p>
            {#if s.prompt && s.prompt.flow === 'trustset'}
              <div class="xi-prompt" data-testid="xrpl-icusd-prompt">
                <p class="xi-muted">Approve in Xaman. <a href={s.prompt.deepLink} target="_blank" rel="noopener noreferrer">Open in Xaman</a></p>
                {#if s.prompt.qrPng}<img class="xi-qr" src={s.prompt.qrPng} alt="Xaman signing QR code" />{/if}
              </div>
            {/if}
          {:else if s.trustSet.phase === 'submitted'}
            <p class="xi-muted" role="status" aria-live="polite">Submitted. Waiting for the XRP Ledger to show the trust line.</p>
          {:else}
            <button class="xi-primary" disabled={!networkOk} on:click={() => controller?.proposeTrustSet()}>Enable icUSD</button>
          {/if}
          {#if s.trustSet.error}<p class="xi-warn" role="alert">{s.trustSet.error}</p>{/if}
          <button class="xi-link" on:click={() => controller?.checkTrustLine()}>Re-check</button>
        {/if}
      </div>

      <!-- Vault actions -->
      <div class="xi-group">
        <div class="xi-group-title">2. Borrow and repay on XRPL</div>
        {#if nativeVaults.length === 0}
          <p class="xi-muted">Open and fund a native XRP vault first.</p>
        {:else}
          <label class="xi-field">
            <span>Vault</span>
            <select bind:value={selectedVaultId}>
              {#each nativeVaults as v (v.vaultId)}<option value={v.vaultId}>#{v.vaultId}</option>{/each}
            </select>
          </label>

          <div class="xi-card" data-testid="xrpl-icusd-borrow">
            <label class="xi-field">
              <span>Borrow (icUSD, adds to vault debt)</span>
              <input bind:value={borrowText} inputmode="decimal" placeholder="0.00" spellcheck="false" autocomplete="off" />
            </label>
            {#if estimate}
              <dl class="xi-dl">
                <dt>Debt recorded</dt><dd>{formatE8s(estimate.grossE8s)} icUSD</dd>
                <dt>Borrowing fee (estimate)</dt><dd>{formatE8s(estimate.feeE8s)} icUSD</dd>
                <dt>icUSD delivered to you</dt><dd>{formatE8s(estimate.netE8s)} icUSD</dd>
                <dt>Delivered to</dt><dd>{conn ? formatAddress(conn.address, 8, 6) : 'connect a wallet'}</dd>
              </dl>
              <p class="xi-muted">Rumi records the debt first; the icUSD arrives on XRPL after validation. The final fee is set by the protocol when you confirm.</p>
            {:else if borrowText.trim() !== ''}
              <p class="xi-warn" role="alert">Enter an amount with at most 8 decimals.</p>
            {/if}
            <button class="xi-primary" disabled={!conn || !networkOk || !amountOrNull(grossE8s) || s.borrow.phase === 'submitting'} on:click={doBorrow}>
              {s.borrow.phase === 'submitting' ? 'Authorizing…' : 'Borrow to XRPL'}
            </button>
            {#if s.borrow.error}<p class="xi-warn" role="alert">{s.borrow.error}</p>{/if}
          </div>

          <div class="xi-card" data-testid="xrpl-icusd-repay">
            <label class="xi-field">
              <span>Repay with XRPL icUSD</span>
              <input bind:value={repayText} disabled={fullClose} inputmode="decimal" placeholder="0.00" spellcheck="false" autocomplete="off" />
            </label>
            <label class="xi-check">
              <input type="checkbox" bind:checked={fullClose} />
              <span>Pay the whole debt{vaultDebtE8s !== null ? ` (${formatE8s(vaultDebtE8s)} icUSD now)` : ''}</span>
            </label>
            {#if fullClose && debtReadFailed}
              <p class="xi-warn" role="alert">Could not read the exact debt right now. Try again in a moment.</p>
            {/if}
            {#if fullClose}
              <p class="xi-muted">Interest may grow before your payment lands. If it falls short, Rumi holds the payment untouched and you can add the difference.</p>
            {/if}

            {#if s.repay.phase === 'idle' || s.repay.phase === 'error' || s.repay.phase === 'rejected' || s.repay.phase === 'invalidated' || s.repay.phase === 'observed'}
              <button class="xi-primary" disabled={!conn || !networkOk || !amountOrNull(repayE8s)} on:click={() => doPrepareRepay()}>
                Prepare repayment
              </button>
            {/if}
            {#if s.repay.phase === 'preparing'}<p class="xi-muted" role="status" aria-live="polite">Preparing…</p>{/if}
            {#if s.repay.phase === 'prepared' && s.repay.intent}
              <dl class="xi-dl">
                <dt>You will send at most</dt><dd>{formatE8s(s.repay.intent.maxE8s)} icUSD</dd>
                <dt>To</dt><dd>Rumi issuer {formatAddress(s.repay.intent.issuer, 8, 6)}</dd>
                <dt>Destination tag</dt><dd>{s.repay.intent.destinationTag}</dd>
                <dt>From</dt><dd>{formatAddress(s.repay.intent.payer, 8, 6)}</dd>
              </dl>
              <div class="xi-row">
                <button class="xi-primary" on:click={() => controller?.payRepayment()}>Pay in wallet</button>
                <button class="xi-link" on:click={() => controller?.cancelRepayment()}>Cancel</button>
              </div>
            {/if}
            {#if s.repay.phase === 'awaiting_wallet' || s.repay.phase === 'wallet_pending'}
              <p class="xi-muted" role="status" aria-live="polite">Waiting for your wallet to approve the payment…</p>
              {#if s.repay.phase === 'wallet_pending'}
                <button class="xi-link" on:click={() => controller?.payRepayment()}>Check wallet again</button>
              {/if}
            {/if}
            {#if s.repay.phase === 'submitted'}
              <p class="xi-muted" role="status" aria-live="polite">Payment submitted. Your debt changes only after XRPL validates it and Rumi verifies it.</p>
              <button class="xi-primary" on:click={() => controller?.observeRepayment()}>Tell Rumi about my payment</button>
            {/if}
            {#if s.repay.phase === 'observing'}<p class="xi-muted" role="status" aria-live="polite">Rumi is checking your payment…</p>{/if}
            {#if s.prompt && s.prompt.flow === 'repay'}
              <div class="xi-prompt" data-testid="xrpl-icusd-prompt">
                <p class="xi-muted">Approve in Xaman. <a href={s.prompt.deepLink} target="_blank" rel="noopener noreferrer">Open in Xaman</a></p>
                {#if s.prompt.qrPng}<img class="xi-qr" src={s.prompt.qrPng} alt="Xaman signing QR code" />{/if}
              </div>
            {/if}
            {#if repayOp && (repayOp.observedExact !== null || repayOp.debtAppliedE8s > 0n)}
              <dl class="xi-dl" data-testid="xrpl-icusd-repay-amounts">
                {#if repayOp.observedExact !== null}<dt>Received by Rumi</dt><dd>{repayOp.observedExact} icUSD</dd>{/if}
                <dt>Applied to debt</dt><dd>{formatE8s(repayOp.debtAppliedE8s)} icUSD</dd>
              </dl>
            {/if}
            {#if s.repay.error}<p class="xi-warn" role="alert">{s.repay.error}</p>{/if}
          </div>
        {/if}
      </div>

      <!-- Import -->
      <div class="xi-group" data-testid="xrpl-icusd-import">
        <div class="xi-group-title">Move existing icUSD to XRPL</div>
        <p class="xi-muted">Moves icUSD you already hold into XRPL icUSD. This creates no new debt.</p>
        <label class="xi-field">
          <span>Amount (icUSD)</span>
          <input bind:value={importText} inputmode="decimal" placeholder="0.00" spellcheck="false" autocomplete="off" />
        </label>
        {#if importInfo}
          <dl class="xi-dl">
            <dt>Allowance you grant Rumi</dt><dd>{formatE8s(importInfo.allowanceE8s)} icUSD</dd>
            <dt>Ledger fees (approve + transfer)</dt><dd>{formatE8s(importInfo.totalLedgerFeesE8s)} icUSD</dd>
            <dt>Leaves your balance</dt><dd>{formatE8s(importInfo.debitedE8s)} icUSD</dd>
            <dt>Delivered on XRPL</dt><dd>{formatE8s(importInfo.amountE8s)} icUSD</dd>
          </dl>
        {/if}
        {#if s.importFlow.phase === 'approved'}
          <p class="xi-ok">Approved. {oisy ? 'Click again to move the icUSD.' : ''}</p>
          <button class="xi-primary" on:click={() => controller?.finishImport()}>Move icUSD to XRPL</button>
        {:else}
          <button class="xi-primary" disabled={!conn || !networkOk || !amountOrNull(importE8s) || s.importFlow.phase === 'approving' || s.importFlow.phase === 'submitting'} on:click={() => amountOrNull(importE8s) && controller?.importCanonical(importE8s)}>
            {s.importFlow.phase === 'approving' ? 'Approving…' : s.importFlow.phase === 'submitting' ? 'Moving…' : oisy ? 'Approve icUSD' : 'Move icUSD to XRPL'}
          </button>
        {/if}
        {#if s.importFlow.phase === 'done'}<p class="xi-ok" role="status">Import recorded. Delivery completes after XRPL validation.</p>{/if}
        {#if s.importFlow.error}<p class="xi-warn" role="alert">{s.importFlow.error}</p>{/if}
      </div>
    {/if}

    <!-- Operations (always shown, even when the lane is paused) -->
    <div class="xi-group" data-testid="xrpl-icusd-operations">
      <div class="xi-group-title">
        Your XRPL operations
        {#if s.operationsSource === 'cache'}<span class="xi-muted">(saved view; click Refresh to update)</span>{/if}
      </div>
      {#if s.operationsError}<p class="xi-warn" role="alert">{s.operationsError}</p>{/if}
      {#if s.operations.length === 0}
        <p class="xi-muted">Nothing yet.</p>
      {/if}
      {#each s.operations as o (o.id)}
        <div class="xi-op" data-state={o.state}>
          <div class="xi-op-head">
            <span class="xi-op-title">#{o.id} {opTitle(o)}</span>
            <span class="xi-chip xi-{o.state}">{o.label}</span>
          </div>
          {#if o.detail}<p class="xi-muted" role="status" aria-live="polite">{o.detail}</p>{/if}
          {#if o.settlementCutoffNs !== null}
            <p class="xi-muted" data-testid="xrpl-icusd-cutoff">Rumi holds this until {cutoffText(o.settlementCutoffNs)}.</p>
          {/if}

          {#if o.kind === 'Borrow' || o.kind === 'Import'}
            <dl class="xi-dl">
              {#if o.kind === 'Borrow'}
                <dt>Debt recorded</dt><dd>{formatE8s(o.grossE8s)} icUSD</dd>
                <dt>Fee</dt><dd>{formatE8s(o.feeE8s)} icUSD</dd>
              {/if}
              <dt>{o.deliveredE8s !== null ? 'Delivered' : 'To be delivered'}</dt><dd>{formatE8s(o.netE8s)} icUSD</dd>
              {#if o.recipient}<dt>To</dt><dd>{formatAddress(o.recipient, 8, 6)}</dd>{/if}
            </dl>
          {/if}

          {#if o.kind === 'Repayment'}
            {#if stepIndex(o.state) >= 0}
              <ol class="xi-steps">
                {#each REPAY_STEPS as [, name], i}
                  <li class:done={stepIndex(o.state) > i} class:current={stepIndex(o.state) === i}>{name}</li>
                {/each}
              </ol>
            {/if}
            {@const sm = summary(o)}
            <dl class="xi-dl">
              {#if sm.receivedExact !== null}<dt>Received by Rumi</dt><dd>{sm.receivedExact} icUSD</dd>{/if}
              <dt>Applied to debt</dt><dd>{formatE8s(sm.appliedE8s)} icUSD</dd>
              {#if sm.heldExact !== null}<dt>Held for you</dt><dd>{sm.heldExact} icUSD</dd>{/if}
              {#if sm.remainingDebtE8s !== null}<dt>Vault debt now</dt><dd>{formatE8s(sm.remainingDebtE8s)} icUSD</dd>{/if}
            </dl>
          {/if}

          {#if o.notes.length}<p class="xi-muted">{o.notes.join(' ')}</p>{/if}
          {#if o.xrplTxHashes.length}<p class="xi-muted">XRPL tx {formatAddress(o.xrplTxHashes[o.xrplTxHashes.length - 1], 8, 6)}</p>{/if}

          <div class="xi-row">
            {#each o.claimActions as action}
              <button class="xi-secondary" disabled={s.busy === `claim:${o.id}`} on:click={() => controller?.resolveClaim(o.id, action)}>{CLAIM_LABEL[action]}</button>
            {/each}
            {#if o.canResume}
              <button class="xi-secondary" disabled={s.busy === `resume:${o.id}`} on:click={() => controller?.resume(o.id)}>Resume</button>
            {/if}
          </div>

          {#if o.kind === 'Repayment' && o.state === 'held' && active}
            <div class="xi-row">
              <input class="xi-small" placeholder="Add payment (icUSD)" bind:value={topUpText[o.id.toString()]} inputmode="decimal" spellcheck="false" autocomplete="off" />
              <button class="xi-secondary" disabled={!conn || !networkOk} on:click={() => doAddPayment(o)}>Add payment</button>
            </div>
            <p class="xi-muted">Adding a payment prepares a new tagged payment tied to this one. You approve it in your wallet; nothing is sent automatically.</p>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</section>

<style>
  .xi-panel { display: flex; flex-direction: column; gap: 14px; padding: 16px; border: 1px solid var(--rumi-border, rgba(148, 163, 184, 0.2)); border-radius: 14px; background: var(--rumi-surface, rgba(15, 23, 42, 0.4)); }
  .xi-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
  .xi-head h3 { margin: 0; font-size: 1rem; color: var(--rumi-text, #e2e8f0); }
  .xi-group { display: flex; flex-direction: column; gap: 8px; }
  .xi-group-title { font-size: 0.78rem; text-transform: uppercase; letter-spacing: 0.04em; color: var(--rumi-text-muted, #94a3b8); }
  .xi-card, .xi-op { display: flex; flex-direction: column; gap: 8px; padding: 10px 12px; border-radius: 10px; background: var(--rumi-surface-2, rgba(30, 41, 59, 0.5)); }
  .xi-op-head { display: flex; justify-content: space-between; gap: 8px; align-items: center; flex-wrap: wrap; }
  .xi-op-title { font-weight: 600; color: var(--rumi-text, #e2e8f0); }
  .xi-chip { font-size: 0.75rem; padding: 2px 8px; border-radius: 999px; background: rgba(148, 163, 184, 0.18); color: var(--rumi-text-secondary, #cbd5e1); }
  .xi-delivered, .xi-complete, .xi-repay_debt_updated { background: rgba(20, 184, 166, 0.2); color: #5eead4; }
  .xi-held, .xi-refund_pending, .xi-recoverable_error { background: rgba(245, 158, 11, 0.2); color: #fcd34d; }
  .xi-muted { margin: 0; font-size: 0.82rem; color: var(--rumi-text-muted, #94a3b8); }
  .xi-note { margin: 0; font-size: 0.85rem; color: var(--rumi-text-secondary, #cbd5e1); padding: 8px 10px; border-radius: 8px; background: rgba(148, 163, 184, 0.12); }
  .xi-warn { margin: 0; font-size: 0.82rem; color: #fca5a5; }
  .xi-ok { margin: 0; font-size: 0.85rem; color: #5eead4; }
  .xi-field { display: flex; flex-direction: column; gap: 4px; font-size: 0.8rem; color: var(--rumi-text-muted, #94a3b8); }
  .xi-field input, .xi-field select, .xi-small { padding: 7px 10px; border-radius: 8px; border: 1px solid var(--rumi-border, rgba(148, 163, 184, 0.25)); background: rgba(15, 23, 42, 0.45); color: var(--rumi-text, #e2e8f0); }
  .xi-check { display: flex; gap: 8px; align-items: center; font-size: 0.85rem; color: var(--rumi-text-secondary, #cbd5e1); }
  .xi-dl { display: grid; grid-template-columns: auto 1fr; gap: 2px 12px; margin: 0; font-size: 0.85rem; }
  .xi-dl dt { color: var(--rumi-text-muted, #94a3b8); }
  .xi-dl dd { margin: 0; text-align: right; color: var(--rumi-text, #e2e8f0); }
  .xi-steps { display: flex; gap: 6px; list-style: none; margin: 0; padding: 0; font-size: 0.78rem; flex-wrap: wrap; }
  .xi-steps li { padding: 2px 8px; border-radius: 999px; background: rgba(148, 163, 184, 0.12); color: var(--rumi-text-muted, #94a3b8); }
  .xi-steps li.current { background: rgba(59, 130, 246, 0.25); color: #bfdbfe; }
  .xi-steps li.done { background: rgba(20, 184, 166, 0.2); color: #5eead4; }
  .xi-row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .xi-primary { padding: 7px 14px; border-radius: 10px; border: none; background: var(--rumi-accent, #14b8a6); color: #04211d; font-weight: 600; cursor: pointer; align-self: flex-start; }
  .xi-secondary { padding: 6px 12px; border-radius: 10px; border: 1px solid var(--rumi-border, rgba(148, 163, 184, 0.25)); background: rgba(15, 23, 42, 0.45); color: var(--rumi-text-secondary, #cbd5e1); font-weight: 600; cursor: pointer; }
  .xi-link { background: none; border: none; color: var(--rumi-accent, #14b8a6); cursor: pointer; padding: 0; font-size: 0.82rem; align-self: flex-start; }
  .xi-qr { width: 160px; height: 160px; border-radius: 8px; background: #fff; padding: 6px; }
  .xi-prompt { display: flex; flex-direction: column; gap: 6px; }
  .xi-prompt a { color: var(--rumi-accent, #14b8a6); }
  .xi-primary:disabled, .xi-secondary:disabled { opacity: 0.5; cursor: not-allowed; }
</style>
