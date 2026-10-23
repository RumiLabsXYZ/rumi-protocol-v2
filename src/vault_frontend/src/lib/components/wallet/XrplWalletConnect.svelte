<script lang="ts">
  import { createEventDispatcher, onMount } from 'svelte';
  import { formatAddress } from '$lib/utils/format';
  import {
    expectedXrplNetwork,
    getXrplWalletAdapter,
    type XrplConnection,
  } from '$lib/services/xrplWalletService';

  const dispatch = createEventDispatcher<{ change: XrplConnection | null }>();
  const adapter = getXrplWalletAdapter();
  const expected = expectedXrplNetwork();

  let connection: XrplConnection | null = adapter.getConnection();
  let busy = false;
  let error = '';
  $: availability = adapter.availability();

  onMount(() => {
    const off = adapter.onChange((c) => {
      connection = c;
      dispatch('change', c);
    });
    if (!connection && availability.available) {
      void adapter.restore?.().catch(() => {});
    }
    return off;
  });

  async function connect() {
    busy = true;
    error = '';
    try {
      connection = await adapter.connect();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Could not connect the XRP wallet.';
    } finally {
      busy = false;
    }
  }

  function disconnect() {
    adapter.disconnect();
    connection = null;
  }
</script>

<div class="xrpl-wallet" data-testid="xrpl-wallet-connect">
  {#if !availability.available}
    <p class="muted">Wallet signing is not available here. Use the address or QR code below to send XRP manually.</p>
  {:else if connection}
    <div class="row">
      <span class="dot ok" aria-hidden="true"></span>
      <span class="addr" title={connection.address}>{formatAddress(connection.address, 8, 6)}</span>
      <span class="net">{connection.network}</span>
      <button type="button" class="link" on:click={disconnect}>Disconnect</button>
    </div>
  {:else}
    <div class="row">
      <button type="button" class="connect" disabled={busy} on:click={connect}>
        {busy ? 'Waiting for Xaman...' : 'Connect XRP wallet (Xaman)'}
      </button>
      <span class="net">{expected}</span>
    </div>
  {/if}
  {#if error}<p class="err">{error}</p>{/if}
</div>

<style>
  .xrpl-wallet { margin: 0.75rem 0; }
  .row { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
  .dot { width: 0.5rem; height: 0.5rem; border-radius: 50%; background: var(--rumi-text-muted); }
  .dot.ok { background: var(--rumi-teal); }
  .addr { font-variant-numeric: tabular-nums; color: var(--rumi-text-primary); }
  .net { font-size: 0.6875rem; letter-spacing: 0.08em; text-transform: uppercase; color: var(--rumi-text-muted); }
  .connect { min-height: 2.25rem; padding: 0 0.875rem; border: 1px solid var(--rumi-border-teal); border-radius: 8px; background: var(--rumi-teal-dim); color: var(--rumi-teal); font-weight: 700; }
  .connect:disabled { opacity: 0.6; cursor: not-allowed; }
  .link { background: none; border: none; color: var(--rumi-teal); font-weight: 700; cursor: pointer; }
  .muted { margin: 0; color: var(--rumi-text-secondary); font-size: 0.8125rem; }
  .err { margin: 0.5rem 0 0; color: var(--rumi-danger); font-size: 0.8125rem; }
</style>
