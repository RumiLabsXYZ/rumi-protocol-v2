<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { browser } from '$app/environment';
  import { appDataStore } from '$lib/stores/appDataStore';
  import { walletStore } from '$lib/stores/wallet';
  import {
    XRP_PENDING_DEPOSITS_CHANGED,
    XrpVaultService,
    type XrpPendingDepositView,
    dropsToXrp,
  } from '$lib/services/xrpVaultService';
  import { toastStore } from '$lib/stores/toast';
  import { formatAddress } from '$lib/utils/format';

  let pending: XrpPendingDepositView[] = [];
  let loading = false;
  let confirmingVaultId: number | null = null;
  let lastError = '';
  let lastPrincipal = '';
  let refreshInterval: ReturnType<typeof setInterval> | null = null;

  $: connected = $walletStore.isConnected;
  $: principalText = $walletStore.principal?.toText?.() ?? '';

  async function refreshPending() {
    if (!connected) {
      pending = [];
      lastError = '';
      return;
    }
    loading = true;
    try {
      pending = await XrpVaultService.getMyPendingDeposits();
    } finally {
      loading = false;
    }
  }

  $: if (browser && principalText !== lastPrincipal) {
    lastPrincipal = principalText;
    refreshPending();
  }

  onMount(() => {
    refreshPending();
    refreshInterval = setInterval(refreshPending, 20_000);
    window.addEventListener(XRP_PENDING_DEPOSITS_CHANGED, refreshPending);
  });

  onDestroy(() => {
    if (refreshInterval) clearInterval(refreshInterval);
    if (browser) window.removeEventListener(XRP_PENDING_DEPOSITS_CHANGED, refreshPending);
  });

  async function confirmDeposit(vaultId: number) {
    confirmingVaultId = vaultId;
    lastError = '';
    try {
      const res = await XrpVaultService.confirmXrpDeposit(vaultId);
      if (res.success) {
        const credited = res.data ? dropsToXrp(res.data.creditedDrops) : 0;
        toastStore.success(
          credited > 0
            ? `XRP deposit confirmed - credited ${credited} XRP`
            : 'XRP deposit confirmed'
        );
        await refreshPending();
        if ($walletStore.principal) await appDataStore.refreshAll($walletStore.principal);
      } else {
        lastError = res.error ?? 'Deposit not confirmed yet. If the XRP just landed, wait a moment and retry.';
      }
    } finally {
      confirmingVaultId = null;
    }
  }

  function copy(text: string) {
    navigator.clipboard?.writeText(text).then(
      () => toastStore.info('Copied XRP deposit address'),
      () => {}
    );
  }
</script>

{#if connected && pending.length > 0}
  <section class="xrp-recovery" aria-label="Pending XRP deposit recovery">
    <div class="xrp-recovery-inner">
      <div class="xrp-recovery-copy">
        <span class="xrp-recovery-kicker">XRP deposit awaiting confirmation</span>
        <strong>{pending.length === 1 ? 'Finish opening your XRP vault' : `Finish ${pending.length} XRP vault deposits`}</strong>
        <span class="xrp-recovery-note">
          Your XRP deposit address is still linked to your wallet. Confirm it here after the XRP arrives.
        </span>
      </div>

      <div class="xrp-recovery-list">
        {#each pending as p (p.vaultId)}
          <div class="xrp-recovery-item">
            <span class="xrp-vault-id">Vault #{p.vaultId}</span>
            <button class="xrp-address" title={p.custodyAddress} on:click={() => copy(p.custodyAddress)}>
              {formatAddress(p.custodyAddress, 8, 6)}
              <span>Copy</span>
            </button>
            <button
              class="xrp-confirm"
              disabled={loading || confirmingVaultId === p.vaultId}
              on:click={() => confirmDeposit(p.vaultId)}
            >
              {confirmingVaultId === p.vaultId ? 'Checking...' : "I've sent the XRP"}
            </button>
          </div>
        {/each}
      </div>

      {#if lastError}
        <div class="xrp-recovery-error">{lastError}</div>
      {/if}
    </div>
  </section>
{/if}

<style>
  .xrp-recovery {
    width: 100%;
    border-bottom: 1px solid rgba(45, 212, 191, 0.18);
    background:
      linear-gradient(90deg, rgba(45, 212, 191, 0.13), rgba(74, 144, 217, 0.1)),
      var(--rumi-bg-surface-1);
  }

  .xrp-recovery-inner {
    max-width: 980px;
    margin: 0 auto;
    padding: 0.875rem 1.25rem;
    display: grid;
    grid-template-columns: minmax(220px, 1fr) minmax(320px, auto);
    gap: 1rem;
    align-items: center;
  }

  .xrp-recovery-copy {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.18rem;
  }

  .xrp-recovery-kicker {
    color: var(--rumi-teal);
    font-size: 0.6875rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .xrp-recovery-copy strong {
    color: var(--rumi-text-primary);
    font-size: 0.9375rem;
    line-height: 1.2;
  }

  .xrp-recovery-note {
    color: var(--rumi-text-muted);
    font-size: 0.8125rem;
    line-height: 1.35;
  }

  .xrp-recovery-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .xrp-recovery-item {
    display: grid;
    grid-template-columns: auto minmax(120px, 1fr) auto;
    gap: 0.5rem;
    align-items: center;
    min-width: 0;
  }

  .xrp-vault-id {
    color: var(--rumi-text-secondary);
    font-size: 0.75rem;
    font-weight: 700;
    white-space: nowrap;
  }

  .xrp-address,
  .xrp-confirm {
    min-height: 2.25rem;
    border-radius: 0.5rem;
    font-size: 0.8125rem;
    font-weight: 700;
    cursor: pointer;
  }

  .xrp-address {
    min-width: 0;
    display: inline-flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.45rem 0.65rem;
    border: 1px solid rgba(148, 163, 184, 0.22);
    background: rgba(15, 23, 42, 0.5);
    color: var(--rumi-text-primary);
    font-variant-numeric: tabular-nums;
  }

  .xrp-address span {
    color: var(--rumi-teal);
    font-size: 0.72rem;
  }

  .xrp-confirm {
    padding: 0.45rem 0.85rem;
    border: 0;
    background: var(--rumi-action);
    color: #031a17;
    white-space: nowrap;
  }

  .xrp-confirm:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .xrp-recovery-error {
    grid-column: 1 / -1;
    padding: 0.625rem 0.75rem;
    border: 1px solid rgba(224, 107, 159, 0.32);
    border-radius: 0.5rem;
    background: rgba(224, 107, 159, 0.12);
    color: #e881a8;
    font-size: 0.8125rem;
    line-height: 1.35;
  }

  @media (max-width: 760px) {
    .xrp-recovery-inner {
      grid-template-columns: 1fr;
      padding: 0.875rem 1rem;
    }

    .xrp-recovery-item {
      grid-template-columns: 1fr;
    }

    .xrp-vault-id {
      white-space: normal;
    }

    .xrp-confirm,
    .xrp-address {
      width: 100%;
    }
  }
</style>
