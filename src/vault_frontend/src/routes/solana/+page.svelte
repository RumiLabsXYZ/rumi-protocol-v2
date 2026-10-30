<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { Principal } from '@dfinity/principal';
  import { walletStore } from '$lib/stores/wallet';
  import { openOisySignerChannel } from '$lib/services/oisySigner';
  import { auth, walletSessionGeneration } from '$lib/services/auth';
  import { collateralStore } from '$lib/stores/collateralStore';
  import { CANISTER_IDS } from '$lib/config';
  import { getCksolMinter, getCksolLedger, getCksolRelay, getSolDepositAddress, clearSolDeposit,
    quoteSolWithdrawal, submitSolWithdrawal, loadSolWithdrawal, acknowledgeSolWithdrawal, solWithdrawalKey, isCksolOisyWallet,
    type MinterInfo, type SolWithdrawalQuote, type SolWithdrawalIntent } from '$lib/services/cksolMinter';
  import { parseSol, formatSol, depositView, withdrawalView, type StatusView } from '$lib/utils/cksolMinterFlow';
  import { quoteCksolPayment, submitCksolPayment, loadCksolPayment, saveCksolPayment, recoverCksolPayment, claimCksolRefund, resumeFundedCksolPayment, continueApprovedCksolPayment, cksolPaymentKey, type CksolPaymentQuote, type CksolPaymentIntent } from '$lib/services/cksolPayment';

  let owner: Principal | null = null;
  let ownerText = '';
  let epoch = 0;
  let destroyed = false;
  let busy = false;
  let error = '';
  let notice = '';
  let info: MinterInfo | null = null;
  let paymentQuote: CksolPaymentQuote | null = null;
  let paymentIntent: CksolPaymentIntent | null = null;
  function formatCycles(value: bigint, minDecimals = 2): string {
    const whole = value / 1_000_000_000_000n;
    const fraction = (value % 1_000_000_000_000n).toString().padStart(12, '0');
    const trimmed = fraction.replace(/0+$/, '');
    return `${whole}.${trimmed.padEnd(minDecimals, '0')}T cycles`;
  }
  let cyclesBalance: bigint | null = null;
  let balance: bigint | null = null;
  let depositAddress = '';
  let depositId: bigint | null = null;
  let depositStatus: StatusView | null = null;
  let depositBlocked = false;
  let legacyAttempt: any = null;
  let legacyBlocked = false;
  let withdrawalStatus: StatusView | null = null;
  let withdrawalStatusFor: { intentId: string; burnBlock: string } | null = null;
  let intent: SolWithdrawalIntent | null = null;
  let invalidSavedIntent = false;
  let amount = '';
  let destination = '';
  let quote: SolWithdrawalQuote | null = null;
  let oisy = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let pollCount = 0;
  let statusGeneration = 0;
  let selectedTab: 'mint' | 'redeem' = 'mint';
  const unsubscribers: Array<() => void> = [];
  $: activeCollateral = $collateralStore.collaterals.find((c) => c.principal === CANISTER_IDS.CKSOL_LEDGER);
  $: if (quote && (quote.address !== destination || formatSol(quote.amount) !== amount)) quote = null;

  function live(capturedEpoch: number, capturedOwner: string) {
    const current = get(walletStore);
    return !destroyed && epoch === capturedEpoch && ownerText === capturedOwner
      && !current.loading && (current.isConnected ? current.principal?.toText() ?? '' : '') === capturedOwner;
  }
  function stopPoll() { if (timer) clearTimeout(timer); timer = null; }
  function paidIntentSettled(value: CksolPaymentIntent | null) {
    return value?.phase === 'NotStarted' || (value?.phase === 'Receipt' && ['Complete', 'FundingRejected'].includes(value.receipt?.phase));
  }
  function adoptPaidIntent(value: CksolPaymentIntent) {
    paymentIntent = value;
    const id = value.receipt?.deposit_id?.[0];
    if (id !== undefined) { depositId = BigInt(id); depositStatus = null; pollCount = 0; void checkStatus(true); }
  }
  function loadSaved() {
    if (!owner) return;
    try {
      const saved = loadSolWithdrawal(localStorage, ownerText);
      if (saved?.id !== intent?.id || saved?.burnBlock !== intent?.burnBlock) {
        withdrawalStatus = null; withdrawalStatusFor = null; statusGeneration++;
      }
      intent = saved; invalidSavedIntent = false;
    } catch (e) { invalidSavedIntent = true; withdrawalStatus = null; withdrawalStatusFor = null; error = String(e); }
  }
  async function refresh() {
    const e = epoch, account = owner, text = ownerText;
    error = ''; busy = true;
    try {
      const data = await getCksolMinter().get_minter_info();
      if (!live(e, text)) return;
      info = data;
      // New address display is gated by the paid quote endpoint. Never use the legacy sponsorship budget as readiness.
      if (account) {
        try {
          const q=await quoteCksolPayment(account,isCksolOisyWallet()?'oisy':(get(auth).walletType ?? 'unknown'));
          if(live(e,text)) {
            const recovered=await recoverCksolPayment(account,localStorage);
            if(live(e,text)) { paymentQuote=q; cyclesBalance=q.balance; paymentIntent=recovered; if(paymentIntent) adoptPaidIntent(paymentIntent); }
          }
        } catch { if(live(e,text)) { paymentQuote=null; cyclesBalance=null; notice='Paid SOL deposit processing is unavailable on this backend. Do not send SOL to a new deposit address.'; } }
      }
      if (!account || !live(e, text)) return;
      const [funds, attempt] = await Promise.all([
        getCksolLedger().icrc1_balance_of({ owner: account, subaccount: [] }),
        getCksolRelay().get_cksol_deposit(account),
      ]);
      if (!live(e, text)) return;
      balance = BigInt(funds); loadSaved();
      if (attempt[0]) {
        legacyAttempt=attempt[0]; legacyBlocked=true;
        if ('Queued' in attempt[0].status) depositId = BigInt(attempt[0].status.Queued);
        else if ('SubmittedOrUnknown' in attempt[0].status) notice = 'A previous deposit notification has an unknown outcome. Keep its record for reconciliation; paid processing is blocked.';
        else if ('Rejected' in attempt[0].status) notice = attempt[0].status.Rejected;
        else notice='A previous deposit attempt has an unrecognized status. Paid processing is blocked.';
      } else {
        legacyAttempt=null; legacyBlocked=false;
      }
      await checkStatus(false);
    } catch (err) { if (live(e, text)) { error = String(err); legacyBlocked=true; } }
    finally { if (live(e, text)) busy = false; }
  }
  async function showAddress() {
    if (!owner || depositBlocked || legacyBlocked || !info || !paymentQuote) return;
    const e = epoch, text = ownerText; busy = true; error = '';
    try { const address = await getSolDepositAddress(owner); if (live(e, text)) depositAddress = address; }
    catch (err) { if (live(e, text)) error = String(err); }
    finally { if (live(e, text)) busy = false; }
  }
  async function processPaidDeposit() {
    if(!owner||!paymentQuote||busy||legacyBlocked||depositBlocked||(paymentIntent&&!paidIntentSettled(paymentIntent))) return;
    const e=epoch,text=ownerText,account=owner,quote=paymentQuote;
    if(!navigator.locks) { error='This browser cannot safely coordinate paid deposits across tabs. Use a browser with Web Locks support.'; return; }
    busy=true;error='';notice='';
    try { if(oisy) await openOisySignerChannel(account);
      if(!live(e,text)) return;
      await navigator.locks.request(cksolPaymentKey(text),{ifAvailable:true},async lock=>{
      if(!lock) throw new Error('A paid ckSOL payment is active in another tab.');
      if(!live(e,text)) return;
      const result=await submitCksolPayment(quote,account,localStorage,()=>live(e,text));
      if(live(e,text)) { adoptPaidIntent(result); paymentQuote=null; notice=result.message??'Payment request saved. Refresh the receipt before taking another action.'; }
    }); } catch(err) { if(live(e,text)) { error=String(err); try { paymentIntent=loadCksolPayment(localStorage,text); } catch {} } }
    finally { if(live(e,text)) busy=false; }
  }
  async function refreshPaidReceipt() {
    if(!owner||busy) return; const e=epoch,text=ownerText,account=owner; busy=true;error='';
    try { const recovered=await recoverCksolPayment(account,localStorage); if(!live(e,text))return; if(recovered) adoptPaidIntent(recovered); const q=await quoteCksolPayment(account,get(auth).walletType ?? 'unknown'); if(live(e,text)){paymentQuote=q;cyclesBalance=q.balance;} }
    catch(err) { if(live(e,text)) error=String(err); } finally { if(live(e,text)) busy=false; }
  }
  async function refundPaidCycles() {
    if(!owner||!paymentIntent||!paymentIntent.receipt||busy) return;
    const e=epoch,text=ownerText,account=owner,payment=paymentIntent.receipt; busy=true;error='';
    try { if(!navigator.locks) throw new Error('Web Locks support is required to claim a refund safely.');
      if(oisy) await openOisySignerChannel(account);
      if(!live(e,text)) return;
      await navigator.locks.request(cksolPaymentKey(text),{ifAvailable:true},async lock=>{if(!lock) throw new Error('A paid ckSOL payment is active in another tab.'); if(!live(e,text)) return; const result=await claimCksolRefund(account,payment,()=>live(e,text)); if('Err' in result) throw new Error(String(result.Err)); const recovered=await recoverCksolPayment(account,localStorage); if(live(e,text)&&recovered) adoptPaidIntent(recovered);});
    } catch(err) { if(live(e,text)) error=String(err); } finally { if(live(e,text)) busy=false; }
  }
  async function resumePaidDispatch() {
    if(!owner||!paymentIntent||busy) return; const e=epoch,text=ownerText,account=owner,displayed=paymentIntent; busy=true;error='';
    try { if(!navigator.locks) throw new Error('Web Locks support is required to resume safely.');
      if(oisy) await openOisySignerChannel(account);
      if(!live(e,text)) return;
      await navigator.locks.request(cksolPaymentKey(text),{ifAvailable:true},async lock=>{if(!lock) throw new Error('A paid ckSOL payment is active in another tab.'); if(!live(e,text)) return; const resumed=await resumeFundedCksolPayment(account,displayed,localStorage,()=>live(e,text)); if(!live(e,text))return; adoptPaidIntent(resumed);});
    } catch(err) { if(live(e,text)) error=String(err); } finally { if(live(e,text)) busy=false; }
  }
  async function continuePaidApproval() {
    if(!owner||!paymentIntent||busy) return;const e=epoch,text=ownerText,account=owner,displayed=paymentIntent;busy=true;error='';
    try { if(!navigator.locks) throw new Error('Web Locks support is required to continue safely.');
      if(oisy) await openOisySignerChannel(account);
      if(!live(e,text)) return;
      await navigator.locks.request(cksolPaymentKey(text),{ifAvailable:true},async lock=>{if(!lock) throw new Error('A paid ckSOL payment is active in another tab.');if(!live(e,text))return;const latest=loadCksolPayment(localStorage,text);if(!latest||latest.sequence!==displayed.sequence)throw new Error('Saved payment changed in another tab. Refresh it before continuing.');const continued=await continueApprovedCksolPayment(account,latest,localStorage,()=>live(e,text));if(live(e,text))adoptPaidIntent(continued);});
    } catch(err) { if(live(e,text)) error=String(err); } finally { if(live(e,text))busy=false; }
  }
  async function acknowledgeDeposit() {
    if(!owner) return;
    const e = epoch, text = ownerText;
    const account=owner;
    busy = true; error = '';
    try {
      await clearSolDeposit(() => live(e, text));
      if (!live(e, text)) return;
      const remaining=await getCksolRelay().get_cksol_deposit(account);
      if(!live(e,text)) return;
      if(remaining?.[0]) { legacyAttempt=remaining[0];legacyBlocked=true;throw new Error('The legacy deposit attempt is still retained; keep it for reconciliation.'); }
      legacyAttempt=null;legacyBlocked=false;
      statusGeneration++; depositId = null; depositStatus = null; depositBlocked = false; depositAddress = '';
      notice = 'Previous deposit attempt acknowledged. The paid deposit flow is available after refreshing its quote.';
    } catch (err) { if (live(e, text)) error = String(err); }
    finally { if (live(e, text)) busy = false; }
  }
  async function checkStatus(schedule = true) {
    stopPoll();
    const e = epoch, text = ownerText, request = ++statusGeneration;
    const requestedDepositId = depositId, requestedIntentId = intent?.id, requestedBurnBlock = intent?.burnBlock;
    const currentRequest = () => live(e, text) && request === statusGeneration;
    let pending = false;
    try {
      if (requestedDepositId !== null) {
        const raw = await getCksolMinter().deposit_sol_status(requestedDepositId);
        if (!currentRequest() || depositId !== requestedDepositId) return;
        depositStatus = depositView(raw); depositBlocked = 'Quarantined' in raw || 'NotFound' in raw; pending ||= !depositStatus.terminal;
        if ('Minted' in raw) notice = `${formatSol(raw.Minted.minted_amount)} ckSOL minted. Ledger block ${raw.Minted.block_index}.`;
      }
      if (requestedBurnBlock) {
        const raw = await getCksolMinter().withdraw_sol_status({ block_index: BigInt(requestedBurnBlock) });
        if (!currentRequest() || intent?.id !== requestedIntentId || intent?.burnBlock !== requestedBurnBlock) return;
        withdrawalStatus = withdrawalView(raw); withdrawalStatusFor = { intentId: requestedIntentId!, burnBlock: requestedBurnBlock }; pending ||= !withdrawalStatus.terminal;
      }
      if (schedule && pending && pollCount++ < 60) timer = setTimeout(() => { void checkStatus(true); }, 5_000);
      else if (pending && schedule) notice = 'Automatic checking paused after five minutes. Use Check status to continue.';
    } catch (err) { if (live(e, text)) error = `Status check failed: ${String(err)}. Keep the saved ID and check again.`; }
  }
  async function prepareQuote() {
    if (!owner) return;
    const e = epoch, text = ownerText; busy = true; error = ''; quote = null;
    try {
      const result = await quoteSolWithdrawal(owner, parseSol(amount), destination);
      if (live(e, text)) { quote = result; amount = formatSol(result.amount); }
    } catch (err) { if (live(e, text)) error = String(err); }
    finally { if (live(e, text)) busy = false; }
  }
  async function redeem() {
    if (!owner || !quote || busy || intent || invalidSavedIntent) return;
    const e = epoch, text = ownerText, account = owner, terms = quote;
    if (!navigator.locks) { error = 'This browser cannot safely coordinate withdrawals across tabs. Use a browser with Web Locks support.'; return; }
    busy = true; error = ''; notice = '';
    try {
      await navigator.locks.request(solWithdrawalKey(text), { ifAvailable: true }, async (lock) => {
        if (!lock) throw new Error('A ckSOL withdrawal is already active in another tab.');
        if (!live(e, text)) return;
        const result = await submitSolWithdrawal(terms, account, localStorage, () => live(e, text));
        if (!live(e, text)) return;
        intent = result; quote = null; pollCount = 0;
        if (result.message) notice = result.message;
        await checkStatus(true);
      });
    } catch (err) { if (live(e, text)) { error = String(err); loadSaved(); } }
    finally { if (live(e, text)) busy = false; }
  }
  async function clearIntent() {
    if (!owner || busy) return;
    try {
      if (!navigator.locks) throw new Error('Web Locks support is required to dismiss a request safely.');
      const e = epoch, text = ownerText, displayed = intent, receipt = withdrawalStatus?.success ? withdrawalStatusFor ?? undefined : undefined;
      if (!displayed) return;
      await navigator.locks.request(solWithdrawalKey(text), { ifAvailable: true }, async (lock) => {
        if (!lock) throw new Error('A withdrawal is still active in another session. Wait for its result.');
        if (!live(e, text)) return;
        acknowledgeSolWithdrawal(localStorage, displayed, receipt);
        intent = null; withdrawalStatus = null; withdrawalStatusFor = null; quote = null;
      });
    } catch (err) { error = String(err); loadSaved(); }
  }
  function resetSession() {
    epoch++; statusGeneration++; stopPoll(); pollCount = 0;
    const wallet = get(walletStore);
    owner = wallet.isConnected && !wallet.loading ? wallet.principal : null; ownerText = owner?.toText() ?? '';
    busy = false; error = ''; notice = ''; depositAddress = ''; depositId = null;
    depositStatus = null; depositBlocked = false; legacyAttempt=null;legacyBlocked=false;withdrawalStatus = null; withdrawalStatusFor = null; intent = null; invalidSavedIntent = false;
    balance = null; quote = null; amount = ''; destination = ''; oisy = isCksolOisyWallet(); paymentQuote=null;cyclesBalance=null;paymentIntent=null;
    if(owner) { try { paymentIntent=loadCksolPayment(localStorage,ownerText); } catch { error='Saved paid ckSOL request is invalid. Preserve it for support.'; } }
    if (owner) loadSaved();
    void refresh();
  }
  onMount(() => {
    unsubscribers.push(walletStore.subscribe((wallet) => {
      const nextOwner = wallet.isConnected && !wallet.loading ? wallet.principal?.toText() ?? '' : '';
      if (nextOwner !== ownerText) resetSession();
    }));
    // Generation invalidates immediately when a wallet transition begins, including A -> B -> A.
    unsubscribers.push(walletSessionGeneration.subscribe(() => resetSession()));
    void collateralStore.fetchSupportedCollateral();
  });
  onDestroy(() => { destroyed = true; epoch++; stopPoll(); unsubscribers.forEach((fn) => fn()); });
</script>

<svelte:head><title>ckSOL Minter | Rumi</title><meta name="description" content="Mint ckSOL from SOL, redeem ckSOL to Solana, and use active ckSOL collateral on Rumi." /></svelte:head>

<main class="mx-auto max-w-4xl px-4 py-10 text-gray-100">
  <div class="mb-8"><p class="text-sm font-medium text-emerald-400">SOLANA · INTERNET COMPUTER</p><h1 class="mt-2 flex items-center gap-3 text-3xl font-semibold"><img src="/cksol-logo.svg" alt="" class="h-9 w-9" />ckSOL Minter</h1><p class="mt-3 max-w-2xl text-gray-400">Convert SOL and ckSOL through the official chain-key Solana minter. Your ICP wallet owns the ckSOL; your Solana wallet sends and receives SOL.</p></div>
  <div class="mb-6 rounded-xl border border-gray-700 bg-gray-800/60 p-4 text-sm text-gray-300">Use <strong>Solana mainnet</strong> only. ckSOL has 9 decimals, matching SOL lamports. The minter processes deposits and withdrawals asynchronously. Keep your deposit ID or withdrawal burn block until completion.</div>
  <div class="mb-6 flex gap-3" role="tablist" aria-label="Conversion direction">
    <button role="tab" aria-selected={selectedTab === 'mint'} class="rounded-lg px-4 py-2" class:bg-gray-700={selectedTab === 'mint'} onclick={() => selectedTab = 'mint'}>Mint ckSOL</button>
    <button role="tab" aria-selected={selectedTab === 'redeem'} class="rounded-lg px-4 py-2" class:bg-gray-700={selectedTab === 'redeem'} onclick={() => selectedTab = 'redeem'}>Redeem SOL</button>
  </div>
  {#if !owner}<p class="mb-6 rounded-lg bg-gray-800 p-4">Connect your ICP wallet using the wallet button above to continue.</p>{/if}
  {#if error}<p role="alert" class="mb-4 break-words rounded-lg border border-red-700 bg-red-950/40 p-4 text-sm">{error}</p>{/if}
  {#if notice}<p role="status" class="mb-4 break-words rounded-lg border border-gray-600 bg-gray-800 p-4 text-sm">{notice}</p>{/if}
  <section class="rounded-xl border border-gray-700 bg-gray-900 p-6">
    {#if selectedTab === 'mint'}
      <h2 class="text-xl font-medium">SOL → ckSOL</h2>
      <ol class="mt-4 list-decimal space-y-3 pl-5 text-gray-300"><li>Review the paid processing quote and cycles ledger balance.</li><li>Get your account-bound SOL mainnet deposit address and send SOL.</li><li>Approve the capped cycles payment, then track the minter receipt and any refund.</li></ol>
      {#if info}<div class="mt-5 space-y-1 text-sm text-gray-400"><p>Minimum deposit balance: {formatSol(info.minimum_deposit_amount)} SOL.</p><p>The sweep’s transaction fee is deducted before minting. The first sweep also leaves SOL on the deposit address for rent exemption.</p></div>{/if}
      {#if paymentQuote}<div class="mt-5 space-y-2 rounded-lg bg-gray-800 p-4 text-sm"><p>Cycles ledger balance: {cyclesBalance === null ? 'Loading' : formatCycles((cyclesBalance / 10_000_000_000n) * 10_000_000_000n)}.</p><p>Funding: {formatCycles(paymentQuote.funding_cycles)}, including {formatCycles(paymentQuote.processing_fee)} processing fee (which covers the refund reserve).</p><p>Approval cap: {formatCycles(paymentQuote.approval_amount)}, including the {formatCycles(paymentQuote.ledger_fee)} withdrawal fee. The approval transaction also costs {formatCycles(paymentQuote.ledger_fee)}.</p><p>Maximum balance needed: {formatCycles((paymentQuote.approval_amount+paymentQuote.ledger_fee))}. Processing costs are rechecked when you click approve. Approval is exact and expires after 10 minutes.</p><a class="text-emerald-400 underline" href="https://internetcomputer.org/docs/current/developer-docs/smart-contracts/maintain/cycles-management/" target="_blank" rel="noreferrer">How to add cycles to your cycles ledger</a><button class="mt-3 rounded-lg border border-gray-500 px-3 py-2" disabled={busy} onclick={refreshPaidReceipt}>Refresh processing fee and receipt</button></div>{/if}
      {#if legacyAttempt && 'Rejected' in legacyAttempt.status}<div class="mt-5 space-y-2 rounded-lg border border-amber-700 bg-gray-800 p-4"><p>Previous legacy deposit attempt was rejected: {legacyAttempt.status.Rejected}</p><p>Acknowledge this rejected attempt before starting a paid deposit.</p><button class="underline" disabled={busy} onclick={acknowledgeDeposit}>Acknowledge rejected legacy attempt</button></div>{:else if legacyBlocked}<p class="mt-5 text-amber-300">A previous legacy deposit attempt is still retained or could not be checked. Paid processing stays blocked until its outcome is resolved.</p>{/if}
      {#if !paymentQuote && !paymentIntent}<p class="mt-5 text-amber-300">Paid deposit processing is unavailable until this backend provides its payment quote. Do not send SOL to a new address.</p>{/if}
      <button class="mt-5 rounded-lg bg-emerald-600 px-4 py-2 font-medium text-white disabled:opacity-40" disabled={!owner || busy || depositBlocked || legacyBlocked || !info || !paymentQuote || !!paymentIntent && !paidIntentSettled(paymentIntent)} onclick={showAddress}>Get my SOL deposit address</button>
      {#if depositAddress}<div class="mt-5 rounded-lg bg-gray-800 p-4"><p class="text-sm text-gray-400">SOL mainnet deposit address · credits ICP account {ownerText}</p><code class="mt-2 block break-all select-all">{depositAddress}</code><p class="mt-3 text-xs text-gray-300">After SOL arrives, approve the displayed cycles cap to queue its sweep. Rumi does not sponsor this processing.</p><button class="mt-4 rounded-lg bg-emerald-600 px-4 py-2 disabled:opacity-40" disabled={busy || depositBlocked || legacyBlocked || !paymentQuote || !!paymentIntent && !paidIntentSettled(paymentIntent)} onclick={processPaidDeposit}>{paymentIntent?.phase==='NotStarted'?'Refresh and retry payment':paidIntentSettled(paymentIntent)?'Start next paid deposit':'Approve and process my deposit'}</button></div>{/if}
      {#if paymentIntent}<div class="mt-5 space-y-2 rounded-lg border border-gray-700 bg-gray-800 p-4"><p>Paid payment sequence: {paymentIntent.sequence}; state: {paymentIntent.receipt?.phase ?? paymentIntent.phase}.</p>{#if paymentIntent.receipt?.funding_block?.[0] !== undefined}<p>Funding ledger block: <code>{paymentIntent.receipt.funding_block[0].toString()}</code></p>{/if}{#if paymentIntent.receipt?.deposit_id?.[0] !== undefined}<p>Deposit ID: <code>{paymentIntent.receipt.deposit_id[0].toString()}</code></p>{/if}<p>{paymentIntent.receipt?.message?.[0] ?? paymentIntent.message ?? 'Keep this sequence and refresh its receipt before starting another payment.'}</p>{#if paymentIntent.phase === 'NotStarted'}<p>No funding was started for this sequence. Refresh the quote, then retry to select the current backend sequence.</p>{/if}{#if paymentIntent.phase === 'Approved' && !paymentIntent.receipt}<button class="rounded-lg bg-emerald-600 px-4 py-2 disabled:opacity-40" disabled={busy} onclick={continuePaidApproval}>Process my deposit</button>{/if}{#if paymentIntent.receipt?.phase === 'Funded'}<button class="underline" disabled={busy} onclick={resumePaidDispatch}>Resume minter dispatch for this funded sequence</button>{/if}{#if ['RefundReady','Funded'].includes(paymentIntent.receipt?.phase)}<p>Refundable cycles: {formatCycles(paymentIntent.receipt.refundable_cycles)}.</p><button class="underline" disabled={busy} onclick={refundPaidCycles}>Claim unused cycles refund</button>{/if}{#if paymentIntent.receipt?.refund_block?.[0] !== undefined}<p>Refund ledger block: <code>{paymentIntent.receipt.refund_block[0].toString()}</code></p>{/if}<button class="ml-3 underline" disabled={busy} onclick={refreshPaidReceipt}>Refresh payment receipt</button></div>{/if}
      {#if depositId !== null}<div class="mt-5 rounded-lg bg-gray-800 p-4"><p>Deposit ID: <code>{depositId.toString()}</code></p><p class="mt-2">{depositStatus?.label ?? 'Check the deposit status.'}</p>{#if depositStatus?.signature}<a class="mt-2 block break-all text-emerald-400 underline" href="https://solscan.io/tx/{depositStatus.signature}" target="_blank" rel="noreferrer">View sweep on Solana</a>{/if}<button class="mt-3 underline" disabled={busy} onclick={() => { pollCount = 0; void checkStatus(true); }}>Check deposit status</button>{#if depositStatus?.success || depositStatus?.kind === 'Dropped'}<button class="ml-3 underline" disabled={busy} onclick={acknowledgeDeposit}>Acknowledge completed deposit</button>{/if}</div>{/if}
    {:else}
      <h2 class="text-xl font-medium">ckSOL → SOL</h2>
      <p class="mt-3 text-sm text-gray-400">{balance === null ? 'Connect and refresh to see your ckSOL balance.' : `Balance: ${formatSol(balance)} ckSOL`}</p>
      {#if oisy}<p class="mt-4 text-amber-300">The official ckSOL minter does not provide the consent interface required by OISY for withdrawals. Use Internet Identity or Plug. No ckSOL approval will be requested here.</p>{/if}
      {#if intent}<div class="mt-5 space-y-2 rounded-lg bg-gray-800 p-4"><p>Saved withdrawal: {formatSol(BigInt(intent.amount))} ckSOL → <span class="break-all">{intent.address}</span></p><p>State: {intent.phase}</p>{#if intent.approveBlock}<p>Approval block: <code>{intent.approveBlock}</code></p>{/if}{#if intent.burnBlock}<p>Burn block: <code>{intent.burnBlock}</code></p>{/if}<p>{withdrawalStatus?.label ?? intent.message ?? 'No confirmed burn receipt. Preserve this request; do not repeat a withdrawal with an uncertain outcome.'}</p>{#if withdrawalStatus?.signature}<a class="block break-all text-emerald-400 underline" href="https://solscan.io/tx/{withdrawalStatus.signature}" target="_blank" rel="noreferrer">View withdrawal on Solana</a>{/if}
        {#if intent.burnBlock}<button class="underline" disabled={busy} onclick={() => { pollCount = 0; void checkStatus(true); }}>Check withdrawal status</button>{/if}
        {#if ['Approving', 'Approved', 'Rejected'].includes(intent.phase) || withdrawalStatus?.success}<button class="ml-3 underline" disabled={busy} onclick={clearIntent}>{withdrawalStatus?.success ? 'Acknowledge completed withdrawal' : 'Dismiss request without a burn dispatch'}</button>{/if}
      </div>{/if}
      <label class="mt-5 block text-sm" for="sol-destination">Destination Solana mainnet address</label><input id="sol-destination" class="mt-2 w-full rounded-lg border border-gray-600 bg-gray-800 p-3" bind:value={destination} disabled={busy || !!intent} autocomplete="off" />
      <label class="mt-4 block text-sm" for="sol-amount">ckSOL to redeem</label><input id="sol-amount" class="mt-2 w-full rounded-lg border border-gray-600 bg-gray-800 p-3" bind:value={amount} disabled={busy || !!intent} inputmode="decimal" placeholder="0.1" />
      <button class="mt-4 rounded-lg border border-gray-600 px-4 py-2 disabled:opacity-40" disabled={!owner || busy || !!intent || invalidSavedIntent || oisy} onclick={prepareQuote}>Get withdrawal quote</button>
      {#if quote}<div class="mt-5 space-y-2 rounded-lg bg-gray-800 p-4 text-sm"><p>Minter fee deducted from amount: {formatSol(quote.minterFee)} SOL.</p><p>Approval fee: {formatSol(quote.ledgerFee)} ckSOL. Additional ledger fee reserve: {formatSol(quote.ledgerFee)} ckSOL.</p><p>Estimated SOL received: {formatSol(quote.amount - quote.minterFee)} SOL.</p><p>Approve {formatSol(quote.amount + quote.ledgerFee)} ckSOL for the official minter, then request withdrawal. Quote expires after 60 seconds.</p><button class="mt-3 rounded-lg bg-emerald-600 px-4 py-2 font-medium disabled:opacity-40" disabled={busy || !!intent || oisy} onclick={redeem}>Approve and request SOL</button></div>{/if}
    {/if}
  </section>
  <div class="mt-6 flex flex-wrap items-center gap-4 text-sm"><button class="text-gray-300 underline" disabled={busy} onclick={refresh}>Refresh balance and minter terms</button>{#if activeCollateral?.status === 'Active'}<a class="text-emerald-400 underline" href="/?collateral={CANISTER_IDS.CKSOL_LEDGER}">Borrow icUSD with ckSOL</a>{:else}<span class="text-gray-500">Borrowing appears when ckSOL collateral is active on Rumi.</span>{/if}<a class="text-gray-400 underline" href="https://internetcomputer.org/docs/current/developer-docs/smart-contracts/maintain/cycles-management/" target="_blank" rel="noreferrer">Cycles ledger documentation</a><a class="text-gray-400 underline" href="https://github.com/dfinity/cksol" target="_blank" rel="noreferrer">Official ckSOL documentation</a></div>
</main>
