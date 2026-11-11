<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { currentWalletType, walletSessionGeneration, WALLET_TYPES } from '$lib/services/auth';
  import { openOisySignerChannel } from '$lib/services/oisySigner';
  import { ApiClient } from '$lib/services/protocol/apiClient';
  import { VAULT_PULL_STATUS_EVENT, sameVaultPullRequest, type VaultPullRequest, type VaultPullRecoverySnapshot } from '$lib/services/protocol/vaultOperationJournal';
  import { walletStore } from '$lib/stores/wallet';
  import { vaultStore } from '$lib/stores/vaultStore';
  import { collateralStore } from '$lib/stores/collateralStore';
  import { savedVaultRequestActions, savedVaultRequestSummary } from '$lib/services/protocol/vaultPullRecoveryDetails';
  import { toastStore } from '$lib/stores/toast';
  import { isCurrentVaultPullStatusRead } from '$lib/services/protocol/vaultPullRecoveryViewState';

  import { VAULT_PULL_REVIEW_EVENT } from '$lib/services/protocol/vaultPullRecoveryNavigation';

  let recoveryRegion: HTMLDivElement;
  let reviewRequested = false;
  let reviewEpoch = 0;
  let submitted: { operationId: bigint; request: VaultPullRequest; confirmedWideBlockIndex: bigint | null } | null = null;
  let recovery: VaultPullRecoverySnapshot | null = null;
  let restored = false;
  let confirmingCancel = false;
  let actionEpoch = 0;
  let actionNotice = '';
  let blockIndex = '';
  let busy = false;
  let statusReadError = '';
  let checkedOwner = '';
  let checkedWalletType: string | null = null;
  let checkedSession = -1;
  let manualCheckPending = false;
  $: requiresUserCheck = $currentWalletType === WALLET_TYPES.OISY;
  let generation = 0;
  $: owner = $walletStore.isConnected && !$walletStore.loading ? ($walletStore.principal?.toString() ?? '') : '';
  $: if (owner && (owner !== checkedOwner || $currentWalletType !== checkedWalletType || $walletSessionGeneration !== checkedSession)) {
    generation++;
    checkedOwner = owner;
    checkedWalletType = $currentWalletType;
    checkedSession = $walletSessionGeneration;
    actionEpoch++;
    reviewEpoch++;
    reviewRequested = false;
    recovery = null;
    restored = false;
    confirmingCancel = false;
    actionNotice = '';
    busy = false;
    submitted = null;
    statusReadError = '';
    manualCheckPending = requiresUserCheck;
    if (!requiresUserCheck) refresh();
  }
  $: if (!owner) {
    if (checkedOwner) generation++;
    checkedOwner = '';
    checkedWalletType = null;
    checkedSession = -1;
    actionEpoch++;
    reviewEpoch++;
    reviewRequested = false;
    recovery = null;
    restored = false;
    confirmingCancel = false;
    actionNotice = '';
    busy = false;
    manualCheckPending = false;
    submitted = null;
    statusReadError = '';
  }

  async function onReviewRequest(event: Event) {
    if (!(event instanceof CustomEvent) || !owner || event.detail?.owner !== owner) return;
    const ownerAtClick = owner;
    const walletAtClick = $currentWalletType;
    const sessionAtClick = $walletSessionGeneration;
    const review = busy ? reviewEpoch : ++reviewEpoch;
    const current = () => review === reviewEpoch && owner === ownerAtClick
      && $currentWalletType === walletAtClick && $walletSessionGeneration === sessionAtClick;
    reviewRequested = true;
    const alreadyBusy = busy;
    // Calling before any await preserves OISY's user-click popup boundary.
    const reading = requestRefresh();
    await tick();
    if (current()) {
      recoveryRegion?.scrollIntoView?.({block: 'start', behavior: 'smooth'});
      recoveryRegion?.focus({preventScroll: true});
    }
    await reading;
    if (!current() || alreadyBusy || statusReadError || recovery?.intent) return;
    actionNotice = recovery?.status.active[0]
      ? 'A vault operation is pending, but its saved request is not available in this browser. Keep the current browser data and contact support before another attempt.'
      : 'No saved request is pending in this wallet. Refresh your vaults and balance before creating another request.';
  }

  async function requestRefresh() {
    if (busy) return;
    const ownerAtClick = owner;
    const walletTypeAtClick = $currentWalletType;
    const generationAtClick = generation;
    const sessionAtClick = $walletSessionGeneration;
    const action = ++actionEpoch;
    busy = true;
    try {
      // Open directly from this click. Oisy signs authenticated queries too.
      if (requiresUserCheck && $walletStore.principal) {
        await openOisySignerChannel($walletStore.principal);
      }
      if (owner !== ownerAtClick || $currentWalletType !== walletTypeAtClick || generation !== generationAtClick || $walletSessionGeneration !== sessionAtClick) return;
      await refresh();
    } catch (error) {
      if (owner === ownerAtClick && $currentWalletType === walletTypeAtClick && generation === generationAtClick && $walletSessionGeneration === sessionAtClick) {
        statusReadError = error instanceof Error ? error.message : 'Could not open the wallet status check.';
      }
    } finally {
      if (action === actionEpoch) busy = false;
    }
  }

  async function refresh() {
    const current = ++generation;
    const ownerAtStart = owner;
    try {
      const result = await ApiClient.getVaultPullRecovery();
      if (isCurrentVaultPullStatusRead(current, generation, ownerAtStart, owner)) {
        const previousIntent = recovery?.intent;
        const incomingIntent = result?.intent;
        const previousActive = recovery?.status.active[0];
        const incomingActive = result?.status.active[0];
        if ((incomingIntent && (!previousIntent || incomingIntent.owner !== previousIntent.owner
          || incomingIntent.operationId !== previousIntent.operationId || incomingIntent.savedAt !== previousIntent.savedAt
          || !sameVaultPullRequest(incomingIntent.request, previousIntent.request)))
          || (incomingActive && (!previousActive || incomingActive.operation_id !== previousActive.operation_id
          || !sameVaultPullRequest(incomingActive.request, previousActive.request)))) {
          actionNotice = '';
        }
        recovery = result;
        const active = result?.status.active[0];
        const local = result?.intent;
        const same = !local || (!!active && active.operation_id.toString() === local.operationId && sameVaultPullRequest(active.request, local.request));
        submitted = active && 'Submitted' in active.phase && same ? {
          operationId: active.operation_id, request: active.request,
          confirmedWideBlockIndex: active.confirmed_wide_block_index?.[0] ?? null,
        } : null;
        restored = false;
        confirmingCancel = false;
        manualCheckPending = false;
        statusReadError = '';
      }
    } catch (error) {
      if (isCurrentVaultPullStatusRead(current, generation, ownerAtStart, owner)) statusReadError = error instanceof Error ? error.message : 'Could not refresh vault recovery status.';
      console.warn('Could not read saved vault pull recovery status:', error);
    }
  }

  function onRecoverySignal(event?: Event) {
    if (
      event instanceof StorageEvent &&
      event.key !== `rumi:vault-pull-operation:v1:${owner}`
    ) return;
    const signalOwner = event instanceof CustomEvent
      ? event.detail?.owner
      : event instanceof MessageEvent
        ? event.data?.owner
        : undefined;
    if (!signalOwner || signalOwner === owner) {
      if (busy) { manualCheckPending = true; return; }
      if (requiresUserCheck) manualCheckPending = true;
      else refresh();
    }
  }

  $: savedIntent = recovery?.intent ?? null;
  $: actions = recovery ? savedVaultRequestActions(recovery) : null;
  $: targetVaultId = savedIntent && 'AddMargin' in savedIntent.request ? savedIntent.request.AddMargin.vault_id : null;
  $: collateralType = savedIntent && ('OpenVault' in savedIntent.request
    ? savedIntent.request.OpenVault.collateral_type
    : 'OpenVaultAndBorrow' in savedIntent.request ? savedIntent.request.OpenVaultAndBorrow.collateral_type
    : $vaultStore.vaults?.find(v => BigInt(v.vaultId) === targetVaultId)?.collateralType);
  $: displayCollateral = collateralType ? ($collateralStore.collaterals.find(info => info.principal === collateralType) ?? collateralStore.getCollateralInfo(collateralType)) : undefined;
  $: summary = savedIntent ? savedVaultRequestSummary(savedIntent.request, displayCollateral) : '';

  async function savedAction(kind: 'resume' | 'cancel') {
    if (!savedIntent || busy || manualCheckPending || statusReadError) return;
    const exact = savedIntent;
    const action = ++actionEpoch;
    const ownerAtClick = owner;
    const walletTypeAtClick = $currentWalletType;
    const sessionAtClick = $walletSessionGeneration;
    const current = () => action === actionEpoch && ownerAtClick === owner
      && walletTypeAtClick === $currentWalletType && sessionAtClick === $walletSessionGeneration;
    busy = true;
    actionNotice = '';
    try {
      if (requiresUserCheck && $walletStore.principal) await openOisySignerChannel($walletStore.principal);
      if (!current()) return;
      if (kind === 'resume') {
        const result = await ApiClient.resumeSavedVaultCollateralPull(exact);
        if (!current()) return;
        if (!result.success) throw new Error(result.error || 'The saved request remains held.');
        actionNotice = result.message || 'Saved request completed. Refresh your vaults and balance before starting another request.';
      } else {
        await ApiClient.cancelSavedVaultCollateralPull(exact);
        if (!current()) return;
        actionNotice = 'The request is retired. It may have been resolved elsewhere; refresh your vaults and balance. This does not reverse an approval or transfer.';
      }
      await refresh();
      if (!current()) return;
      await vaultStore.refreshVaults();
      if (current()) walletStore.refreshBalance({ skipCache: true });
    } catch (error) {
      if (current()) statusReadError = error instanceof Error ? error.message : 'The saved request remains held. Refresh status before another action.';
    } finally {
      if (current()) busy = false;
    }
  }

  function requestDetails(request: VaultPullRequest): string {
    if ('OpenVault' in request) {
      return `Open vault · ${request.OpenVault.amount_e8s.toString()} token base units · collateral ledger ${request.OpenVault.collateral_type}`;
    }
    if ('OpenVaultAndBorrow' in request) {
      return `Open and borrow · ${request.OpenVaultAndBorrow.amount_e8s.toString()} collateral base units · ledger ${request.OpenVaultAndBorrow.collateral_type}`;
    }
    return `Add margin · vault ${request.AddMargin.vault_id.toString()} · ${request.AddMargin.amount_e8s.toString()} collateral base units`;
  }

  onMount(() => {
    window.addEventListener(VAULT_PULL_STATUS_EVENT, onRecoverySignal);
    window.addEventListener(VAULT_PULL_REVIEW_EVENT, onReviewRequest);
    window.addEventListener('storage', onRecoverySignal);
    const channel = typeof BroadcastChannel !== 'undefined'
      ? new BroadcastChannel(VAULT_PULL_STATUS_EVENT)
      : null;
    channel?.addEventListener('message', onRecoverySignal);
    return () => {
      window.removeEventListener(VAULT_PULL_STATUS_EVENT, onRecoverySignal);
      window.removeEventListener(VAULT_PULL_REVIEW_EVENT, onReviewRequest);
      window.removeEventListener('storage', onRecoverySignal);
      channel?.removeEventListener('message', onRecoverySignal);
      channel?.close();
    };
  });

  async function recover() {
    if (submitted && submitted.confirmedWideBlockIndex !== null) {
      toastStore.error('This transfer is already confirmed and held. Do not submit another block or retry it.', 8000);
      return;
    }
    const exact = blockIndex.trim();
    if (!/^\d+$/.test(exact)) {
      toastStore.error('Enter the exact nonnegative ledger-global block index for this transfer.', 8000);
      return;
    }
    busy = true;
    try {
      const result = await ApiClient.recoverSubmittedVaultCollateralPull(BigInt(exact));
      if (!result.success) {
        toastStore.error(result.error || 'Vault operation remains held.', 8000);
        return;
      }
      toastStore.success(`Recovered vault operation ${submitted?.operationId.toString() ?? ''}.`, 8000);
      blockIndex = '';
      await vaultStore.refreshVaults();
      walletStore.refreshBalance({ skipCache: true });
    } catch (error) {
      toastStore.error(error instanceof Error ? error.message : 'Vault operation remains held.', 8000);
    } finally {
      busy = false;
      await refresh();
    }
  }
</script>

<div id="vault-pull-recovery" class="recovery-region" bind:this={recoveryRegion} tabindex="-1" role="region" aria-label="Saved vault request recovery">
{#if owner && reviewRequested && busy && !recovery}
  <section class="recovery" role="status"><p>Checking your saved request…</p></section>
{:else if owner && submitted}
  <section class="recovery" aria-label="Recover submitted vault collateral pull">
    {#if submitted.confirmedWideBlockIndex !== null}
    <p>
      Vault operation {submitted.operationId.toString()} has a confirmed collateral transfer at ledger block {submitted.confirmedWideBlockIndex.toString()}.
      Its index is beyond the range supported by vault recovery, so the operation is held. Do not retry the transfer or enter another block. Contact Rumi support with the operation ID and this exact ledger block.
    </p>
    <p class="operation-details">{requestDetails(submitted.request)}</p>
    {#if statusReadError}<p class="status-error" role="status">Status refresh failed: {statusReadError} The last confirmed receipt shown above may be stale.</p>{/if}
    <button on:click={requestRefresh} disabled={busy}>Refresh status</button>
    {:else}
    <p>
      Vault operation {submitted.operationId.toString()} has an unknown collateral transfer result.
      Enter the exact ledger-global block index shown for its matching transfer. A block that does not prove this transfer leaves the operation held.
    </p>
    <p class="operation-details">{requestDetails(submitted.request)}</p>
    {#if statusReadError}<p class="status-error" role="status">Status refresh failed: {statusReadError} The saved recovery action remains available.</p>{/if}
    <label for="vault-pull-recovery-block">Collateral ledger block index</label>
    <input
      id="vault-pull-recovery-block"
      type="text"
      inputmode="numeric"
      autocomplete="off"
      pattern="[0-9]+"
      bind:value={blockIndex}
      disabled={busy}
      placeholder="Exact block index"
    />
    <button on:click={recover} disabled={busy || !/^\d+$/.test(blockIndex.trim())}>
      {busy ? 'Checking exact block…' : 'Verify block and resume'}
    </button>
    <button on:click={requestRefresh} disabled={busy}>Refresh status</button>
    {/if}
  </section>
{:else if owner && savedIntent && actions}
  <section class="recovery saved-request" aria-label="Saved vault request">
    <p><strong>{summary}</strong></p>
    <p>{actions.message}</p>
    <details>
      <summary>Advanced recovery details</summary>
      <p>Operation {savedIntent.operationId}. {requestDetails(savedIntent.request)}</p>
      {#if 'OpenVaultAndBorrow' in savedIntent.request}<p>Borrow amount: {savedIntent.request.OpenVaultAndBorrow.borrow_amount_e8s.toString()} icUSD base units.</p>{/if}
    </details>
    {#if actionNotice}<p role="status">{actionNotice}</p>{/if}
    {#if statusReadError}<p class="status-error" role="status">{statusReadError} The saved request remains protected. Refresh status before continuing.</p>{/if}
    {#if manualCheckPending}<p>Check the current wallet status before continuing.</p>{/if}
    {#if confirmingCancel && actions.canCancel}
      <p>Ask the backend to retire this exact saved request? If a transfer has started, cancellation will be refused. This does not revoke an existing token allowance.</p>
      <button on:click={() => savedAction('cancel')} disabled={busy || manualCheckPending || !!statusReadError}>Confirm cancellation</button>
      <button on:click={() => { confirmingCancel = false; }} disabled={busy}>Keep saved request</button>
    {:else}
      {#if actions.canRestore}
        {#if restored}
          <p>Review the saved amounts shown here. Continue uses this exact request, including its original collateral and borrow amounts; edits in the borrow form do not change it. Continuing may ask for token approval and submit this request.</p>
          <button on:click={() => savedAction('resume')} disabled={busy || manualCheckPending || !!statusReadError}>Continue saved request</button>
        {:else}
          <button on:click={() => { restored = true; }} disabled={busy || manualCheckPending || !!statusReadError}>Restore saved request</button>
        {/if}
      {/if}
      {#if actions.canCancel}<button on:click={() => { confirmingCancel = true; restored = false; }} disabled={busy || manualCheckPending || !!statusReadError}>Cancel saved request</button>{/if}
      {#if actions.retired}<button on:click={() => savedAction('cancel')} disabled={busy || manualCheckPending || !!statusReadError}>Clear retired request</button>{/if}
    {/if}
    <button on:click={requestRefresh} disabled={busy}>Refresh status</button>
  </section>
{:else if owner && actionNotice}
  <section class="recovery" role="status"><p>{actionNotice}</p><button on:click={requestRefresh} disabled={busy}>Refresh status</button></section>
{:else if owner && manualCheckPending && !statusReadError}
  <section class="recovery" aria-label="Check pending vault action">
    <p>Need to resume a vault action? Checking its status will ask for confirmation in Oisy.</p>
    <button on:click={requestRefresh} disabled={busy}>Check pending vault action</button>
  </section>
{:else if owner && statusReadError}
  <section class="recovery recovery-error" role="status">
    <p>Could not check for a submitted vault collateral pull: {statusReadError}</p>
    <button on:click={requestRefresh} disabled={busy}>Retry status check</button>
  </section>
{/if}

</div>
<style>
  .recovery-region { scroll-margin-top: calc(4.75rem + var(--rumi-xrp-recovery-height, 0px) + var(--rumi-strip-height, 0px)); }
  .recovery-region:focus { outline: 2px solid #2dd4bf; outline-offset: 3px; border-radius: .75rem; }

  .recovery {
    min-width: 0;
    max-width: 70rem;
    margin: 0.75rem auto;
    padding: 1rem;
    display: grid;
    grid-template-columns: minmax(12rem, 1fr) minmax(12rem, 0.8fr) auto auto;
    align-items: end;
    gap: 0.6rem;
    border: 1px solid rgba(45, 212, 191, 0.25);
    border-radius: 0.75rem;
    background: rgba(45, 212, 191, 0.05);
  }
  details { grid-column: 1 / -1; overflow-wrap: anywhere; color: var(--rumi-text-muted); font-size: .75rem; }
  .saved-request { grid-template-columns: repeat(3, minmax(0, 1fr)); }
  .recovery > * { min-width: 0; }
  p { grid-column: 1 / -1; margin: 0; color: var(--rumi-text-muted); font-size: 0.8125rem; overflow-wrap: anywhere; }
  .status-error { color: #fbbf24; }
  label { color: var(--rumi-text-muted); font-size: 0.75rem; }
  input {
    min-width: 0;
    height: 2.5rem;
    padding: 0 0.75rem;
    border: 1px solid var(--rumi-border);
    border-radius: 0.5rem;
    background: var(--rumi-bg-surface1);
    color: var(--rumi-text-primary);
    font-variant-numeric: tabular-nums;
  }
  button {
    min-height: 2.5rem;
    padding: 0 0.8rem;
    border: 1px solid rgba(45, 212, 191, 0.4);
    border-radius: 0.5rem;
    background: rgba(45, 212, 191, 0.12);
    color: var(--rumi-text-primary);
    cursor: pointer;
    white-space: nowrap;
  }
  button:disabled { cursor: not-allowed; opacity: 0.5; }
  @media (max-width: 700px) {
    .recovery { grid-template-columns: repeat(2, minmax(0, 1fr)); margin-right: 1rem; margin-left: 1rem; }
    .saved-request { grid-template-columns: minmax(0, 1fr); }
    button { width: 100%; white-space: normal; overflow-wrap: anywhere; }
    input { grid-column: 1 / -1; }
  }
</style>
