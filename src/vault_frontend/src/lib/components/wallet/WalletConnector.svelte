<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { pnp, initializePNP } from '../../services/pnp';
  import { walletStore } from '../../stores/wallet';
  import { get } from 'svelte/store';
  import { permissionManager } from '../../services/PermissionManager';
  import { WALLET_TYPES, currentWalletType } from '../../services/auth';
  import { truncatePrincipal, copyToClipboard } from '../../utils/principalHelpers';
  import Toast from '../common/Toast.svelte';
  import ReceiveModal from './ReceiveModal.svelte';
  import SendModal from './SendModal.svelte';

  interface WalletInfo {
    id: string;
    name: string;
    icon?: string;
  }

  // Whitelist: only include these wallet IDs from PNP library
  const ALLOWED_PNP_WALLETS = ['plug', 'oisy'];
  
  // Internet Identity is always available (hardcoded, doesn't need PNP)
  const internetIdentityWallet: WalletInfo = {
    id: WALLET_TYPES.INTERNET_IDENTITY,
    name: 'Internet Identity',
    icon: '/main-icp-logo.png'
  };

  // Reactive wallet list - starts with just II, PNP wallets added on mount
  let walletList: WalletInfo[] = [internetIdentityWallet];
  let walletsLoading = true;

  // Helper function to build wallet list from PNP
  function buildWalletList(): WalletInfo[] {
    const pnpWallets = pnp.getEnabledWallets();
    console.log("🔍 Available wallets from PNP:", pnpWallets);
    console.log("🔍 PNP wallet IDs:", pnpWallets.map(w => w.id));
    
    // Define proper display names and icons for each wallet
    const walletDisplayConfig: Record<string, { name: string; icon: string }> = {
      'plug': { name: 'Plug', icon: '/wallets/plug.svg' },
      'oisy': { name: 'Oisy', icon: '/wallets/oisy.svg' }
    };
    
    const filteredWallets = pnpWallets
      .filter(wallet => {
        const walletIdLower = wallet.id?.toLowerCase();
        const isAllowed = ALLOWED_PNP_WALLETS.includes(walletIdLower);
        console.log(`🔍 Wallet "${wallet.id}" (lowercase: "${walletIdLower}") - allowed: ${isAllowed}`);
        return isAllowed;
      })
      .map(wallet => {
        const walletIdLower = wallet.id?.toLowerCase();
        const config = walletDisplayConfig[walletIdLower];
        return {
          id: wallet.id,
          name: config?.name || wallet.name,
          icon: config?.icon || wallet.icon || `/wallets/${wallet.id}.svg`
        };
      });
    
    console.log("✅ Final filtered PNP wallets:", filteredWallets);
    return [internetIdentityWallet, ...filteredWallets];
  }

  let error: string | null = null;
  let showWalletDialog = false;
  let connecting = false;
  let abortController = new AbortController();
  let isRefreshingBalance = false;

  // Send/Receive modal state
  let showReceiveModal = false;
  let showSendModal = false;
  let toasts: Array<{ id: number; message: string; type: 'success' | 'error' | 'info' }> = [];
  let toastId = 0;

  function addToast(message: string, type: 'success' | 'error' | 'info' = 'success') {
    const id = toastId++;
    toasts = [...toasts, { id, message, type }];
  }

  function removeToast(id: number) {
    toasts = toasts.filter(t => t.id !== id);
  }

  // Check if current wallet is II (only II gets send/receive)
  $: isInternetIdentity = $currentWalletType === WALLET_TYPES.INTERNET_IDENTITY;

  async function handleCopyPrincipal(e: MouseEvent) {
    e.stopPropagation();
    if (!account) return;
    const success = await copyToClipboard(account);
    if (success) {
      addToast('Principal copied', 'success');
    }
  }

  function handleSendSuccess() {
    showSendModal = false;
    walletStore.refreshBalance();
  }

  onDestroy(() => {
    if (connecting) {
      connecting = false;
      abortController.abort();
    }
  });

  async function connectWallet(walletId: string) {
    if (!walletId || connecting) return;
    
    try {
      connecting = true;
      error = null;
      abortController = new AbortController();
      
      const timeoutId = setTimeout(() => abortController.abort(), 30000);
      
      // FIXED: Connect wallet FIRST, then permissions are handled automatically
      // The walletStore.connect() will handle permission requests internally
      await walletStore.connect(walletId);
      clearTimeout(timeoutId);
      showWalletDialog = false;
      
      // Add a short delay then refresh balance explicitly
      setTimeout(async () => {
        try {
          await walletStore.refreshBalance();
          console.log('Initial balance refresh completed');
        } catch (err) {
          console.warn('Initial balance refresh failed:', err);
        }
      }, 1000);
      
    } catch (err) {
      console.error('Connection error:', err);
      error = err instanceof Error ? err.message : 'Failed to connect';
    } finally {
      connecting = false;
    }
  }

  async function disconnectWallet() {
    try {
      await walletStore.disconnect();
      showWalletDialog = false; // Close the dropdown after disconnect
    } catch (err) {
      console.error('Disconnection failed:', err);
    }
  }

  function formatAddress(addr: string | null): string {
    if (!addr) return '';
    return truncatePrincipal(addr);
  }

  // Handle clicks outside the wallet dialog to close it
  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (!showWalletDialog) return;
    
    // Don't close dropdown if a modal is open
    if (showReceiveModal || showSendModal) return;
    
    const walletDialog = document.getElementById('wallet-dialog');
    const walletButton = document.getElementById('wallet-button');
    
    // Close dialog if clicking outside and not on the button
    if (walletDialog && !walletDialog.contains(target) && 
        walletButton && !walletButton.contains(target)) {
      showWalletDialog = false;
    }
  }

  // Setup click handler on mount
  onMount(() => {
    document.addEventListener('click', handleClickOutside);
    
    // Initialize PNP and build wallet list
    try {
      console.log("🚀 WalletConnector onMount: Initializing PNP...");
      initializePNP();
      walletList = buildWalletList();
      console.log("✅ Wallet list built:", walletList.map(w => w.id));
    } catch (err) {
      console.error("❌ Failed to initialize PNP:", err);
    } finally {
      walletsLoading = false;
    }
    
    // Perform an initial balance refresh if connected
    if ($walletStore.isConnected && $walletStore.principal) {
      walletStore.refreshBalance().catch(err => {
        console.warn('Initial balance refresh failed:', err);
      });
    }
    
    return () => {
      document.removeEventListener('click', handleClickOutside);
    };
  });

  // Add manual refresh function
  async function handleRefreshBalance(e: MouseEvent | KeyboardEvent) {
    e.stopPropagation();
    
    if (isRefreshingBalance) return; // Prevent multiple concurrent refreshes
    
    try {
      isRefreshingBalance = true;
      console.log('Manual balance refresh requested');
      await walletStore.refreshBalance();
      console.log('Balance refresh completed');
    } catch (err) {
      console.error('Manual balance refresh failed:', err);
    } finally {
      isRefreshingBalance = false;
    }
  }
  
  $: isConnected = $walletStore.isConnected;
  $: account = $walletStore.principal?.toString() ?? null;
  $: currentIcon = $walletStore.icon;
  $: tokenBalances = $walletStore.tokenBalances ?? {};
  
  // Log the token balances whenever they change
  $: {
    console.log('Current wallet state:', $walletStore);
    if ($walletStore.tokenBalances?.ICP) {
      console.log('Displayed ICP balance:', $walletStore.tokenBalances.ICP.formatted,
                  'Raw:', $walletStore.tokenBalances.ICP.raw.toString());
    }
  }
</script>

<svelte:head>
  <!-- Add any necessary script imports here -->
</svelte:head>

<div id="wallet-container">
  {#if !isConnected}
    <button
      id="wallet-button"
      class="icp-button flex items-center gap-2"
      on:click|stopPropagation={() => { showWalletDialog = true; console.log("Dialog open state:", showWalletDialog); }}
      disabled={connecting}
    >
      {#if connecting}
        <div class="w-4 h-4 border-2 border-white border-t-transparent rounded-full animate-spin"></div>
      {:else}
        <svg xmlns="http://www.w3.org/2000/svg" class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M21 12V7H5a2 2 0 0 1 0-4h14v4" />
          <path d="M3 5v14a2 2 0 0 0 2 2h16v-5" />
          <path d="M18 12a2 2 0 0 0 0 4h4v-4Z" />
        </svg>
      {/if}
      {connecting ? 'Connecting...' : 'Connect Wallet'}
    </button>

    {#if showWalletDialog}
      <div class="fixed inset-0 z-50 flex items-center justify-center p-4 min-h-screen">
        <div class="absolute inset-0 bg-black/50 backdrop-blur-sm" on:click|stopPropagation={() => showWalletDialog = false}></div>
        <div id="wallet-dialog" class="relative w-full max-w-md p-6 rounded-xl border shadow-xl transform transition-all" style="background: var(--rumi-bg-elevated); border-color: var(--rumi-border)">
          <div class="flex justify-between mb-6">
            <h2 class="text-xl font-semibold text-white">Connect Wallet</h2>
            <button 
              class="text-gray-400 hover:text-gray-200"
              on:click|stopPropagation={() => showWalletDialog = false}
              disabled={connecting}
            >
              ✕
            </button>
          </div>
          
          <div class="flex flex-col gap-3">
            {#if walletsLoading}
              <div class="flex items-center justify-center py-4">
                <div class="w-6 h-6 border-2 border-teal-400 border-t-transparent rounded-full animate-spin mr-2"></div>
                <span class="text-gray-400">Loading wallets...</span>
              </div>
            {:else}
              {#each walletList as wallet (wallet.id)}
              <button
                class="flex items-center justify-between w-full px-4 py-3 text-white rounded-xl border transition-all duration-200" style="background: var(--rumi-bg-card); border-color: var(--rumi-border); hover: var(--rumi-bg-card-hover)"
                on:click|stopPropagation={() => connectWallet(wallet.id)}
                disabled={connecting}
              >
                <div class="flex items-center gap-4">
                  {#if wallet.icon}
                    <img 
                      src={wallet.icon}
                      alt={wallet.name} 
                      class="w-10 h-10 rounded-lg object-contain"
                    />
                  {:else}
                    <div class="w-10 h-10 bg-gray-700 rounded-full flex items-center justify-center">
                      <span>{wallet.name[0]}</span>
                    </div>
                  {/if}
                  <span class="text-lg">{wallet.name}</span>
                </div>
                <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5 text-gray-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M9 18l6-6-6-6"/>
                </svg>
              </button>
            {/each}
              
            {#if walletList.length === 1}
              <p class="text-sm text-yellow-400 text-center mt-2">
                ⚠️ Only Internet Identity available. Plug and Oisy may not have loaded.
              </p>
            {/if}
            {/if}
          </div>

          {#if connecting}
            <div class="flex justify-center mt-4">
              <div class="w-6 h-6 border-2 border-teal-400 border-t-transparent rounded-full animate-spin"></div>
            </div>
          {/if}
          
          {#if error}
            <div class="mt-4 p-3 bg-red-900/50 text-red-200 rounded-lg">
              <div class="flex justify-between">
                <div>{error}</div>
                <button 
                  class="text-gray-400 hover:text-gray-200"
                  on:click|stopPropagation={() => error = null}
                  aria-label="Close error message"
                >
                  ✕
                </button>
              </div>
            </div>
          {/if}
        </div>
      </div>
    {/if}
  {:else}
    <div class="relative">
      <button
        id="wallet-button"
        class="wallet-pill"
        on:click|stopPropagation={() => { showWalletDialog = !showWalletDialog; }}
        aria-expanded={showWalletDialog}
        aria-controls="wallet-dialog"
      >
        <!-- Wallet Icon -->
        {#if currentIcon}
          <img src={currentIcon} alt="" class="pill-icon" />
        {:else}
          <div class="pill-icon-fallback">
            <div class="pill-icon-dot"></div>
          </div>
        {/if}

        <!-- Balances: icUSD primary, ICP secondary -->
        <div class="pill-balances">
          {#if tokenBalances.ICUSD && Number(tokenBalances.ICUSD.formatted) > 0}
            <span class="pill-balance-primary">{tokenBalances.ICUSD.formatted} icUSD</span>
          {/if}
          {#if tokenBalances.ICP}
            <span class="pill-balance-secondary">{tokenBalances.ICP.formatted} ICP</span>
          {/if}
        </div>

        <!-- Principal (metadata) -->
        <span class="pill-principal">{formatAddress(account)}</span>

        <!-- Controls separator + icons -->
        <span class="pill-divider"></span>

        <div
          class="pill-control"
          on:click|stopPropagation={handleRefreshBalance}
          on:keydown={e => e.key === 'Enter' && handleRefreshBalance(e)}
          role="button"
          tabindex="0"
          title="Refresh balance"
        >
          <svg class:animate-spin={isRefreshingBalance} class="pill-control-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </div>

        <svg
          class="pill-control-icon pill-caret {showWalletDialog ? 'pill-caret-open' : ''}"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7"></path>
        </svg>
      </button>

      {#if showWalletDialog}
        <div 
          class="dropdown"
          id="wallet-dialog"
          role="dialog"
          aria-label="Wallet options"
        >
          <!-- Principal header -->
          <div class="dropdown-header">
            <div class="dropdown-identity">
              {#if currentIcon}
                <img src={currentIcon} alt="" class="dropdown-wallet-icon" />
              {:else}
                <div class="dropdown-wallet-icon-fallback">
                  <svg class="w-4 h-4 text-white" fill="currentColor" viewBox="0 0 20 20">
                    <path d="M4 4a2 2 0 00-2 2v1h16V6a2 2 0 00-2-2H4zM18 9H2v5a2 2 0 002 2h12a2 2 0 002-2V9zM4 13a1 1 0 011-1h1a1 1 0 110 2H5a1 1 0 01-1-1zm5-1a1 1 0 100 2h1a1 1 0 100-2H9z"></path>
                  </svg>
                </div>
              {/if}
              <div class="dropdown-identity-text">
                <p class="dropdown-label">Connected</p>
                <p
                  class="dropdown-principal"
                  on:click|stopPropagation={handleCopyPrincipal}
                  title="Click to copy full principal"
                >{formatAddress(account)}</p>
              </div>
            </div>
          </div>

          <!-- Balances -->
          <div class="dropdown-balances">
            {#if tokenBalances.ICP}
              <div class="dropdown-balance-row">
                <div class="dropdown-balance-left">
                  <img src="/icp_logo.png" alt="ICP" class="dropdown-token-icon" />
                  <span class="dropdown-balance-amount">{tokenBalances.ICP.formatted} ICP</span>
                </div>
                {#if tokenBalances.ICP.usdValue}
                  <span class="dropdown-balance-usd">${tokenBalances.ICP.usdValue.toFixed(2)}</span>
                {/if}
              </div>
            {/if}

            {#if tokenBalances.ICUSD}
              <div class="dropdown-balance-row">
                <div class="dropdown-balance-left">
                  <img src="/icUSD-logo.png" alt="icUSD" class="dropdown-token-icon" />
                  <span class="dropdown-balance-amount">{tokenBalances.ICUSD.formatted} icUSD</span>
                </div>
              </div>
            {/if}

            {#if (!tokenBalances.ICP || Number(tokenBalances.ICP.formatted) === 0) && (!tokenBalances.ICUSD || Number(tokenBalances.ICUSD.formatted) === 0)}
              <div class="dropdown-empty">
                <p>No balances found</p>
                <p class="dropdown-empty-hint">Try refreshing your balance</p>
              </div>
            {/if}
          </div>
          
          <!-- Actions -->
          <div class="dropdown-actions">
            {#if isInternetIdentity}
              <div class="dropdown-action-pair">
                <button
                  class="dropdown-btn dropdown-btn-receive"
                  on:click|stopPropagation={() => { showReceiveModal = true; showWalletDialog = false; }}
                >
                  <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 5v14M5 12l7 7 7-7"/>
                  </svg>
                  Receive
                </button>
                <button
                  class="dropdown-btn dropdown-btn-send"
                  on:click|stopPropagation={() => { showSendModal = true; showWalletDialog = false; }}
                >
                  <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 19V5M5 12l7-7 7 7"/>
                  </svg>
                  Send
                </button>
              </div>
            {:else}
              <div class="dropdown-action-pair" title="Use your wallet app to send and receive">
                <button class="dropdown-btn dropdown-btn-disabled" disabled>
                  <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 5v14M5 12l7 7 7-7"/></svg>
                  Receive
                </button>
                <button class="dropdown-btn dropdown-btn-disabled" disabled>
                  <svg class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 19V5M5 12l7-7 7 7"/></svg>
                  Send
                </button>
              </div>
            {/if}

            <button 
              class="dropdown-action-row dropdown-action-refresh"
              on:click|stopPropagation={handleRefreshBalance}
              disabled={isRefreshingBalance}
            >
              <svg class:animate-spin={isRefreshingBalance} class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
              <span>{isRefreshingBalance ? 'Refreshing...' : 'Refresh Balance'}</span>
            </button>
            
            <button
              class="dropdown-action-row dropdown-action-disconnect"
              on:click|stopPropagation={disconnectWallet}
            >
              <svg xmlns="http://www.w3.org/2000/svg" class="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
                <polyline points="16 17 21 12 16 7" />
                <line x1="21" y1="12" x2="9" y2="12" />
              </svg>
              <span>Disconnect</span>
            </button>
          </div>
        </div>
      {/if}
    </div>
  {/if}
</div>

{#if error}
  <div class="fixed bottom-4 right-4 p-4 bg-red-500 text-white rounded-lg shadow-lg z-50">
    {error}
    <button class="ml-2" on:click={() => error = null}>✕</button>
  </div>
{/if}

{#if showReceiveModal && account}
  <ReceiveModal
    principal={account}
    onClose={() => showReceiveModal = false}
    onToast={addToast}
  />
{/if}

{#if showSendModal}
  <SendModal
    onClose={() => showSendModal = false}
    onSuccess={handleSendSuccess}
    onToast={addToast}
    icpBalance={tokenBalances.ICP?.formatted ?? '0'}
    icusdBalance={tokenBalances.ICUSD?.formatted ?? '0'}
  />
{/if}

{#if toasts.length > 0}
  <div class="toast-container">
    {#each toasts as toast (toast.id)}
      <Toast
        message={toast.message}
        type={toast.type}
        onClose={() => removeToast(toast.id)}
      />
    {/each}
  </div>
{/if}

<style>
  /* ── Header Pill ── */
  .wallet-pill {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.4rem 0.6rem 0.4rem 0.5rem;
    background: rgba(15, 15, 25, 0.85);
    backdrop-filter: blur(12px);
    border: 1px solid rgba(139, 92, 246, 0.15);
    border-radius: 0.75rem;
    cursor: pointer;
    transition: border-color 0.15s ease, background 0.15s ease;
    color: white;
    font-family: inherit;
  }
  .wallet-pill:hover {
    border-color: rgba(139, 92, 246, 0.35);
    background: rgba(20, 20, 35, 0.9);
  }

  .pill-icon {
    width: 1.375rem;
    height: 1.375rem;
    border-radius: 50%;
    flex-shrink: 0;
  }
  .pill-icon-fallback {
    width: 1.375rem;
    height: 1.375rem;
    border-radius: 50%;
    background: var(--rumi-teal-dim);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .pill-icon-dot {
    width: 0.35rem;
    height: 0.35rem;
    background: white;
    border-radius: 50%;
  }

  .pill-balances {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
  }
  .pill-balance-primary {
    font-size: 0.825rem;
    font-weight: 600;
    color: rgba(255, 255, 255, 0.95);
    white-space: nowrap;
  }
  .pill-balance-secondary {
    font-size: 0.75rem;
    font-weight: 400;
    color: rgba(255, 255, 255, 0.45);
    white-space: nowrap;
  }

  .pill-principal {
    font-size: 0.65rem;
    font-family: 'SF Mono', 'Fira Code', monospace;
    color: rgba(255, 255, 255, 0.3);
    white-space: nowrap;
  }

  .pill-divider {
    width: 1px;
    height: 1rem;
    background: rgba(255, 255, 255, 0.1);
    margin: 0 0.1rem;
    flex-shrink: 0;
  }

  .pill-control {
    padding: 0.25rem;
    border-radius: 0.25rem;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    transition: background 0.12s ease;
  }
  .pill-control:hover {
    background: rgba(255, 255, 255, 0.08);
  }

  .pill-control-icon {
    width: 0.875rem;
    height: 0.875rem;
    color: rgba(255, 255, 255, 0.35);
    flex-shrink: 0;
  }

  .pill-caret {
    transition: transform 0.2s ease;
  }
  .pill-caret-open {
    transform: rotate(180deg);
  }

  /* ── Dropdown ── */
  .dropdown {
    position: absolute;
    right: 0;
    top: calc(100% + 0.5rem);
    width: 19rem;
    background: rgba(12, 12, 22, 0.97);
    backdrop-filter: blur(16px);
    border: 1px solid rgba(139, 92, 246, 0.15);
    border-radius: 0.75rem;
    box-shadow: 0 20px 40px -8px rgba(0, 0, 0, 0.6);
    z-index: 50;
    overflow: hidden;
  }

  .dropdown-header {
    padding: 0.875rem 1rem;
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
  }
  .dropdown-identity {
    display: flex;
    align-items: center;
    gap: 0.625rem;
  }
  .dropdown-wallet-icon {
    width: 1.75rem;
    height: 1.75rem;
    border-radius: 50%;
    flex-shrink: 0;
  }
  .dropdown-wallet-icon-fallback {
    width: 1.75rem;
    height: 1.75rem;
    border-radius: 50%;
    background: var(--rumi-teal-dim);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .dropdown-identity-text {
    min-width: 0;
  }
  .dropdown-label {
    margin: 0;
    font-size: 0.75rem;
    font-weight: 500;
    color: rgba(255, 255, 255, 0.5);
    line-height: 1;
  }
  .dropdown-principal {
    margin: 0.2rem 0 0;
    font-size: 0.7rem;
    font-family: 'SF Mono', 'Fira Code', monospace;
    color: rgba(255, 255, 255, 0.6);
    cursor: pointer;
    transition: color 0.12s ease;
  }
  .dropdown-principal:hover {
    color: rgba(167, 139, 250, 0.9);
  }

  /* ── Dropdown balances ── */
  .dropdown-balances {
    padding: 0.625rem 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
  }
  .dropdown-balance-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.5rem 0.5rem;
    background: rgba(255, 255, 255, 0.03);
    border-radius: 0.5rem;
  }
  .dropdown-balance-left {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .dropdown-token-icon {
    width: 1.125rem;
    height: 1.125rem;
    border-radius: 50%;
  }
  .dropdown-balance-amount {
    font-size: 0.825rem;
    font-weight: 500;
    color: rgba(255, 255, 255, 0.9);
  }
  .dropdown-balance-usd {
    font-size: 0.7rem;
    color: rgba(255, 255, 255, 0.35);
  }
  .dropdown-empty {
    text-align: center;
    padding: 1rem 0;
    font-size: 0.8rem;
    color: rgba(255, 255, 255, 0.4);
  }
  .dropdown-empty-hint {
    font-size: 0.7rem;
    color: rgba(255, 255, 255, 0.25);
    margin-top: 0.2rem;
  }

  /* ── Dropdown actions ── */
  .dropdown-actions {
    padding: 0.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .dropdown-action-pair {
    display: flex;
    gap: 0.375rem;
    margin-bottom: 0.25rem;
  }
  .dropdown-btn {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.375rem;
    padding: 0.5rem;
    font-size: 0.8rem;
    font-weight: 500;
    border-radius: 0.5rem;
    border: 1px solid transparent;
    cursor: pointer;
    transition: all 0.12s ease;
  }
  .dropdown-btn-receive {
    color: rgba(74, 222, 128, 0.9);
    background: rgba(34, 197, 94, 0.08);
    border-color: rgba(34, 197, 94, 0.15);
  }
  .dropdown-btn-receive:hover {
    background: rgba(34, 197, 94, 0.15);
  }
  .dropdown-btn-send {
    color: rgba(167, 139, 250, 0.9);
    background: rgba(139, 92, 246, 0.08);
    border-color: rgba(139, 92, 246, 0.15);
  }
  .dropdown-btn-send:hover {
    background: rgba(139, 92, 246, 0.15);
  }
  .dropdown-btn-disabled {
    color: rgba(255, 255, 255, 0.25);
    background: rgba(255, 255, 255, 0.03);
    border-color: rgba(255, 255, 255, 0.06);
    cursor: not-allowed;
    opacity: 0.5;
  }

  .dropdown-action-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    padding: 0.5rem 0.625rem;
    font-size: 0.8rem;
    border: none;
    border-radius: 0.375rem;
    background: transparent;
    cursor: pointer;
    transition: background 0.12s ease;
  }
  .dropdown-action-refresh {
    color: rgba(96, 165, 250, 0.85);
  }
  .dropdown-action-refresh:hover {
    background: rgba(59, 130, 246, 0.08);
  }
  .dropdown-action-disconnect {
    color: rgba(248, 113, 113, 0.85);
  }
  .dropdown-action-disconnect:hover {
    background: rgba(239, 68, 68, 0.08);
  }

  /* ── Toast container ── */
  .toast-container {
    position: fixed;
    top: 4.5rem; /* header height + 14px offset */
    right: 1rem;
    z-index: 8000; /* above content, below modals (9000) */
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    pointer-events: none;
    max-width: 340px;
  }
  .toast-container > :global(*) {
    pointer-events: auto;
  }
</style>
