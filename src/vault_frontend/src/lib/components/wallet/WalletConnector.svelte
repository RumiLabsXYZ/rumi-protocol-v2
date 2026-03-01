<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { pnp, initializePNP } from '../../services/pnp';
  import { walletStore } from '../../stores/wallet';
  import { get } from 'svelte/store';
  import { permissionManager } from '../../services/PermissionManager';
  import { WALLET_TYPES, currentWalletType } from '../../services/auth';
  import { truncatePrincipal, copyToClipboard } from '../../utils/principalHelpers';
  import { formatTokenBalance } from '../../utils/format';
  import { TokenService } from '../../services/tokenService';
  import { CONFIG } from '../../config';
  import Toast from '../common/Toast.svelte';

  interface WalletInfo {
    id: string;
    name: string;
    icon?: string;
  }

  // Token display metadata — ICP and icUSD use local assets; ck tokens fetch from ledger
  const TOKEN_META: Record<string, { name: string; symbol: string; icon: string; fallbackColor: string; canisterId?: string }> = {
    ICP:    { name: 'Internet Computer', symbol: 'ICP',    icon: '/icp-token-dark.svg', fallbackColor: '#3B00B9' },
    ICUSD:  { name: 'icUSD',             symbol: 'icUSD',  icon: '/icusd-logo_v3.svg',     fallbackColor: '#8B5CF6' },
    CKUSDT: { name: 'ckUSDT',            symbol: 'ckUSDT', icon: '',                    fallbackColor: '#26A17B', canisterId: CONFIG.ckusdtLedgerId },
    CKUSDC: { name: 'ckUSDC',            symbol: 'ckUSDC', icon: '',                    fallbackColor: '#2775CA', canisterId: CONFIG.ckusdcLedgerId },
  };

  // Dynamically fetched logos (canisterId → data URL)
  let dynamicLogos: Record<string, string> = {};

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

    const walletDisplayConfig: Record<string, { name: string; icon: string }> = {
      'plug': { name: 'Plug', icon: '/wallets/plug.svg' },
      'oisy': { name: 'Oisy', icon: '/wallets/oisy.svg' }
    };

    const filteredWallets = pnpWallets
      .filter(wallet => ALLOWED_PNP_WALLETS.includes(wallet.id?.toLowerCase()))
      .map(wallet => {
        const walletIdLower = wallet.id?.toLowerCase();
        const config = walletDisplayConfig[walletIdLower];
        const w = wallet as any;
        return {
          id: wallet.id,
          name: config?.name || w.name || wallet.id,
          icon: config?.icon || w.icon || `/wallets/${wallet.id}.svg`
        };
      });

    return [internetIdentityWallet, ...filteredWallets];
  }

  let error: string | null = null;
  let showWalletDialog = false;
  let connecting = false;
  let abortController = new AbortController();
  let isRefreshingBalance = false;
  let copiedPrincipal = false;

  let toasts: Array<{ id: number; message: string; type: 'success' | 'error' | 'info' }> = [];
  let toastId = 0;

  function addToast(message: string, type: 'success' | 'error' | 'info' = 'success') {
    const id = toastId++;
    toasts = [...toasts, { id, message, type }];
  }

  function removeToast(id: number) {
    toasts = toasts.filter(t => t.id !== id);
  }

  async function handleCopyPrincipal(e: MouseEvent) {
    e.stopPropagation();
    if (!account) return;
    const success = await copyToClipboard(account);
    if (success) {
      copiedPrincipal = true;
      setTimeout(() => { copiedPrincipal = false; }, 2000);
    }
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

      await walletStore.connect(walletId);
      clearTimeout(timeoutId);
      showWalletDialog = false;

      setTimeout(async () => {
        try {
          await walletStore.refreshBalance();
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
      showWalletDialog = false;
    } catch (err) {
      console.error('Disconnection failed:', err);
    }
  }

  // Handle clicks outside the wallet dialog to close it
  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (!showWalletDialog) return;

    const walletDialog = document.getElementById('wallet-dialog');
    const walletButton = document.getElementById('wallet-button');

    if (walletDialog && !walletDialog.contains(target) &&
        walletButton && !walletButton.contains(target)) {
      showWalletDialog = false;
    }
  }

  // Fetch logos from ICRC-1 ledger metadata for tokens without local icons
  async function fetchDynamicLogos() {
    for (const [key, meta] of Object.entries(TOKEN_META)) {
      if (!meta.icon && meta.canisterId) {
        try {
          const logo = await TokenService.fetchTokenLogo(meta.canisterId);
          if (logo) {
            dynamicLogos = { ...dynamicLogos, [key]: logo };
          }
        } catch (err) {
          console.warn(`Failed to fetch logo for ${key}:`, err);
        }
      }
    }
  }

  onMount(() => {
    document.addEventListener('click', handleClickOutside);

    try {
      initializePNP();
      walletList = buildWalletList();
    } catch (err) {
      console.error("Failed to initialize PNP:", err);
    } finally {
      walletsLoading = false;
    }

    if ($walletStore.isConnected && $walletStore.principal) {
      walletStore.refreshBalance().catch(err => {
        console.warn('Initial balance refresh failed:', err);
      });
    }

    // Fetch ck token logos from their ledger metadata (non-blocking)
    fetchDynamicLogos();

    return () => {
      document.removeEventListener('click', handleClickOutside);
    };
  });

  async function handleRefreshBalance(e: MouseEvent | KeyboardEvent) {
    e.stopPropagation();
    if (isRefreshingBalance) return;

    try {
      isRefreshingBalance = true;
      await walletStore.refreshBalance();
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

  // Compute total USD value
  $: totalUsdValue = Object.values(tokenBalances).reduce((sum, tb) => {
    return sum + (tb?.usdValue ?? 0);
  }, 0);

  // Build active token list: icUSD first, then ICP, then rest by USD value
  $: activeTokens = Object.entries(tokenBalances)
    .filter(([_, tb]) => tb && tb.raw > 0n)
    .map(([key, tb]) => {
      const baseMeta = TOKEN_META[key] || { name: key, symbol: key, icon: '', fallbackColor: '#666' };
      const resolvedIcon = baseMeta.icon || dynamicLogos[key] || '';
      return {
        key,
        meta: { ...baseMeta, icon: resolvedIcon },
        balance: tb!
      };
    })
    .sort((a, b) => {
      // icUSD always first
      if (a.key === 'ICUSD') return -1;
      if (b.key === 'ICUSD') return 1;
      // ICP always second
      if (a.key === 'ICP') return -1;
      if (b.key === 'ICP') return 1;
      // Rest sorted by USD value descending
      return (b.balance.usdValue ?? 0) - (a.balance.usdValue ?? 0);
    });

  // Quick reconnect — skip for Oisy (auto-reconnect handles it; the button
  // would flash briefly during page load before auto-reconnect completes)
  let pendingReconnectWallet: string | null = null;
  $: if (!isConnected && typeof window !== 'undefined') {
    const lastWallet = localStorage.getItem('rumi_last_wallet');
    const wasConnected = localStorage.getItem('rumi_was_connected');
    const isOisy = lastWallet?.toLowerCase().includes('oisy');
    pendingReconnectWallet = (lastWallet && wasConnected && !isOisy) ? lastWallet : null;
  } else {
    pendingReconnectWallet = null;
  }

  const walletDisplayNames: Record<string, string> = {
    'oisy': 'Oisy',
    'plug': 'Plug',
    'internet-identity': 'Internet Identity'
  };

  function getReconnectLabel(walletId: string): string {
    return walletDisplayNames[walletId] || walletId;
  }

  function getReconnectIcon(walletId: string): string | null {
    const icons: Record<string, string> = {
      'oisy': '/wallets/oisy.svg',
      'plug': '/wallets/plug.svg',
      'internet-identity': '/main-icp-logo.png'
    };
    return icons[walletId] || null;
  }
</script>

<div id="wallet-container">
  {#if !isConnected}
    {#if pendingReconnectWallet && !connecting}
      <!-- Quick reconnect -->
      <div class="flex items-center gap-2">
        <button
          id="wallet-button"
          class="icp-button reconnect-btn flex items-center gap-2"
          on:click|stopPropagation={() => connectWallet(pendingReconnectWallet)}
        >
          {#if getReconnectIcon(pendingReconnectWallet)}
            <img src={getReconnectIcon(pendingReconnectWallet)} alt="" class="w-4 h-4 rounded-sm" />
          {/if}
          Reconnect to {getReconnectLabel(pendingReconnectWallet)}
        </button>
        <button
          class="icp-button-secondary"
          on:click|stopPropagation={() => { pendingReconnectWallet = null; localStorage.removeItem('rumi_last_wallet'); localStorage.removeItem('rumi_was_connected'); showWalletDialog = true; }}
          title="Choose a different wallet"
        >
          ⋯
        </button>
      </div>
    {:else}
    <button
      id="wallet-button"
      class="icp-button flex items-center gap-2"
      on:click|stopPropagation={() => { showWalletDialog = true; }}
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
    {/if}

    {#if showWalletDialog}
      <div class="fixed inset-0 z-50 flex items-center justify-center p-4 min-h-screen">
        <div class="absolute inset-0 bg-black/50 backdrop-blur-sm" on:click|stopPropagation={() => showWalletDialog = false}></div>
        <div id="wallet-dialog" class="relative w-full max-w-md p-6 rounded-xl border shadow-xl transform transition-all" style="background: var(--rumi-bg-surface2); border-color: var(--rumi-border)">
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
                <div class="w-6 h-6 border-2 border-green-400 border-t-transparent rounded-full animate-spin mr-2"></div>
                <span class="text-gray-400">Loading wallets...</span>
              </div>
            {:else}
              {#each walletList as wallet (wallet.id)}
              <button
                class="flex items-center justify-between w-full px-4 py-3 text-white rounded-xl border transition-all duration-200"
                style="background: var(--rumi-bg-surface1); border-color: var(--rumi-border);"
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
                  <div class="flex flex-col items-start">
                    <span class="text-lg">{wallet.name}</span>
                  </div>
                </div>
                  <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5 text-gray-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <path d="M9 18l6-6-6-6"/>
                  </svg>
              </button>
            {/each}

            {#if walletList.length === 1}
              <p class="text-sm text-yellow-400 text-center mt-2">
                Only Internet Identity available. Plug and Oisy may not have loaded.
              </p>
            {/if}
            {/if}
          </div>

          {#if connecting}
            <div class="flex justify-center mt-4">
              <div class="w-6 h-6 border-2 border-green-400 border-t-transparent rounded-full animate-spin"></div>
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
    <!-- ═══ Connected: Icon-only header button ═══ -->
    <div class="relative">
      <button
        id="wallet-button"
        class="wallet-icon-btn"
        on:click|stopPropagation={() => { showWalletDialog = !showWalletDialog; }}
        aria-expanded={showWalletDialog}
        aria-controls="wallet-dialog"
        title="Wallet"
      >
        {#if currentIcon}
          <img src={currentIcon} alt="Wallet" class="wallet-icon-img" />
        {:else}
          <svg class="wallet-icon-svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M21 12V7H5a2 2 0 0 1 0-4h14v4" />
            <path d="M3 5v14a2 2 0 0 0 2 2h16v-5" />
            <path d="M18 12a2 2 0 0 0 0 4h4v-4Z" />
          </svg>
        {/if}
        <span class="wallet-connected-dot"></span>
      </button>

      {#if showWalletDialog}
        <div
          class="dropdown"
          id="wallet-dialog"
          role="dialog"
          aria-label="Wallet details"
        >
          <!-- USD Total + Rumi logo -->
          <div class="dropdown-total">
            <div class="dropdown-total-left">
              <span class="dropdown-total-label">Total Balance</span>
              <span class="dropdown-total-value">${totalUsdValue.toFixed(2)}</span>
            </div>
            <img src="/main-logo-without-BG.png" alt="Rumi" class="dropdown-total-logo" />
          </div>

          <!-- Principal (full, wrapping, with inline copy) -->
          <button
            class="dropdown-principal-row"
            on:click|stopPropagation={handleCopyPrincipal}
            title={copiedPrincipal ? 'Copied!' : 'Click to copy principal'}
          >
            <span class="dropdown-principal-text">{account ?? ''}</span>
            {#if copiedPrincipal}
              <svg class="dropdown-copy-icon dropdown-copy-icon-ok" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                <polyline points="20 6 9 17 4 12"/>
              </svg>
            {:else}
              <svg class="dropdown-copy-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <rect x="9" y="9" width="13" height="13" rx="2" ry="2"/>
                <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>
              </svg>
            {/if}
          </button>

          <!-- Token Balances -->
          <div class="dropdown-tokens">
            {#if activeTokens.length > 0}
              {#each activeTokens as token (token.key)}
                <div class="dropdown-token-row">
                  <div class="dropdown-token-left">
                    {#if token.meta.icon}
                      <img
                        src={token.meta.icon}
                        alt={token.meta.symbol}
                        class="dropdown-token-icon"
                      />
                    {:else}
                      <div class="dropdown-token-icon-fallback" style="background: {token.meta.fallbackColor}">
                        <span>{token.meta.symbol.charAt(0)}</span>
                      </div>
                    {/if}
                    <div class="dropdown-token-info">
                      <span class="dropdown-token-symbol">{token.meta.symbol}</span>
                      <span class="dropdown-token-name">{token.meta.name}</span>
                    </div>
                  </div>
                  <div class="dropdown-token-right">
                    <span class="dropdown-token-amount">{formatTokenBalance(token.balance.formatted)}</span>
                    {#if token.balance.usdValue !== null && token.balance.usdValue > 0}
                      <span class="dropdown-token-usd">${token.balance.usdValue.toFixed(2)}</span>
                    {/if}
                  </div>
                </div>
              {/each}
            {:else}
              <div class="dropdown-empty">
                <p>No balances found</p>
                <p class="dropdown-empty-hint">Try refreshing your balance</p>
              </div>
            {/if}
          </div>

          <!-- Actions -->
          <div class="dropdown-actions">
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
  /* ═══ Connected: Icon-only header button ═══ */
  .wallet-icon-btn {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 2.25rem;
    height: 2.25rem;
    background: rgba(15, 15, 25, 0.85);
    backdrop-filter: blur(12px);
    border: 1px solid rgba(139, 92, 246, 0.15);
    border-radius: 50%;
    cursor: pointer;
    transition: border-color 0.15s ease, background 0.15s ease;
    padding: 0;
  }
  .wallet-icon-btn:hover {
    border-color: rgba(139, 92, 246, 0.4);
    background: rgba(20, 20, 35, 0.95);
  }

  .wallet-icon-img {
    width: 1.375rem;
    height: 1.375rem;
    border-radius: 50%;
    object-fit: contain;
  }
  .wallet-icon-svg {
    width: 1.125rem;
    height: 1.125rem;
    color: rgba(255, 255, 255, 0.7);
  }

  .wallet-connected-dot {
    position: absolute;
    bottom: 0;
    right: 0;
    width: 0.5rem;
    height: 0.5rem;
    background: #2DD4BF;
    border: 1.5px solid rgba(12, 12, 22, 0.97);
    border-radius: 50%;
  }

  /* ═══ Dropdown ═══ */
  .dropdown {
    position: absolute;
    right: 0;
    top: calc(100% + 0.5rem);
    width: 20rem;
    background: rgba(12, 12, 22, 0.97);
    backdrop-filter: blur(16px);
    border: 1px solid rgba(139, 92, 246, 0.15);
    border-radius: 0.75rem;
    box-shadow: 0 20px 40px -8px rgba(0, 0, 0, 0.6);
    z-index: 50;
    overflow: hidden;
  }

  /* ── USD Total + Rumi logo ── */
  .dropdown-total {
    padding: 1rem 1rem 0.625rem;
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .dropdown-total-left {
    display: flex;
    flex-direction: column;
    gap: 0.125rem;
  }
  .dropdown-total-label {
    font-size: 0.7rem;
    font-weight: 500;
    color: rgba(255, 255, 255, 0.4);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .dropdown-total-value {
    font-size: 1.375rem;
    font-weight: 600;
    color: white;
    letter-spacing: -0.01em;
  }
  .dropdown-total-logo {
    width: 3.5rem;
    height: 3.5rem;
    object-fit: contain;
    opacity: 0.4;
  }

  /* ── Principal (full, wrapping, clickable to copy) ── */
  .dropdown-principal-row {
    display: flex;
    align-items: flex-start;
    gap: 0.375rem;
    width: 100%;
    padding: 0 1rem 0.75rem;
    border: none;
    background: transparent;
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
    cursor: pointer;
    transition: background 0.12s ease;
    text-align: left;
  }
  .dropdown-principal-row:hover {
    background: rgba(255, 255, 255, 0.03);
  }
  .dropdown-principal-text {
    font-size: 0.65rem;
    font-family: 'SF Mono', 'Fira Code', monospace;
    color: rgba(255, 255, 255, 0.35);
    word-break: break-all;
    line-height: 1.4;
    flex: 1;
    min-width: 0;
  }
  .dropdown-principal-row:hover .dropdown-principal-text {
    color: rgba(167, 139, 250, 0.7);
  }
  .dropdown-copy-icon {
    width: 0.75rem;
    height: 0.75rem;
    color: rgba(255, 255, 255, 0.3);
    flex-shrink: 0;
    margin-top: 0.1rem;
  }
  .dropdown-principal-row:hover .dropdown-copy-icon {
    color: rgba(167, 139, 250, 0.9);
  }
  .dropdown-copy-icon-ok {
    color: rgba(74, 222, 128, 0.9);
  }

  /* ── Token Balances ── */
  .dropdown-tokens {
    padding: 0.5rem 0.625rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    max-height: 16rem;
    overflow-y: auto;
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
  }
  .dropdown-token-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.5rem;
    border-radius: 0.5rem;
    transition: background 0.12s ease;
  }
  .dropdown-token-row:hover {
    background: rgba(255, 255, 255, 0.03);
  }
  .dropdown-token-left {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .dropdown-token-icon {
    width: 1.375rem;
    height: 1.375rem;
    border-radius: 50%;
    flex-shrink: 0;
    object-fit: cover;
  }
  .dropdown-token-icon-fallback {
    width: 1.375rem;
    height: 1.375rem;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    font-size: 0.65rem;
    font-weight: 700;
    color: white;
  }
  .dropdown-token-info {
    display: flex;
    flex-direction: column;
    gap: 0;
  }
  .dropdown-token-symbol {
    font-size: 0.8rem;
    font-weight: 600;
    color: rgba(255, 255, 255, 0.9);
    line-height: 1.2;
  }
  .dropdown-token-name {
    font-size: 0.625rem;
    color: rgba(255, 255, 255, 0.3);
    line-height: 1.2;
  }
  .dropdown-token-right {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 0;
  }
  .dropdown-token-amount {
    font-size: 0.8rem;
    font-weight: 500;
    color: rgba(255, 255, 255, 0.85);
    line-height: 1.2;
  }
  .dropdown-token-usd {
    font-size: 0.625rem;
    color: rgba(255, 255, 255, 0.3);
    line-height: 1.2;
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

  /* ── Actions ── */
  .dropdown-actions {
    padding: 0.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
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
    color: rgba(167, 139, 250, 0.85);
  }
  .dropdown-action-refresh:hover {
    background: rgba(139, 92, 246, 0.08);
  }
  .dropdown-action-disconnect {
    color: rgba(255, 255, 255, 0.4);
  }
  .dropdown-action-disconnect:hover {
    background: rgba(255, 255, 255, 0.05);
    color: rgba(255, 255, 255, 0.6);
  }

  /* ── Quick Reconnect ── */
  .reconnect-btn {
    animation: pulse-reconnect 2.5s ease-in-out infinite;
  }
  @keyframes pulse-reconnect {
    0%, 100% { box-shadow: 0 0 0 0 rgba(139, 92, 246, 0.3); }
    50% { box-shadow: 0 0 0 6px rgba(139, 92, 246, 0); }
  }
  .icp-button-secondary {
    padding: 0.4rem 0.6rem;
    background: rgba(15, 15, 25, 0.85);
    border: 1px solid rgba(139, 92, 246, 0.15);
    border-radius: 0.5rem;
    color: rgba(255, 255, 255, 0.5);
    cursor: pointer;
    font-size: 0.875rem;
    transition: all 0.15s ease;
  }
  .icp-button-secondary:hover {
    border-color: rgba(139, 92, 246, 0.35);
    color: rgba(255, 255, 255, 0.8);
  }

  /* ── Toast container ── */
  .toast-container {
    position: fixed;
    top: 4.5rem;
    right: 1rem;
    z-index: 8000;
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
