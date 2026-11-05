<script lang="ts">
  import { onMount } from 'svelte';
  import { currentWalletType, walletSessionGeneration, WALLET_TYPES } from '$lib/services/auth';
  import { openOisySignerChannel } from '$lib/services/oisySigner';
  import { ApiClient } from '$lib/services/protocol/apiClient';
  import { VAULT_PULL_STATUS_EVENT, type VaultPullRequest } from '$lib/services/protocol/vaultOperationJournal';
  import { walletStore } from '$lib/stores/wallet';
  import { vaultStore } from '$lib/stores/vaultStore';
  import { toastStore } from '$lib/stores/toast';
  import { isCurrentVaultPullStatusRead } from '$lib/services/protocol/vaultPullRecoveryViewState';

  let submitted: { operationId: bigint; request: VaultPullRequest; confirmedWideBlockIndex: bigint | null } | null = null;
  let blockIndex = '';
  let busy = false;
  let statusReadError = '';
  let checkedOwner = '';
  let checkedWalletType: string | null = null;
  let manualCheckPending = false;
  $: requiresUserCheck = $currentWalletType === WALLET_TYPES.OISY;
  let generation = 0;
  $: owner = $walletStore.isConnected && !$walletStore.loading ? ($walletStore.principal?.toString() ?? '') : '';
  $: if (owner && (owner !== checkedOwner || $currentWalletType !== checkedWalletType)) {
    generation++;
    checkedOwner = owner;
    checkedWalletType = $currentWalletType;
    submitted = null;
    statusReadError = '';
    manualCheckPending = requiresUserCheck;
    if (!requiresUserCheck) refresh();
  }
  $: if (!owner) {
    if (checkedOwner) generation++;
    checkedOwner = '';
    checkedWalletType = null;
    manualCheckPending = false;
    submitted = null;
    statusReadError = '';
  }

  async function requestRefresh() {
    if (busy) return;
    const ownerAtClick = owner;
    const walletTypeAtClick = $currentWalletType;
    const generationAtClick = generation;
    const sessionAtClick = $walletSessionGeneration;
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
      busy = false;
    }
  }

  async function refresh() {
    const current = ++generation;
    const ownerAtStart = owner;
    try {
      const result = await ApiClient.getSubmittedVaultCollateralPull();
      if (isCurrentVaultPullStatusRead(current, generation, ownerAtStart, owner)) {
        submitted = result;
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
      if (requiresUserCheck) manualCheckPending = true;
      else refresh();
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
    window.addEventListener('storage', onRecoverySignal);
    const channel = typeof BroadcastChannel !== 'undefined'
      ? new BroadcastChannel(VAULT_PULL_STATUS_EVENT)
      : null;
    channel?.addEventListener('message', onRecoverySignal);
    return () => {
      window.removeEventListener(VAULT_PULL_STATUS_EVENT, onRecoverySignal);
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

{#if owner && submitted}
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

<style>
  .recovery {
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
  p { grid-column: 1 / -1; margin: 0; color: var(--rumi-text-muted); font-size: 0.8125rem; }
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
    .recovery { grid-template-columns: 1fr 1fr; margin-right: 1rem; margin-left: 1rem; }
    input { grid-column: 1 / -1; }
  }
</style>
