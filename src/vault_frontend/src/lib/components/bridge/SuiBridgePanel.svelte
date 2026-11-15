<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import type { Principal } from '@dfinity/principal';
  import type { DestinationView } from '$lib/services/baseBridge/destinations';
  import { browserLocalStorage } from '$lib/services/baseBridge/intents';
  import type { SuiBridgeConfig } from '$lib/services/suiBridge/config';
  import { SuiIntentStore } from '$lib/services/suiBridge/intents';
  import { createSuiWalletStore } from '$lib/services/suiBridge/walletStandard';
  import { getSuiRpc } from '$lib/services/suiBridge/suiDeps';
  import SuiWalletBar from './SuiWalletBar.svelte';
  import IcpToSuiForm from './IcpToSuiForm.svelte';
  import SuiToIcpForm from './SuiToIcpForm.svelte';
  import SuiHistory from './SuiHistory.svelte';

  interface Props {
    config: SuiBridgeConfig;
    /** The destination this panel is for. It is enabled while this panel exists. */
    destination: DestinationView;
    readFreshDestination?: () => DestinationView | null;
    onRecheck?: () => void;
    principal: Principal | null;
  }

  let { config, destination, readFreshDestination, onRecheck, principal }: Props = $props();

  type Direction = 'icp_to_sui' | 'sui_to_icp';
  let direction = $state<Direction>('icp_to_sui');
  let historyVersion = $state(0);
  let store = $state.raw<SuiIntentStore | null>(null);
  let storeNotice = $state('');

  // The wallet store exists only while an ENABLED Sui destination is on screen. It discovers wallets when it is
  // started and asks none of them to connect until the user clicks Connect.
  const sui = createSuiWalletStore();
  const wallet = $derived($sui);
  const rpc = $derived(getSuiRpc(config.network));
  const principalText = $derived(principal && !principal.isAnonymous() ? principal.toText() : null);

  $effect(() => {
    const text = principalText;
    if (!text) {
      store = null;
      storeNotice = '';
      return;
    }
    const opened = new SuiIntentStore(browserLocalStorage(), text);
    store = opened;
    storeNotice =
      opened.loadStatus === 'newer_version'
        ? 'This device holds Sui bridge records written by a newer version of the app. Reload the page; transfers are paused until then.'
        : opened.loadStatus === 'unavailable'
          ? 'Your browser is blocking local storage, so the bridge cannot save a recovery record. It will not start a transfer without one.'
          : opened.loadStatus === 'recovered'
            ? 'Some saved Sui bridge records on this device were unreadable. A copy was kept and everything readable was restored.'
            : '';
  });

  onMount(() => sui.init());
  onDestroy(() => sui.destroy());

  const DIRECTIONS: Direction[] = ['icp_to_sui', 'sui_to_icp'];
  function onTabKeydown(event: KeyboardEvent) {
    if (event.key !== 'ArrowRight' && event.key !== 'ArrowLeft' && event.key !== 'Home' && event.key !== 'End') return;
    event.preventDefault();
    const at = DIRECTIONS.indexOf(direction);
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? DIRECTIONS.length - 1 : (at + (event.key === 'ArrowRight' ? 1 : -1) + DIRECTIONS.length) % DIRECTIONS.length;
    direction = DIRECTIONS[next];
    document.getElementById(direction === 'icp_to_sui' ? 'tab-icp-to-sui' : 'tab-sui-to-icp')?.focus();
  }

  function bump() {
    historyVersion += 1;
  }
</script>

<!-- Direction state, kept apart from the identity that enabled this panel: what the operator has stopped, and what the
     canister's own mint lane says about whether a deposit can be minted right now. -->
{#if config.halted}
  <p class="banner bad" role="alert" data-testid="halt-notice">The bridge is halted for safety: {config.halted}</p>
{:else}
  {#if direction === 'icp_to_sui'}
    {#if config.depositsPaused}
      <p class="banner warn" role="status" data-testid="deposits-paused">Deposits to {config.network.name} are paused. Your icUSD has not moved.</p>
    {:else if !config.mintLaneReady}
      <p class="banner warn" role="status" data-testid="mint-notice">
        The bridge cannot mint on {config.network.name} right now{config.mintLaneBlockedBy.length > 0 ? `: ${config.mintLaneBlockedBy.join('; ')}` : ''}. A deposit waits in escrow, and you can ask for a refund while it is held.
      </p>
    {/if}
  {:else if config.releasesPaused}
    <p class="banner warn" role="status" data-testid="releases-paused">Releases to the Internet Computer are paused. A return is recorded and waits for releases to resume.</p>
  {/if}
{/if}
<div class="layout">
  <section class="glass-card main-card">
    <div class="tabs" role="tablist" aria-label="Bridge direction" tabindex="-1" onkeydown={onTabKeydown}>
      <button type="button" role="tab" id="tab-icp-to-sui" aria-selected={direction === 'icp_to_sui'} aria-controls="panel-icp-to-sui" tabindex={direction === 'icp_to_sui' ? 0 : -1} class:active={direction === 'icp_to_sui'} onclick={() => (direction = 'icp_to_sui')}>ICP to Sui</button>
      <button type="button" role="tab" id="tab-sui-to-icp" aria-selected={direction === 'sui_to_icp'} aria-controls="panel-sui-to-icp" tabindex={direction === 'sui_to_icp' ? 0 : -1} class:active={direction === 'sui_to_icp'} onclick={() => (direction = 'sui_to_icp')}>Sui to ICP</button>
    </div>

    <SuiWalletBar {wallet} network={config.network} onConnect={(name) => void sui.connect(name)} onDisconnect={() => void sui.disconnect()} />

    {#if storeNotice}<p class="notice" role="status">{storeNotice}</p>{/if}

    <div id="panel-icp-to-sui" role="tabpanel" aria-labelledby="tab-icp-to-sui" hidden={direction !== 'icp_to_sui'}>
      <IcpToSuiForm {config} {destination} {readFreshDestination} {onRecheck} {wallet} {principal} {store} onActivity={bump} />
    </div>
    <div id="panel-sui-to-icp" role="tabpanel" aria-labelledby="tab-sui-to-icp" hidden={direction !== 'sui_to_icp'}>
      <SuiToIcpForm
        {config}
        {destination}
        {readFreshDestination}
        {onRecheck}
        {wallet}
        getSession={() => sui.session()}
        {rpc}
        {principal}
        {store}
        onConnect={() => void sui.connect()}
        onActivity={bump}
      />
    </div>
  </section>

  <aside class="side">
    <section class="glass-card"><SuiHistory {principal} {config} suiAddress={wallet.address} version={historyVersion} /></section>
    <section class="glass-card facts">
      <h2>How it works</h2>
      <ul>
        <li>Network: <strong>{config.network.name}</strong>.</li>
        <li>Package: <code>{config.packageId}</code>.</li>
        <li>Gate: <code>{config.gateId}</code>. Coin: <code>{config.coinType}</code> (8 decimals).</li>
        <li>A return burns icUSD through the Gate. The bridge releases on the Internet Computer only after it verifies that burn from the Gate's own record.</li>
        <li>Sui gas is paid in SUI by your wallet and is separate from icUSD.</li>
        <li data-testid="module-disclosure">
          {#if config.moduleBytesMatched}
            The bridge matched the published Gate module against a reviewed build.
          {:else}
            The bridge does not yet compare the published Gate module's bytes with a reviewed build, so this page cannot show that they match. It checks the package, Gate, caps, decimals and signing roles against the bridge's own observation of {config.network.name}.
          {/if}
        </li>
        <li>The bridge canister is the only authority. Wallet confirmations and transaction digests on this page are hints.</li>
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
  .banner.bad { border-color:rgba(224,107,159,0.4); background:rgba(224,107,159,0.08); color:#ffb3c5; }
  .notice { margin:0; padding:0.6rem 0.8rem; border:1px solid rgba(229,168,90,0.4); border-radius:0.5rem; font-size:0.78rem; color:#ffd9a8; }
  .facts h2 { margin:0 0 0.5rem; font-size:1rem; }
  .facts ul { margin:0; padding-left:1.1rem; display:flex; flex-direction:column; gap:0.4rem; font-size:0.78rem; line-height:1.5; color:var(--rumi-text-secondary); }
  .facts code { overflow-wrap:anywhere; font-size:0.7rem; }
  [hidden] { display:none !important; }
  @media (max-width: 900px) {
    .layout { grid-template-columns:minmax(0,1fr); }
  }
</style>
