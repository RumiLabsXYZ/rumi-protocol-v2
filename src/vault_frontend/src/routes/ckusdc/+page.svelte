<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import { isConnected as isConnectedStore, principal as principalStore } from '$lib/stores/wallet';
  import { CANISTER_IDS } from '$lib/config';
  import CkErc20TokenSelect from '$lib/components/common/CkErc20TokenSelect.svelte';
  import { ckErc20Logo, featuredCkErc20Symbols } from '$lib/utils/ckerc20Logos';
  import {
    CKERC20_MINTER_DASHBOARD,
    discoverCkErc20Tokens,
    approveAndWithdrawCkErc20,
    assertTokenSupported,
    encodeAddressWord,
    encodeDepositErc20,
    encodeUint256,
    formatTokenAmount,
    getCkErc20LedgerActor,
    getCkErc20MinterActor,
    getCkErc20WithdrawalQuote,
    parseTokenAmount,
    validateEthereumAddress,
    type CkErc20TokenConfig,
  } from '$lib/services/ckerc20Minter';

  type Eip1193Provider = {
    request(args: { method: string; params?: unknown[] }): Promise<any>;
    on?: (event: string, listener: (...args: any[]) => void) => void;
    removeListener?: (event: string, listener: (...args: any[]) => void) => void;
  };

  function getEthereumProvider(): Eip1193Provider | undefined {
    return (window as Window & { ethereum?: Eip1193Provider }).ethereum;
  }

  let connected = false;
  let ownerPrincipal: Principal | null = null;
  let activeTab: 'mint' | 'redeem' = 'mint';
  let supportedTokens: CkErc20TokenConfig[] = [];
  let selectedTokenLedgerId = '';
  let selectedToken: CkErc20TokenConfig | null = null;
  $: selectedToken = supportedTokens.find((token) => token.ledgerId === selectedTokenLedgerId) ?? null;
  let evmAccount = '';
  let evmConnectBusy = false;
  let evmManuallyDisconnected = false;
  let evmSessionEpoch = 0;
  const EVM_DISCONNECT_KEY = 'rumi:ckerc20:evm-disconnected';
  let evmMessage = '';
  let evmTokenBalance: bigint | null = null;
  let evmTokenBalanceBusy = false;
  let evmTokenBalanceError = '';
  let evmTokenBalanceRequestId = 0;
  let minterReady = false;
  let minterError = '';
  let helperAddress = '';
  let ckTokenBalance: bigint | null = null;
  let ckEthBalance: bigint | null = null;
  let refreshBusy = false;
  let depositAmount = '';
  let redeemAmount = '';
  let redeemAddress = '';
  let withdrawalQuote: Awaited<ReturnType<typeof getCkErc20WithdrawalQuote>> | null = null;
  let quoteBusy = false;
  let busy = false;
  let notice = '';
  let error = '';
  let approveHash = '';
  let depositHash = '';
  let pendingDeposit: { hash: string; amount: string; recipient: string; evmAccount: string; principal: string; tokenLedgerId?: string; tokenSymbol?: string; createdAt: number } | null = null;
  let pendingWithdrawal: { amount: string; recipient: string; owner: string; tokenLedgerId?: string; tokenSymbol?: string; createdAt: number } | null = null;
  let redeemIndices: { ckEth: bigint; ckToken: bigint } | null = null;
  let destroyed = false;
  let tokenSupply: bigint | null = null;
  let ckEthSupply: bigint | null = null;
  let supplyBusy = false;
  let supplyError = '';
  let supplyUpdatedAt: Date | null = null;
  let supplyRequestId = 0;
  $: featuredTokens = supportedTokens.filter((token) => featuredCkErc20Symbols.includes(token.symbol));
  $: otherTokens = supportedTokens.filter((token) => !featuredCkErc20Symbols.includes(token.symbol));
  $: withdrawalPendingForOwner = !!pendingWithdrawal && !!ownerPrincipal && pendingWithdrawal.owner === ownerPrincipal.toText();

  const unsubs: Array<() => void> = [];
  let previousPrincipal = '';
  let ethereumProvider: Eip1193Provider | undefined;
  let accountChangedListener: ((accounts: unknown) => void) | null = null;
  let chainChangedListener: ((chainId: unknown) => void) | null = null;

  function depositStorageKey(evm: string, principal: string, ledgerId: string) {
    return `rumi:ckerc20:pending-deposit:${evm.toLowerCase()}:${principal}:${ledgerId}`;
  }

  function legacyCkUsdcDepositStorageKey(evm: string, principal: string) {
    return `rumi:ckusdc:pending-deposit:${evm.toLowerCase()}:${principal}`;
  }

  function requirePersistentOperationState() {
    const key = `rumi:ckusdc:storage-check:${Date.now()}`;
    localStorage.setItem(key, 'ready');
    localStorage.removeItem(key);
  }

  function syncPendingDeposit(token = selectedToken) {
    pendingDeposit = null;
    if (!evmAccount || !ownerPrincipal || ownerPrincipal.isAnonymous() || !token) return;
    try {
      const value = localStorage.getItem(depositStorageKey(evmAccount, ownerPrincipal.toText(), token.ledgerId)) ??
        (token.ledgerId === CANISTER_IDS.CKUSDC_LEDGER
          ? localStorage.getItem(legacyCkUsdcDepositStorageKey(evmAccount, ownerPrincipal.toText()))
          : null);
      if (value) {
        const pending = JSON.parse(value);
        pendingDeposit = { ...pending, tokenLedgerId: pending.tokenLedgerId ?? token.ledgerId, tokenSymbol: pending.tokenSymbol ?? token.symbol };
      }
    } catch { /* Storage may be unavailable; the in-memory guard still applies. */ }
  }

  function savePendingDeposit(hash: string, amount: string, recipient: string, evm: string, principal: string, token: CkErc20TokenConfig) {
    pendingDeposit = { hash, amount, recipient, evmAccount: evm, principal, tokenLedgerId: token.ledgerId, tokenSymbol: token.symbol, createdAt: Date.now() };
    try { localStorage.setItem(depositStorageKey(evm, principal, token.ledgerId), JSON.stringify(pendingDeposit)); } catch { /* The unresolved intent written before wallet dispatch remains as a conservative lock. */ }
  }

  function beginPendingDeposit(amount: string, recipient: string, evm: string, principal: string, token: CkErc20TokenConfig) {
    const pending = { hash: '', amount, recipient, evmAccount: evm, principal, tokenLedgerId: token.ledgerId, tokenSymbol: token.symbol, createdAt: Date.now() };
    localStorage.setItem(depositStorageKey(evm, principal, token.ledgerId), JSON.stringify(pending));
    pendingDeposit = pending;
  }

  function clearPendingDepositMarker(evm: string, principal: string, token: CkErc20TokenConfig) {
    try {
      localStorage.removeItem(depositStorageKey(evm, principal, token.ledgerId));
      if (token.ledgerId === CANISTER_IDS.CKUSDC_LEDGER) localStorage.removeItem(legacyCkUsdcDepositStorageKey(evm, principal));
    } catch { /* Keep a stale persistent lock rather than risk a retry. */ }
    if (pendingDeposit?.evmAccount.toLowerCase() === evm.toLowerCase() && pendingDeposit?.principal === principal) pendingDeposit = null;
  }

  function clearPendingDeposit() {
    if (!pendingDeposit) return;
    const confirmed = window.confirm('Only clear this retry lock after checking Ethereum wallet activity and confirming the deposit did not complete. Clearing it while the transaction is pending could allow a duplicate deposit.');
    if (!confirmed) return;
    const token = supportedTokens.find((candidate) => candidate.ledgerId === pendingDeposit?.tokenLedgerId);
    if (!token) return;
    clearPendingDepositMarker(pendingDeposit.evmAccount, pendingDeposit.principal, token);
  }

  function withdrawalStorageKey(owner: string) {
    return `rumi:ckerc20:pending-withdrawal:${owner}`;
  }

  function syncPendingWithdrawal(owner: Principal | null) {
    pendingWithdrawal = null;
    if (!owner || owner.isAnonymous()) return;
    try {
      const value = localStorage.getItem(withdrawalStorageKey(owner.toText())) ?? localStorage.getItem(`rumi:ckusdc:pending-withdrawal:${owner.toText()}`);
      if (value) {
        const pending = JSON.parse(value);
        pendingWithdrawal = { ...pending, tokenLedgerId: pending.tokenLedgerId ?? CANISTER_IDS.CKUSDC_LEDGER, tokenSymbol: pending.tokenSymbol ?? 'ckUSDC' };
      }
    } catch { /* Storage may be unavailable; the in-memory guard still applies. */ }
  }

  function markWithdrawalPending(amount: bigint, recipient: string, owner: Principal, token: CkErc20TokenConfig) {
    const pending = { amount: amount.toString(), recipient, owner: owner.toText(), tokenLedgerId: token.ledgerId, tokenSymbol: token.symbol, createdAt: Date.now() };
    localStorage.setItem(withdrawalStorageKey(owner.toText()), JSON.stringify(pending));
    pendingWithdrawal = pending;
  }

  function clearPendingWithdrawal() {
    if (!pendingWithdrawal) return;
    const confirmed = window.confirm(`Only clear this retry lock after checking the ${pendingWithdrawal.tokenSymbol ?? 'ckUSDC'} and ckETH ledger history and the minter dashboard to confirm the request was not accepted. Clearing it without reconciliation could burn the amount twice.`);
    if (!confirmed) return;
    try {
      localStorage.removeItem(withdrawalStorageKey(pendingWithdrawal.owner));
      localStorage.removeItem(`rumi:ckusdc:pending-withdrawal:${pendingWithdrawal.owner}`);
    } catch { /* The current page lock is still cleared below. */ }
    pendingWithdrawal = null;
  }

  function resetForWallet() {
    ckTokenBalance = null;
    ckEthBalance = null;
    notice = '';
    error = '';
    approveHash = '';
    depositHash = '';
    redeemIndices = null;
    withdrawalQuote = null;
  }

  onMount(() => {
    try { evmManuallyDisconnected = sessionStorage.getItem(EVM_DISCONNECT_KEY) === 'true'; } catch { /* In-memory disconnect still works. */ }
    unsubs.push(isConnectedStore.subscribe((value) => {
      connected = value;
      if (!value) resetForWallet();
      else if (ownerPrincipal && !ownerPrincipal.isAnonymous()) void refreshBalances(ownerPrincipal);
    }));
    unsubs.push(principalStore.subscribe((value) => {
      ownerPrincipal = value;
      syncPendingWithdrawal(value);
      syncPendingDeposit();
      const key = value?.toText() ?? '';
      if (key !== previousPrincipal) {
        previousPrincipal = key;
        resetForWallet();
      }
      if (value && !value.isAnonymous()) void refreshBalances(value);
    }));
    ethereumProvider = getEthereumProvider();
    accountChangedListener = (accounts) => {
      if (evmManuallyDisconnected) return;
      evmTokenBalanceRequestId += 1;
      evmTokenBalanceBusy = false;
      evmAccount = Array.isArray(accounts) && accounts[0] ? String(accounts[0]) : '';
      syncPendingDeposit();
      evmTokenBalance = null;
      evmTokenBalanceError = '';
      evmMessage = '';
      if (evmAccount) void refreshEvmTokenBalance(evmAccount);
    };
    chainChangedListener = (chainId) => {
      if (evmManuallyDisconnected) return;
      evmTokenBalanceRequestId += 1;
      evmTokenBalanceBusy = false;
      evmTokenBalance = null;
      evmTokenBalanceError = '';
      if (chainId === '0x1') {
        evmMessage = '';
        if (evmAccount) void refreshEvmTokenBalance(evmAccount);
      } else {
        evmMessage = 'Choose Ethereum Mainnet in your EVM wallet to read your token balance and deposit.';
      }
    };
    ethereumProvider?.on?.('accountsChanged', accountChangedListener);
    ethereumProvider?.on?.('chainChanged', chainChangedListener);
    void loadMinterInfo();
    void (async () => {
      const sessionEpoch = evmSessionEpoch;
      if (evmManuallyDisconnected) return;
      try {
        const [accounts, chainId] = await Promise.all([
          ethereumProvider?.request({ method: 'eth_accounts' }),
          ethereumProvider?.request({ method: 'eth_chainId' }),
        ]);
        if (destroyed || evmManuallyDisconnected || sessionEpoch !== evmSessionEpoch) return;
        evmAccount = Array.isArray(accounts) && accounts[0] ? String(accounts[0]) : '';
        syncPendingDeposit();
        if (chainId === '0x1') {
          evmMessage = '';
          if (evmAccount) await refreshEvmTokenBalance(evmAccount);
        } else if (evmAccount) {
          evmMessage = 'Choose Ethereum Mainnet in your EVM wallet to read your token balance and deposit.';
        }
      } catch { /* A missing or locked EVM wallet is handled when the user connects. */ }
    })();
  });

  onDestroy(() => {
    destroyed = true;
    unsubs.forEach((unsubscribe) => unsubscribe());
    if (accountChangedListener) ethereumProvider?.removeListener?.('accountsChanged', accountChangedListener);
    if (chainChangedListener) ethereumProvider?.removeListener?.('chainChanged', chainChangedListener);
  });

  async function refreshEvmTokenBalance(account = evmAccount, token = selectedToken) {
    const provider = getEthereumProvider();
    if (!provider || !account || !token) return;
    const requestId = ++evmTokenBalanceRequestId;
    evmTokenBalanceBusy = true;
    evmTokenBalanceError = '';
    try {
      const chainId = await provider.request({ method: 'eth_chainId' });
      if (chainId !== '0x1') {
        if (requestId === evmTokenBalanceRequestId) {
          evmTokenBalance = null;
          evmMessage = 'Choose Ethereum Mainnet in your EVM wallet to read your token balance and deposit.';
        }
        return;
      }
      const decimalsHex = await provider.request({ method: 'eth_call', params: [{ to: token.erc20Address, data: '0x313ce567' }, 'latest'] });
      if (Number(BigInt(String(decimalsHex))) !== token.decimals) {
        throw new Error(`${token.symbol} Ethereum and ckERC20 ledgers report different decimal counts.`);
      }
      const data = `0x70a08231${encodeAddressWord(account)}`;
      const rawBalance = await provider.request({ method: 'eth_call', params: [{ to: token.erc20Address, data }, 'latest'] });
      if (typeof rawBalance !== 'string' || !/^0x[0-9a-fA-F]+$/.test(rawBalance)) {
        throw new Error(`The Ethereum wallet returned an invalid ${token.symbol} balance.`);
      }
      if (!destroyed && requestId === evmTokenBalanceRequestId && evmAccount.toLowerCase() === account.toLowerCase() && selectedTokenLedgerId === token.ledgerId) {
        evmTokenBalance = BigInt(rawBalance);
        evmMessage = '';
      }
    } catch (cause) {
      if (!destroyed && requestId === evmTokenBalanceRequestId) {
        evmTokenBalance = null;
        evmTokenBalanceError = cause instanceof Error ? cause.message : `Could not read ${token.symbol} balance from Ethereum.`;
      }
    } finally {
      if (requestId === evmTokenBalanceRequestId) evmTokenBalanceBusy = false;
    }
  }

  function setMaxDeposit() {
    if (evmTokenBalance === null || evmTokenBalance <= 0n || !selectedToken) return;
    depositAmount = formatTokenAmount(evmTokenBalance, selectedToken.decimals, selectedToken.decimals);
  }

  async function selectToken(ledgerId: string) {
    const token = supportedTokens.find((candidate) => candidate.ledgerId === ledgerId);
    if (!token || busy || quoteBusy || refreshBusy) return;
    selectedTokenLedgerId = token.ledgerId;
    depositAmount = '';
    redeemAmount = '';
    withdrawalQuote = null;
    redeemIndices = null;
    evmTokenBalance = null;
    evmTokenBalanceError = '';
    ckTokenBalance = null;
    notice = '';
    error = '';
    approveHash = '';
    depositHash = '';
    syncPendingDeposit(token);
    void refreshCirculatingSupply(token);
    if (evmAccount) void refreshEvmTokenBalance(evmAccount, token);
    if (ownerPrincipal && !ownerPrincipal.isAnonymous()) await refreshBalances(ownerPrincipal, token);
  }

  async function loadMinterInfo() {
    minterError = '';
    minterReady = false;
    try {
      const actor = await getCkErc20MinterActor();
      const info = await actor.get_minter_info();
      const tokens = await discoverCkErc20Tokens(info);
      if (destroyed) return;
      const helper = info.deposit_with_subaccount_helper_contract_address?.[0];
      if (!helper || !/^0x[0-9a-fA-F]{40}$/.test(helper)) throw new Error('The minter did not return a valid live Ethereum helper address.');
      const previousToken = selectedTokenLedgerId;
      supportedTokens = tokens;
      selectedTokenLedgerId = tokens.some((token) => token.ledgerId === previousToken)
        ? previousToken
        : (tokens.find((token) => token.symbol === 'ckUSDC') ?? tokens[0]).ledgerId;
      helperAddress = helper;
      minterReady = true;
      const token = tokens.find((candidate) => candidate.ledgerId === selectedTokenLedgerId)!;
      syncPendingDeposit(token);
      void refreshCirculatingSupply(token);
      if (evmAccount) void refreshEvmTokenBalance(evmAccount, token);
      if (ownerPrincipal && !ownerPrincipal.isAnonymous()) await refreshBalances(ownerPrincipal, token);
    } catch (cause) {
      if (destroyed) return;
      minterError = cause instanceof Error ? cause.message : 'Could not read ckERC20 minter configuration.';
    }
  }

  async function refreshCirculatingSupply(token = selectedToken) {
    if (!token) return;
    const requestId = ++supplyRequestId;
    tokenSupply = null;
    ckEthSupply = null;
    supplyUpdatedAt = null;
    supplyError = '';
    supplyBusy = true;
    try {
      const [tokenResult, ethResult] = await Promise.allSettled([
        getCkErc20LedgerActor(token.ledgerId).then((ledger) => ledger.icrc1_total_supply()),
        getCkErc20LedgerActor(CANISTER_IDS.CKETH_LEDGER).then((ledger) => ledger.icrc1_total_supply()),
      ]);
      if (destroyed || requestId !== supplyRequestId || selectedTokenLedgerId !== token.ledgerId) return;
      if (tokenResult.status === 'fulfilled') tokenSupply = BigInt(tokenResult.value);
      if (ethResult.status === 'fulfilled') ckEthSupply = BigInt(ethResult.value);
      if (tokenResult.status === 'rejected' || ethResult.status === 'rejected') supplyError = 'Some supply data is unavailable. Try refreshing.';
      supplyUpdatedAt = new Date();
    } catch { if (!destroyed && requestId === supplyRequestId) supplyError = 'Supply data is unavailable. Try refreshing.'; }
    finally { if (requestId === supplyRequestId) supplyBusy = false; }
  }

  function formatSupply(amount: bigint, decimals: number): string {
    const [whole, fraction] = formatTokenAmount(amount, decimals, 4).split('.');
    return whole.replace(/\B(?=(\d{3})+(?!\d))/g, ',') + (fraction ? `.${fraction}` : '');
  }

  async function refreshBalances(principal = ownerPrincipal, token = selectedToken) {
    if (!principal || principal.isAnonymous() || !token || refreshBusy) return;
    refreshBusy = true;
    try {
      const [tokenLedger, ethLedger] = await Promise.all([
        getCkErc20LedgerActor(token.ledgerId),
        getCkErc20LedgerActor(CANISTER_IDS.CKETH_LEDGER),
      ]);
      const [tokenBalance, eth] = await Promise.all([
        tokenLedger.icrc1_balance_of({ owner: principal, subaccount: [] }),
        ethLedger.icrc1_balance_of({ owner: principal, subaccount: [] }),
      ]);
      if (destroyed || ownerPrincipal?.toText() !== principal.toText() || selectedTokenLedgerId !== token.ledgerId) return;
      ckTokenBalance = BigInt(tokenBalance);
      ckEthBalance = BigInt(eth);
    } catch (cause) {
      error = cause instanceof Error ? cause.message : 'Could not refresh ledger balances.';
    } finally {
      refreshBusy = false;
    }
  }

  async function connectEthereum() {
    if (busy || evmConnectBusy) return;
    error = '';
    evmMessage = '';
    const provider = getEthereumProvider();
    if (!provider) {
      evmMessage = 'Install or unlock an EVM wallet such as Rabby or MetaMask to deposit.';
      return;
    }
    const sessionEpoch = ++evmSessionEpoch;
    evmConnectBusy = true;
    try {
      const accounts = await provider.request({ method: 'eth_requestAccounts' });
      if (!Array.isArray(accounts) || !accounts[0]) throw new Error('The EVM wallet did not return an account.');
      const chainId = await provider.request({ method: 'eth_chainId' });
      if (destroyed || sessionEpoch !== evmSessionEpoch) return;
      if (chainId !== '0x1') {
        evmMessage = 'Switch your EVM wallet to Ethereum Mainnet, then connect again.';
        return;
      }
      evmAccount = String(accounts[0]);
      evmManuallyDisconnected = false;
      try { sessionStorage.removeItem(EVM_DISCONNECT_KEY); } catch { /* In-memory connection still works. */ }
      syncPendingDeposit();
      void refreshEvmTokenBalance(evmAccount);
    } catch (cause) {
      evmMessage = cause instanceof Error ? cause.message : 'EVM wallet connection was not completed.';
    } finally { if (sessionEpoch === evmSessionEpoch) evmConnectBusy = false; }
  }

  function disconnectEthereum() {
    if (busy || evmConnectBusy) return;
    evmManuallyDisconnected = true;
    evmSessionEpoch += 1;
    evmTokenBalanceRequestId += 1;
    evmAccount = '';
    evmTokenBalance = null;
    evmTokenBalanceBusy = false;
    evmTokenBalanceError = '';
    evmMessage = '';
    approveHash = '';
    depositHash = '';
    pendingDeposit = null;
    try { sessionStorage.setItem(EVM_DISCONNECT_KEY, 'true'); } catch { /* In-memory disconnect still works. */ }
    // Keep persistent transaction markers so reconnecting restores unresolved operations.
  }

  async function waitForReceipt(hash: string, onReceipt?: () => void) {
    const provider = getEthereumProvider();
    if (!provider) throw new Error('EVM wallet disconnected while waiting for transaction confirmation.');
    const deadline = Date.now() + 180_000;
    while (Date.now() < deadline) {
      const receipt = await provider.request({ method: 'eth_getTransactionReceipt', params: [hash] });
      if (receipt) {
        onReceipt?.();
        if (BigInt(receipt.status) !== 1n) throw new Error(`Ethereum transaction failed: ${hash}`);
        return receipt;
      }
      await new Promise((resolve) => setTimeout(resolve, 3000));
      if (destroyed) throw new Error(`Transaction is still pending: ${hash}`);
    }
    throw new Error(`Transaction was submitted but is still unconfirmed. Check its status before retrying: ${hash}`);
  }

  async function submitEthereumTransaction(
    to: string,
    data: string,
    onSubmitted: (hash: string) => void,
    onReceipt?: () => void,
    onSubmitting?: () => void,
    onRejected?: () => void,
  ): Promise<string> {
    const provider = getEthereumProvider();
    if (!provider || !evmAccount) throw new Error('Connect an Ethereum wallet first.');
    const chainId = await provider.request({ method: 'eth_chainId' });
    if (chainId !== '0x1') throw new Error('Switch your EVM wallet to Ethereum Mainnet before submitting.');
    const accounts = await provider.request({ method: 'eth_accounts' });
    if (!Array.isArray(accounts) || String(accounts[0]).toLowerCase() !== evmAccount.toLowerCase()) {
      throw new Error('The selected Ethereum account changed. Reconnect it and review the recipient identity before submitting.');
    }
    onSubmitting?.();
    let response: any;
    try {
      response = await provider.request({ method: 'eth_sendTransaction', params: [{ from: evmAccount, to, data, value: '0x0' }] });
    } catch (cause) {
      if ((cause as any)?.code === 4001) onRejected?.();
      throw cause;
    }
    const hash = String(response);
    if (!/^0x[0-9a-f]{64}$/i.test(hash)) throw new Error('The wallet did not return a valid transaction hash. The submission remains locked until you reconcile Ethereum wallet activity.');
    onSubmitted(hash);
    await waitForReceipt(hash, onReceipt);
    return hash;
  }

  async function submitDeposit() {
    error = '';
    notice = '';
    approveHash = '';
    depositHash = '';
    const token = selectedToken;
    if (!token) { error = 'Choose a supported ckERC20 token first.'; return; }
    if (pendingDeposit) { error = pendingDeposit.hash ? `A deposit transaction is still unresolved: ${pendingDeposit.hash}. Check its status before starting another deposit.` : 'A prior deposit submission has no confirmed result. Check Ethereum wallet activity before retrying.'; return; }
    if (!connected || !ownerPrincipal || ownerPrincipal.isAnonymous()) {
      error = 'Connect an Internet Identity or another Rumi wallet first. This wallet receives the selected ckERC20 token.';
      return;
    }
    if (!minterReady || !helperAddress) { error = minterError || 'The live ckERC20 minter configuration is not ready.'; return; }
    if (!evmAccount) { evmMessage = 'Connect an Ethereum wallet to pay for the token approval and deposit transactions.'; return; }
    let amount: bigint;
    try { amount = parseTokenAmount(depositAmount, token.decimals); }
    catch (cause) { error = cause instanceof Error ? cause.message : 'Invalid amount.'; return; }
    if (token.minimumDepositAmount === null) { error = `The minter has not provided a minimum deposit for ${token.symbol}; deposits for this token are disabled.`; return; }
    if (amount < token.minimumDepositAmount) {
      error = `The minimum ${token.symbol} deposit is ${formatTokenAmount(token.minimumDepositAmount, token.decimals)} ${token.symbol}.`;
      return;
    }
    const liveOwner = ownerPrincipal;
    const locks = (navigator as any).locks;
    if (!locks?.request) { error = 'This browser cannot safely coordinate minter transactions across tabs. Use a supported browser with Web Locks enabled.'; return; }
    busy = true;
    try {
      await locks.request(`rumi:ckerc20:deposit:${evmAccount.toLowerCase()}:${token.ledgerId}`, { mode: 'exclusive', ifAvailable: true }, async (lock: unknown) => {
      if (!lock) throw new Error(`Another ${token.symbol} deposit is already active in another tab. Wait for it to finish, then check its status.`);
      syncPendingDeposit(token);
      if (pendingDeposit) throw new Error(pendingDeposit.hash ? `A deposit transaction is still unresolved: ${pendingDeposit.hash}. Check its status before starting another deposit.` : 'A prior deposit submission has no confirmed result. Check Ethereum wallet activity before retrying.');
      requirePersistentOperationState();
      const minterActor = await getCkErc20MinterActor();
      const currentInfo = await minterActor.get_minter_info();
      assertTokenSupported(currentInfo, token);
      const liveMinimum = (currentInfo.minimum_deposit_amounts?.[0] ?? []).find(
        (item: any) => String(item.erc20_contract_address).toLowerCase() === token.erc20Address.toLowerCase(),
      );
      if (!liveMinimum) throw new Error(`The minter did not return a current minimum deposit for ${token.symbol}.`);
      if (amount < BigInt(liveMinimum.minimum_deposit_amount)) {
        throw new Error(`The current minimum ${token.symbol} deposit is ${formatTokenAmount(BigInt(liveMinimum.minimum_deposit_amount), token.decimals)} ${token.symbol}.`);
      }
      const transactionHelper = currentInfo.deposit_with_subaccount_helper_contract_address?.[0];
      if (!transactionHelper || !/^0x[0-9a-fA-F]{40}$/.test(transactionHelper)) throw new Error('The minter did not return a valid live Ethereum helper address.');
      helperAddress = transactionHelper;
      const provider = getEthereumProvider();
      if (!provider) throw new Error('Connect an Ethereum wallet before checking its token allowance.');
      const decimalsHex = await provider.request({ method: 'eth_call', params: [{ to: token.erc20Address, data: '0x313ce567' }, 'latest'] });
      if (Number(BigInt(String(decimalsHex))) !== token.decimals) throw new Error(`${token.symbol} Ethereum and ckERC20 ledgers report different decimal counts.`);
      const balanceData = `0x70a08231${encodeAddressWord(evmAccount)}`;
      const balanceHex = await provider.request({ method: 'eth_call', params: [{ to: token.erc20Address, data: balanceData }, 'latest'] });
      const freshEvmBalance = BigInt(String(balanceHex));
      evmTokenBalance = freshEvmBalance;
      if (freshEvmBalance < amount) throw new Error(`Not enough ${token.symbol} in the connected Ethereum wallet. Current balance: ${formatTokenAmount(freshEvmBalance, token.decimals)} ${token.symbol}.`);
      const allowanceData = `0xdd62ed3e${encodeAddressWord(evmAccount)}${encodeAddressWord(transactionHelper)}`;
      const allowanceHex = await provider.request({ method: 'eth_call', params: [{ to: token.erc20Address, data: allowanceData }, 'latest'] });
      const existingAllowance = BigInt(String(allowanceHex));
      if (existingAllowance > 0n) {
        notice = `Reset the existing ${token.symbol} allowance to zero before setting this deposit amount.`;
        const resetData = `0x095ea7b3${encodeAddressWord(transactionHelper)}${encodeUint256(0n)}`;
        approveHash = await submitEthereumTransaction(token.erc20Address, resetData, (hash) => approveHash = hash);
      }
      const approveData = `0x095ea7b3${encodeAddressWord(transactionHelper)}${encodeUint256(amount)}`;
      notice = `Approve exactly ${formatTokenAmount(amount, token.decimals)} ${token.symbol} in your Ethereum wallet.`;
      approveHash = await submitEthereumTransaction(token.erc20Address, approveData, (hash) => approveHash = hash);
      if (ownerPrincipal?.toText() !== liveOwner.toText()) throw new Error(`${token.symbol} approval confirmed, but the receiving Internet Identity changed. No deposit was submitted. Approval transaction: ${approveHash}`);
      const latestInfo = await minterActor.get_minter_info();
      assertTokenSupported(latestInfo, token);
      const latestHelper = latestInfo.deposit_with_subaccount_helper_contract_address?.[0];
      if (!latestHelper || latestHelper.toLowerCase() !== transactionHelper.toLowerCase()) {
        throw new Error(`${token.symbol} approval confirmed for ${transactionHelper}, but the minter helper changed. No deposit was submitted. Check the helper and allowance before continuing. Approval transaction: ${approveHash}`);
      }
      notice = `${token.symbol} approval confirmed. Confirm the deposit transaction in your Ethereum wallet.`;
      const liveRecipient = liveOwner.toText();
      const liveDepositKey = depositStorageKey(evmAccount, liveRecipient, token.ledgerId);
      depositHash = await submitEthereumTransaction(
        latestHelper,
        encodeDepositErc20(token, amount, liveOwner),
        (hash) => { depositHash = hash; savePendingDeposit(hash, depositAmount, liveRecipient, evmAccount, liveRecipient, token); },
        () => {
          try { localStorage.removeItem(liveDepositKey); } catch { /* Continue with confirmed receipt. */ }
          pendingDeposit = null;
        },
        () => beginPendingDeposit(depositAmount, liveRecipient, evmAccount, liveRecipient, token),
        () => clearPendingDepositMarker(evmAccount, liveRecipient, token),
      );
      notice = `Ethereum deposit confirmed. The minter still needs to detect and mint ${token.symbol}; refresh the ICP balance to confirm arrival.`;
      depositAmount = '';
      });
    } catch (cause) {
      error = cause instanceof Error ? cause.message : 'Deposit did not complete.';
    } finally {
      busy = false;
    }
  }

  async function submitWithdrawal() {
    error = '';
    notice = '';
    redeemIndices = null;
    const token = selectedToken;
    if (!token) { error = 'Choose a supported ckERC20 token first.'; return; }
    if (pendingWithdrawal && pendingWithdrawal.owner === ownerPrincipal?.toText()) {
      error = 'A prior withdrawal request has no confirmed response. Reconcile its ledger activity and minter status before retrying.';
      return;
    }
    if (!connected || !ownerPrincipal || ownerPrincipal.isAnonymous()) {
      error = `Connect the Rumi wallet holding ${token.symbol} and ckETH first.`;
      return;
    }
    if (!validateEthereumAddress(redeemAddress)) { error = 'Enter a valid Ethereum destination address.'; return; }
    let amount: bigint;
    try { amount = parseTokenAmount(redeemAmount, token.decimals); }
    catch (cause) { error = cause instanceof Error ? cause.message : 'Invalid amount.'; return; }
    if (!withdrawalQuote || withdrawalQuote.owner.toText() !== ownerPrincipal.toText() || withdrawalQuote.amount !== amount || Date.now() - withdrawalQuote.quotedAtMs > 60_000) {
      error = 'Get a fresh fee quote for this wallet and amount before approving.';
      return;
    }
    const owner = ownerPrincipal;
    const quote = withdrawalQuote;
    const locks = (navigator as any).locks;
    if (!locks?.request) { error = 'This browser cannot safely coordinate minter transactions across tabs. Use a supported browser with Web Locks enabled.'; return; }
    busy = true;
    try {
      await locks.request(`rumi:ckerc20:withdrawal:${owner.toText()}:${token.ledgerId}`, { mode: 'exclusive', ifAvailable: true }, async (lock: unknown) => {
      if (!lock) throw new Error(`Another ${token.symbol} withdrawal is active in another tab. Wait for it to finish, then reconcile its status.`);
      syncPendingWithdrawal(owner);
      if (pendingWithdrawal?.owner === owner.toText()) throw new Error('A prior withdrawal request has no confirmed response. Reconcile its ledger activity and minter status before retrying.');
      requirePersistentOperationState();
      const isLive = () => !destroyed && ownerPrincipal?.toText() === owner.toText();
      const minterActor = await getCkErc20MinterActor();
      assertTokenSupported(await minterActor.get_minter_info(), token);
      notice = `Review and approve the ckETH fee and ${token.symbol} allowances in your Rumi wallet.`;
      const recipient = redeemAddress.trim();
      const result = await approveAndWithdrawCkErc20({
        token, amount, recipient, owner, quote, isLive,
        onWithdrawalSubmitted: () => markWithdrawalPending(amount, recipient, owner, token),
        onWithdrawalResolved: () => {
          try { localStorage.removeItem(withdrawalStorageKey(owner.toText())); } catch { /* Continue with resolved response. */ }
          pendingWithdrawal = null;
        },
      });
      redeemIndices = { ckEth: result.ckEthBurnBlock, ckToken: result.ckTokenBurnBlock };
      notice = 'The ckERC20 minter accepted the withdrawal request. The Ethereum payout is pending; the burn blocks below only confirm the request, not payout finality.';
      redeemAmount = '';
      });
    } catch (cause) {
      error = cause instanceof Error ? cause.message : 'Withdrawal request did not complete.';
    } finally {
      busy = false;
      if (ownerPrincipal) await refreshBalances(ownerPrincipal);
    }
  }

  async function refreshWithdrawalQuote() {
    error = '';
    withdrawalQuote = null;
    const token = selectedToken;
    if (!token) { error = 'Choose a supported ckERC20 token first.'; return; }
    if (!ownerPrincipal || ownerPrincipal.isAnonymous()) { error = `Connect the Rumi wallet that holds ${token.symbol} and ckETH first.`; return; }
    let amount: bigint;
    try { amount = parseTokenAmount(redeemAmount, token.decimals); }
    catch (cause) { error = cause instanceof Error ? cause.message : 'Invalid amount.'; return; }
    quoteBusy = true;
    try {
      const owner = ownerPrincipal;
      const quote = await getCkErc20WithdrawalQuote(token, amount, owner);
      if (destroyed || ownerPrincipal?.toText() !== owner.toText() || selectedTokenLedgerId !== token.ledgerId) return;
      withdrawalQuote = quote;
    } catch (cause) {
      error = cause instanceof Error ? cause.message : 'Could not load a current withdrawal quote.';
    } finally {
      quoteBusy = false;
    }
  }

  function invalidateWithdrawalQuote() {
    withdrawalQuote = null;
  }

  function withdrawalQuoteIsExecutable(quote: NonNullable<typeof withdrawalQuote>): boolean {
    return quote.ckEthBalance >= quote.ckEthAllowance + quote.ckEthFee &&
      quote.ckTokenBalance >= quote.amount + quote.ckTokenFee * 2n;
  }
</script>

<svelte:head>
  <title>ckERC20 Minter | Rumi</title>
  <meta name="description" content="Mint and redeem DFINITY-supported ckERC20 tokens with Rumi." />
</svelte:head>

<main class="minter-page">
  <div class="page-grid">
    <section class="overview" aria-label="ckERC20 minter overview">
      <p class="eyebrow">ETHEREUM ↔ INTERNET COMPUTER</p>
      <h1>ckERC20 Minter</h1>
      <p class="subtitle">Bring supported Ethereum assets to the Internet Computer, then redeem them back when needed.</p>
      <a class="activity-link" href={CKERC20_MINTER_DASHBOARD} target="_blank" rel="noreferrer">View minter activity on DFINITY <span aria-hidden="true">↗</span></a>
      <div class="asset-showcase" aria-label="Supported ckERC20 assets">
        <div class="eth-feature"><img src={ckErc20Logo('ckETH')} alt="ckETH" width="72" height="72" /><div><span class="asset-kicker">FEE TOKEN</span><strong>ckETH</strong><small>Used for Ethereum redemption fees</small></div></div>
        <div class="featured-assets" aria-label="Featured supported assets">
          {#each featuredTokens as token, index (token.ledgerId)}{#if ckErc20Logo(token.symbol)}<span class="fan-token" style={`--fan-index:${index}`} title={token.symbol}><img src={ckErc20Logo(token.symbol)} alt={token.symbol} width="42" height="42" /></span>{/if}{/each}
          {#if featuredTokens.length === 0}<span class="asset-loading">{minterReady ? 'Featured tokens unavailable' : 'Loading supported tokens…'}</span>{/if}
        </div>
        <div class="other-assets"><span class="asset-kicker">MORE SUPPORTED ASSETS ({Math.max(0, otherTokens.length - (otherTokens.some((token) => token.symbol === 'ckETH') ? 1 : 0))})</span><div class="small-token-list">
          {#each otherTokens.filter((token) => token.symbol !== 'ckETH') as token (token.ledgerId)}{#if ckErc20Logo(token.symbol)}<span title={token.symbol}><img src={ckErc20Logo(token.symbol)} alt={token.symbol} width="25" height="25" /></span>{/if}{/each}
        </div></div>
      </div>
      <div class="supply-card" aria-label="Circulating supply on ICP">
        <div class="supply-heading"><span class="asset-kicker">SUPPLY ON ICP</span><button class="text-button" on:click={() => refreshCirculatingSupply()} disabled={supplyBusy || !selectedToken}>{supplyBusy ? 'Refreshing…' : 'Refresh'}</button></div>
        {#if selectedToken}<div class="supply-row"><span>{#if ckErc20Logo(selectedToken.symbol)}<img src={ckErc20Logo(selectedToken.symbol)} alt="" width="20" height="20" />{/if} {selectedToken.symbol}</span><strong>{supplyBusy && tokenSupply === null ? 'Loading…' : tokenSupply === null ? 'Unavailable' : `≈ ${formatSupply(tokenSupply, selectedToken.decimals)}`}</strong></div>{/if}
        <div class="supply-row"><span><img src={ckErc20Logo('ckETH')} alt="" width="20" height="20" /> ckETH</span><strong>{supplyBusy && ckEthSupply === null ? 'Loading…' : ckEthSupply === null ? 'Unavailable' : `≈ ${formatSupply(ckEthSupply, 18)}`}</strong></div>
        {#if supplyError}<p class="supply-error">{supplyError}</p>{/if}{#if supplyUpdatedAt}<p class="updated-at">Updated {supplyUpdatedAt.toLocaleTimeString()}</p>{/if}
      </div>
      <p class="powered">Supported tokens are loaded from DFINITY’s live ckERC20 minter.</p><p class="overview-note">ckERC20 tokens are minted by DFINITY’s ckETH minter against supported Ethereum ERC-20 deposits. Confirm the destination account, selected asset, and network before each transaction. Deposits and redemptions depend on Ethereum finality and minter processing.</p>
    </section>

    <section class="card" aria-label="ckERC20 mint and redeem">
      <div class="evm-bar"><div class="evm-status"><span class:online={!!evmAccount} class="status-dot"></span><span>{evmAccount ? `${evmAccount.slice(0, 7)}…${evmAccount.slice(-5)}` : 'Ethereum wallet'}</span></div>
        {#if evmAccount}<button class="secondary wallet-action" title="Disconnect Ethereum wallet from this page" on:click={disconnectEthereum} disabled={busy || evmConnectBusy}>Disconnect</button>{:else}<button class="secondary wallet-action" on:click={connectEthereum} disabled={busy || evmConnectBusy}>{evmConnectBusy ? 'Connecting…' : 'Connect Ethereum'}</button>{/if}
      </div>
      {#if evmMessage}<p class="inline-hint evm-message">{evmMessage}</p>{/if}
      <div class="tabs" role="tablist" aria-label="Minter direction"><button role="tab" aria-selected={activeTab === 'mint'} class:active={activeTab === 'mint'} on:click={() => activeTab = 'mint'}>Mint</button><button role="tab" aria-selected={activeTab === 'redeem'} class:active={activeTab === 'redeem'} on:click={() => activeTab = 'redeem'}>Redeem</button></div>
      {#if !connected}<div class="wallet-hint">Connect a Rumi wallet in the page header to receive tokens and approve ICP transactions.</div>{/if}
      {#if minterError}<div class="alert error">Could not verify the live ckERC20 minter configuration: {minterError}</div>{:else if !minterReady}<div class="wallet-hint">Loading supported tokens and the current helper address…</div>{:else if supportedTokens.length === 0}<div class="wallet-hint">The minter did not return any supported tokens.</div>{/if}
      {#if !selectedToken}<div class="wallet-hint">{minterReady ? 'No token is available to select.' : 'The supported token list will appear here when the minter responds.'}</div>
      {:else if activeTab === 'mint'}
        <div class="flow-heading"><div><span class="asset-kicker">MINT FROM ETHEREUM</span><h2>{selectedToken.symbol.replace(/^ck/, '')} <i>→</i> {selectedToken.symbol}</h2></div><span class="step-current">1 / 3</span></div>
        <div class="steps"><span class="step-current">Approve</span><i></i><span>Deposit</span><i></i><span>Minted</span></div>
        <div class="risk-note">Send only Ethereum Mainnet {selectedToken.symbol.replace(/^ck/, '')}. Minting starts after Ethereum finality and the minter’s next scan. Token and helper configuration is checked live.</div>
        <label class="field-label" for="deposit-amount">Amount to deposit</label>
        <div class="amount-input"><input id="deposit-amount" type="text" inputmode="decimal" autocomplete="off" placeholder="0.00" bind:value={depositAmount} disabled={busy} /><CkErc20TokenSelect tokens={supportedTokens} selectedLedgerId={selectedTokenLedgerId} disabled={!minterReady || busy || quoteBusy || refreshBusy} label="Token to mint" id="mint-token-options" onSelect={selectToken} /></div>
        {#if selectedToken.minimumDepositAmount !== null}<p class="minimum-note">Minimum {formatTokenAmount(selectedToken.minimumDepositAmount, selectedToken.decimals)} {selectedToken.symbol.replace(/^ck/, '')}</p>{:else}<p class="minimum-note">Minimum deposit unavailable. Deposits for this token are disabled.</p>{/if}
        <div class="amount-meta"><span>{!evmAccount ? 'Connect Ethereum to view balance' : evmTokenBalanceBusy ? 'Reading wallet balance…' : evmTokenBalance === null ? `${selectedToken.symbol.replace(/^ck/, '')} balance unavailable` : `Ethereum: ${formatTokenAmount(evmTokenBalance, selectedToken.decimals)} ${selectedToken.symbol.replace(/^ck/, '')}`}</span><span>ICP: {ckTokenBalance === null ? '—' : `${formatTokenAmount(ckTokenBalance, selectedToken.decimals)} ${selectedToken.symbol}`}</span><div class="amount-actions"><button class="text-button" on:click={() => refreshEvmTokenBalance()} disabled={!evmAccount || evmTokenBalanceBusy || busy}>{evmTokenBalanceBusy ? 'Refreshing…' : 'Refresh'}</button><button class="text-button" on:click={setMaxDeposit} disabled={evmTokenBalance === null || evmTokenBalance <= 0n || busy}>Max</button></div></div>
        {#if evmTokenBalanceError}<p class="inline-hint">Could not read the Ethereum token balance: {evmTokenBalanceError}</p>{/if}
        <div class="destination"><span class="label">ICP RECEIVING ACCOUNT · {selectedToken.symbol}</span><code>{connected && ownerPrincipal ? ownerPrincipal.toText() : 'Connect a Rumi wallet to choose the recipient'}</code></div>
        <p class="fee-note">Your Ethereum wallet needs ETH for gas. Deposits usually take two transactions; a nonzero helper allowance may need a zero reset first. Gas is not sponsored.</p>
        <button class="primary" on:click={submitDeposit} disabled={busy || !!pendingDeposit || !connected || !minterReady || !evmAccount || !selectedToken.minimumDepositAmount}>{pendingDeposit ? 'Check pending deposit before retrying' : busy ? 'Waiting for wallet…' : `Approve and mint ${selectedToken.symbol}`}</button>
        {#if approveHash}<p class="tx-line">{selectedToken.symbol.replace(/^ck/, '')} approval: <a href={`https://etherscan.io/tx/${approveHash}`} target="_blank" rel="noreferrer">{approveHash.slice(0, 14)}…</a></p>{/if}
        {#if depositHash}<p class="tx-line">Deposit: <a href={`https://etherscan.io/tx/${depositHash}`} target="_blank" rel="noreferrer">{depositHash.slice(0, 14)}…</a> · <a href={CKERC20_MINTER_DASHBOARD} target="_blank" rel="noreferrer">Track mint</a></p>{/if}
        {#if pendingDeposit}<div class="alert error">{#if pendingDeposit.hash}A previous {pendingDeposit.tokenSymbol ?? selectedToken.symbol} deposit is unresolved: <a href={`https://etherscan.io/tx/${pendingDeposit.hash}`} target="_blank" rel="noreferrer">check Ethereum status</a> or search the <a href={CKERC20_MINTER_DASHBOARD} target="_blank" rel="noreferrer">DFINITY minter dashboard</a>.{:else}A previous deposit submission did not return a transaction hash. Check your Ethereum wallet activity.{/if} New deposits for this token are locked. Clear the retry lock only after confirming the deposit did not complete.<button class="text-button recovery-button" on:click={clearPendingDeposit}>Clear retry lock after reconciliation</button></div>{/if}
        {#if helperAddress}<p class="small-note">Live minter helper: <code>{helperAddress}</code></p>{/if}
      {:else}
        <div class="flow-heading"><div><span class="asset-kicker">REDEEM TO ETHEREUM</span><h2>{selectedToken.symbol} <i>→</i> {selectedToken.symbol.replace(/^ck/, '')}</h2></div><span class="step-current">1 / 3</span></div>
        <div class="steps"><span class="step-current">Approve fees</span><i></i><span>Approve token</span><i></i><span>Payout</span></div>
        <div class="risk-note">Redeeming uses {selectedToken.symbol} plus ckETH for the Ethereum transaction fee. Your Ethereum wallet does not sign or pay for the payout.</div>
        <label class="field-label" for="redeem-amount">Amount to redeem</label>
        <div class="amount-input"><input id="redeem-amount" type="text" inputmode="decimal" autocomplete="off" placeholder="0.00" bind:value={redeemAmount} on:input={invalidateWithdrawalQuote} disabled={busy} /><CkErc20TokenSelect tokens={supportedTokens} selectedLedgerId={selectedTokenLedgerId} disabled={!minterReady || busy || quoteBusy || refreshBusy} label="Token to redeem" id="redeem-token-options" onSelect={selectToken} /></div>
        <div class="amount-meta"><span>{ckTokenBalance === null ? 'ICP token balance unavailable' : `Balance: ${formatTokenAmount(ckTokenBalance, selectedToken.decimals)} ${selectedToken.symbol}`}</span><span>{ckEthBalance === null ? 'ckETH fee balance unavailable' : `${formatTokenAmount(ckEthBalance, 18, 8)} ckETH fee balance`}</span></div>
        <label class="field-label" for="redeem-address">Ethereum destination</label><input id="redeem-address" class="address-input" type="text" autocomplete="off" spellcheck="false" placeholder="0x…" bind:value={redeemAddress} disabled={busy} />
        <div class="wallet-row redeem-balance"><span class="label">RUMI WALLET BALANCES</span><button class="text-button" disabled={!ownerPrincipal || refreshBusy} on:click={() => refreshBalances()}>{refreshBusy ? 'Refreshing…' : 'Refresh'}</button></div>
        <button class="secondary quote-button" on:click={refreshWithdrawalQuote} disabled={quoteBusy || busy || !connected}>{quoteBusy ? 'Loading current fees…' : 'Get current fee quote'}</button>
        {#if withdrawalQuote}<div class="quote-card"><span class="label">CURRENT QUOTE · REFRESHED {new Date(withdrawalQuote.quotedAtMs).toLocaleTimeString()}{#if withdrawalQuote.minterPriceTimestampMs} · MINTER PRICE {new Date(withdrawalQuote.minterPriceTimestampMs).toLocaleTimeString()}{/if}</span><div><span>{selectedToken.symbol} amount</span><strong>{formatTokenAmount(withdrawalQuote.amount, selectedToken.decimals)} {selectedToken.symbol}</strong></div><div><span>ckETH max Ethereum fee</span><strong>{formatTokenAmount(withdrawalQuote.maxTransactionFee, 18, 8)} ckETH</strong></div><div><span>ckETH allowance cap (includes ledger fee)</span><strong>{formatTokenAmount(withdrawalQuote.ckEthAllowance, 18, 8)} ckETH</strong></div><div><span>{selectedToken.symbol} allowance cap (includes ledger fee)</span><strong>{formatTokenAmount(withdrawalQuote.ckTokenAllowance, selectedToken.decimals)} {selectedToken.symbol}</strong></div><p>{withdrawalQuoteIsExecutable(withdrawalQuote) ? 'Balances cover this quote. It expires in 60 seconds.' : 'Your current balances do not cover this quote.'}</p></div>{/if}
        {#if withdrawalPendingForOwner}<div class="alert error">A previous {pendingWithdrawal?.tokenSymbol ?? 'ckERC20'} withdrawal request has no confirmed response. Check its ckERC20 and ckETH ledger activity and the <a href={CKERC20_MINTER_DASHBOARD} target="_blank" rel="noreferrer">minter dashboard</a> before retrying. A lock prevents a second burn.<button class="text-button recovery-button" on:click={clearPendingWithdrawal}>Clear retry lock after reconciliation</button></div>{/if}
        <button class="primary" on:click={submitWithdrawal} disabled={busy || withdrawalPendingForOwner || !connected || !minterReady || !withdrawalQuote || !withdrawalQuoteIsExecutable(withdrawalQuote)}>{withdrawalPendingForOwner ? 'Reconcile previous request first' : busy ? 'Confirm approvals in your wallet…' : `Approve and request ${selectedToken.symbol.replace(/^ck/, '')} redemption`}</button>
        {#if redeemIndices}<div class="success-box">Withdrawal request accepted by the minter.<br />ckETH fee burn block: {redeemIndices.ckEth}<br />{selectedToken.symbol} burn block: {redeemIndices.ckToken}<br /><a href={CKERC20_MINTER_DASHBOARD} target="_blank" rel="noreferrer">Open ckERC20 minter dashboard</a></div>{/if}
        <p class="small-note">Before approval, current ckETH and {selectedToken.symbol} fees, balances, and the minter’s Ethereum fee estimate are checked. Approvals are limited to this withdrawal and expire after 10 minutes.</p>
      {/if}
      {#if notice}<div class="alert notice" aria-live="polite">{notice}</div>{/if}{#if error}<div class="alert error" role="alert">{error}</div>{/if}
    </section>
  </div>
</main>
<style>
  .minter-page { max-width:1200px; margin:0 auto; padding:0 32px 20px; color:var(--rumi-text-primary); }
  .page-grid { display:grid; grid-template-columns:minmax(250px,.78fr) minmax(500px,1.45fr); align-items:start; gap:26px; }
  .overview { min-width:0; padding-top:6px; }
  .eyebrow,.label,.asset-kicker { color:var(--rumi-text-secondary); font-size:10px; letter-spacing:.09em; font-weight:600; }
  .eyebrow { margin:0 0 10px; }
  h1 { margin:0 0 12px; font-size:clamp(30px,3.2vw,42px); line-height:1.08; letter-spacing:-.04em; }
  .subtitle { max-width:380px; margin:0; color:var(--rumi-text-secondary); font-size:13px; line-height:1.5; }
  .activity-link { display:inline-flex; gap:7px; margin-top:13px; color:var(--rumi-action-bright); text-decoration:none; font-size:12px; font-weight:600; }
  .asset-showcase { margin-top:19px; }
  .eth-feature { display:flex; align-items:center; gap:14px; padding-bottom:17px; border-bottom:1px solid var(--rumi-border); }
  .eth-feature img { width:72px; height:72px; object-fit:contain; filter:drop-shadow(0 8px 14px #0004); }
  .eth-feature div { display:grid; gap:3px; }.eth-feature strong { font-size:20px; }.eth-feature small { color:var(--rumi-text-secondary); font-size:11px; }
  .featured-assets { display:flex; align-items:center; min-height:67px; padding-top:6px; }
  .fan-token { position:relative; z-index:calc(5 - var(--fan-index)); display:grid; place-items:center; width:54px; height:54px; margin-left:calc(var(--fan-index) * -9px); border:1px solid var(--rumi-bg-surface1); border-radius:50%; background:var(--rumi-bg-surface2); }
  .fan-token:first-child { margin-left:0; }.fan-token img { width:46px; height:46px; border-radius:50%; object-fit:contain; }
  .asset-loading { color:var(--rumi-text-muted); font-size:12px; }
  .small-token-list { display:flex; flex-wrap:wrap; gap:8px; margin-top:9px; }.small-token-list span { display:grid; place-items:center; width:33px; height:33px; border:1px solid var(--rumi-border); border-radius:50%; background:var(--rumi-bg-surface1); }.small-token-list img { width:24px; height:24px; object-fit:contain; }
  .supply-card { margin-top:20px; padding:13px 14px 10px; border:1px solid var(--rumi-border); border-radius:9px; background:var(--rumi-bg-surface1); }
  .supply-heading,.supply-row { display:flex; justify-content:space-between; align-items:center; gap:10px; }.supply-row { margin-top:11px; font-size:11px; }
  .supply-row span { display:flex; align-items:center; gap:7px; color:var(--rumi-text-secondary); }.supply-row img { border-radius:50%; object-fit:contain; }.supply-row strong { text-align:right; font-variant-numeric:tabular-nums; }
  .supply-error,.updated-at { margin:8px 0 0; color:var(--rumi-text-muted); font-size:10px; }.supply-error { color:#e5a0b4; }.powered { margin-top:10px; color:var(--rumi-text-muted); font-size:10px; line-height:1.5; }.overview-note { max-width:390px; margin:12px 0 0; padding-top:12px; border-top:1px solid var(--rumi-border); color:var(--rumi-text-secondary); font-size:12px; line-height:1.6; }
  .card { min-width:0; padding:12px 20px 12px; border:1px solid var(--rumi-border); border-radius:11px; background:var(--rumi-bg-surface1); box-shadow:inset 0 1px 0 rgba(200,210,240,.03); }
  .evm-bar,.wallet-row { display:flex; justify-content:space-between; align-items:center; gap:12px; }.evm-bar { min-height:31px; padding-bottom:7px; border-bottom:1px solid var(--rumi-border); }
  .evm-status { display:flex; align-items:center; gap:8px; min-width:0; color:var(--rumi-text-secondary); font-size:11px; }.evm-status>span:last-child { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .status-dot { width:7px; height:7px; flex:0 0 7px; border-radius:50%; background:var(--rumi-text-muted); }.status-dot.online { background:var(--rumi-action); }
  .secondary { padding:7px 11px; color:var(--rumi-text-primary); background:var(--rumi-bg-surface2); border:1px solid var(--rumi-border-hover); border-radius:7px; font:inherit; font-size:11px; font-weight:600; cursor:pointer; }.wallet-action { min-width:124px; }
  button:disabled { opacity:.5; cursor:not-allowed; }
  .tabs { display:grid; grid-template-columns:1fr 1fr; gap:4px; margin:8px 0 10px; padding:3px; border:1px solid var(--rumi-border); border-radius:8px; background:var(--rumi-bg-primary); }
  .tabs button { height:30px; color:var(--rumi-text-secondary); background:transparent; border:1px solid transparent; border-radius:5px; font:inherit; font-size:12px; font-weight:600; cursor:pointer; }.tabs button.active { color:var(--rumi-text-primary); background:var(--rumi-bg-surface2); border-color:var(--rumi-border-hover); }
  .wallet-hint,.risk-note,.success-box { margin:6px 0 8px; padding:7px 10px; border:1px solid var(--rumi-border); border-radius:7px; background:var(--rumi-bg-surface2); color:var(--rumi-text-secondary); font-size:10px; line-height:1.5; }.risk-note { border-color:rgba(167,139,250,.18); background:rgba(167,139,250,.045); }
  .flow-heading { display:flex; justify-content:space-between; align-items:center; gap:12px; }.flow-heading h2 { margin:3px 0 0; font-size:17px; letter-spacing:-.02em; }.flow-heading h2 i { color:var(--rumi-text-muted); font-style:normal; font-weight:400; }.flow-heading>.step-current { padding:4px 8px; border:1px solid var(--rumi-border-accent); border-radius:20px; font-size:10px; }
  .steps { display:flex; align-items:center; gap:8px; margin:6px 0 8px; color:var(--rumi-text-muted); font-size:10px; }.steps i { height:1px; flex:1; background:var(--rumi-border-hover); }.steps .step-current { color:var(--rumi-action-bright); white-space:nowrap; }
  .field-label { display:block; margin:9px 0 4px; color:var(--rumi-text-secondary); font-size:11px; font-weight:600; }
  .amount-input { min-height:48px; display:flex; align-items:center; gap:7px; padding:4px 7px 4px 12px; border:1px solid var(--rumi-border-hover); border-radius:8px; background:var(--rumi-bg-primary); }.amount-input:focus-within,.address-input:focus-visible { border-color:var(--rumi-border-accent); box-shadow:0 0 0 2px var(--rumi-action-dim); }
  .minimum-note { margin:4px 0 0; color:var(--rumi-text-muted); font-size:10px; }
  .amount-meta { display:flex; justify-content:space-between; align-items:center; gap:8px; min-height:23px; color:var(--rumi-text-secondary); font-size:10px; }.amount-actions { display:flex; align-items:center; gap:12px; flex-shrink:0; }.amount-actions .text-button { margin:0; }
  input { min-width:0; width:100%; color:var(--rumi-text-primary); background:transparent; border:0; outline:none; font:inherit; font-size:19px; font-variant-numeric:tabular-nums; }.destination { display:grid; gap:4px; margin-top:6px; padding:6px 9px; border:1px solid var(--rumi-border); border-radius:7px; background:rgba(255,255,255,.015); }.destination code,.small-note code { color:var(--rumi-text-secondary); font-size:10px; overflow-wrap:anywhere; }
  .fee-note,.small-note { margin:6px 0; color:var(--rumi-text-muted); font-size:10px; line-height:1.45; }.primary { width:100%; min-height:40px; border:1px solid rgba(52,211,153,.22); border-radius:7px; background:var(--rumi-action); color:#06251b; font:inherit; font-size:12px; font-weight:700; cursor:pointer; }.primary:hover:not(:disabled) { background:var(--rumi-action-bright); }
  .inline-hint { margin:6px 0; color:var(--rumi-text-secondary); font-size:10px; line-height:1.4; }.address-input { height:43px; padding:0 11px; border:1px solid var(--rumi-border-hover); border-radius:7px; background:var(--rumi-bg-primary); font-size:14px; }
  .quote-button { width:100%; margin:7px 0 9px; }.quote-card { display:grid; gap:7px; margin:9px 0; padding:10px; border:1px solid var(--rumi-border); border-radius:7px; background:var(--rumi-bg-primary); }.quote-card>div { display:flex; justify-content:space-between; gap:10px; color:var(--rumi-text-secondary); font-size:10px; }.quote-card strong { color:var(--rumi-text-primary); text-align:right; font-weight:600; }.quote-card p { margin:0; color:var(--rumi-text-secondary); font-size:10px; }
  .redeem-balance { margin-top:8px; padding-top:8px; border-top:1px solid var(--rumi-border); }.tx-line { color:var(--rumi-text-secondary); font-size:10px; overflow-wrap:anywhere; }a { color:var(--rumi-action-bright); }
  .text-button { display:block; margin:3px 0 0; padding:0; border:0; background:none; color:var(--rumi-action-bright); cursor:pointer; font:inherit; font-size:10px; }.recovery-button { margin-top:8px; color:#ffb3c5; text-decoration:underline; }
  .alert { margin-top:10px; padding:10px 11px; border-radius:7px; font-size:10px; line-height:1.45; overflow-wrap:anywhere; }.notice { border:1px solid rgba(45,212,191,.25); background:var(--rumi-teal-dim); color:#9cebd0; }.error { border:1px solid rgba(224,107,159,.3); background:rgba(224,107,159,.09); color:#ffb3c5; }.success-box { color:#a8efda; border-color:rgba(45,212,191,.25); }
  @media(max-width:850px) { .minter-page { padding:0 22px 28px; }.page-grid { grid-template-columns:minmax(0,1fr); gap:20px; }.card { grid-row:1; padding:17px 19px; }.overview { grid-row:2; padding-top:0; }.asset-showcase { margin-top:18px; }.supply-card { max-width:500px; } }
  @media(max-width:480px) { .minter-page { padding:0 12px 24px; }.page-grid { gap:17px; }.card { padding:13px 12px; }h1 { font-size:29px; }.amount-meta { align-items:flex-start; flex-wrap:wrap; padding:4px 0; }.amount-actions { margin-left:auto; }.steps { gap:5px; font-size:9px; }.steps i { min-width:8px; } }
</style>
