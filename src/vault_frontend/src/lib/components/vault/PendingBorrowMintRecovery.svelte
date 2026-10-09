<script lang="ts">
  import { get } from 'svelte/store';
  import { walletStore } from '$lib/stores/wallet';
  import { currentWalletType, walletSessionGeneration } from '$lib/services/auth';
  import { protocolService } from '$lib/services/protocol';
  import type { ActionBoundContext } from '$lib/services/protocol';

  export let principalText: string | null = null;
  export let refreshToken = 0;
  export let onResolved: (() => void) | undefined = undefined;

  type PendingBorrow = Awaited<ReturnType<typeof protocolService.getMyPendingBorrowMintsBound>>[number];
  let pending: PendingBorrow[] = [];
  let loadedPrincipal: string | null = null;
  let loadedRefreshToken = -1;
  let busyVaultId: bigint | null = null;
  let error = '';
  let notice = '';
  let candidateBlockIndices: Record<string, string> = {};

  $: if (principalText !== loadedPrincipal || refreshToken !== loadedRefreshToken) void refresh();

  async function refresh() {
    const capturedPrincipal = principalText;
    const capturedRefreshToken = refreshToken;
    const walletAtStart = get(walletStore);
    const capturedGeneration = get(walletSessionGeneration);
    const capturedWalletType = get(currentWalletType);
    loadedPrincipal = capturedPrincipal;
    loadedRefreshToken = capturedRefreshToken;
    pending = [];
    error = '';
    if (!capturedPrincipal || !walletAtStart.isConnected || walletAtStart.principal?.toText() !== capturedPrincipal) return;
    const ctx: ActionBoundContext = {
      expectedPrincipalText: capturedPrincipal,
      assertCurrent: () => principalText === capturedPrincipal
        && refreshToken === capturedRefreshToken
        && get(walletStore).isConnected
        && get(walletStore).principal?.toText() === capturedPrincipal
        && get(walletSessionGeneration) === capturedGeneration
        && get(currentWalletType) === capturedWalletType,
    };
    try {
      const result = await protocolService.getMyPendingBorrowMintsBound(ctx);
      if (!ctx.assertCurrent()) return;
      pending = result;
    } catch (cause) {
      if (ctx.assertCurrent()) {
        error = cause instanceof Error ? cause.message : 'Could not check for a pending borrow.';
      }
    }
  }

  function formatAmount(raw: bigint): string {
    const whole = raw / 100_000_000n;
    const fraction = (raw % 100_000_000n).toString().padStart(8, '0').replace(/0+$/, '');
    return fraction ? `${whole}.${fraction}` : whole.toString();
  }

  async function retryExactBorrow(row: PendingBorrow) {
    const capturedPrincipal = principalText;
    const walletAtStart = get(walletStore);
    const capturedGeneration = get(walletSessionGeneration);
    const capturedWalletType = get(currentWalletType);
    if (!capturedPrincipal || !walletAtStart.isConnected || walletAtStart.principal?.toText() !== capturedPrincipal || busyVaultId !== null) return;
    busyVaultId = row.vault_id;
    error = '';
    notice = '';
    const ctx: ActionBoundContext = {
      expectedPrincipalText: capturedPrincipal,
      assertCurrent: () => principalText === capturedPrincipal
        && get(walletStore).isConnected
        && get(walletStore).principal?.toText() === capturedPrincipal
        && get(walletSessionGeneration) === capturedGeneration
        && get(currentWalletType) === capturedWalletType,
    };
    try {
      // This uses the original raw amount from the backend journal. No current
      // calculator, CR, cap, or edited form value participates in recovery.
      const result = await protocolService.retryPendingBorrowMintBound(ctx, row.vault_id, row.borrowed_amount_e8s);
      if (!ctx.assertCurrent()) return;
      if (result.kind === 'dispatched_ok') {
        notice = `Borrow on vault #${row.vault_id} is recorded. Refreshing its status…`;
        onResolved?.();
      } else {
        error = result.errorMessage ?? 'The exact retry did not resolve. The original borrow remains available below.';
      }
      await refresh();
    } catch (cause) {
      error = cause instanceof Error ? cause.message : 'The exact retry did not resolve.';
      await refresh();
    } finally {
      busyVaultId = null;
    }
  }

  async function reconcileCandidateReceipt(row: PendingBorrow) {
    const capturedPrincipal = principalText;
    const walletAtStart = get(walletStore);
    const capturedGeneration = get(walletSessionGeneration);
    const capturedWalletType = get(currentWalletType);
    const key = row.vault_id.toString();
    const rawBlockIndex = candidateBlockIndices[key] ?? '';
    if (!/^[0-9]+$/.test(rawBlockIndex)) {
      error = 'Enter the nonnegative icUSD ledger block index that contains this exact mint.';
      return;
    }
    if (!capturedPrincipal || !walletAtStart.isConnected || walletAtStart.principal?.toText() !== capturedPrincipal || busyVaultId !== null) return;
    busyVaultId = row.vault_id;
    error = '';
    notice = '';
    const ctx: ActionBoundContext = {
      expectedPrincipalText: capturedPrincipal,
      assertCurrent: () => principalText === capturedPrincipal
        && get(walletStore).isConnected
        && get(walletStore).principal?.toText() === capturedPrincipal
        && get(walletSessionGeneration) === capturedGeneration
        && get(currentWalletType) === capturedWalletType,
    };
    try {
      const result = await protocolService.reconcilePendingBorrowMintFromBlockBound(
        ctx,
        row.vault_id,
        BigInt(rawBlockIndex),
        row.borrowed_amount_e8s
      );
      if (!ctx.assertCurrent()) return;
      if (result.kind === 'dispatched_ok') {
        onResolved?.();
        notice = `The exact mint receipt was verified and debt for vault #${row.vault_id} was recorded.`;
      } else {
        error = result.errorMessage ?? 'The candidate block did not resolve the pending borrow.';
      }
      await refresh();
      if (result.kind !== 'dispatched_ok') error = result.errorMessage ?? 'The candidate block did not resolve the pending borrow.';
    } catch (cause) {
      await refresh();
      error = cause instanceof Error ? cause.message : 'Receipt verification failed; the journal remains held.';
    } finally {
      busyVaultId = null;
    }
  }
</script>

{#if principalText && (pending.length > 0 || error || notice)}
  <section class="pending-borrow" aria-live="polite">
    <h3>Pending borrow recovery</h3>
    {#if notice}<p class="success">{notice}</p>{/if}
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#each pending as row (row.vault_id)}
      <div class="pending-row">
        <p>
          Vault #{row.vault_id} has an unresolved borrow of <strong>{formatAmount(row.borrowed_amount_e8s)} icUSD</strong>.
          {#if 'MintConfirmedHeld' in row.phase}
            The ledger confirmed this mint at block {row.phase.MintConfirmedHeld.block_index}; retrying records its debt without minting again.
          {:else if 'ReceiptRecoveryRequired' in row.phase}
            The retry is TooOld and may have minted earlier. No further mint will be sent; enter the exact matching icUSD ledger mint block index to verify it.
          {:else}
            Its ledger outcome is unknown. Retrying uses the same ledger operation, so do not start another borrow.
          {/if}
        </p>
        {#if 'ReceiptRecoveryRequired' in row.phase}
          <label>
            <span>icUSD ledger block index</span>
            <input type="text" inputmode="numeric" autocomplete="off" bind:value={candidateBlockIndices[row.vault_id.toString()]} />
          </label>
          <button type="button" disabled={busyVaultId !== null} on:click={() => reconcileCandidateReceipt(row)}>
            {busyVaultId === row.vault_id ? 'Verifying receipt…' : 'Verify mint receipt and record debt'}
          </button>
        {:else}
          <button type="button" disabled={busyVaultId !== null} on:click={() => retryExactBorrow(row)}>
            {busyVaultId === row.vault_id
              ? 'Reconciling…'
              : ('MintConfirmedHeld' in row.phase
                ? `Retry debt recording for vault #${row.vault_id}`
                : `Retry exact ${formatAmount(row.borrowed_amount_e8s)} icUSD borrow`)}
          </button>
        {/if}
      </div>
    {/each}
  </section>
{/if}

<style>
  .pending-borrow { margin: 0 0 1rem; padding: 1rem; border: 1px solid rgba(214, 164, 66, .45); border-radius: 12px; background: rgba(214, 164, 66, .08); }
  h3 { margin: 0 0 .5rem; font-size: 1rem; }
  p { margin: .35rem 0 .75rem; }
  .pending-row + .pending-row { margin-top: .75rem; padding-top: .75rem; border-top: 1px solid rgba(214, 164, 66, .25); }
  .pending-row label { display: grid; gap: .35rem; max-width: 24rem; margin: 0 0 .65rem; font-size: .85rem; }
  .pending-row input { padding: .6rem .7rem; border: 1px solid rgba(80, 65, 40, .35); border-radius: 7px; background: var(--surface, #fff); color: inherit; font: inherit; }
  button { padding: .65rem .9rem; border: 0; border-radius: 8px; color: #fff; background: #76551b; font: inherit; font-weight: 650; cursor: pointer; }
  button:disabled { opacity: .6; cursor: wait; }
  .error { color: #a52222; }
  .success { color: #237345; }
</style>
