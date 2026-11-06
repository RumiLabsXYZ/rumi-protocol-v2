<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import type { Principal } from '@dfinity/principal';
  import type { DestinationView } from '$lib/services/baseBridge/destinations';
  import { browserLocalStorage } from '$lib/services/baseBridge/intents';
  import type { SolanaBridgeConfig } from '$lib/services/solanaBridge/config';
  import { SolanaIntentStore } from '$lib/services/solanaBridge/intents';
  import { createSolanaWalletStore } from '$lib/services/solanaBridge/walletStandard';
  import { getSolanaRpc } from '$lib/services/solanaBridge/solanaDeps';
  import SolanaWalletBar from './SolanaWalletBar.svelte';
  import IcpToSolanaForm from './IcpToSolanaForm.svelte';
  import SolanaToIcpForm from './SolanaToIcpForm.svelte';
  import SolanaHistory from './SolanaHistory.svelte';

  interface Props {
    config: SolanaBridgeConfig;
    /** The destination this panel is for. It is enabled while this panel exists. */
    destination: DestinationView;
    readFreshDestination?: () => DestinationView | null;
    onRecheck?: () => void;
    principal: Principal | null;
  }

  let { config, destination, readFreshDestination, onRecheck, principal }: Props = $props();

  type Direction = 'icp_to_solana' | 'solana_to_icp';
  let direction = $state<Direction>('icp_to_solana');
  let historyVersion = $state(0);
  let store = $state.raw<SolanaIntentStore | null>(null);
  let storeNotice = $state('');

  // The wallet store exists only while an ENABLED Solana destination is on screen. It discovers wallets when it is
  // started and asks none of them to connect until the user clicks Connect.
  const solana = createSolanaWalletStore();
  const wallet = $derived($solana);
  const rpc = $derived(getSolanaRpc(config.network));
  const principalText = $derived(principal && !principal.isAnonymous() ? principal.toText() : null);

  $effect(() => {
    const text = principalText;
    if (!text) {
      store = null;
      storeNotice = '';
      return;
    }
    const opened = new SolanaIntentStore(browserLocalStorage(), text);
    store = opened;
    storeNotice =
      opened.loadStatus === 'newer_version'
        ? 'This device holds Solana bridge records written by a newer version of the app. Reload the page; transfers are paused until then.'
        : opened.loadStatus === 'unavailable'
          ? 'Your browser is blocking local storage, so the bridge cannot save a recovery record. It will not start a transfer without one.'
          : opened.loadStatus === 'recovered'
            ? 'Some saved Solana bridge records on this device were unreadable. A copy was kept and everything readable was restored.'
            : '';
  });

  onMount(() => solana.init());
  onDestroy(() => solana.destroy());

  const DIRECTIONS: Direction[] = ['icp_to_solana', 'solana_to_icp'];
  function onTabKeydown(event: KeyboardEvent) {
    if (event.key !== 'ArrowRight' && event.key !== 'ArrowLeft' && event.key !== 'Home' && event.key !== 'End') return;
    event.preventDefault();
    const at = DIRECTIONS.indexOf(direction);
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? DIRECTIONS.length - 1 : (at + (event.key === 'ArrowRight' ? 1 : -1) + DIRECTIONS.length) % DIRECTIONS.length;
    direction = DIRECTIONS[next];
    document.getElementById(direction === 'icp_to_solana' ? 'tab-icp-to-solana' : 'tab-solana-to-icp')?.focus();
  }

  function bump() {
    historyVersion += 1;
  }
</script>

{#if config.mintsPaused && direction === 'icp_to_solana'}
  <p class="banner warn" role="status" data-testid="mint-notice">Mints on {config.network.name} are paused. A deposit waits in escrow until they resume.</p>
{/if}
<div class="layout">
  <section class="glass-card main-card">
    <div class="tabs" role="tablist" aria-label="Bridge direction" tabindex="-1" onkeydown={onTabKeydown}>
      <button type="button" role="tab" id="tab-icp-to-solana" aria-selected={direction === 'icp_to_solana'} aria-controls="panel-icp-to-solana" tabindex={direction === 'icp_to_solana' ? 0 : -1} class:active={direction === 'icp_to_solana'} onclick={() => (direction = 'icp_to_solana')}>ICP to Solana</button>
      <button type="button" role="tab" id="tab-solana-to-icp" aria-selected={direction === 'solana_to_icp'} aria-controls="panel-solana-to-icp" tabindex={direction === 'solana_to_icp' ? 0 : -1} class:active={direction === 'solana_to_icp'} onclick={() => (direction = 'solana_to_icp')}>Solana to ICP</button>
    </div>

    <SolanaWalletBar {wallet} network={config.network} onConnect={(name) => void solana.connect(name)} onDisconnect={() => void solana.disconnect()} />

    {#if storeNotice}<p class="notice" role="status">{storeNotice}</p>{/if}

    <div id="panel-icp-to-solana" role="tabpanel" aria-labelledby="tab-icp-to-solana" hidden={direction !== 'icp_to_solana'}>
      <IcpToSolanaForm {config} {destination} {readFreshDestination} {onRecheck} {wallet} {principal} {store} onActivity={bump} />
    </div>
    <div id="panel-solana-to-icp" role="tabpanel" aria-labelledby="tab-solana-to-icp" hidden={direction !== 'solana_to_icp'}>
      <SolanaToIcpForm
        {config}
        {destination}
        {readFreshDestination}
        {onRecheck}
        {wallet}
        getSession={() => solana.session()}
        {rpc}
        {principal}
        {store}
        onConnect={() => void solana.connect()}
        onActivity={bump}
      />
    </div>
  </section>

  <aside class="side">
    <section class="glass-card"><SolanaHistory {principal} {config} solanaAccount={wallet.address} version={historyVersion} /></section>
    <section class="glass-card facts">
      <h2>How it works</h2>
      <ul>
        <li>Network: <strong>{config.network.name}</strong>.</li>
        <li>icUSD mint on Solana: <code>{config.mint}</code> ({config.tokenProgram === 'token2022' ? 'Token-2022' : 'SPL Token'}).</li>
        <li>Returns go to the bridge's return account <code>{config.returnAccount}</code> and nowhere else.</li>
        <li>The bridge canister is the only authority. Wallet confirmations and transaction signatures on this page are hints.</li>
      </ul>
    </section>
  </aside>
</div>

<style>
  .layout { display:grid; grid-template-columns:minmax(0,1.4fr) minmax(0,1fr); gap:1.25rem; align-items:start; }
  .side { display:flex; flex-direction:column; gap:1.25rem; min-width:0; }
  .main-card { display:flex; flex-direction:column; gap:1rem; min-width:0; }
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
