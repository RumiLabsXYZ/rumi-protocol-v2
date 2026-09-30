<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import QRCode from 'qrcode';
  import { isConnected as connectedStore, principal as principalStore } from '$lib/stores/wallet';
  import { CANISTER_IDS, CONFIG } from '$lib/config';
  import { ICRC1_IDL } from '$lib/idls/ledger.idl.js';
  import { fetchLedgerFeeStrict } from '$lib/services/ledgerFeeService';
  import {
    getPublicCkbtcLedgerActor,
    getPublicCkbtcMinterActor,
    submitCkbtcWithdrawal,
    updateBtcBalanceForOwner,
  } from '$lib/services/ckbtcMinterActors';
  import { formatBtcSats, isValidBitcoinMainnetAddress, parseBtcSats, txidBytesToHex } from '$lib/utils/bitcoinMinterFlow';

  const ledgerFeeRef = { ledgerId: CANISTER_IDS.CKBTC_LEDGER, decimals: 8, symbol: 'ckBTC' };
  const POLL_MS = 60_000;
  const POLL_LIMIT = 120;
  const NETWORK_KEY = CONFIG.isLocal ? `local:${CONFIG.host}` : 'mainnet';

  let connected = false;
  let principal: Principal | null = null;
  let ownerText = '';
  let lastOwnerText = '';
  let lastConnected = false;
  let clockNow = Date.now();
  let clockTimer: ReturnType<typeof setInterval> | null = null;
  let generation = 0;
  let destroyed = false;
  let mode: 'mint' | 'redeem' = 'mint';
  let address = '';
  let qrDataUrl = '';
  let amount = '';
  let recipient = '';
  let minterInfo: any = null;
  let depositFee: bigint | null = null;
  let balance: bigint | null = null;
  let ledgerFee: bigint | null = null;
  let quote: { bitcoinFee: bigint; minterFee: bigint; amount: bigint; address: string; owner: string; at: number } | null = null;
  let busyAddress = false;
  let mintBusy = false;
  let withdrawalBusy = false;
  let statusBusy = false;
  let statusBlock: bigint | null = null;
  let statusText = '';
  let blockedAttempt: { owner: string; address: string; amount: string; startedAt: number; attemptId: string; state: 'uncertain' | 'accepted'; blockIndex?: string } | null = null;
  let txid = '';
  let mintText = '';
  let error = '';
  let notice = '';
  let mintTimer: ReturnType<typeof setTimeout> | null = null;
  let statusTimer: ReturnType<typeof setTimeout> | null = null;
  let mintAttempt = 0;
  let statusAttempt = 0;
  let mintOwner = '';
  let withdrawalOwner = '';
  let mintStopped = false;
  let mintConfirmations: number | null = null;
  let mintRequiredConfirmations: number | null = null;
  $: mintStepIndex = mintText.startsWith('Minted ') ? 3 : mintConfirmations !== null ? 2 : 1;

  $: connected = $connectedStore;
  $: principal = $principalStore;
  $: ownerText = principal?.toText() ?? '';
  $: if (ownerText !== lastOwnerText || connected !== lastConnected) resetForOwnerChange();
  $: parsedAmount = parseBtcSats(amount);
  $: quoteFresh = !!quote && quote.owner === ownerText && quote.amount === parsedAmount && quote.address === recipient.trim() && clockNow - quote.at <= 60_000;
  $: receivedEstimate = quoteFresh && quote ? formatBtcSats(parsedAmount! > quote.bitcoinFee + quote.minterFee ? parsedAmount! - quote.bitcoinFee - quote.minterFee : 0n) : null;

  onMount(() => {
    void loadMinterInfo();
    clockTimer = setInterval(() => { clockNow = Date.now(); }, 1000);
  });
  onDestroy(() => {
    destroyed = true;
    generation++;
    clearTimers();
    if (clockTimer) clearInterval(clockTimer);
  });

  function resetForOwnerChange() {
    lastOwnerText = ownerText;
    lastConnected = connected;
    generation++;
    clearTimers();
    address = '';
    qrDataUrl = '';
    amount = '';
    recipient = '';
    balance = null;
    ledgerFee = null;
    quote = null;
    statusBlock = null;
    statusText = '';
    blockedAttempt = readBlockedAttempt(ownerText);
    txid = '';
    mintText = '';
    error = '';
    notice = '';
    mintBusy = false;
    withdrawalBusy = false;
    busyAddress = false;
    mintAttempt = 0;
    statusAttempt = 0;
    mintStopped = false;
    mintConfirmations = null;
    mintRequiredConfirmations = minterInfo?.min_confirmations ?? null;
    if (connected && principal && !principal.isAnonymous()) void refreshBalance(principal, generation);
    if (blockedAttempt?.state === 'accepted' && blockedAttempt.blockIndex) {
      statusBlock = BigInt(blockedAttempt.blockIndex);
      withdrawalOwner = ownerText;
      statusText = 'Restored a previously accepted withdrawal. Checking its current minter status.';
      void refreshWithdrawalStatus(generation, statusBlock);
    } else if (blockedAttempt?.state === 'uncertain') {
      statusText = 'A previous withdrawal has an uncertain result. Reconcile its status before starting another request.';
    }
  }

  function clearTimers() {
    if (mintTimer) clearTimeout(mintTimer);
    if (statusTimer) clearTimeout(statusTimer);
    mintTimer = null;
    statusTimer = null;
  }

  function storageKey(owner: string): string { return `rumi:ckbtc-withdrawal:${NETWORK_KEY}:${CANISTER_IDS.CKBTC_LEDGER}:${CANISTER_IDS.CKBTC_MINTER}:${owner}`; }

  function readBlockedAttempt(owner: string) {
    if (typeof localStorage === 'undefined' || !owner) return null;
    try {
      const raw = localStorage.getItem(storageKey(owner));
      if (!raw) return null;
      const saved = JSON.parse(raw);
      if (saved?.owner !== owner || !['uncertain', 'accepted'].includes(saved.state)) return null;
      if (typeof saved.attemptId !== 'string') return null;
      return saved as { owner: string; address: string; amount: string; startedAt: number; attemptId: string; state: 'uncertain' | 'accepted'; blockIndex?: string };
    } catch { return null; }
  }

  function saveBlockedAttempt(attempt: NonNullable<typeof blockedAttempt>, paint = connected && ownerText === attempt.owner): boolean {
    try {
      const saved = readBlockedAttempt(attempt.owner);
      if (saved && saved.attemptId !== attempt.attemptId) return false;
      localStorage.setItem(storageKey(attempt.owner), JSON.stringify(attempt));
      if (paint && connected && ownerText === attempt.owner) blockedAttempt = attempt;
      return true;
    }
    catch { error = 'The withdrawal safety record could not be saved in this browser. Check browser storage before continuing.'; return false; }
  }

  function clearBlockedAttempt(owner: string) {
    try { localStorage.removeItem(storageKey(owner)); } catch { /* keep the visible in-memory lock */ }
    if (connected && principal?.toText() === owner) {
      blockedAttempt = null;
      quote = null;
      balance = null;
      void refreshBalance(principal, generation);
    }
  }

  async function reconcileUncertainWithdrawal() {
    if (!blockedAttempt || blockedAttempt.state !== 'uncertain' || !principal || principal.toText() !== blockedAttempt.owner) return;
    const owner = blockedAttempt.owner;
    const token = generation;
    statusBusy = true;
    try {
      const actor = await getPublicCkbtcMinterActor();
      const rows = await actor.retrieve_btc_status_v2_by_account([{ owner: principal, subaccount: [] }]);
      if (!live(owner, token)) return;
      statusText = rows.length
        ? `The minter returned ${rows.length} request status record(s). Their destination and amount are not included in this query, so this browser cannot identify which request matches the saved attempt. Keep it blocked until you verify the saved request against minter history.`
        : 'No matching recent request was returned. An empty history result does not prove the call was not accepted; this account remains blocked from another withdrawal.';
    } catch (cause) {
      if (live(owner, token)) statusText = `Could not reconcile with minter history: ${readableError(cause)}. This account remains blocked.`;
    } finally {
      if (live(owner, token)) statusBusy = false;
    }
  }

  function live(owner: string, token: number): boolean {
    return !destroyed && generation === token && connected && ownerText === owner;
  }

  function isQuoteFresh(): boolean {
    return !!quote && quote.owner === ownerText && quote.amount === parsedAmount && quote.address === recipient.trim() && clockNow - quote.at <= 60_000;
  }

  async function loadMinterInfo() {
    try {
      const actor = await getPublicCkbtcMinterActor();
      const [info, fee] = await Promise.all([actor.get_minter_info(), actor.get_deposit_fee()]);
      if (destroyed) return;
      minterInfo = info;
      depositFee = BigInt(fee);
    } catch (cause) {
      if (!destroyed) error = `Could not load current ckBTC minter requirements: ${readableError(cause)}`;
    }
  }

  async function refreshBalance(owner = principal, token = generation) {
    if (!owner || owner.isAnonymous()) return;
    const key = owner.toText();
    try {
      const actor = await getPublicCkbtcLedgerActor();
      const next = BigInt(await actor.icrc1_balance_of({ owner, subaccount: [] }));
      if (live(key, token)) balance = next;
    } catch (cause) {
      if (live(key, token)) error = `Could not load your ckBTC balance: ${readableError(cause)}`;
    }
  }

  async function createAddress() {
    if (!connected || !principal || principal.isAnonymous() || busyAddress) {
      error = 'Connect the wallet that should receive ckBTC first.';
      return;
    }
    const owner = principal;
    const key = owner.toText();
    const token = generation;
    busyAddress = true;
    error = '';
    notice = '';
    try {
      const actor = await getPublicCkbtcMinterActor();
      const value = await actor.get_btc_address({ owner: [owner], subaccount: [] });
      if (!live(key, token)) return;
      const qr = await QRCode.toDataURL(`bitcoin:${value}`, {
        errorCorrectionLevel: 'M', margin: 1, width: 196,
        color: { dark: '#171717', light: '#f4f0e7' },
      });
      if (!live(key, token)) return;
      address = value;
      qrDataUrl = qr;
    } catch (cause) {
      if (live(key, token)) error = `Could not get a Bitcoin deposit address: ${readableError(cause)}`;
    } finally {
      if (live(key, token)) busyAddress = false;
    }
  }

  async function copyAddress() {
    if (!address) return;
    try { await navigator.clipboard.writeText(address); notice = 'Bitcoin address copied.'; }
    catch { error = 'Could not copy the address. Select and copy it manually.'; }
  }

  async function beginMintCheck() {
    if (!connected || !principal || principal.isAnonymous() || !address || mintBusy) return;
    if (mintTimer) clearTimeout(mintTimer);
    mintAttempt = 0;
    mintStopped = false;
    mintText = '';
    error = '';
    mintOwner = principal.toText();
    mintBusy = true;
    await runMintCheck(generation);
  }

  async function runMintCheck(token: number) {
    const owner = mintOwner;
    if (!live(owner, token)) return;
    mintAttempt++;
    try {
      const result = await updateBtcBalanceForOwner(Principal.fromText(owner), () => live(owner, token));
      if (!live(owner, token)) return;
      if ('Ok' in result) {
        const minted = result.Ok.filter((row: any) => 'Minted' in row);
        const total = minted.reduce((sum: bigint, row: any) => sum + BigInt(row.Minted.minted_amount), 0n);
        if (minted.length) {
          mintText = `Minted ${formatBtcSats(total)} ckBTC to ${owner}.`;
          mintConfirmations = mintRequiredConfirmations;
          mintBusy = false;
          mintStopped = true;
          await refreshBalance(principal, token);
          return;
        }
        const terminal = result.Ok.find((row: any) => 'Tainted' in row || 'ValueTooSmall' in row);
        if (terminal) {
          mintText = 'The minter returned a deposit that needs review. No further checks will run automatically.';
          mintBusy = false;
          mintStopped = true;
          return;
        }
        mintText = 'The minter checked a deposit and is still processing it.';
        mintConfirmations = mintRequiredConfirmations;
      } else if ('Err' in result) {
        const issue = result.Err;
        if ('NoNewUtxos' in issue) {
          const data = issue.NoNewUtxos;
          const confirmations = data.current_confirmations?.[0];
          mintConfirmations = confirmations ?? 0;
          mintRequiredConfirmations = Number(data.required_confirmations);
          mintText = confirmations === undefined
            ? `No confirmed deposit is available yet. The current requirement is ${data.required_confirmations} Bitcoin confirmations.`
            : `Bitcoin confirmations: ${confirmations} of ${data.required_confirmations}.`;
        } else if ('AlreadyProcessing' in issue) mintText = 'A deposit check is already running.';
        else if ('TemporarilyUnavailable' in issue) throw new Error(issue.TemporarilyUnavailable);
        else if ('GenericError' in issue) throw new Error(issue.GenericError.error_message);
        else throw new Error('The minter returned an unrecognized deposit result.');
      }
    } catch (cause) {
      if (live(owner, token)) {
        error = `Deposit check stopped: ${readableError(cause)}. Check again manually before retrying.`;
        mintBusy = false;
        mintStopped = true;
      }
      return;
    }
    if (!live(owner, token)) return;
    if (mintAttempt >= POLL_LIMIT) {
      mintBusy = false;
      mintStopped = true;
      mintText += ' Automatic checks stopped; you can check again when ready.';
      return;
    }
    mintTimer = setTimeout(() => void runMintCheck(token), POLL_MS);
  }

  async function refreshQuote() {
    quote = null;
    const sats = parseBtcSats(amount);
    if (!connected || !principal || principal.isAnonymous() || !sats || !isValidBitcoinMainnetAddress(recipient)) return;
    const owner = principal.toText();
    const token = generation;
    const addressAtRequest = recipient.trim();
    const amountAtRequest = sats;
    error = '';
    try {
      const actor = await getPublicCkbtcMinterActor();
      const [estimate, fee, info] = await Promise.all([
        actor.estimate_withdrawal_fee({ amount: [sats] }),
        fetchLedgerFeeStrict(ledgerFeeRef),
        actor.get_minter_info(),
      ]);
      if (!live(owner, token) || parseBtcSats(amount) !== amountAtRequest || recipient.trim() !== addressAtRequest) return;
      minterInfo = info;
      quote = { bitcoinFee: BigInt(estimate.bitcoin_fee), minterFee: BigInt(estimate.minter_fee), amount: amountAtRequest, address: addressAtRequest, owner, at: Date.now() };
      ledgerFee = fee;
    } catch (cause) {
      if (live(owner, token)) error = `Could not refresh withdrawal fees: ${readableError(cause)}`;
    }
  }

  async function withdraw() {
    if (withdrawalBusy || !connected || !principal || principal.isAnonymous()) return;
    const owner = principal;
    const ownerKey = owner.toText();
    const token = generation;
    const amountSats = parseBtcSats(amount);
    const addressAtClick = recipient.trim();
    if (!amountSats) { error = 'Enter a positive BTC amount with at most eight decimal places.'; return; }
    if (!isValidBitcoinMainnetAddress(addressAtClick)) { error = 'Enter a valid Bitcoin mainnet address.'; return; }
    const lockManager = (typeof navigator !== 'undefined' ? (navigator as any).locks : null);
    if (!lockManager?.request) {
      error = 'This browser cannot safely prevent duplicate withdrawal requests. Use a browser with Web Locks support to continue.';
      return;
    }
    try {
      await lockManager.request(
        storageKey(ownerKey),
        { mode: 'exclusive', ifAvailable: true },
        async (lock: unknown) => {
          if (!lock) {
            const saved = readBlockedAttempt(ownerKey);
            if (saved) { blockedAttempt = saved; showBlockedAttempt(saved, token); }
            error = 'Another tab is preparing or submitting a ckBTC withdrawal for this account. Check its status before continuing.';
            return;
          }
          const persisted = readBlockedAttempt(ownerKey);
          if (persisted) {
            blockedAttempt = persisted;
            showBlockedAttempt(persisted, token);
            error = 'A previous withdrawal is pending or uncertain. Reconcile its minter status before making another request.';
            return;
          }
          await runWithdrawalAttempt(owner, ownerKey, token, amountSats, addressAtClick, ledgerFee);
        },
      );
    } catch (cause) {
      if (live(ownerKey, token)) error = `Could not safely acquire the withdrawal lock: ${readableError(cause)}`;
    }
  }

  function showBlockedAttempt(saved: NonNullable<typeof blockedAttempt>, token: number) {
    if (saved.state === 'accepted' && saved.blockIndex) {
      statusBlock = BigInt(saved.blockIndex);
      withdrawalOwner = saved.owner;
      statusText = 'Restored a previously accepted withdrawal. Checking its current minter status.';
      void refreshWithdrawalStatus(token, statusBlock);
    } else {
      statusText = 'A previous withdrawal has an uncertain result. Reconcile its status before starting another request.';
    }
  }

  async function runWithdrawalAttempt(owner: Principal, ownerKey: string, token: number, amountSats: bigint, addressAtClick: string, feeAtClick: bigint | null) {
    if (!quote || !isQuoteFresh() || feeAtClick === null) { error = 'Refresh the current withdrawal estimate and ledger fee before submitting.'; return; }
    if (minterInfo && amountSats < BigInt(minterInfo.retrieve_btc_min_amount)) {
      error = `The current minimum withdrawal is ${formatBtcSats(BigInt(minterInfo.retrieve_btc_min_amount))} ckBTC.`;
      return;
    }
    if (balance === null || amountSats + feeAtClick > balance) {
      error = balance === null ? 'Your ckBTC balance has not loaded yet.' : 'Your balance does not cover the requested amount and approval ledger fee.';
      return;
    }
    withdrawalBusy = true;
    withdrawalOwner = ownerKey;
    error = '';
    notice = '';
    statusBlock = null;
    statusText = '';
    txid = '';
    const attemptId = typeof crypto !== 'undefined' && 'randomUUID' in crypto ? crypto.randomUUID() : `${Date.now()}-${Math.random().toString(16).slice(2)}`;
    if (!saveBlockedAttempt({ owner: ownerKey, address: addressAtClick, amount: amountSats.toString(), startedAt: Date.now(), attemptId, state: 'uncertain' }, live(ownerKey, token))) {
      withdrawalBusy = false;
      error = 'A previous withdrawal attempt is already saved for this account. Reconcile it before continuing.';
      return;
    }
    try {
      const now = BigInt(Date.now()) * 1_000_000n;
      const outcome = await submitCkbtcWithdrawal({
        owner,
        ledgerCanisterId: CANISTER_IDS.CKBTC_LEDGER,
        minterCanisterId: CANISTER_IDS.CKBTC_MINTER,
        ledgerIdl: ICRC1_IDL,
        approveArgs: {
          spender: { owner: Principal.fromText(CANISTER_IDS.CKBTC_MINTER), subaccount: [] },
          amount: amountSats + feeAtClick,
          fee: [feeAtClick], memo: [], from_subaccount: [], created_at_time: [now],
          expected_allowance: [], expires_at: [now + 10n * 60n * 1_000_000_000n],
        },
        retrieveArgs: { address: addressAtClick, amount: amountSats, from_subaccount: [] },
        isLive: () => live(ownerKey, token) && parseBtcSats(amount) === amountSats && recipient.trim() === addressAtClick,
      });
      if (outcome.kind === 'stale') { clearBlockedAttempt(ownerKey); return; }
      if (outcome.kind === 'approval-only') {
        clearBlockedAttempt(ownerKey);
        if (live(ownerKey, token)) notice = `Approval block ${outcome.approveBlockIndex} completed, but the wallet changed before a withdrawal request was sent. Refresh fees before any new request.`;
        return;
      }
      if (outcome.kind === 'approve-error') { clearBlockedAttempt(ownerKey); if (live(ownerKey, token)) error = `Approval was rejected: ${outcome.message}`; return; }
      if (outcome.kind === 'approve-uncertain') { if (live(ownerKey, token)) error = `Approval outcome is uncertain (${outcome.message}). Check the ledger before trying again.`; return; }
      if (outcome.kind === 'retrieve-error') {
        if (outcome.retrySafety === 'unknown') {
          if (live(ownerKey, token)) error = `Approval block ${outcome.approveBlockIndex} succeeded, but the minter result may be uncertain (${outcome.message}). This account remains blocked until status is reconciled.`;
        } else {
          clearBlockedAttempt(ownerKey);
          if (live(ownerKey, token)) error = `Approval block ${outcome.approveBlockIndex} succeeded, but the minter rejected this withdrawal: ${outcome.message}. Refresh the estimate before trying again.`;
        }
        return;
      }
      if (outcome.kind === 'retrieve-uncertain') { if (live(ownerKey, token)) error = `Approval block ${outcome.approveBlockIndex} succeeded, but the withdrawal result is uncertain (${outcome.message}). Check minter status before retrying.`; return; }
      const acceptedBlock = outcome.withdrawalBlockIndex;
      const recorded = saveBlockedAttempt({ owner: ownerKey, address: addressAtClick, amount: amountSats.toString(), startedAt: Date.now(), attemptId, state: 'accepted', blockIndex: acceptedBlock.toString() }, live(ownerKey, token));
      if (!live(ownerKey, token)) {
        if (connected && ownerText === ownerKey) {
          const saved = readBlockedAttempt(ownerKey);
          if (saved) { blockedAttempt = saved; showBlockedAttempt(saved, generation); }
        }
        return;
      }
      statusBlock = acceptedBlock;
      if (!recorded) error = 'The minter accepted the withdrawal, but the browser could not update its safety record. Keep this account blocked and check the displayed request status.';
      statusText = `Withdrawal accepted at ledger block ${statusBlock}. Bitcoin settlement is still pending.`;
      statusAttempt = 0;
      await refreshWithdrawalStatus(token, statusBlock);
    } catch (cause) {
      if (live(ownerKey, token)) error = `Withdrawal result is uncertain: ${readableError(cause)}. Check wallet and minter status before trying again.`;
    } finally {
      if (live(ownerKey, token)) withdrawalBusy = false;
    }
  }

  async function refreshWithdrawalStatus(token = generation, block = statusBlock) {
    if (block === null || !connected || ownerText !== withdrawalOwner) return;
    const owner = withdrawalOwner;
    statusBusy = true;
    try {
      const actor = await getPublicCkbtcMinterActor();
      const status = await actor.retrieve_btc_status_v2({ block_index: block });
      if (!live(owner, token) || statusBlock !== block) return;
      if ('Unknown' in status) statusText = 'The minter has no status for this request yet.';
      else if ('Pending' in status) statusText = 'Queued by the minter.';
      else if ('Signing' in status) statusText = 'The minter is signing the Bitcoin transaction.';
      else if ('Sending' in status) { statusText = 'Sent to the Bitcoin canister; waiting for its response.'; txid = txidBytesToHex(status.Sending.txid); }
      else if ('Submitted' in status) { statusText = 'Submitted to Bitcoin; waiting for confirmations.'; txid = txidBytesToHex(status.Submitted.txid); }
      else if ('Confirmed' in status) { statusText = 'Confirmed on Bitcoin.'; txid = txidBytesToHex(status.Confirmed.txid); statusAttempt = POLL_LIMIT; clearBlockedAttempt(owner); }
      else if ('AmountTooLow' in status) { statusText = 'The minter reports that the amount was too low after current fees.'; statusAttempt = POLL_LIMIT; clearBlockedAttempt(owner); }
      else if ('Reimbursed' in status || 'WillReimburse' in status) {
        statusText = 'The minter reports a reimbursement state. Review this request before attempting another withdrawal.';
        statusAttempt = POLL_LIMIT;
        if ('Reimbursed' in status) clearBlockedAttempt(owner);
      }
    } catch (cause) {
      if (live(owner, token)) statusText = `Could not refresh minter status: ${readableError(cause)}`;
    } finally {
      if (live(owner, token)) statusBusy = false;
    }
    if (live(owner, token) && statusBlock === block && statusAttempt < POLL_LIMIT && !/Confirmed|reimbursement|too low/i.test(statusText)) {
      statusAttempt++;
      statusTimer = setTimeout(() => void refreshWithdrawalStatus(token, block), POLL_MS);
    }
  }

  function changeQuoteInput() { quote = null; }
  function formatWholeBtc(value: bigint | null): string { return value === null ? 'Loading…' : `${formatBtcSats(value)} BTC`; }
  function readableError(cause: unknown): string { return cause instanceof Error ? cause.message : String(cause); }
</script>

<svelte:head>
  <title>Bitcoin minter | Rumi Protocol</title>
  <meta name="description" content="Mint ckBTC from Bitcoin and redeem ckBTC to Bitcoin through DFINITY's official minter." />
</svelte:head>

<main class="shell">
  <section class="hero">
    <div class="bitcoin-mark" aria-hidden="true">₿</div>
    <p class="eyebrow">BITCOIN · INTERNET COMPUTER</p>
    <h1>Bitcoin minter</h1>
    <p class="intro">Move between Bitcoin and ckBTC through DFINITY’s ckBTC minter.</p>
  </section>

  <section class="workspace" aria-label="Bitcoin minter">
    <div class="tabs" role="tablist" aria-label="Choose an action">
      <button role="tab" aria-selected={mode === 'mint'} class:active={mode === 'mint'} on:click={() => { mode = 'mint'; error = ''; notice = ''; }}>Mint ckBTC</button>
      <button role="tab" aria-selected={mode === 'redeem'} class:active={mode === 'redeem'} on:click={() => { mode = 'redeem'; error = ''; notice = ''; }}>Redeem BTC</button>
    </div>

    {#if mode === 'mint'}
      <div class="flow-line"><span class:active={mintStepIndex === 1} class="step">01 <b>Deposit BTC</b></span><i></i><span class:active={mintStepIndex === 2} class="step">02 <b>Confirm</b></span><i></i><span class:active={mintStepIndex === 3} class="step">03 <b>Mint ckBTC</b></span></div>
      <h2>Receive ckBTC</h2>
      <p class="muted">Use a Bitcoin wallet to send BTC to your DFINITY minter address. The returned ckBTC is credited to your connected wallet.</p>
      {#if !connected}
        <div class="callout">Connect the account that should receive ckBTC to continue.</div>
      {:else if !address}
        <button class="primary" disabled={busyAddress} on:click={createAddress}>{busyAddress ? 'Getting address…' : 'Get Bitcoin deposit address'}</button>
      {:else}
        <div class="deposit-grid">
          {#if qrDataUrl}<img class="qr" src={qrDataUrl} alt="QR code for your Bitcoin deposit address" />{/if}
          <div class="deposit-content">
            <p class="label">Your Bitcoin deposit address</p>
            <code class="address">{address}</code>
            <button class="quiet" on:click={copyAddress}>Copy address</button>
          </div>
        </div>
        <dl class="facts">
          <div><dt>Receiving wallet</dt><dd class="mono">{ownerText}</dd></div>
          <div><dt>Confirmations required</dt><dd>{minterInfo ? minterInfo.min_confirmations : 'Loading…'}</dd></div>
          <div><dt>Minimum deposit output</dt><dd>{minterInfo?.deposit_btc_min_amount?.[0] !== undefined ? `${formatBtcSats(BigInt(minterInfo.deposit_btc_min_amount[0]))} BTC` : 'Loading…'}</dd></div>
          <div><dt>Deposit check fee</dt><dd>{formatWholeBtc(depositFee)}</dd></div>
        </dl>
        <p class="callout">Send only Bitcoin to this address. Your Bitcoin wallet pays the network fee. The minter checks deposits after the displayed confirmation requirement; timing depends on Bitcoin block production and minter processing.</p>
        <button class="primary" disabled={mintBusy} on:click={beginMintCheck}>{mintBusy ? 'Checking deposit…' : mintStopped ? 'Check deposit again' : 'Check deposit and mint'}</button>
        {#if mintConfirmations !== null && mintRequiredConfirmations !== null}<p class="small">Confirmations observed: {mintConfirmations} of {mintRequiredConfirmations} required.</p>{/if}
        {#if mintBusy}<p class="small" aria-live="polite">Check {mintAttempt} of {POLL_LIMIT} · next check in about 60 seconds</p>{/if}
        {#if mintText}<div class="result" role="status">{mintText}</div>{/if}
      {/if}
    {:else}
      <div class="flow-line"><span class="step active">01 <b>Review</b></span><i></i><span class:active={withdrawalBusy || statusBlock !== null} class="step">02 <b>Request</b></span><i></i><span class:active={statusText.includes('Confirmed')} class="step">03 <b>Bitcoin confirms</b></span></div>
      <h2>Send BTC to a Bitcoin address</h2>
      <p class="muted">The minter estimates network and minter fees for your amount. Approval and withdrawal happen only after you submit with your connected wallet.</p>
      {#if !connected}<div class="callout">Connect the account holding ckBTC to continue.</div>{/if}
      <label>Bitcoin mainnet recipient
        <input bind:value={recipient} on:input={changeQuoteInput} placeholder="bc1…" autocomplete="off" spellcheck="false" disabled={withdrawalBusy || !!blockedAttempt} />
      </label>
      <label>Amount in BTC
        <input bind:value={amount} on:input={changeQuoteInput} placeholder="0.001" inputmode="decimal" autocomplete="off" disabled={withdrawalBusy || !!blockedAttempt} />
      </label>
      <button class="quiet" disabled={!connected || !parsedAmount || !isValidBitcoinMainnetAddress(recipient) || withdrawalBusy} on:click={refreshQuote}>Refresh fee estimate</button>
      <dl class="facts">
        <div><dt>Connected ckBTC balance</dt><dd>{balance === null ? 'Loading…' : `${formatBtcSats(balance)} ckBTC`}</dd></div>
        {#if quoteFresh && quote}
          <div><dt>Requested amount</dt><dd>{formatBtcSats(quote.amount)} ckBTC</dd></div>
          <div><dt>Bitcoin network fee estimate</dt><dd>{formatBtcSats(quote.bitcoinFee)} BTC</dd></div>
          <div><dt>Minter fee estimate</dt><dd>{formatBtcSats(quote.minterFee)} ckBTC</dd></div>
          <div><dt>Estimated Bitcoin received</dt><dd>{receivedEstimate} BTC</dd></div>
          <div><dt>Approval ledger fee</dt><dd>{ledgerFee === null ? 'Loading…' : `${formatBtcSats(ledgerFee)} ckBTC`}</dd></div>
          <div><dt>Minimum withdrawal</dt><dd>{minterInfo ? `${formatBtcSats(BigInt(minterInfo.retrieve_btc_min_amount))} ckBTC` : 'Loading…'}</dd></div>
          <div><dt>Estimate age</dt><dd>{Math.max(0, Math.floor((Date.now() - quote.at) / 1000))} seconds · expires after 60 seconds</dd></div>
        {:else}
          <div><dt>Withdrawal estimate</dt><dd>Enter a valid address and amount, then refresh.</dd></div>
        {/if}
      </dl>
      <p class="callout">The estimate can change before the minter processes the request. The approval also pays the current ckBTC ledger fee. BTC settlement is asynchronous; verify minter status before making another request.</p>
      <button class="primary" disabled={!connected || withdrawalBusy || !!blockedAttempt || !quoteFresh || ledgerFee === null || balance === null} on:click={withdraw}>{blockedAttempt ? 'Resolve previous request first' : withdrawalBusy ? 'Waiting for wallet and minter…' : 'Approve and request BTC'}</button>
      {#if blockedAttempt?.state === 'uncertain'}
        <div class="status" role="alert"><strong>Previous result needs reconciliation</strong><p>{statusText || 'The previous approval or withdrawal call may have completed even though the response was not received.'}</p><p>Saved request: {formatBtcSats(BigInt(blockedAttempt.amount))} ckBTC to <span class="mono">{blockedAttempt.address}</span></p><button class="quiet" disabled={statusBusy} on:click={reconcileUncertainWithdrawal}>{statusBusy ? 'Checking history…' : 'Check minter request history'}</button></div>
      {/if}
      {#if statusBlock !== null}
        <div class="status" aria-live="polite"><strong>Withdrawal status</strong><p>{statusText}</p><small>Request block {statusBlock.toString()} · check {statusAttempt} of {POLL_LIMIT}</small>
          {#if txid}<p><a href={`https://mempool.space/tx/${txid}`} target="_blank" rel="noreferrer">View Bitcoin transaction ↗</a></p>{/if}
          {#if statusAttempt >= POLL_LIMIT && !statusText.includes('Confirmed')}<button class="quiet" disabled={statusBusy} on:click={() => { statusAttempt = 0; void refreshWithdrawalStatus(); }}>{statusBusy ? 'Refreshing…' : 'Refresh status'}</button>{/if}
        </div>
      {/if}
    {/if}
    {#if notice}<div class="result" role="status">{notice}</div>{/if}
    {#if error}<div class="error" role="alert">{error}</div>{/if}
    <p class="footnote">ckBTC is issued by DFINITY’s ckBTC minter against Bitcoin held by that minter. Review the live fees, destination, amount, and status before acting.</p>
  </section>
</main>

<style>
  .shell { min-height: 100vh; padding: 60px 22px 80px; color: #f4f0e7; background: #171717; }
  .hero { width: min(760px, 100%); margin: 0 auto 34px; text-align: center; }
  .bitcoin-mark { display: grid; width: 62px; height: 62px; margin: 0 auto 18px; place-items: center; border-radius: 50%; background: #f7931a; color: #171717; font: 700 38px/1 ui-sans-serif, system-ui, sans-serif; }
  .eyebrow { margin: 0; color: #d0c8b8; font-size: 11px; font-weight: 650; letter-spacing: .16em; }
  h1 { margin: 10px 0 8px; font-size: clamp(34px, 5vw, 48px); font-weight: 600; letter-spacing: -.045em; }
  .intro, .muted { color: #b5afa3; line-height: 1.65; }
  .intro { margin: 0; }
  .workspace { box-sizing: border-box; width: min(680px, 100%); margin: 0 auto; padding: 26px clamp(18px, 5vw, 38px); border: 1px solid #3a3935; border-radius: 16px; background: #222220; }
  .tabs { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin-bottom: 28px; padding-bottom: 18px; border-bottom: 1px solid #41403b; }
  button, input { font: inherit; }
  button { cursor: pointer; }
  .tabs button { padding: 12px 8px; border: 0; border-radius: 8px; background: transparent; color: #b5afa3; font-weight: 600; }
  .tabs button.active { background: #38352f; color: #ffb44f; }
  .flow-line { display: flex; align-items: center; gap: 11px; margin: 0 0 26px; color: #89857c; font-size: 11px; }
  .flow-line .step { display: flex; flex: 0 0 auto; align-items: center; gap: 7px; }
  .flow-line .step b { font-weight: 500; }
  .flow-line .step.active { color: #ffb44f; }
  .flow-line i { height: 1px; flex: 1; background: #4a4842; }
  h2 { margin: 0 0 6px; font-size: 22px; font-weight: 560; letter-spacing: -.02em; }
  .muted { margin: 0 0 20px; font-size: 14px; }
  .primary { width: 100%; min-height: 48px; margin-top: 16px; border: 0; border-radius: 8px; background: #f7931a; color: #171717; font-weight: 700; }
  .primary:disabled { opacity: .48; cursor: not-allowed; }
  .deposit-grid { display: flex; align-items: center; gap: 22px; margin: 24px 0; padding: 18px; border: 1px solid #46443e; border-radius: 10px; background: #1b1b1a; }
  .qr { width: 132px; height: 132px; border-radius: 5px; }
  .deposit-content { min-width: 0; }
  .label { margin: 0 0 8px; color: #b5afa3; font-size: 12px; }
  .address { display: block; overflow-wrap: anywhere; color: #fffaf0; font: 13px/1.6 ui-monospace, SFMono-Regular, Menlo, monospace; }
  .quiet { min-height: 36px; margin-top: 8px; padding: 7px 11px; border: 1px solid #62594b; border-radius: 7px; background: transparent; color: #f8bc69; font-size: 12px; font-weight: 600; }
  .quiet:disabled { opacity: .5; cursor: not-allowed; }
  .facts { display: grid; gap: 0; margin: 18px 0; border-top: 1px solid #41403b; }
  .facts div { display: flex; justify-content: space-between; gap: 16px; padding: 11px 0; border-bottom: 1px solid #41403b; font-size: 13px; }
  dt { color: #b5afa3; } dd { margin: 0; color: #f4f0e7; text-align: right; overflow-wrap: anywhere; }
  .mono { font: 12px ui-monospace, SFMono-Regular, Menlo, monospace; }
  label { display: grid; gap: 8px; margin: 18px 0; color: #d7d1c5; font-size: 13px; font-weight: 600; }
  input { box-sizing: border-box; width: 100%; padding: 12px 13px; border: 1px solid #555148; border-radius: 7px; outline: none; background: #191918; color: #f4f0e7; }
  input:focus { border-color: #f7931a; }
  .callout { margin: 17px 0; padding: 13px 14px; border-left: 2px solid #f7931a; background: #2b2924; color: #d1cabb; font-size: 12px; line-height: 1.65; }
  .small, .footnote { color: #aaa397; font-size: 12px; line-height: 1.6; }
  .result, .error, .status { margin-top: 16px; padding: 13px 14px; border-radius: 8px; font-size: 13px; line-height: 1.6; }
  .result { border: 1px solid #63502d; background: #30291e; color: #f2d7aa; }
  .error { border: 1px solid #744946; background: #302321; color: #f2c1bb; }
  .status { border: 1px solid #555148; background: #1b1b1a; color: #e7e0d3; }
  .status p { margin: 7px 0; }
  .status small { color: #aaa397; }
  .status a { color: #ffb44f; }
  .footnote { margin: 22px 0 0; }
  @media (max-width: 560px) {
    .shell { padding: 38px 14px 54px; }
    .workspace { padding: 20px 16px; }
    .deposit-grid { align-items: flex-start; gap: 13px; padding: 12px; }
    .qr { width: 94px; height: 94px; }
    .flow-line { gap: 6px; font-size: 10px; }
    .flow-line .step { gap: 4px; }
    .facts div { font-size: 12px; }
  }
</style>
