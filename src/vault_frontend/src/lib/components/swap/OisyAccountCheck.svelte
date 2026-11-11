<script lang="ts">
  import { onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { walletStore } from '../../stores/wallet';
  import { currentWalletType, walletSessionGeneration } from '../../services/auth';
  import { inspectOisySwapAccount } from '../../services/swapOisySigner';

  let mounted = true;
  let ticket = 0;
  let busy = false;
  let message = '';
  let result: Awaited<ReturnType<typeof inspectOisySwapAccount>> | null = null;
  let observedContext = '';
  $: context = `${$currentWalletType}:${$walletSessionGeneration}:${$walletStore.isConnected}:${$walletStore.loading}:${$walletStore.principal?.toText() ?? ''}`;
  $: if (context !== observedContext) {
    observedContext = context;
    ticket++;
    result = null;
    message = '';
    busy = false;
  }
  onDestroy(() => { mounted = false; ticket++; });

  async function check() {
    if (busy) return;
    const wallet = get(walletStore);
    if (!wallet.isConnected || wallet.loading || !wallet.principal || get(currentWalletType) !== 'oisy') return;
    const owner = wallet.principal, ownerText = owner.toText();
    const generation = get(walletSessionGeneration), run = ++ticket;
    const current = () => {
      const live = get(walletStore);
      return mounted && ticket === run && get(currentWalletType) === 'oisy'
        && get(walletSessionGeneration) === generation && live.isConnected && !live.loading
        && live.principal?.toText() === ownerText;
    };
    const assertCurrent = () => { if (!current()) throw Error('Wallet session changed; this account check was discarded.'); };
    busy = true; result = null; message = '';
    try {
      // The helper opens synchronously in this click and makes only an accounts request.
      const report = await inspectOisySwapAccount(owner, assertCurrent);
      assertCurrent();
      result = report;
    } catch (error) {
      if (current()) message = error instanceof Error ? error.message : 'Account check unavailable.';
    } finally {
      if (current()) busy = false;
    }
  }
</script>

{#if $currentWalletType === 'oisy' && $walletStore.isConnected}
  <section class="account-check" aria-label="OISY signing account check">
    <button type="button" on:click={check} disabled={busy || $walletStore.loading}>
      {busy ? 'Checking signing account…' : 'Check signing account (read only)'}
    </button>
    <p>Opens OISY to read its account identity. This check does not approve tokens or submit a swap.</p>
    {#if result}
      <div role="status">
        <p>{result.matches ? 'The signing account matches this Rumi connection.' : 'The signing account does not match this Rumi connection.'}</p>
        <p>Connected Rumi account: <code>{result.connectedOwner}</code></p>
        <p>OISY signing account{result.signerOwners.length === 1 ? '' : 's'}: <code>{result.signerOwners.join(', ') || 'None returned'}</code></p>
        <p>This does not verify earlier approvals or swaps. No account was changed.</p>
      </div>
    {/if}
    {#if message}<p role="alert">{message}</p>{/if}
  </section>
{/if}

<style>
  .account-check { margin-top: 0.75rem; padding: 0.85rem; border: 1px solid #475569; border-radius: 0.5rem; font-size: 0.8rem; line-height: 1.5; }
  button { padding: 0.5rem 0.75rem; border: 1px solid #64748b; border-radius: 0.4rem; color: inherit; background: transparent; cursor: pointer; }
  button:disabled { opacity: 0.5; cursor: default; }
  p { margin: 0.5rem 0 0; }
  code { display: block; overflow-wrap: anywhere; white-space: normal; }
</style>
