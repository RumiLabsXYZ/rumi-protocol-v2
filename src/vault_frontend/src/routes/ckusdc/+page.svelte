<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import { isConnected as isConnectedStore, principal as principalStore } from '$lib/stores/wallet';
  import { CANISTER_IDS } from '$lib/config';
  import {
    CKUSDC,
    CKERC20_MINTER_DASHBOARD,
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
  let evmAccount = '';
  let evmMessage = '';
  let usdcBalance: bigint | null = null;
  let usdcBalanceBusy = false;
  let usdcBalanceError = '';
  let usdcBalanceRequestId = 0;
  let minterReady = false;
  let minterError = '';
  let helperAddress = '';
  let ckUsdcBalance: bigint | null = null;
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
  let pendingDeposit: { hash: string; amount: string; recipient: string; evmAccount: string; principal: string; createdAt: number } | null = null;
  let pendingWithdrawal: { amount: string; recipient: string; owner: string; createdAt: number } | null = null;
  let redeemIndices: { ckEth: bigint; ckUsdc: bigint } | null = null;
  let destroyed = false;

  const unsubs: Array<() => void> = [];
  let previousPrincipal = '';
  let ethereumProvider: Eip1193Provider | undefined;
  let accountChangedListener: ((accounts: unknown) => void) | null = null;
  let chainChangedListener: ((chainId: unknown) => void) | null = null;

  function depositStorageKey(evm: string, principal: string) {
    return `rumi:ckusdc:pending-deposit:${evm.toLowerCase()}:${principal}`;
  }

  function requirePersistentOperationState() {
    const key = `rumi:ckusdc:storage-check:${Date.now()}`;
    localStorage.setItem(key, 'ready');
    localStorage.removeItem(key);
  }

  function syncPendingDeposit() {
    pendingDeposit = null;
    if (!evmAccount || !ownerPrincipal || ownerPrincipal.isAnonymous()) return;
    try {
      const value = localStorage.getItem(depositStorageKey(evmAccount, ownerPrincipal.toText()));
      if (value) pendingDeposit = JSON.parse(value);
    } catch { /* Storage may be unavailable; the in-memory guard still applies. */ }
  }

  function savePendingDeposit(hash: string, amount: string, recipient: string, evm: string, principal: string) {
    pendingDeposit = { hash, amount, recipient, evmAccount: evm, principal, createdAt: Date.now() };
    try { localStorage.setItem(depositStorageKey(evm, principal), JSON.stringify(pendingDeposit)); } catch { /* The unresolved intent written before wallet dispatch remains as a conservative lock. */ }
  }

  function beginPendingDeposit(amount: string, recipient: string, evm: string, principal: string) {
    const pending = { hash: '', amount, recipient, evmAccount: evm, principal, createdAt: Date.now() };
    localStorage.setItem(depositStorageKey(evm, principal), JSON.stringify(pending));
    pendingDeposit = pending;
  }

  function clearPendingDepositMarker(evm: string, principal: string) {
    try { localStorage.removeItem(depositStorageKey(evm, principal)); } catch { /* Keep a stale persistent lock rather than risk a retry. */ }
    if (pendingDeposit?.evmAccount.toLowerCase() === evm.toLowerCase() && pendingDeposit?.principal === principal) pendingDeposit = null;
  }

  function clearPendingDeposit() {
    if (!pendingDeposit) return;
    const confirmed = window.confirm('Only clear this retry lock after checking Ethereum wallet activity and confirming the deposit did not complete. Clearing it while the transaction is pending could allow a duplicate deposit.');
    if (!confirmed) return;
    clearPendingDepositMarker(pendingDeposit.evmAccount, pendingDeposit.principal);
  }

  function withdrawalStorageKey(owner: string) {
    return `rumi:ckusdc:pending-withdrawal:${owner}`;
  }

  function syncPendingWithdrawal(owner: Principal | null) {
    pendingWithdrawal = null;
    if (!owner || owner.isAnonymous()) return;
    try {
      const value = localStorage.getItem(withdrawalStorageKey(owner.toText()));
      if (value) pendingWithdrawal = JSON.parse(value);
    } catch { /* Storage may be unavailable; the in-memory guard still applies. */ }
  }

  function markWithdrawalPending(amount: bigint, recipient: string, owner: Principal) {
    const pending = { amount: amount.toString(), recipient, owner: owner.toText(), createdAt: Date.now() };
    localStorage.setItem(withdrawalStorageKey(owner.toText()), JSON.stringify(pending));
    pendingWithdrawal = pending;
  }

  function clearPendingWithdrawal() {
    if (!pendingWithdrawal) return;
    const confirmed = window.confirm('Only clear this retry lock after checking the ckUSDC and ckETH ledger history and the minter dashboard to confirm the request was not accepted. Clearing it without reconciliation could burn the amount twice.');
    if (!confirmed) return;
    try { localStorage.removeItem(withdrawalStorageKey(pendingWithdrawal.owner)); } catch { /* The current page lock is still cleared below. */ }
    pendingWithdrawal = null;
  }

  function resetForWallet() {
    ckUsdcBalance = null;
    ckEthBalance = null;
    notice = '';
    error = '';
    approveHash = '';
    depositHash = '';
    redeemIndices = null;
    withdrawalQuote = null;
    minterReady = false;
    minterError = '';
    helperAddress = '';
  }

  onMount(() => {
    unsubs.push(isConnectedStore.subscribe((value) => {
      connected = value;
      if (!value) resetForWallet();
      else if (ownerPrincipal && !ownerPrincipal.isAnonymous()) void loadMinterInfo(ownerPrincipal);
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
      if (value && !value.isAnonymous()) void loadMinterInfo(value);
    }));
    ethereumProvider = getEthereumProvider();
    accountChangedListener = (accounts) => {
      usdcBalanceRequestId += 1;
      usdcBalanceBusy = false;
      evmAccount = Array.isArray(accounts) && accounts[0] ? String(accounts[0]) : '';
      syncPendingDeposit();
      usdcBalance = null;
      usdcBalanceError = '';
      evmMessage = '';
      if (evmAccount) void refreshUsdcBalance(evmAccount);
    };
    chainChangedListener = (chainId) => {
      usdcBalanceRequestId += 1;
      usdcBalanceBusy = false;
      usdcBalance = null;
      usdcBalanceError = '';
      if (chainId === '0x1') {
        evmMessage = '';
        if (evmAccount) void refreshUsdcBalance(evmAccount);
      } else {
        evmMessage = 'Choose Ethereum Mainnet in your EVM wallet to read USDC and deposit.';
      }
    };
    ethereumProvider?.on?.('accountsChanged', accountChangedListener);
    ethereumProvider?.on?.('chainChanged', chainChangedListener);
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
          if (evmAccount) await refreshUsdcBalance(evmAccount);
        } else if (evmAccount) {
          evmMessage = 'Choose Ethereum Mainnet in your EVM wallet to read USDC and deposit.';
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

  async function refreshUsdcBalance(account = evmAccount) {
    const provider = getEthereumProvider();
    if (!provider || !account) return;
    const requestId = ++usdcBalanceRequestId;
    usdcBalanceBusy = true;
    usdcBalanceError = '';
    try {
      const chainId = await provider.request({ method: 'eth_chainId' });
      if (chainId !== '0x1') {
        if (requestId === usdcBalanceRequestId) {
          usdcBalance = null;
          evmMessage = 'Choose Ethereum Mainnet in your EVM wallet to read USDC and deposit.';
        }
        return;
      }
      const data = `0x70a08231${encodeAddressWord(account)}`;
      const rawBalance = await provider.request({ method: 'eth_call', params: [{ to: CKUSDC.erc20Address, data }, 'latest'] });
      if (typeof rawBalance !== 'string' || !/^0x[0-9a-fA-F]+$/.test(rawBalance)) {
        throw new Error('The Ethereum wallet returned an invalid USDC balance.');
      }
      if (!destroyed && requestId === usdcBalanceRequestId && evmAccount.toLowerCase() === account.toLowerCase()) {
        usdcBalance = BigInt(rawBalance);
        evmMessage = '';
      }
    } catch (cause) {
      if (!destroyed && requestId === usdcBalanceRequestId) {
        usdcBalance = null;
        usdcBalanceError = cause instanceof Error ? cause.message : 'Could not read USDC balance from Ethereum.';
      }
    } finally {
      if (requestId === usdcBalanceRequestId) usdcBalanceBusy = false;
    }
  }

  function setMaxDeposit() {
    if (usdcBalance === null || usdcBalance <= 0n) return;
    depositAmount = formatTokenAmount(usdcBalance, CKUSDC.decimals, CKUSDC.decimals);
  }

  async function loadMinterInfo(principal: Principal) {
    minterError = '';
    minterReady = false;
    try {
      const actor = await getCkErc20MinterActor();
      const info = await actor.get_minter_info();
      if (destroyed || ownerPrincipal?.toText() !== principal.toText()) return;
      assertTokenSupported(info, CKUSDC);
      const helper = info.deposit_with_subaccount_helper_contract_address?.[0];
      if (!helper || !/^0x[0-9a-fA-F]{40}$/.test(helper)) throw new Error('The minter did not return a valid live Ethereum helper address.');
      helperAddress = helper;
      minterReady = true;
      await refreshBalances(principal);
    } catch (cause) {
      if (destroyed) return;
      minterError = cause instanceof Error ? cause.message : 'Could not read ckERC20 minter configuration.';
    }
  }

  async function refreshBalances(principal = ownerPrincipal) {
    if (!principal || principal.isAnonymous() || refreshBusy) return;
    refreshBusy = true;
    try {
      const [usdcLedger, ethLedger] = await Promise.all([
        getCkErc20LedgerActor(CKUSDC.ledgerId),
        getCkErc20LedgerActor(CANISTER_IDS.CKETH_LEDGER),
      ]);
      const [usdc, eth] = await Promise.all([
        usdcLedger.icrc1_balance_of({ owner: principal, subaccount: [] }),
        ethLedger.icrc1_balance_of({ owner: principal, subaccount: [] }),
      ]);
      if (destroyed || ownerPrincipal?.toText() !== principal.toText()) return;
      ckUsdcBalance = BigInt(usdc);
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
      evmMessage = 'Install or unlock an EVM wallet such as Rabby or MetaMask to deposit USDC.';
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
      await refreshUsdcBalance(evmAccount);
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
    if (pendingDeposit) { error = pendingDeposit.hash ? `A deposit transaction is still unresolved: ${pendingDeposit.hash}. Check its status before starting another deposit.` : 'A prior deposit submission has no confirmed result. Check Ethereum wallet activity before retrying.'; return; }
    if (!connected || !ownerPrincipal || ownerPrincipal.isAnonymous()) {
      error = 'Connect an Internet Identity or another Rumi wallet first. This wallet receives ckUSDC.';
      return;
    }
    if (!minterReady || !helperAddress) { error = minterError || 'The live ckERC20 minter configuration is not ready.'; return; }
    if (!evmAccount) { evmMessage = 'Connect an Ethereum wallet to pay for the USDC approval and deposit transactions.'; return; }
    let amount: bigint;
    try { amount = parseTokenAmount(depositAmount); }
    catch (cause) { error = cause instanceof Error ? cause.message : 'Invalid amount.'; return; }
    const liveOwner = ownerPrincipal;
    const locks = (navigator as any).locks;
    if (!locks?.request) { error = 'This browser cannot safely coordinate minter transactions across tabs. Use a supported browser with Web Locks enabled.'; return; }
    busy = true;
    try {
      await locks.request(`rumi:ckusdc:deposit:${evmAccount.toLowerCase()}`, { mode: 'exclusive', ifAvailable: true }, async (lock: unknown) => {
      if (!lock) throw new Error('Another ckUSDC deposit is already active in another tab. Wait for it to finish, then check its status.');
      syncPendingDeposit();
      if (pendingDeposit) throw new Error(pendingDeposit.hash ? `A deposit transaction is still unresolved: ${pendingDeposit.hash}. Check its status before starting another deposit.` : 'A prior deposit submission has no confirmed result. Check Ethereum wallet activity before retrying.');
      requirePersistentOperationState();
      const minterActor = await getCkErc20MinterActor();
      const currentInfo = await minterActor.get_minter_info();
      assertTokenSupported(currentInfo, CKUSDC);
      const transactionHelper = currentInfo.deposit_with_subaccount_helper_contract_address?.[0];
      if (!transactionHelper || !/^0x[0-9a-fA-F]{40}$/.test(transactionHelper)) throw new Error('The minter did not return a valid live Ethereum helper address.');
      helperAddress = transactionHelper;
      const allowanceData = `0xdd62ed3e${encodeAddressWord(evmAccount)}${encodeAddressWord(transactionHelper)}`;
      const provider = getEthereumProvider();
      if (!provider) throw new Error('Connect an Ethereum wallet before checking its USDC allowance.');
      const allowanceHex = await provider.request({ method: 'eth_call', params: [{ to: CKUSDC.erc20Address, data: allowanceData }, 'latest'] });
      const existingAllowance = BigInt(String(allowanceHex));
      if (existingAllowance > 0n) {
        notice = 'Reset the existing USDC allowance to zero before setting this deposit amount.';
        const resetData = `0x095ea7b3${encodeAddressWord(transactionHelper)}${encodeUint256(0n)}`;
        approveHash = await submitEthereumTransaction(CKUSDC.erc20Address, resetData, (hash) => approveHash = hash);
      }
      const approveData = `0x095ea7b3${encodeAddressWord(transactionHelper)}${encodeUint256(amount)}`;
      notice = 'Approve the exact USDC amount in your Ethereum wallet.';
      approveHash = await submitEthereumTransaction(CKUSDC.erc20Address, approveData, (hash) => approveHash = hash);
      if (ownerPrincipal?.toText() !== liveOwner.toText()) throw new Error(`USDC approval confirmed, but the receiving Internet Identity changed. No deposit was submitted. Approval transaction: ${approveHash}`);
      const latestInfo = await minterActor.get_minter_info();
      assertTokenSupported(latestInfo, CKUSDC);
      const latestHelper = latestInfo.deposit_with_subaccount_helper_contract_address?.[0];
      if (!latestHelper || latestHelper.toLowerCase() !== transactionHelper.toLowerCase()) {
        throw new Error(`USDC approval confirmed for ${transactionHelper}, but the minter helper changed. No deposit was submitted. Check the helper and allowance before continuing. Approval transaction: ${approveHash}`);
      }
      notice = 'USDC approval confirmed. Confirm the deposit transaction in your Ethereum wallet.';
      const liveRecipient = liveOwner.toText();
      const liveDepositKey = depositStorageKey(evmAccount, liveRecipient);
      depositHash = await submitEthereumTransaction(
        latestHelper,
        encodeDepositErc20(CKUSDC, amount, liveOwner),
        (hash) => { depositHash = hash; savePendingDeposit(hash, depositAmount, liveRecipient, evmAccount, liveRecipient); },
        () => {
          try { localStorage.removeItem(liveDepositKey); } catch { /* Continue with confirmed receipt. */ }
          pendingDeposit = null;
        },
        () => beginPendingDeposit(depositAmount, liveRecipient, evmAccount, liveRecipient),
        () => clearPendingDepositMarker(evmAccount, liveRecipient),
      );
      notice = 'Ethereum deposit confirmed. The minter still needs to detect and mint ckUSDC; refresh the ICP balance to confirm arrival.';
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
    if (pendingWithdrawal && pendingWithdrawal.owner === ownerPrincipal?.toText()) {
      error = 'A prior withdrawal request has no confirmed response. Reconcile its ledger activity and minter status before retrying.';
      return;
    }
    if (!connected || !ownerPrincipal || ownerPrincipal.isAnonymous()) {
      error = 'Connect the Rumi wallet holding ckUSDC and ckETH first.';
      return;
    }
    if (!validateEthereumAddress(redeemAddress)) { error = 'Enter a valid Ethereum destination address.'; return; }
    let amount: bigint;
    try { amount = parseTokenAmount(redeemAmount); }
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
      await locks.request(`rumi:ckusdc:withdrawal:${owner.toText()}`, { mode: 'exclusive', ifAvailable: true }, async (lock: unknown) => {
      if (!lock) throw new Error('Another ckUSDC withdrawal is active in another tab. Wait for it to finish, then reconcile its status.');
      syncPendingWithdrawal(owner);
      if (pendingWithdrawal?.owner === owner.toText()) throw new Error('A prior withdrawal request has no confirmed response. Reconcile its ledger activity and minter status before retrying.');
      requirePersistentOperationState();
      const isLive = () => !destroyed && ownerPrincipal?.toText() === owner.toText();
      notice = 'Review and approve the ckETH transaction fee allowance in your wallet.';
      const recipient = redeemAddress.trim();
      const result = await approveAndWithdrawCkErc20({
        token: CKUSDC, amount, recipient, owner, quote, isLive,
        onWithdrawalSubmitted: () => markWithdrawalPending(amount, recipient, owner),
        onWithdrawalResolved: () => {
          try { localStorage.removeItem(withdrawalStorageKey(owner.toText())); } catch { /* Continue with resolved response. */ }
          pendingWithdrawal = null;
        },
      });
      redeemIndices = { ckEth: result.ckEthBurnBlock, ckUsdc: result.ckUsdcBurnBlock };
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
    if (!ownerPrincipal || ownerPrincipal.isAnonymous()) { error = 'Connect the Rumi wallet that holds ckUSDC and ckETH first.'; return; }
    let amount: bigint;
    try { amount = parseTokenAmount(redeemAmount); }
    catch (cause) { error = cause instanceof Error ? cause.message : 'Invalid amount.'; return; }
    quoteBusy = true;
    try {
      const owner = ownerPrincipal;
      const quote = await getCkErc20WithdrawalQuote(CKUSDC, amount, owner);
      if (destroyed || ownerPrincipal?.toText() !== owner.toText()) return;
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
  <title>ckUSDC Minter | Rumi</title>
  <meta name="description" content="Mint ckUSDC on the Internet Computer from Ethereum USDC using Rumi." />
</svelte:head>

<main class="minter-page">
  <a class="back-link" href="/">← Rumi</a>
  <header class="hero">
    <div class="coin-mark">$</div>
    <p class="eyebrow">ETHEREUM ↔ INTERNET COMPUTER</p>
    <h1>ckUSDC Minter</h1>
    <p class="subtitle">Move USDC to the Internet Computer and redeem it back to Ethereum.</p>
    <p class="powered">Rumi’s minter experience uses DFINITY’s ckERC20 minter.</p>
  </header>

  <section class="card" aria-label="ckUSDC mint and redeem">
    <div class="wallet-summary">
      <div>
        <span class="label">RECEIVING INTERNET IDENTITY</span>
        <strong>{#if connected && ownerPrincipal}{ownerPrincipal.toText()}{:else}Connect a Rumi wallet{/if}</strong>
      </div>
      <div class="balance-box">
        <span class="label">CKUSDC BALANCE</span>
        <strong>{ckUsdcBalance === null ? '—' : `${formatTokenAmount(ckUsdcBalance)} ckUSDC`}</strong>
        <button class="text-button" disabled={!ownerPrincipal || refreshBusy} on:click={() => refreshBalances()}>{refreshBusy ? 'Refreshing…' : 'Refresh'}</button>
      </div>
    </div>

    <div class="tabs" role="tablist" aria-label="Minter direction">
      <button role="tab" aria-selected={activeTab === 'mint'} class:active={activeTab === 'mint'} on:click={() => activeTab = 'mint'}>Mint ckUSDC</button>
      <button role="tab" aria-selected={activeTab === 'redeem'} class:active={activeTab === 'redeem'} on:click={() => activeTab = 'redeem'}>Redeem USDC</button>
    </div>

    {#if !connected}
      <div class="wallet-hint">Connect your Internet Identity or Rumi wallet with the wallet button in the header. That identity receives ckUSDC and signs any ICP approvals.</div>
    {/if}

    {#if minterError}
      <div class="alert error">Could not verify the live ckERC20 minter configuration: {minterError}</div>
    {:else if connected && !minterReady}
      <div class="wallet-hint">Checking live ckUSDC support and the current helper address…</div>
    {/if}

    {#if activeTab === 'mint'}
      <div class="flow-label">USDC → CKUSDC</div>
      <div class="steps"><span class="step-current">1&nbsp; Approve USDC</span><i></i><span>2&nbsp; Deposit USDC</span><i></i><span>3&nbsp; ckUSDC minted</span></div>
      <div class="risk-note">Send only Ethereum Mainnet USDC through this page. Minting may take around 20 minutes after Ethereum finality. The minter helper and supported token are checked live before transactions are enabled.</div>

      <label class="field-label" for="deposit-amount">USDC amount</label>
      <div class="amount-input"><input id="deposit-amount" type="text" inputmode="decimal" autocomplete="off" placeholder="0.00" bind:value={depositAmount} disabled={busy} /><span>USDC</span></div>
      <div class="amount-meta">
        <span>{!evmAccount ? 'Connect an Ethereum wallet to view its USDC balance' : usdcBalanceBusy ? 'Reading wallet balance…' : usdcBalance === null ? 'USDC wallet balance unavailable' : `Wallet balance: ${formatTokenAmount(usdcBalance, CKUSDC.decimals)} USDC`}</span>
        <div class="amount-actions">
          <button class="text-button" on:click={() => refreshUsdcBalance()} disabled={!evmAccount || usdcBalanceBusy || busy}>{usdcBalanceBusy ? 'Refreshing…' : 'Refresh'}</button>
          <button class="text-button" on:click={setMaxDeposit} disabled={usdcBalance === null || usdcBalance <= 0n || busy}>Max</button>
        </div>
      </div>
      {#if usdcBalanceError}<p class="inline-hint">Could not read the Ethereum USDC balance: {usdcBalanceError}</p>{/if}
      <div class="destination"><span class="label">CKUSDC WILL BE MINTED TO</span><code>{connected && ownerPrincipal ? ownerPrincipal.toText() : 'Connect a Rumi wallet to choose the recipient'}</code></div>

      <div class="wallet-row">
        <div><span class="label">ETHEREUM WALLET</span><strong>{evmAccount ? `${evmAccount.slice(0, 7)}…${evmAccount.slice(-5)}` : 'Not connected'}</strong></div>
        {#if !evmAccount}<button class="secondary" on:click={connectEthereum} disabled={busy}>Connect Ethereum wallet</button>{/if}
      </div>
      {#if evmMessage}<p class="inline-hint">{evmMessage}</p>{/if}
      <p class="fee-note">You need ETH in this Ethereum wallet for gas. The flow usually takes two transactions; if USDC already has an allowance for the minter helper, it first resets that allowance to zero, so it can take three. Rumi does not sponsor Ethereum gas in this flow.</p>
      <button class="primary" on:click={submitDeposit} disabled={busy || !!pendingDeposit || !connected || !minterReady || !evmAccount}>{pendingDeposit ? 'Check pending deposit before retrying' : busy ? 'Waiting for wallet…' : 'Approve and mint ckUSDC'}</button>
      {#if approveHash}<p class="tx-line">USDC approval: <a href={`https://etherscan.io/tx/${approveHash}`} target="_blank" rel="noreferrer">{approveHash.slice(0, 14)}…</a></p>{/if}
      {#if depositHash}<p class="tx-line">Deposit transaction: <a href={`https://etherscan.io/tx/${depositHash}`} target="_blank" rel="noreferrer">{depositHash.slice(0, 14)}…</a></p>{/if}
      {#if pendingDeposit}<div class="alert error">{#if pendingDeposit.hash}A previous deposit transaction has no confirmed receipt yet: <a href={`https://etherscan.io/tx/${pendingDeposit.hash}`} target="_blank" rel="noreferrer">check Ethereum status</a>.{:else}A previous deposit submission did not return a transaction hash. Check the connected Ethereum wallet's activity.{/if} New deposits are locked to prevent a duplicate. After checking the transaction, use the recovery control only if you confirmed it did not complete.<button class="text-button recovery-button" on:click={clearPendingDeposit}>Clear retry lock after reconciliation</button></div>{/if}
      {#if helperAddress}<p class="small-note">Live minter helper: <code>{helperAddress}</code></p>{/if}
    {:else}
      <div class="flow-label">CKUSDC → USDC</div>
      <div class="steps"><span class="step-current">1&nbsp; Approve ckETH fee</span><i></i><span>2&nbsp; Approve ckUSDC</span><i></i><span>3&nbsp; Ethereum payout</span></div>
      <div class="risk-note">Redeeming requires ckUSDC and ckETH. ckETH pays the Ethereum transaction fee through the DFINITY minter; your own Ethereum wallet does not sign or pay gas for the payout.</div>
      <label class="field-label" for="redeem-amount">ckUSDC amount</label>
      <div class="amount-input"><input id="redeem-amount" type="text" inputmode="decimal" autocomplete="off" placeholder="0.00" bind:value={redeemAmount} on:input={invalidateWithdrawalQuote} disabled={busy} /><span>ckUSDC</span></div>
      <label class="field-label" for="redeem-address">Ethereum destination</label>
      <input id="redeem-address" class="address-input" type="text" autocomplete="off" spellcheck="false" placeholder="0x…" bind:value={redeemAddress} disabled={busy} />
      <div class="wallet-row redeem-balance"><div><span class="label">CKETH FEE BALANCE</span><strong>{ckEthBalance === null ? '—' : `${formatTokenAmount(ckEthBalance, 18, 8)} ckETH`}</strong></div><button class="text-button" disabled={!ownerPrincipal || refreshBusy} on:click={() => refreshBalances()}>{refreshBusy ? 'Refreshing…' : 'Refresh balances'}</button></div>
      <button class="secondary quote-button" on:click={refreshWithdrawalQuote} disabled={quoteBusy || busy || !connected}>{quoteBusy ? 'Loading current fees…' : 'Get current fee quote'}</button>
      {#if withdrawalQuote}
        <div class="quote-card">
          <span class="label">CURRENT WITHDRAWAL QUOTE · REFRESHED {new Date(withdrawalQuote.quotedAtMs).toLocaleTimeString()}{#if withdrawalQuote.minterPriceTimestampMs} · MINTER PRICE {new Date(withdrawalQuote.minterPriceTimestampMs).toLocaleTimeString()}{/if}</span>
          <div><span>ckUSDC amount</span><strong>{formatTokenAmount(withdrawalQuote.amount)} ckUSDC</strong></div>
          <div><span>ckETH max Ethereum fee</span><strong>{formatTokenAmount(withdrawalQuote.maxTransactionFee, 18, 8)} ckETH</strong></div>
          <div><span>ckETH allowance cap (includes ledger fee)</span><strong>{formatTokenAmount(withdrawalQuote.ckEthAllowance, 18, 8)} ckETH</strong></div>
          <div><span>ckUSDC allowance cap (includes ledger fee)</span><strong>{formatTokenAmount(withdrawalQuote.ckTokenAllowance)} ckUSDC</strong></div>
          <p>{withdrawalQuoteIsExecutable(withdrawalQuote) ? 'Balances cover this quote. Quote expires in 60 seconds.' : 'Your current balances do not cover this quote.'}</p>
        </div>
      {/if}
      {#if pendingWithdrawal && pendingWithdrawal.owner === ownerPrincipal?.toText()}<div class="alert error">A previous withdrawal request has no confirmed response. Check the ckUSDC and ckETH ledger activity and the <a href={CKERC20_MINTER_DASHBOARD} target="_blank" rel="noreferrer">minter dashboard</a> before retrying. A lock prevents an accidental second burn.<button class="text-button recovery-button" on:click={clearPendingWithdrawal}>Clear retry lock after reconciliation</button></div>{/if}
      <button class="primary" on:click={submitWithdrawal} disabled={busy || pendingWithdrawal?.owner === ownerPrincipal?.toText() || !connected || !minterReady || !withdrawalQuote || !withdrawalQuoteIsExecutable(withdrawalQuote)}>{pendingWithdrawal?.owner === ownerPrincipal?.toText() ? 'Reconcile previous request first' : busy ? 'Confirm approvals in your wallet…' : 'Approve and request USDC redemption'}</button>
      {#if redeemIndices}<div class="success-box">Withdrawal request accepted by the minter.<br />ckETH fee burn block: {redeemIndices.ckEth}<br />ckUSDC burn block: {redeemIndices.ckUsdc}<br /><a href={CKERC20_MINTER_DASHBOARD} target="_blank" rel="noreferrer">Open ckERC20 minter dashboard</a></div>{/if}
      <p class="small-note">Before any approval, the page reads current ckETH and ckUSDC ledger fees, your balances, and the minter’s current Ethereum fee estimate. Approvals are limited to this withdrawal and expire after 10 minutes.</p>
    {/if}

    {#if notice}<div class="alert notice" aria-live="polite">{notice}</div>{/if}
    {#if error}<div class="alert error" role="alert">{error}</div>{/if}
  </section>

  <footer class="disclaimer">ckUSDC is minted by DFINITY’s ckETH minter against Ethereum USDC deposits. Confirm the destination identity, asset, and network before every transaction. Deposits and redemptions are subject to Ethereum finality and minter processing.</footer>
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
  .amount-meta { display: flex; justify-content: space-between; align-items: center; gap: 12px; min-height: 30px; color: #8f8aa1; font-size: 12px; }
  .amount-actions { display: flex; align-items: center; gap: 18px; }
  .amount-actions .text-button { margin: 0; }
  input { min-width: 0; width: 100%; color: #f1edff; background: transparent; border: 0; outline: none; font-size: 18px; }
  .amount-input span { color: #a4a0b3; font-weight: 700; }
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
