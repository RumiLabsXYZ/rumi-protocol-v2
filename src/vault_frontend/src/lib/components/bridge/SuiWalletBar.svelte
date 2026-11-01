<script lang="ts">
  import type { SuiWalletState } from '$lib/services/suiBridge/walletStandard';
  import type { SuiNetworkInfo } from '$lib/services/suiBridge/networks';
  import { shortAddress } from '$lib/services/baseBridge/status';

  interface Props {
    wallet: SuiWalletState;
    network: SuiNetworkInfo;
    busy?: boolean;
    onConnect: (walletName?: string) => void;
    onDisconnect: () => void;
  }

  let { wallet, network, busy = false, onConnect, onDisconnect }: Props = $props();

  const connected = $derived(wallet.status === 'connected' && !!wallet.address);
  /** A Wallet Standard account lists the chains it supports; this destination's chain must be one of them. */
  const onWrongNetwork = $derived(connected && !wallet.chains.includes(network.chain));
</script>

<div class="wallet-bar" aria-label="Sui wallet">
  <div class="identity">
    <span class="dot" class:online={connected} aria-hidden="true"></span>
    {#if connected}
      <span class="account" title={wallet.address}>{shortAddress(wallet.address ?? '')}</span>
      <span class="network" class:mismatch={onWrongNetwork}>{wallet.walletName}</span>
    {:else if wallet.status === 'unavailable'}
      <span class="account muted">No Sui wallet found</span>
    {:else}
      <span class="account muted">Sui wallet not connected</span>
    {/if}
  </div>
  <div class="buttons">
    {#if connected}
      <button type="button" class="btn-secondary small" onclick={onDisconnect} disabled={busy}>Disconnect</button>
    {:else if wallet.wallets.length > 1}
      {#each wallet.wallets as option (option.name)}
        <button type="button" class="btn-secondary small" onclick={() => onConnect(option.name)} disabled={busy || wallet.status === 'connecting'}>
          Connect {option.name}
        </button>
      {/each}
    {:else if wallet.status !== 'unavailable'}
      <button type="button" class="btn-secondary small" onclick={() => onConnect()} disabled={busy || wallet.status === 'connecting'}>
        {wallet.status === 'connecting' ? 'Connecting...' : 'Connect Sui wallet'}
      </button>
    {/if}
  </div>
</div>
{#if onWrongNetwork}
  <p class="wallet-error" role="alert">This wallet account is not on {network.name}. Switch the network in your wallet, then connect again.</p>
{/if}
{#if wallet.error}<p class="wallet-error" role="alert">{wallet.error}</p>{/if}

<style>
  .wallet-bar { display:flex; flex-wrap:wrap; justify-content:space-between; align-items:center; gap:0.6rem; padding:0.55rem 0.75rem; border:1px solid var(--rumi-border); border-radius:0.5rem; background:var(--rumi-bg-primary); }
  .identity { display:flex; align-items:center; gap:0.5rem; min-width:0; font-size:0.8rem; }
  .dot { width:0.5rem; height:0.5rem; flex:0 0 0.5rem; border-radius:50%; background:var(--rumi-text-muted); }
  .dot.online { background:var(--rumi-action); }
  .account { color:var(--rumi-text-primary); font-variant-numeric:tabular-nums; }
  .account.muted { color:var(--rumi-text-secondary); }
  .network { padding:0.1rem 0.5rem; border:1px solid var(--rumi-border-hover); border-radius:999px; font-size:0.7rem; color:var(--rumi-text-secondary); }
  .network.mismatch { border-color:rgba(229,168,90,0.5); color:#ffd9a8; }
  .buttons { display:flex; flex-wrap:wrap; gap:0.4rem; }
  .small { padding:0.4rem 0.8rem; font-size:0.75rem; }
  .wallet-error { margin:0.5rem 0 0; font-size:0.75rem; color:#ffb3c5; }
</style>
