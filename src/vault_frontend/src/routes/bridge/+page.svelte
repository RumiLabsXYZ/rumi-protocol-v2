<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { BRIDGE_ENABLED } from '$lib/config';
  import { principal as principalStore } from '$lib/stores/wallet';
  import { loadBridgeConfig } from '$lib/services/baseBridge/bridgeActor';
  import type { BridgeConfigResult } from '$lib/services/baseBridge/chains';
  import { createEvmWalletStore } from '$lib/services/baseBridge/evmWallet';
  import { IntentStore, browserLocalStorage } from '$lib/services/baseBridge/intents';
  import { DEPOSIT_DELIVERY_NOTE, GAS_NOTICE_MESSAGE } from '$lib/services/baseBridge/status';
  import BaseWalletBar from '$lib/components/bridge/BaseWalletBar.svelte';
  import BaseToIcpForm from '$lib/components/bridge/BaseToIcpForm.svelte';
  import BridgeHistory from '$lib/components/bridge/BridgeHistory.svelte';
  import IcpToBaseForm from '$lib/components/bridge/IcpToBaseForm.svelte';

  type Direction = 'icp_to_base' | 'base_to_icp';

  let direction = $state<Direction>('icp_to_base');
  let configResult = $state.raw<BridgeConfigResult | null>(null);
  let configError = $state('');
  let historyVersion = $state(0);
  let store = $state.raw<IntentStore | null>(null);
  let storeNotice = $state('');

  const evm = createEvmWalletStore();
  const wallet = $derived($evm);
  const principal = $derived($principalStore);
  const principalText = $derived(principal && !principal.isAnonymous() ? principal.toText() : null);

  // The recovery journal is scoped to the signed-in principal.
  $effect(() => {
    const text = principalText;
    if (!text) {
      store = null;
      storeNotice = '';
      return;
    }
    const opened = new IntentStore(browserLocalStorage(), text);
    store = opened;
    storeNotice =
      opened.loadStatus === 'newer_version'
        ? 'This device holds bridge records written by a newer version of the app. Reload the page; transfers are paused until then.'
        : opened.loadStatus === 'unavailable'
          ? 'Your browser is blocking local storage, so the bridge cannot save a recovery record. It will not start a transfer without one.'
          : opened.loadStatus === 'recovered'
            ? 'Some saved bridge records on this device were unreadable. A copy was kept and everything readable was restored.'
            : '';
  });

  async function refreshConfig() {
    try {
      configResult = await loadBridgeConfig();
      configError = '';
    } catch (cause) {
      configError = cause instanceof Error ? cause.message : String(cause);
    }
  }

  onMount(() => {
    // Nothing is called until the bridge canister id is configured.
    if (!BRIDGE_ENABLED) return;
    void evm.init();
    void refreshConfig();
    const timer = setInterval(refreshConfig, 60_000);
    return () => clearInterval(timer);
  });

  onDestroy(() => evm.destroy());

  const config = $derived(configResult && configResult.ok ? configResult.config : null);
  const unavailableReason = $derived(
    configResult && !configResult.ok ? configResult.message : configError ? `The bridge could not be reached: ${configError}` : '',
  );

  function bumpHistory() {
    historyVersion += 1;
  }

  const DIRECTIONS: Direction[] = ['icp_to_base', 'base_to_icp'];
  function onTabKeydown(event: KeyboardEvent) {
    if (event.key !== 'ArrowRight' && event.key !== 'ArrowLeft' && event.key !== 'Home' && event.key !== 'End') return;
    event.preventDefault();
    const at = DIRECTIONS.indexOf(direction);
    const next =
      event.key === 'Home' ? 0 : event.key === 'End' ? DIRECTIONS.length - 1 : (at + (event.key === 'ArrowRight' ? 1 : -1) + DIRECTIONS.length) % DIRECTIONS.length;
    direction = DIRECTIONS[next];
    document.getElementById(direction === 'icp_to_base' ? 'tab-icp-to-base' : 'tab-base-to-icp')?.focus();
  }
</script>

<svelte:head>
  <title>Bridge | Rumi</title>
  <meta name="description" content="Move icUSD between the Internet Computer and Base with the Rumi escrow-backed bridge." />
</svelte:head>

<main class="bridge-page">
  <header class="intro">
    <p class="eyebrow">INTERNET COMPUTER ↔ BASE</p>
    <h1 class="page-title">Bridge icUSD</h1>
    <p class="subtitle">
      Move icUSD between the Internet Computer and Base. Every icUSD on Base is backed one to one by icUSD held in the bridge's escrow on the Internet Computer. Bridging does not borrow or create debt.
    </p>
  </header>

  {#if !BRIDGE_ENABLED}
    <section class="glass-card unavailable" aria-live="polite">
      <h2>Bridge is not available yet</h2>
      <p>The icUSD bridge between the Internet Computer and Base has not launched. Nothing here is active and no requests are made.</p>
    </section>
  {:else if !config}
    <section class="glass-card unavailable" aria-live="polite">
      {#if unavailableReason}
        <h2>Bridge is not available yet</h2>
        <p>{unavailableReason}</p>
        <button type="button" class="btn-secondary" onclick={refreshConfig}>Try again</button>
      {:else}
        <p>Loading bridge status...</p>
      {/if}
    </section>
  {:else}
    {#if config.minterAwaitingGas && direction === 'icp_to_base'}
      <p class="banner warn" role="status" data-testid="gas-notice">{GAS_NOTICE_MESSAGE}</p>
    {/if}
    <div class="layout">
      <section class="glass-card main-card">
        <div class="tabs" role="tablist" aria-label="Bridge direction" tabindex="-1" onkeydown={onTabKeydown}>
          <button
            type="button"
            role="tab"
            id="tab-icp-to-base"
            aria-selected={direction === 'icp_to_base'}
            aria-controls="panel-icp-to-base"
            tabindex={direction === 'icp_to_base' ? 0 : -1}
            class:active={direction === 'icp_to_base'}
            onclick={() => (direction = 'icp_to_base')}>ICP to Base</button>
          <button
            type="button"
            role="tab"
            id="tab-base-to-icp"
            aria-selected={direction === 'base_to_icp'}
            aria-controls="panel-base-to-icp"
            tabindex={direction === 'base_to_icp' ? 0 : -1}
            class:active={direction === 'base_to_icp'}
            onclick={() => (direction = 'base_to_icp')}>Base to ICP</button>
        </div>

        <BaseWalletBar
          {wallet}
          chain={config.chain}
          onConnect={(walletId) => void evm.connect(walletId)}
          onDisconnect={() => evm.disconnect()}
          onSwitch={() => void evm.switchNetwork(config.chain)}
        />

        {#if storeNotice}<p class="notice" role="status">{storeNotice}</p>{/if}

        <div id="panel-icp-to-base" role="tabpanel" aria-labelledby="tab-icp-to-base" hidden={direction !== 'icp_to_base'}>
          <IcpToBaseForm {config} {wallet} {principal} {store} onActivity={bumpHistory} />
        </div>
        <div id="panel-base-to-icp" role="tabpanel" aria-labelledby="tab-base-to-icp" hidden={direction !== 'base_to_icp'}>
          <BaseToIcpForm
            {config}
            {wallet}
            getProvider={() => evm.getProvider()}
            {principal}
            {store}
            onConnect={() => void evm.connect()}
            onSwitch={() => void evm.switchNetwork(config.chain)}
            onActivity={bumpHistory}
          />
        </div>
      </section>

      <aside class="side">
        <section class="glass-card">
          <BridgeHistory {principal} {config} baseAccount={wallet.account} version={historyVersion} />
        </section>
        <section class="glass-card facts">
          <h2>How it works</h2>
          <ul>
            <li>Network: <strong>{config.chain.name}</strong> (chain {config.chain.chainId}).</li>
            <li>icUSD contract on Base: <code>{config.contract}</code></li>
            <li>Deposits to Base are delivered after Base L1 batch finality, {DEPOSIT_DELIVERY_NOTE}. Releases back to the Internet Computer wait for the same finality.</li>
            <li>The bridge canister is the only authority. Wallet confirmations and transaction hashes on this page are hints.</li>
          </ul>
        </section>
      </aside>
    </div>
  {/if}
</main>

<style>
  .bridge-page { max-width:1100px; margin:0 auto; padding:0 0 2rem; color:var(--rumi-text-primary); }
  .intro { margin-bottom:1.25rem; }
  .eyebrow { margin:0 0 0.5rem; font-size:0.65rem; letter-spacing:0.09em; font-weight:600; color:var(--rumi-text-secondary); }
  .subtitle { max-width:44rem; margin:0; font-size:0.85rem; line-height:1.55; color:var(--rumi-text-secondary); }
  .layout { display:grid; grid-template-columns:minmax(0,1.4fr) minmax(0,1fr); gap:1.25rem; align-items:start; }
  .side { display:flex; flex-direction:column; gap:1.25rem; min-width:0; }
  .main-card { display:flex; flex-direction:column; gap:1rem; min-width:0; }
  .unavailable { max-width:36rem; }
  .unavailable h2 { margin:0 0 0.4rem; font-size:1.1rem; }
  .unavailable p { margin:0 0 0.8rem; font-size:0.85rem; color:var(--rumi-text-secondary); line-height:1.5; }
  .tabs { display:grid; grid-template-columns:1fr 1fr; gap:0.25rem; padding:0.2rem; border:1px solid var(--rumi-border); border-radius:0.5rem; background:var(--rumi-bg-primary); }
  .tabs button { padding:0.55rem 0.5rem; border:1px solid transparent; border-radius:0.35rem; background:transparent; color:var(--rumi-text-secondary); font:inherit; font-size:0.85rem; font-weight:600; cursor:pointer; }
  .tabs button.active { background:var(--rumi-bg-surface2); border-color:var(--rumi-border-hover); color:var(--rumi-text-primary); }
  .tabs button:focus-visible { outline:2px solid var(--rumi-action); outline-offset:2px; }
  .banner { margin:0 0 1.25rem; padding:0.7rem 0.9rem; border:1px solid var(--rumi-border); border-radius:0.5rem; background:var(--rumi-bg-surface2); font-size:0.82rem; line-height:1.5; color:var(--rumi-text-secondary); }
  .banner.warn { border-color:rgba(229,168,90,0.4); color:#ffd9a8; }
  .notice { margin:0; padding:0.6rem 0.8rem; border:1px solid rgba(229,168,90,0.4); border-radius:0.5rem; font-size:0.78rem; color:#ffd9a8; }
  .facts h2 { margin:0 0 0.5rem; font-size:1rem; }
  .facts ul { margin:0; padding-left:1.1rem; display:flex; flex-direction:column; gap:0.4rem; font-size:0.78rem; line-height:1.5; color:var(--rumi-text-secondary); }
  .facts code { overflow-wrap:anywhere; font-size:0.7rem; }
  [hidden] { display:none !important; }
  @media (max-width: 900px) {
    .layout { grid-template-columns:minmax(0,1fr); }
  }
</style>
