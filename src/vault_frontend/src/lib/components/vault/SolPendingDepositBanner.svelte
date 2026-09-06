<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { browser } from '$app/environment';
  import { appDataStore } from '$lib/stores/appDataStore';
  import { walletStore } from '$lib/stores/wallet';
  import {
    SOL_PENDING_DEPOSITS_CHANGED,
    SolVaultService,
    type SolPendingDepositView,
    lamportsToSol,
  } from '$lib/services/solVaultService';
  import { toastStore } from '$lib/stores/toast';
  import { formatAddress } from '$lib/utils/format';

  let pending: SolPendingDepositView[] = [];
  let loading = false;
  let confirmingVaultId: number | null = null;
  let lastError = '';
  let lastPrincipal = '';
  let refreshInterval: ReturnType<typeof setInterval> | null = null;
  let recoveryHeight = 0;

  $: connected = $walletStore.isConnected;
  $: principalText = $walletStore.principal?.toText?.() ?? '';
  // Own CSS var, distinct from --rumi-xrp-recovery-height: this banner stacks
  // BELOW the XRP recovery banner (see .sol-recovery-slot's `top` below), and the
  // page-level padding math in +layout.svelte / PositionStrip.svelte must SUM
  // both vars rather than one replacing the other, or content gets covered.
  $: if (browser) {
    document.documentElement.style.setProperty('--rumi-sol-recovery-height', `${recoveryHeight}px`);
  }

  async function refreshPending() {
    if (!connected) {
      pending = [];
      lastError = '';
      return;
    }
    loading = true;
    try {
      pending = await SolVaultService.getMyPendingDeposits();
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
    window.addEventListener(SOL_PENDING_DEPOSITS_CHANGED, refreshPending);
  });

  onDestroy(() => {
    if (refreshInterval) clearInterval(refreshInterval);
    if (browser) window.removeEventListener(SOL_PENDING_DEPOSITS_CHANGED, refreshPending);
    if (browser) document.documentElement.style.setProperty('--rumi-sol-recovery-height', '0px');
  });

  async function confirmDeposit(vaultId: number) {
    confirmingVaultId = vaultId;
    lastError = '';
    try {
      const res = await SolVaultService.confirmSolDeposit(vaultId);
      if (res.success) {
        const credited = res.data ? lamportsToSol(res.data.creditedLamports) : 0;
        toastStore.success(
          credited > 0
            ? `SOL deposit confirmed - credited ${credited} SOL`
            : 'SOL deposit confirmed'
        );
        await refreshPending();
        if ($walletStore.principal) await appDataStore.refreshAll($walletStore.principal);
      } else {
        lastError = res.error ?? 'Deposit not confirmed yet. If the SOL just landed, wait a moment and retry.';
      }
    } finally {
      confirmingVaultId = null;
    }
  }

  function copy(text: string) {
    navigator.clipboard?.writeText(text).then(
      () => toastStore.info('Copied SOL deposit address'),
      () => {}
    );
  }
</script>

<div class="sol-recovery-slot" bind:clientHeight={recoveryHeight}>
  {#if connected && pending.length > 0}
    <section class="sol-recovery" aria-label="Pending SOL deposit recovery">
      <div class="sol-recovery-inner">
        <div class="sol-recovery-copy">
          <span class="sol-recovery-kicker">SOL deposit awaiting confirmation</span>
          <strong>{pending.length === 1 ? 'Finish opening your SOL vault' : `Finish ${pending.length} SOL vault deposits`}</strong>
          <span class="sol-recovery-note">
            Your SOL deposit address is still linked to your wallet. Confirm it here after the SOL arrives.
          </span>
        </div>

        <div class="sol-recovery-list">
          {#each pending as p (p.vaultId)}
            <div class="sol-recovery-item">
              <span class="sol-vault-id">Vault #{p.vaultId}</span>
              <button class="sol-address" title={p.custodyAddress} on:click={() => copy(p.custodyAddress)}>
                {formatAddress(p.custodyAddress, 8, 6)}
                <span>Copy</span>
              </button>
              <button
                class="sol-confirm"
                disabled={loading || confirmingVaultId === p.vaultId}
                on:click={() => confirmDeposit(p.vaultId)}
              >
                {confirmingVaultId === p.vaultId ? 'Checking...' : "I've sent the SOL"}
              </button>
            </div>
          {/each}
        </div>

        {#if lastError}
          <div class="sol-recovery-error">{lastError}</div>
        {/if}
      </div>
    </section>
  {/if}
</div>

<style>
  .sol-recovery-slot {
    /* Stacks BELOW the XRP recovery banner, which owns the row right under the
       fixed top bar (top: 3.5rem). Summing --rumi-xrp-recovery-height here (not
       replacing it) is what keeps this banner from overlapping the XRP one when
       both are present. */
    position: fixed;
    top: calc(3.5rem + var(--rumi-xrp-recovery-height, 0px));
    left: 0;
    right: 0;
    z-index: 99;
  }

  .sol-recovery {
    width: 100%;
    border-bottom: 1px solid rgba(153, 69, 255, 0.22);
    background:
      linear-gradient(90deg, rgba(153, 69, 255, 0.13), rgba(20, 241, 149, 0.08)),
      var(--rumi-bg-surface-1);
  }

  .sol-recovery-inner {
    max-width: 980px;
    margin: 0 auto;
    padding: 0.875rem 1.25rem;
    display: grid;
    grid-template-columns: minmax(220px, 1fr) minmax(320px, auto);
    gap: 1rem;
    align-items: center;
  }

  .sol-recovery-copy {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.18rem;
  }

  .sol-recovery-kicker {
    color: #b48bff;
    font-size: 0.6875rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  .sol-recovery-copy strong {
    color: var(--rumi-text-primary);
    font-size: 0.9375rem;
    line-height: 1.2;
  }

  .sol-recovery-note {
    color: var(--rumi-text-muted);
    font-size: 0.8125rem;
    line-height: 1.35;
  }

  .sol-recovery-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .sol-recovery-item {
    display: grid;
    grid-template-columns: auto minmax(120px, 1fr) auto;
    gap: 0.5rem;
    align-items: center;
    min-width: 0;
  }

  .sol-vault-id {
    color: var(--rumi-text-secondary);
    font-size: 0.75rem;
    font-weight: 700;
    white-space: nowrap;
  }

  .sol-address,
  .sol-confirm {
    min-height: 2.25rem;
    border-radius: 0.5rem;
    font-size: 0.8125rem;
    font-weight: 700;
    cursor: pointer;
  }

  .sol-address {
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

  .sol-address span {
    color: #b48bff;
    font-size: 0.72rem;
  }

  .sol-confirm {
    padding: 0.45rem 0.85rem;
    border: 0;
    background: #9945ff;
    color: #f4f0ff;
    white-space: nowrap;
  }

  .sol-confirm:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .sol-recovery-error {
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
    .sol-recovery-inner {
      grid-template-columns: 1fr;
      padding: 0.875rem 1rem;
    }

    .sol-recovery-item {
      grid-template-columns: 1fr;
    }

    .sol-vault-id {
      white-space: normal;
    }

    .sol-confirm,
    .sol-address {
      width: 100%;
    }
  }
</style>
