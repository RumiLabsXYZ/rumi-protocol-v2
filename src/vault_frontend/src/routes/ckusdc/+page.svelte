<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import { isConnected as isConnectedStore, principal as principalStore } from '$lib/stores/wallet';
  import { CANISTER_IDS } from '$lib/config';
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
      try {
        const [accounts, chainId] = await Promise.all([
          ethereumProvider?.request({ method: 'eth_accounts' }),
          ethereumProvider?.request({ method: 'eth_chainId' }),
        ]);
        if (destroyed) return;
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
    syncPendingDeposit(token);
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
      syncPendingDeposit(supportedTokens.find((token) => token.ledgerId === selectedTokenLedgerId));
      if (evmAccount) void refreshEvmTokenBalance(evmAccount, supportedTokens.find((token) => token.ledgerId === selectedTokenLedgerId));
      if (ownerPrincipal && !ownerPrincipal.isAnonymous()) await refreshBalances(ownerPrincipal);
    } catch (cause) {
      if (destroyed) return;
      minterError = cause instanceof Error ? cause.message : 'Could not read ckERC20 minter configuration.';
    }
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
    error = '';
    evmMessage = '';
    const provider = getEthereumProvider();
    if (!provider) {
      evmMessage = 'Install or unlock an EVM wallet such as Rabby or MetaMask to deposit.';
      return;
    }
    try {
      const accounts = await provider.request({ method: 'eth_requestAccounts' });
      if (!Array.isArray(accounts) || !accounts[0]) throw new Error('The EVM wallet did not return an account.');
      const chainId = await provider.request({ method: 'eth_chainId' });
      if (chainId !== '0x1') {
        evmMessage = 'Switch your EVM wallet to Ethereum Mainnet, then connect again.';
        return;
      }
      evmAccount = String(accounts[0]);
      syncPendingDeposit();
      await refreshEvmTokenBalance(evmAccount);
    } catch (cause) {
      evmMessage = cause instanceof Error ? cause.message : 'EVM wallet connection was not completed.';
    }
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
  <a class="back-link" href="/">← Rumi</a>
  <header class="hero">
    <div class="coin-mark">$</div>
    <p class="eyebrow">ETHEREUM ↔ INTERNET COMPUTER</p>
    <h1>ckERC20 Minter</h1>
    <p class="subtitle">Mint supported Ethereum tokens on the Internet Computer and redeem them back.</p>
    <p class="powered">Supported tokens and their ledgers are loaded from DFINITY’s ckERC20 minter.</p>
  </header>

  <section class="card" aria-label="ckERC20 mint and redeem">
    <div class="wallet-summary">
      <div>
        <span class="label">RECEIVING INTERNET IDENTITY</span>
        <strong>{#if connected && ownerPrincipal}{ownerPrincipal.toText()}{:else}Connect a Rumi wallet{/if}</strong>
      </div>
      <div class="balance-box">
        <span class="label">{selectedToken ? `${selectedToken.symbol.toUpperCase()} BALANCE` : 'TOKEN BALANCE'}</span>
        <strong>{ckTokenBalance === null || !selectedToken ? '—' : `${formatTokenAmount(ckTokenBalance, selectedToken.decimals)} ${selectedToken.symbol}`}</strong>
        <button class="text-button" disabled={!ownerPrincipal || refreshBusy} on:click={() => refreshBalances()}>{refreshBusy ? 'Refreshing…' : 'Refresh'}</button>
      </div>
    </div>

    <div class="tabs" role="tablist" aria-label="Minter direction">
      <button role="tab" aria-selected={activeTab === 'mint'} class:active={activeTab === 'mint'} on:click={() => activeTab = 'mint'}>Mint {selectedToken?.symbol ?? 'token'}</button>
      <button role="tab" aria-selected={activeTab === 'redeem'} class:active={activeTab === 'redeem'} on:click={() => activeTab = 'redeem'}>Redeem {selectedToken?.symbol.replace(/^ck/, '') ?? 'token'}</button>
    </div>

    {#if !connected}
      <div class="wallet-hint">Connect your Internet Identity or Rumi wallet with the wallet button in the header. That identity receives the selected ckERC20 token and signs any ICP approvals.</div>
    {/if}

    {#if minterError}
      <div class="alert error">Could not verify the live ckERC20 minter configuration: {minterError}</div>
    {:else if !minterReady}
      <div class="wallet-hint">Loading the live supported token list and current helper address…</div>
    {:else if supportedTokens.length === 0}
      <div class="wallet-hint">The minter did not return any supported tokens.</div>
    {/if}

    {#if !selectedToken}
      <div class="wallet-hint">{minterReady ? 'No token is available to select.' : 'The supported token list will appear here when the minter responds.'}</div>
    {:else if activeTab === 'mint'}
      <div class="flow-label">{selectedToken.symbol.replace(/^ck/, '')} → {selectedToken.symbol}</div>
      <div class="steps"><span class="step-current">1&nbsp; Approve {selectedToken.symbol.replace(/^ck/, '')}</span><i></i><span>2&nbsp; Deposit</span><i></i><span>3&nbsp; {selectedToken.symbol} minted</span></div>
      <div class="risk-note">Send only Ethereum Mainnet {selectedToken.symbol.replace(/^ck/, '')} through this page. Minting starts after Ethereum finality and the minter’s next scan. The selected token and helper are checked live before transactions are enabled.</div>

      <label class="field-label" for="deposit-amount">Deposit amount</label>
      <div class="amount-input">
        <input id="deposit-amount" type="text" inputmode="decimal" autocomplete="off" placeholder="0.00" bind:value={depositAmount} disabled={busy} />
        <select class="token-select" aria-label="Token to mint" value={selectedTokenLedgerId} on:change={(event) => selectToken((event.currentTarget as HTMLSelectElement).value)} disabled={!minterReady || busy || quoteBusy || refreshBusy}>
          {#each supportedTokens as token (token.ledgerId)}<option value={token.ledgerId}>{token.symbol}</option>{/each}
        </select>
      </div>
      {#if selectedToken.minimumDepositAmount !== null}<p class="minimum-note">Minimum deposit: {formatTokenAmount(selectedToken.minimumDepositAmount, selectedToken.decimals)} {selectedToken.symbol.replace(/^ck/, '')}</p>{:else}<p class="minimum-note">Minimum deposit unavailable. Deposits for this token are disabled.</p>{/if}
      <div class="amount-meta">
        <span>{!evmAccount ? 'Connect an Ethereum wallet to view its balance' : evmTokenBalanceBusy ? 'Reading wallet balance…' : evmTokenBalance === null ? `${selectedToken.symbol.replace(/^ck/, '')} wallet balance unavailable` : `Wallet balance: ${formatTokenAmount(evmTokenBalance, selectedToken.decimals)} ${selectedToken.symbol.replace(/^ck/, '')}`}</span>
        <div class="amount-actions">
          <button class="text-button" on:click={() => refreshEvmTokenBalance()} disabled={!evmAccount || evmTokenBalanceBusy || busy}>{evmTokenBalanceBusy ? 'Refreshing…' : 'Refresh'}</button>
          <button class="text-button" on:click={setMaxDeposit} disabled={evmTokenBalance === null || evmTokenBalance <= 0n || busy}>Max</button>
        </div>
      </div>
      {#if evmTokenBalanceError}<p class="inline-hint">Could not read the Ethereum token balance: {evmTokenBalanceError}</p>{/if}
      <div class="destination"><span class="label">{selectedToken.symbol} WILL BE MINTED TO</span><code>{connected && ownerPrincipal ? ownerPrincipal.toText() : 'Connect a Rumi wallet to choose the recipient'}</code></div>

      <div class="wallet-row">
        <div><span class="label">ETHEREUM WALLET</span><strong>{evmAccount ? `${evmAccount.slice(0, 7)}…${evmAccount.slice(-5)}` : 'Not connected'}</strong></div>
        {#if !evmAccount}<button class="secondary" on:click={connectEthereum} disabled={busy}>Connect Ethereum wallet</button>{/if}
      </div>
      {#if evmMessage}<p class="inline-hint">{evmMessage}</p>{/if}
      <p class="fee-note">This Ethereum wallet needs ETH for gas. A deposit usually takes two transactions; a nonzero helper allowance must first be reset to zero, adding a transaction. Rumi does not sponsor Ethereum gas in this flow.</p>
      <button class="primary" on:click={submitDeposit} disabled={busy || !!pendingDeposit || !connected || !minterReady || !evmAccount || !selectedToken.minimumDepositAmount}>{pendingDeposit ? 'Check pending deposit before retrying' : busy ? 'Waiting for wallet…' : `Approve and mint ${selectedToken.symbol}`}</button>
      {#if approveHash}<p class="tx-line">{selectedToken.symbol.replace(/^ck/, '')} approval: <a href={`https://etherscan.io/tx/${approveHash}`} target="_blank" rel="noreferrer">{approveHash.slice(0, 14)}…</a></p>{/if}
      {#if depositHash}<p class="tx-line">Deposit transaction: <a href={`https://etherscan.io/tx/${depositHash}`} target="_blank" rel="noreferrer">{depositHash.slice(0, 14)}…</a> · <a href={CKERC20_MINTER_DASHBOARD} target="_blank" rel="noreferrer">Track mint on DFINITY dashboard</a></p>{/if}
      {#if pendingDeposit}<div class="alert error">{#if pendingDeposit.hash}A previous {pendingDeposit.tokenSymbol ?? selectedToken.symbol} deposit is unresolved: <a href={`https://etherscan.io/tx/${pendingDeposit.hash}`} target="_blank" rel="noreferrer">check Ethereum status</a>. You can also search this hash on the <a href={CKERC20_MINTER_DASHBOARD} target="_blank" rel="noreferrer">DFINITY minter dashboard</a>.{:else}A previous deposit submission did not return a transaction hash. Check the connected Ethereum wallet's activity.{/if} New deposits for this token are locked to prevent a duplicate. Clear the retry lock only after confirming the deposit did not complete.<button class="text-button recovery-button" on:click={clearPendingDeposit}>Clear retry lock after reconciliation</button></div>{/if}
      {#if helperAddress}<p class="small-note">Live minter helper: <code>{helperAddress}</code></p>{/if}
    {:else}
      <div class="flow-label">{selectedToken.symbol} → {selectedToken.symbol.replace(/^ck/, '')}</div>
      <div class="steps"><span class="step-current">1&nbsp; Approve ckETH fee</span><i></i><span>2&nbsp; Approve {selectedToken.symbol}</span><i></i><span>3&nbsp; Ethereum payout</span></div>
      <div class="risk-note">Redeeming requires {selectedToken.symbol} and ckETH. ckETH pays the Ethereum transaction fee through the DFINITY minter; your own Ethereum wallet does not sign or pay gas for the payout.</div>
      <label class="field-label" for="redeem-amount">Redemption amount</label>
      <div class="amount-input">
        <input id="redeem-amount" type="text" inputmode="decimal" autocomplete="off" placeholder="0.00" bind:value={redeemAmount} on:input={invalidateWithdrawalQuote} disabled={busy} />
        <select class="token-select" aria-label="Token to redeem" value={selectedTokenLedgerId} on:change={(event) => selectToken((event.currentTarget as HTMLSelectElement).value)} disabled={!minterReady || busy || quoteBusy || refreshBusy}>
          {#each supportedTokens as token (token.ledgerId)}<option value={token.ledgerId}>{token.symbol}</option>{/each}
        </select>
      </div>
      <label class="field-label" for="redeem-address">Ethereum destination</label>
      <input id="redeem-address" class="address-input" type="text" autocomplete="off" spellcheck="false" placeholder="0x…" bind:value={redeemAddress} disabled={busy} />
      <div class="wallet-row redeem-balance"><div><span class="label">CKETH FEE BALANCE</span><strong>{ckEthBalance === null ? '—' : `${formatTokenAmount(ckEthBalance, 18, 8)} ckETH`}</strong></div><button class="text-button" disabled={!ownerPrincipal || refreshBusy} on:click={() => refreshBalances()}>{refreshBusy ? 'Refreshing…' : 'Refresh balances'}</button></div>
      <button class="secondary quote-button" on:click={refreshWithdrawalQuote} disabled={quoteBusy || busy || !connected}>{quoteBusy ? 'Loading current fees…' : 'Get current fee quote'}</button>
      {#if withdrawalQuote}
        <div class="quote-card">
          <span class="label">CURRENT WITHDRAWAL QUOTE · REFRESHED {new Date(withdrawalQuote.quotedAtMs).toLocaleTimeString()}{#if withdrawalQuote.minterPriceTimestampMs} · MINTER PRICE {new Date(withdrawalQuote.minterPriceTimestampMs).toLocaleTimeString()}{/if}</span>
          <div><span>{selectedToken.symbol} amount</span><strong>{formatTokenAmount(withdrawalQuote.amount, selectedToken.decimals)} {selectedToken.symbol}</strong></div>
          <div><span>ckETH max Ethereum fee</span><strong>{formatTokenAmount(withdrawalQuote.maxTransactionFee, 18, 8)} ckETH</strong></div>
          <div><span>ckETH allowance cap (includes ledger fee)</span><strong>{formatTokenAmount(withdrawalQuote.ckEthAllowance, 18, 8)} ckETH</strong></div>
          <div><span>{selectedToken.symbol} allowance cap (includes ledger fee)</span><strong>{formatTokenAmount(withdrawalQuote.ckTokenAllowance, selectedToken.decimals)} {selectedToken.symbol}</strong></div>
          <p>{withdrawalQuoteIsExecutable(withdrawalQuote) ? 'Balances cover this quote. Quote expires in 60 seconds.' : 'Your current balances do not cover this quote.'}</p>
        </div>
      {/if}
      {#if pendingWithdrawal && pendingWithdrawal.owner === ownerPrincipal?.toText()}<div class="alert error">A previous {pendingWithdrawal.tokenSymbol ?? 'ckERC20'} withdrawal request has no confirmed response. Check its ckERC20 and ckETH ledger activity and the <a href={CKERC20_MINTER_DASHBOARD} target="_blank" rel="noreferrer">minter dashboard</a> before retrying. A lock prevents an accidental second burn.<button class="text-button recovery-button" on:click={clearPendingWithdrawal}>Clear retry lock after reconciliation</button></div>{/if}
      <button class="primary" on:click={submitWithdrawal} disabled={busy || pendingWithdrawal?.owner === ownerPrincipal?.toText() || !connected || !minterReady || !withdrawalQuote || !withdrawalQuoteIsExecutable(withdrawalQuote)}>{pendingWithdrawal?.owner === ownerPrincipal?.toText() ? 'Reconcile previous request first' : busy ? 'Confirm approvals in your wallet…' : `Approve and request ${selectedToken.symbol.replace(/^ck/, '')} redemption`}</button>
      {#if redeemIndices}<div class="success-box">Withdrawal request accepted by the minter.<br />ckETH fee burn block: {redeemIndices.ckEth}<br />{selectedToken.symbol} burn block: {redeemIndices.ckToken}<br /><a href={CKERC20_MINTER_DASHBOARD} target="_blank" rel="noreferrer">Open ckERC20 minter dashboard</a></div>{/if}
      <p class="small-note">Before any approval, the page reads current ckETH and {selectedToken.symbol} ledger fees, your balances, and the minter’s current Ethereum fee estimate. Approvals are limited to this withdrawal and expire after 10 minutes.</p>
    {/if}

    {#if notice}<div class="alert notice" aria-live="polite">{notice}</div>{/if}
    {#if error}<div class="alert error" role="alert">{error}</div>{/if}
  </section>

  <footer class="disclaimer">ckERC20 tokens are minted by DFINITY’s ckETH minter against supported Ethereum ERC-20 deposits. Confirm the destination identity, selected asset, and network before every transaction. Deposits and redemptions are subject to Ethereum finality and minter processing.</footer>
</main>

<style>
  :global(body) { background: #080b16; }
  .minter-page { max-width: 900px; margin: 0 auto; padding: 36px 24px 72px; color: #f2efff; }
  .back-link { color: #9a96ad; text-decoration: none; font-size: 14px; }
  .hero { text-align: center; padding: 40px 0 30px; }
  .coin-mark { width: 68px; height: 68px; margin: 0 auto 16px; display: grid; place-items: center; border: 5px solid #693fe3; border-radius: 50%; color: #b0fff0; font-size: 34px; font-weight: 800; box-shadow: inset 0 0 0 4px #18b889; }
  .eyebrow, .label, .flow-label { color: #7b768e; font-size: 11px; letter-spacing: .12em; font-weight: 700; }
  h1 { margin: 8px 0; font-size: clamp(32px, 5vw, 44px); letter-spacing: -.04em; }
  .subtitle { margin: 0; color: #9e99b1; font-size: 17px; }
  .powered { color: #706b82; font-size: 12px; }
  .card { max-width: 760px; margin: 0 auto; padding: 30px; background: linear-gradient(145deg, #101525, #0c1120); border: 1px solid #1c263b; border-radius: 20px; box-shadow: 0 24px 80px #0003; }
  .wallet-summary, .wallet-row { display: flex; justify-content: space-between; align-items: center; gap: 16px; }
  .wallet-summary strong, .wallet-row strong { display: block; margin-top: 7px; font-size: 13px; overflow-wrap: anywhere; }
  .balance-box { text-align: right; }
  .text-button { display: block; margin: 6px 0 0 auto; padding: 0; border: 0; background: none; color: #54d8ae; cursor: pointer; font-size: 12px; }
  button:disabled { opacity: .5; cursor: not-allowed; }
  .tabs { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; margin: 26px 0; }
  .tabs button { height: 52px; color: #9a95ac; background: #111629; border: 1px solid #1d2740; border-radius: 11px; font-weight: 700; cursor: pointer; }
  .tabs button.active { color: #eeeaff; border-color: #15bc89; box-shadow: inset 0 0 0 1px #15bc89; }
  .wallet-hint, .risk-note, .success-box { margin: 14px 0 22px; padding: 14px 16px; border: 1px solid #242d48; border-radius: 10px; background: #11172a; color: #aaa5bb; font-size: 13px; line-height: 1.6; }
  .risk-note { border-color: #38304e; background: #17142a; }
  .flow-label { margin-bottom: 18px; }
  .steps { display: flex; align-items: center; gap: 10px; margin-bottom: 22px; color: #747087; font-size: 12px; }
  .steps i { height: 1px; flex: 1; background: #272c44; }
  .steps .step-current { color: #59d9b0; white-space: nowrap; }
  .field-label { display: block; margin: 20px 0 8px; color: #d8d4e7; font-size: 14px; font-weight: 700; }
  .amount-input { height: 58px; display: flex; align-items: center; padding: 0 16px; border: 1px solid #242c43; border-radius: 10px; background: #0d1222; }
  .amount-input .token-select { flex: 0 0 auto; max-width: 170px; height: 40px; margin-left: 12px; padding: 0 10px; color: #e9e4f4; background: #171d31; border: 1px solid #313954; border-radius: 8px; font-size: 13px; font-weight: 700; }
  .minimum-note { margin: 6px 0 0; color: #817c91; font-size: 11px; }
  .amount-meta { display: flex; justify-content: space-between; align-items: center; gap: 12px; min-height: 30px; color: #8f8aa1; font-size: 12px; }
  .amount-actions { display: flex; align-items: center; gap: 18px; }
  .amount-actions .text-button { margin: 0; }
  input { min-width: 0; width: 100%; color: #f1edff; background: transparent; border: 0; outline: none; font-size: 18px; }
  .destination { display: grid; gap: 8px; margin: 18px 0; padding: 14px 16px; background: #0b1020; border-radius: 10px; }
  code { color: #b8b2ca; font-size: 12px; overflow-wrap: anywhere; }
  .wallet-row { margin: 20px 0; }
  .secondary { padding: 10px 14px; background: #171d31; border: 1px solid #313954; border-radius: 9px; color: #d9d4e9; cursor: pointer; }
  .inline-hint, .fee-note, .small-note { color: #8f8aa1; font-size: 12px; line-height: 1.6; }
  .fee-note { margin: 18px 0; }
  .primary { width: 100%; min-height: 52px; border: 0; border-radius: 10px; background: linear-gradient(90deg, #24c69a, #8858ed); color: white; font-weight: 800; cursor: pointer; }
  .address-input { height: 54px; padding: 0 16px; border: 1px solid #242c43; border-radius: 10px; background: #0d1222; font-size: 14px; }
  .quote-button { width: 100%; margin: 4px 0 14px; }
  .quote-card { display: grid; gap: 10px; margin: 14px 0; padding: 16px; border: 1px solid #28334c; border-radius: 10px; background: #0d1222; }
  .quote-card > div { display: flex; justify-content: space-between; gap: 12px; color: #a5a0b5; font-size: 12px; }
  .quote-card strong { color: #e4e0ef; text-align: right; }
  .quote-card p { margin: 0; color: #8f8aa1; font-size: 11px; }
  .redeem-balance { padding: 14px 0; border-top: 1px solid #20263b; }
  .tx-line { color: #8f8aa1; font-size: 12px; overflow-wrap: anywhere; }
  a { color: #59d9b0; }
  .small-note { margin-top: 16px; }
  .recovery-button { margin: 12px 0 0; color: #ffb3c5; text-decoration: underline; }
  .alert { margin-top: 16px; padding: 13px 15px; border-radius: 10px; font-size: 13px; line-height: 1.5; overflow-wrap: anywhere; }
  .notice { border: 1px solid #1c735d; background: #0e2825; color: #9cebd0; }
  .error { border: 1px solid #7b3c54; background: #2d1723; color: #ffb3c5; }
  .success-box { color: #a8efda; border-color: #1c735d; }
  .disclaimer { max-width: 740px; margin: 22px auto 0; color: #6f6b7e; text-align: center; font-size: 11px; line-height: 1.7; }
  @media (max-width: 640px) { .minter-page { padding: 22px 14px 50px; } .card { padding: 20px 16px; } .wallet-summary { align-items: flex-start; } .wallet-summary strong { max-width: 47vw; } .steps { gap: 5px; font-size: 10px; } .steps i { min-width: 8px; } .wallet-row { align-items: flex-start; flex-wrap: wrap; } }
</style>
