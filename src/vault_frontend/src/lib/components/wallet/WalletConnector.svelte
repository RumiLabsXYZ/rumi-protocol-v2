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
  import { collateralStore } from '../../stores/collateralStore';
  import Toast from '../common/Toast.svelte';
  import { transferICRC1, queryICRC1Fee, isValidPrincipal } from '../../services/transferService';
  import { threePoolService } from '../../services/threePoolService';
  import { formatTokenAmount } from '../../services/threePoolService';
  import QRCode from 'qrcode';

  interface WalletInfo {
    id: string;
    name: string;
    icon?: string;
  }

  interface TokenMeta {
    name: string;
    symbol: string;
    icon: string;
    fallbackColor: string;
    canisterId: string;
    decimals: number;
  }

  // Static token metadata for ICP, icUSD, and ck stablecoins
  const STATIC_TOKEN_META: Record<string, TokenMeta> = {
    ICP:    { name: 'Internet Computer', symbol: 'ICP',    icon: '/icp-token-dark.svg', fallbackColor: '#3B00B9', canisterId: CONFIG.currentIcpLedgerId,   decimals: 8 },
    ICUSD:  { name: 'icUSD',             symbol: 'icUSD',  icon: '/icusd-logo_v3.svg',  fallbackColor: '#8B5CF6', canisterId: CONFIG.currentIcusdLedgerId,  decimals: 8 },
    CKUSDT: { name: 'ckUSDT',            symbol: 'ckUSDT', icon: '',                    fallbackColor: '#26A17B', canisterId: CONFIG.ckusdtLedgerId,        decimals: 6 },
    CKUSDC: { name: 'ckUSDC',            symbol: 'ckUSDC', icon: '',                    fallbackColor: '#2775CA', canisterId: CONFIG.ckusdcLedgerId,        decimals: 6 },
  };

  // Build full TOKEN_META reactively from static entries + collateral store
  $: collateralTokenMeta = (() => {
    const meta: Record<string, TokenMeta> = { ...STATIC_TOKEN_META };
    for (const c of $collateralStore.collaterals) {
      // Skip if we already have static metadata for this symbol
      if (meta[c.symbol]) continue;
      meta[c.symbol] = {
        name: c.symbol,
        symbol: c.symbol,
        icon: '',  // Will be resolved via dynamicLogos
        fallbackColor: c.color || '#94A3B8',
        canisterId: c.ledgerCanisterId,
        decimals: c.decimals,
      };
    }
    return meta;
  })();

  // Dynamically fetched logos (token symbol → data URL)
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

  // ═══ Dropdown view state ═══
  type DropdownView = 'main' | 'send' | 'receive';
  let dropdownView: DropdownView = 'main';

  // ═══ Send form state ═══
  let sendTokenKey = '';
  let sendRecipient = '';
  let sendAmount = '';
  let sendFeeRaw = 0n;
  let sendFeeLoading = false;
  let sending = false;
  let sendError = '';
  let tokenPickerOpen = false;

  // ═══ Receive state ═══
  let qrDataUrl = '';
  let copiedReceive = false;

  // ═══ Fee cache ═══
  const feeCache: Record<string, bigint> = {};

  // ═══ 3USD (LP) balance ═══
  let threeUsdBalance: bigint = 0n;

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
          await Promise.all([walletStore.refreshBalance(), fetchThreeUsdBalance()]);
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
      threeUsdBalance = 0n;
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
      dropdownView = 'main';
    }
  }

  // Fetch logos from ICRC-1 ledger metadata for tokens without local icons
  async function fetchDynamicLogos(tokenMeta: Record<string, TokenMeta>) {
    for (const [key, meta] of Object.entries(tokenMeta)) {
      if (!meta.icon && meta.canisterId && !dynamicLogos[key]) {
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
      fetchThreeUsdBalance();
    }

    // Fetch logos from ledger metadata for tokens without local icons (non-blocking)
    fetchDynamicLogos(STATIC_TOKEN_META);

    return () => {
      document.removeEventListener('click', handleClickOutside);
    };
  });

  async function fetchThreeUsdBalance() {
    const p = $walletStore.principal;
    if (!p) { threeUsdBalance = 0n; return; }
    try {
      threeUsdBalance = await threePoolService.getLpBalance(p);
    } catch { threeUsdBalance = 0n; }
  }

  async function handleRefreshBalance(e: MouseEvent | KeyboardEvent) {
    e.stopPropagation();
    if (isRefreshingBalance) return;

    try {
      isRefreshingBalance = true;
      await Promise.all([walletStore.refreshBalance(), fetchThreeUsdBalance()]);
    } catch (err) {
      console.error('Manual balance refresh failed:', err);
    } finally {
      isRefreshingBalance = false;
    }
  }

  // Re-fetch logos when collateral metadata changes (new tokens discovered)
  $: if (collateralTokenMeta && Object.keys(collateralTokenMeta).length > Object.keys(STATIC_TOKEN_META).length) {
    fetchDynamicLogos(collateralTokenMeta);
  }

  $: isConnected = $walletStore.isConnected;
  $: account = $walletStore.principal?.toString() ?? null;
  $: currentIcon = $walletStore.icon;
  $: tokenBalances = $walletStore.tokenBalances ?? {};
  $: isInternetIdentity = $currentWalletType === WALLET_TYPES.INTERNET_IDENTITY;

  // Compute total USD value
  $: totalUsdValue = Object.values(tokenBalances).reduce((sum, tb) => {
    return sum + (tb?.usdValue ?? 0);
  }, 0);

  // Build active token list: icUSD first, then ICP, then rest by USD value
  $: activeTokens = Object.entries(tokenBalances)
    .filter(([_, tb]) => tb && tb.raw > 0n)
    .map(([key, tb]) => {
      const baseMeta = collateralTokenMeta[key] || { name: key, symbol: key, icon: '', fallbackColor: '#666', canisterId: '', decimals: 8 };
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

  // ═══ Send helpers ═══

  // Selected token's metadata and balance
  $: sendToken = sendTokenKey ? activeTokens.find(t => t.key === sendTokenKey) : null;
  $: sendBalanceRaw = sendToken?.balance.raw ?? 0n;
  $: sendDecimals = sendToken?.meta.decimals ?? 8;
  $: sendCanisterId = sendToken?.meta.canisterId ?? '';
  $: maxSendableRaw = sendBalanceRaw > sendFeeRaw ? sendBalanceRaw - sendFeeRaw : 0n;

  function formatRaw(raw: bigint, decimals: number): string {
    const num = Number(raw) / Math.pow(10, decimals);
    // Show up to `decimals` places, trim trailing zeros
    return num.toFixed(decimals).replace(/\.?0+$/, '') || '0';
  }

  function parseToRaw(input: string, decimals: number): bigint {
    const num = parseFloat(input);
    if (isNaN(num) || num <= 0) return 0n;
    return BigInt(Math.round(num * Math.pow(10, decimals)));
  }

  $: sendAmountRaw = parseToRaw(sendAmount, sendDecimals);
  $: sendIsValid = sendAmountRaw > 0n && sendAmountRaw <= maxSendableRaw && sendRecipient.trim().length > 0;

  async function fetchFee(canisterId: string) {
    if (!canisterId) return;
    if (feeCache[canisterId] !== undefined) {
      sendFeeRaw = feeCache[canisterId];
      return;
    }
    sendFeeLoading = true;
    try {
      const fee = await queryICRC1Fee(canisterId);
      feeCache[canisterId] = fee;
      sendFeeRaw = fee;
    } finally {
      sendFeeLoading = false;
    }
  }

  function openSendView() {
    dropdownView = 'send';
    sendRecipient = '';
    sendAmount = '';
    sendError = '';
    sending = false;
    // Default to first active token
    if (activeTokens.length > 0) {
      sendTokenKey = activeTokens[0].key;
      fetchFee(activeTokens[0].meta.canisterId);
    }
  }

  function selectSendToken(key: string) {
    sendTokenKey = key;
    sendAmount = '';
    sendError = '';
    tokenPickerOpen = false;
    const token = activeTokens.find(t => t.key === key);
    if (token) fetchFee(token.meta.canisterId);
  }

  function setMax() {
    if (maxSendableRaw > 0n) {
      sendAmount = formatRaw(maxSendableRaw, sendDecimals);
    }
  }

  async function handleSend() {
    sendError = '';

    if (!sendRecipient.trim()) { sendError = 'Enter a recipient principal'; return; }
    if (!isValidPrincipal(sendRecipient.trim())) { sendError = 'Invalid principal ID'; return; }
    if (sendAmountRaw <= 0n) { sendError = 'Enter an amount greater than 0'; return; }
    if (sendAmountRaw > maxSendableRaw) {
      sendError = `Max sendable: ${formatRaw(maxSendableRaw, sendDecimals)} ${sendToken?.meta.symbol}`;
      return;
    }
    if (!sendCanisterId) { sendError = 'Token configuration error'; return; }

    sending = true;
    try {
      const result = await transferICRC1(sendCanisterId, sendRecipient.trim(), sendAmountRaw);
      if (result.success) {
        addToast(`Sent ${sendAmount} ${sendToken?.meta.symbol ?? ''} successfully`, 'success');
        walletStore.refreshBalance();
        dropdownView = 'main';
      } else {
        sendError = result.error || 'Transfer failed';
        addToast(sendError, 'error');
      }
    } catch (err) {
      sendError = err instanceof Error ? err.message : 'Transfer failed';
      addToast(sendError, 'error');
    } finally {
      sending = false;
    }
  }

  // ═══ Receive helpers ═══

  async function openReceiveView() {
    dropdownView = 'receive';
    copiedReceive = false;
    qrDataUrl = '';
    if (account) {
      try {
        qrDataUrl = await QRCode.toDataURL(account, {
          width: 180,
          margin: 2,
          color: { dark: '#ffffffdd', light: '#00000000' },
          errorCorrectionLevel: 'M'
        });
      } catch (err) {
        console.error('QR generation failed:', err);
      }
    }
  }

  async function handleCopyReceive(e: MouseEvent) {
    e.stopPropagation();
    if (!account) return;
    const success = await copyToClipboard(account);
    if (success) {
      copiedReceive = true;
      addToast('Principal copied to clipboard', 'success');
      setTimeout(() => { copiedReceive = false; }, 2000);
    } else {
      addToast('Failed to copy', 'error');
    }
  }

  function goBack() {
    dropdownView = 'main';
  }
</script>

<div id="wallet-container">
  {#if !isConnected}
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
        on:click|stopPropagation={() => { showWalletDialog = !showWalletDialog; dropdownView = 'main'; }}
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
          {#if dropdownView === 'main'}
            <!-- ═══ MAIN VIEW ═══ -->
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
              {#if activeTokens.length > 0 || threeUsdBalance > 0n}
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
                {#if threeUsdBalance > 0n}
                  <div class="dropdown-token-row">
                    <div class="dropdown-token-left">
                      <img src="/3pool-logo-v5.svg" alt="3USD" class="dropdown-token-icon" />
                      <div class="dropdown-token-info">
                        <span class="dropdown-token-symbol">3USD</span>
                        <span class="dropdown-token-name">3USD Stablecoin</span>
                      </div>
                    </div>
                    <div class="dropdown-token-right">
                      <span class="dropdown-token-amount">{formatTokenAmount(threeUsdBalance, 18)}</span>
                    </div>
                  </div>
                {/if}
              {:else}
                <div class="dropdown-empty">
                  <p>No balances found</p>
                  <p class="dropdown-empty-hint">Try refreshing your balance</p>
                </div>
              {/if}
            </div>

            <!-- Actions -->
            <div class="dropdown-actions">
              {#if isInternetIdentity}
                <button
                  class="dropdown-action-row dropdown-action-send"
                  on:click|stopPropagation={openSendView}
                >
                  <svg class="action-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <line x1="22" y1="2" x2="11" y2="13" />
                    <polygon points="22 2 15 22 11 13 2 9 22 2" />
                  </svg>
                  <span>Send</span>
                </button>
                <button
                  class="dropdown-action-row dropdown-action-receive"
                  on:click|stopPropagation={openReceiveView}
                >
                  <svg class="action-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="22 12 16 12 14 15 10 15 8 12 2 12" />
                    <path d="M5.45 5.11L2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z" />
                  </svg>
                  <span>Receive</span>
                </button>
              {/if}

              <button
                class="dropdown-action-row dropdown-action-refresh"
                on:click|stopPropagation={handleRefreshBalance}
                disabled={isRefreshingBalance}
              >
                <svg class:animate-spin={isRefreshingBalance} class="action-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
                <span>{isRefreshingBalance ? 'Refreshing...' : 'Refresh Balance'}</span>
              </button>

              <button
                class="dropdown-action-row dropdown-action-disconnect"
                on:click|stopPropagation={disconnectWallet}
              >
                <svg xmlns="http://www.w3.org/2000/svg" class="action-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
                  <polyline points="16 17 21 12 16 7" />
                  <line x1="21" y1="12" x2="9" y2="12" />
                </svg>
                <span>Disconnect</span>
              </button>
            </div>

          {:else if dropdownView === 'send'}
            <!-- ═══ SEND VIEW ═══ -->
            <div class="inline-view">
              <div class="inline-header">
                <button class="back-btn" on:click|stopPropagation={goBack}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="15 18 9 12 15 6"/>
                  </svg>
                </button>
                <span class="inline-title">Send</span>
              </div>

              <div class="inline-body">
                <!-- Token selector (custom) -->
                <div class="field">
                  <label class="field-label">Token</label>
                  <div class="token-picker-wrap">
                    <button
                      class="token-picker-trigger"
                      on:click|stopPropagation={() => { if (!sending) tokenPickerOpen = !tokenPickerOpen; }}
                      disabled={sending}
                    >
                      {#if sendToken}
                        {#if sendToken.meta.icon}
                          <img src={sendToken.meta.icon} alt={sendToken.meta.symbol} class="tp-icon" />
                        {:else}
                          <div class="tp-icon-dot" style="background: {sendToken.meta.fallbackColor}">
                            <span>{sendToken.meta.symbol.charAt(0)}</span>
                          </div>
                        {/if}
                        <span class="tp-symbol">{sendToken.meta.symbol}</span>
                      {:else}
                        <span class="tp-symbol tp-placeholder">Select token</span>
                      {/if}
                      <svg class="tp-chevron" class:tp-chevron-open={tokenPickerOpen} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="6 9 12 15 18 9"/></svg>
                    </button>
                    {#if tokenPickerOpen}
                      <div class="token-picker-list">
                        {#each activeTokens as token (token.key)}
                          <button
                            class="token-picker-item"
                            class:token-picker-item-active={token.key === sendTokenKey}
                            on:click|stopPropagation={() => selectSendToken(token.key)}
                          >
                            {#if token.meta.icon}
                              <img src={token.meta.icon} alt={token.meta.symbol} class="tp-icon" />
                            {:else}
                              <div class="tp-icon-dot" style="background: {token.meta.fallbackColor}">
                                <span>{token.meta.symbol.charAt(0)}</span>
                              </div>
                            {/if}
                            <span class="tp-symbol">{token.meta.symbol}</span>
                          </button>
                        {/each}
                      </div>
                    {/if}
                  </div>
                </div>

                <!-- Recipient -->
                <div class="field">
                  <label class="field-label" for="send-recipient">To</label>
                  <input
                    id="send-recipient"
                    class="field-input"
                    type="text"
                    placeholder="Recipient principal"
                    bind:value={sendRecipient}
                    disabled={sending}
                  />
                </div>

                <!-- Amount -->
                <div class="field">
                  <label class="field-label" for="send-amount">Amount</label>
                  <div class="amount-row">
                    <input
                      id="send-amount"
                      class="field-input amount-input"
                      type="number"
                      step="any"
                      min="0"
                      placeholder="0.00"
                      bind:value={sendAmount}
                      disabled={sending}
                    />
                    <button class="max-btn" on:click|stopPropagation={setMax} disabled={sending || maxSendableRaw <= 0n}>MAX</button>
                  </div>
                  <div class="field-meta">
                    <span>Bal: {formatRaw(sendBalanceRaw, sendDecimals)}</span>
                    <span>Fee: {sendFeeLoading ? '...' : formatRaw(sendFeeRaw, sendDecimals)}</span>
                  </div>
                </div>

                {#if sendError}
                  <div class="error-box">{sendError}</div>
                {/if}

                <button
                  class="send-btn"
                  on:click|stopPropagation={handleSend}
                  disabled={sending || !sendIsValid}
                >
                  {#if sending}
                    <div class="spinner"></div>
                    Sending...
                  {:else}
                    Send {sendToken?.meta.symbol ?? ''}
                  {/if}
                </button>
              </div>
            </div>

          {:else if dropdownView === 'receive'}
            <!-- ═══ RECEIVE VIEW ═══ -->
            <div class="inline-view">
              <div class="inline-header">
                <button class="back-btn" on:click|stopPropagation={goBack}>
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="15 18 9 12 15 6"/>
                  </svg>
                </button>
                <span class="inline-title">Receive</span>
              </div>

              <div class="inline-body receive-body">
                <!-- QR Code -->
                {#if qrDataUrl}
                  <div class="qr-container">
                    <img src={qrDataUrl} alt="QR code for principal" class="qr-image" />
                  </div>
                {:else}
                  <div class="qr-placeholder">
                    <div class="qr-spinner"></div>
                  </div>
                {/if}

                <!-- Principal -->
                <div class="principal-box">
                  <code class="principal-text">{account ?? ''}</code>
                </div>

                <p class="receive-hint">Use this address to receive tokens.</p>

                <!-- Copy button -->
                <button class="copy-btn" on:click|stopPropagation={handleCopyReceive}>
                  {#if copiedReceive}
                    <svg class="btn-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                      <path d="M20 6L9 17l-5-5" />
                    </svg>
                    Copied!
                  {:else}
                    <svg class="btn-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                      <rect x="9" y="9" width="13" height="13" rx="2" />
                      <path d="M5 15H4a2 2 0 01-2-2V4a2 2 0 012-2h9a2 2 0 012 2v1" />
                    </svg>
                    Copy Principal
                  {/if}
                </button>
              </div>
            </div>
          {/if}
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
  .action-icon {
    width: 1rem;
    height: 1rem;
    flex-shrink: 0;
  }
  .dropdown-action-send,
  .dropdown-action-receive {
    color: rgba(167, 139, 250, 0.85);
  }
  .dropdown-action-send:hover,
  .dropdown-action-receive:hover {
    background: rgba(139, 92, 246, 0.08);
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

  /* ═══ Inline view (Send / Receive) ═══ */
  .inline-view {
    display: flex;
    flex-direction: column;
  }
  .inline-header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.75rem 1rem;
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
  }
  .back-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 1.5rem;
    height: 1.5rem;
    background: transparent;
    border: none;
    border-radius: 0.25rem;
    cursor: pointer;
    color: rgba(255, 255, 255, 0.5);
    transition: color 0.12s ease, background 0.12s ease;
    padding: 0;
  }
  .back-btn:hover {
    color: rgba(255, 255, 255, 0.9);
    background: rgba(255, 255, 255, 0.05);
  }
  .back-btn svg {
    width: 1rem;
    height: 1rem;
  }
  .inline-title {
    font-size: 0.9rem;
    font-weight: 600;
    color: white;
  }
  .inline-body {
    padding: 0.75rem 1rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.625rem;
  }

  /* ── Send form fields ── */
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .field-label {
    color: rgba(255, 255, 255, 0.5);
    font-size: 0.7rem;
    font-weight: 500;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .field-input {
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 0.375rem;
    padding: 0.5rem 0.625rem;
    color: white;
    font-size: 0.8rem;
    width: 100%;
    outline: none;
    transition: border-color 0.15s ease;
  }
  .field-input:focus {
    border-color: rgba(139, 92, 246, 0.5);
  }
  .field-input::placeholder {
    color: rgba(255, 255, 255, 0.2);
  }
  .field-input:disabled {
    opacity: 0.5;
  }

  /* ── Custom token picker ── */
  .token-picker-wrap {
    position: relative;
  }
  .token-picker-trigger {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    padding: 0.45rem 0.625rem;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 0.375rem;
    cursor: pointer;
    transition: border-color 0.15s ease;
    color: white;
  }
  .token-picker-trigger:hover:not(:disabled) {
    border-color: rgba(139, 92, 246, 0.35);
  }
  .token-picker-trigger:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .tp-icon {
    width: 1.25rem;
    height: 1.25rem;
    border-radius: 50%;
    flex-shrink: 0;
    object-fit: cover;
  }
  .tp-icon-dot {
    width: 1.25rem;
    height: 1.25rem;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    font-size: 0.6rem;
    font-weight: 700;
    color: white;
  }
  .tp-symbol {
    font-size: 0.8rem;
    font-weight: 600;
    flex: 1;
    text-align: left;
  }
  .tp-placeholder {
    color: rgba(255, 255, 255, 0.3);
    font-weight: 400;
  }
  .tp-chevron {
    width: 0.75rem;
    height: 0.75rem;
    color: rgba(255, 255, 255, 0.35);
    flex-shrink: 0;
    transition: transform 0.15s ease;
  }
  .tp-chevron-open {
    transform: rotate(180deg);
  }
  .token-picker-list {
    position: absolute;
    top: calc(100% + 0.25rem);
    left: 0;
    right: 0;
    background: rgba(18, 18, 30, 0.98);
    border: 1px solid rgba(139, 92, 246, 0.15);
    border-radius: 0.375rem;
    z-index: 10;
    max-height: 10rem;
    overflow-y: auto;
    box-shadow: 0 8px 20px -4px rgba(0, 0, 0, 0.5);
  }
  .token-picker-item {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    padding: 0.45rem 0.625rem;
    background: transparent;
    border: none;
    cursor: pointer;
    color: white;
    transition: background 0.1s ease;
  }
  .token-picker-item:hover {
    background: rgba(139, 92, 246, 0.1);
  }
  .token-picker-item-active {
    background: rgba(139, 92, 246, 0.15);
  }

  .amount-row {
    display: flex;
    gap: 0.375rem;
  }
  .amount-input {
    flex: 1;
  }
  .amount-input::-webkit-outer-spin-button,
  .amount-input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }
  .amount-input[type=number] {
    -moz-appearance: textfield;
  }

  .max-btn {
    padding: 0.5rem 0.625rem;
    background: rgba(255, 255, 255, 0.06);
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 0.375rem;
    color: rgba(139, 92, 246, 0.9);
    font-size: 0.65rem;
    font-weight: 700;
    letter-spacing: 0.03em;
    cursor: pointer;
    transition: all 0.15s ease;
  }
  .max-btn:hover:not(:disabled) {
    background: rgba(139, 92, 246, 0.12);
  }
  .max-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .field-meta {
    display: flex;
    justify-content: space-between;
    color: rgba(255, 255, 255, 0.35);
    font-size: 0.65rem;
  }

  .error-box {
    background: rgba(224, 107, 159, 0.12);
    border: 1px solid rgba(224, 107, 159, 0.25);
    border-radius: 0.375rem;
    padding: 0.4rem 0.625rem;
    color: #e881a8;
    font-size: 0.75rem;
  }

  .send-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    width: 100%;
    padding: 0.6rem;
    background: linear-gradient(135deg, #7c3aed, #6d28d9);
    border: none;
    border-radius: 0.375rem;
    color: white;
    font-size: 0.8rem;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s ease;
  }
  .send-btn:hover:not(:disabled) {
    background: linear-gradient(135deg, #8b5cf6, #7c3aed);
  }
  .send-btn:disabled {
    opacity: 0.35;
    cursor: not-allowed;
  }

  .spinner {
    width: 0.875rem;
    height: 0.875rem;
    border: 2px solid rgba(255, 255, 255, 0.3);
    border-top-color: white;
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
  }

  /* ═══ Receive view ═══ */
  .receive-body {
    align-items: center;
  }

  .qr-container {
    padding: 0.75rem;
    background: rgba(255, 255, 255, 0.04);
    border-radius: 0.625rem;
    border: 1px solid rgba(255, 255, 255, 0.06);
  }
  .qr-image {
    width: 160px;
    height: 160px;
    display: block;
    image-rendering: pixelated;
  }
  .qr-placeholder {
    width: 160px;
    height: 160px;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .qr-spinner {
    width: 1.25rem;
    height: 1.25rem;
    border: 2px solid rgba(255, 255, 255, 0.15);
    border-top-color: rgba(139, 92, 246, 0.6);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  .principal-box {
    width: 100%;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 0.375rem;
    padding: 0.5rem 0.625rem;
    word-break: break-all;
    text-align: center;
  }
  .principal-text {
    color: rgba(255, 255, 255, 0.65);
    font-size: 0.65rem;
    line-height: 1.5;
    letter-spacing: 0.01em;
  }
  .receive-hint {
    color: rgba(255, 255, 255, 0.35);
    font-size: 0.7rem;
    margin: 0;
  }
  .copy-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.4rem;
    width: 100%;
    padding: 0.55rem;
    background: rgba(139, 92, 246, 0.2);
    border: 1px solid rgba(139, 92, 246, 0.25);
    border-radius: 0.375rem;
    color: white;
    font-size: 0.8rem;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }
  .copy-btn:hover {
    background: rgba(139, 92, 246, 0.35);
  }
  .btn-icon {
    width: 0.875rem;
    height: 0.875rem;
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

  @keyframes spin {
    to { transform: rotate(360deg); }
  }
</style>
