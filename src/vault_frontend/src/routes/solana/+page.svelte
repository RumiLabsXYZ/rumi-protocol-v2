<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import { Principal } from '@dfinity/principal';
  import { walletStore } from '$lib/stores/wallet';
  import { readSolDepositBalance } from '$lib/services/cksolDepositBalance';
  import { beginCksolOisyAction, type CksolOisyAction } from '$lib/services/cksolOisySigner';
  import { auth, walletSessionGeneration } from '$lib/services/auth';
  import { collateralStore } from '$lib/stores/collateralStore';
  import { CANISTER_IDS } from '$lib/config';
  import { getCksolMinter, getCksolLedger, getCksolRelay, getSolDepositAddress,
    quoteSolWithdrawal, submitSolWithdrawal, loadSolWithdrawal, acknowledgeSolWithdrawal, solWithdrawalKey, isCksolOisyWallet,
    type MinterInfo, type SolWithdrawalQuote, type SolWithdrawalIntent } from '$lib/services/cksolMinter';
  import { parseSol, formatSol, depositView, withdrawalView, type StatusView } from '$lib/utils/cksolMinterFlow';
  import { quoteCksolPayment, submitCksolPayment, loadCksolPayment, recoverCksolPayment, claimCksolRefund, resumeFundedCksolPayment, continueApprovedCksolPayment, cksolPaymentKey, type CksolPaymentQuote, type CksolPaymentIntent } from '$lib/services/cksolPayment';
  import CksolIcpPaymentPanel from '$lib/components/minters/CksolIcpPaymentPanel.svelte';

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
  let solBalance: bigint | null = null;
  let balanceUnavailable = false;
  let heartbeat: ReturnType<typeof setInterval> | null = null;
  let copied = false;
  let depositId: bigint | null = null;
  let depositStatus: StatusView | null = null;
  let mintedAmount: bigint | null = null;
  let mintBlock: bigint | null = null;
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
  let paymentMethod: 'icp' | 'cycles' = 'icp';
  let icpReady = false;
  let icpBusy = false;
  let icpPending = false;
  let icpReceipt: any = null;
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
    if (id !== undefined && depositId !== BigInt(id)) { depositId=BigInt(id); depositStatus=null; mintedAmount=null; mintBlock=null; solBalance=null; balanceUnavailable=false; pollCount=0; }
  }
  $: paidPhase = paymentIntent?.receipt?.phase ?? paymentIntent?.phase;
  $: minterPending = depositId !== null && (!depositStatus || !depositStatus.terminal);
  $: uncertainPayment = ['Approving','ApprovalUnknown','Starting','StartUnknown','FundingUnknown','RefundUnknown'].includes(paidPhase ?? '');
  $: pendingPayment = !!paymentIntent && !paidIntentSettled(paymentIntent);
  $: resumeApproval = paymentIntent?.phase === 'Approved' && !paymentIntent.receipt;
  $: resumeFunding = paidPhase === 'Funded';
  $: refundReady = paidPhase === 'RefundReady';
  $: fundsReceived = solBalance !== null && !!info && solBalance >= BigInt(info.minimum_deposit_amount);
  $: cyclesReady = !!paymentQuote && paymentQuote.balance >= paymentQuote.approval_amount + paymentQuote.ledger_fee;
  // ICP funding reserves a shared relay slot before contacting the minter.
  // Its payment phase distinguishes that reservation from an unknown dispatch.
  $: blockingLegacy = legacyBlocked && !(legacyAttempt?.status && 'SubmittedOrUnknown' in legacyAttempt.status &&
    ['FundingPending','FundingUnknown','Funded'].includes(icpReceipt?.phase));
  $: addressReady = (solBalance !== null && solBalance > 0n) || (paymentMethod === 'icp' ? icpReady : cyclesReady);
  $: mintEnabled = !!owner && !busy && !icpBusy && !icpPending && (refundReady || (!depositBlocked && !blockingLegacy && !minterPending &&
    (resumeApproval || resumeFunding || (cyclesReady && !pendingPayment && (fundsReceived || balanceUnavailable)))));
  $: primaryLabel = resumeApproval || resumeFunding ? 'Continue minting' : refundReady ? 'Return unused cycles' : 'Mint ckSOL';
  $: paymentMessage = paidPhase === 'Complete' ? 'Processing payment settled. Any unused cycles have been returned.' :
    paidPhase === 'FundingRejected' ? 'No processing payment was taken. You can try minting again.' :
    paidPhase === 'Approved' ? 'Payment approved. Continue minting to finish authorization.' :
    paidPhase === 'Funded' ? 'Processing is funded. Continue minting without another cycles approval.' :
    paidPhase === 'RefundReady' ? 'Unused cycles are ready to return to your wallet.' :
    ['ApprovalUnknown','StartUnknown','FundingUnknown','RefundUnknown'].includes(paidPhase ?? '') ? 'Checking an uncertain payment result. Your saved request is retained; do not start another payment.' :
    pendingPayment ? 'Processing your request. This page checks its progress automatically.' : '';
  async function mint() {
    if (resumeApproval) await continuePaidApproval();
    else if (resumeFunding) await resumePaidDispatch();
    else if (refundReady) await refundPaidCycles();
    else await processPaidDeposit();
  }
  async function copyAddress() {
    if (!depositAddress) return;
    try { await navigator.clipboard.writeText(depositAddress); copied = true; }
    catch { error = 'Could not copy the address. Select and copy the address shown below.'; }
  }
  async function recoverPaid(account: Principal) {
    // Background reads must not replace a journal while another tab signs.
    if (!navigator.locks) return loadCksolPayment(localStorage,account.toText());
    return navigator.locks.request(cksolPaymentKey(account.toText()),{ifAvailable:true},async lock =>
      lock ? recoverCksolPayment(account,localStorage) : loadCksolPayment(localStorage,account.toText()));
  }
  function refreshWhenVisible() {
    if (owner && !busy && document.visibilityState !== 'hidden') void refresh();
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
    if (busy || icpBusy) return;
    const e = epoch, account = owner, text = ownerText;
    error = ''; busy = true;
    try {
      const data = await getCksolMinter().get_minter_info();
      if (!live(e, text)) return;
      info = data;
      if (account) {
        const recovered=await recoverPaid(account);
        if (!live(e,text)) return;
        if (recovered) adoptPaidIntent(recovered);
        if (recovered && !paidIntentSettled(recovered)) paymentMethod = 'cycles';
        try {
          const q=await quoteCksolPayment(account,isCksolOisyWallet()?'oisy':(get(auth).walletType ?? 'unknown'));
          if (!live(e,text)) return;
          paymentQuote=q; cyclesBalance=q.balance;
        } catch {
          if (!live(e,text)) return;
          paymentQuote=null;
          notice='Processing payment is temporarily unavailable. Do not send SOL to a new address. Existing deposits remain recorded; status checks will continue.';
        }
      }
      if (!account || !live(e,text)) return;
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
        else if ('Rejected' in attempt[0].status) { legacyBlocked=false; notice='The previous processing attempt was rejected. You can try minting again.'; }
        else notice='A previous deposit attempt has an unrecognized status. Paid processing is blocked.';
      } else {
        legacyAttempt=null; legacyBlocked=false;
      }
      await checkStatus(false);
      if (!live(e,text)) return;
      if (paymentQuote && !legacyBlocked && !depositBlocked && !depositAddress) {
        const address=await getSolDepositAddress(account);
        if (!live(e,text)) return;
        depositAddress=address;
      }
      if (depositAddress) {
        try {
          const observed=await readSolDepositBalance(depositAddress);
          if (live(e,text)) { solBalance=observed; balanceUnavailable=false; }
        } catch { if (live(e,text)) { solBalance=null; balanceUnavailable=true; } }
      }
    } catch (err) { if (live(e, text)) { error = String(err); legacyBlocked=true; } }
    finally { if (!destroyed && epoch === e) busy = false; }
  }
  function adoptIcpReceipt(receipt: any) {
    icpReceipt=receipt;
    const id = receipt.deposit_id?.[0];
    if (id !== undefined && depositId !== BigInt(id)) {
      depositId=BigInt(id);depositStatus=null;mintedAmount=null;mintBlock=null;solBalance=null;balanceUnavailable=false;pollCount=0;
      void checkStatus(false);
    }
  }
  async function processPaidDeposit() {
    if(!owner||!paymentQuote||busy||legacyBlocked||depositBlocked||(paymentIntent&&!paidIntentSettled(paymentIntent))) return;
    const e=epoch,text=ownerText,account=owner,quote=paymentQuote;
    if(!navigator.locks) { error='This browser cannot safely coordinate paid deposits across tabs. Use a browser with Web Locks support.'; return; }
    busy=true;error='';notice='';
    let signerAction: CksolOisyAction | null = null;
    try { if(oisy) { signerAction=beginCksolOisyAction(account); await signerAction.ready; }
      if(!live(e,text)) return;
      await navigator.locks.request(cksolPaymentKey(text),{ifAvailable:true},async lock=>{
      if(!lock) throw new Error('A paid ckSOL payment is active in another tab.');
      if(!live(e,text)) return;
      const result=await submitCksolPayment(quote,account,localStorage,()=>live(e,text));
      if(live(e,text)) { adoptPaidIntent(result); notice=''; await checkStatus(false); }
    }); } catch(err) { if(live(e,text)) { error=String(err); try { paymentIntent=loadCksolPayment(localStorage,text); } catch {} } }
    finally { await signerAction?.close().catch(() => {}); if(live(e,text)) busy=false; }
  }
  async function refundPaidCycles() {
    if(!owner||!paymentIntent||!paymentIntent.receipt||busy) return;
    const e=epoch,text=ownerText,account=owner,payment=paymentIntent.receipt; busy=true;error='';
    let signerAction: CksolOisyAction | null = null;
    try { if(!navigator.locks) throw new Error('Web Locks support is required to claim a refund safely.');
      if(oisy) { signerAction=beginCksolOisyAction(account); await signerAction.ready; }
      if(!live(e,text)) return;
      await navigator.locks.request(cksolPaymentKey(text),{ifAvailable:true},async lock=>{if(!lock) throw new Error('A paid ckSOL payment is active in another tab.'); if(!live(e,text)) return; const result=await claimCksolRefund(account,payment,()=>live(e,text)); if('Err' in result) throw new Error(String(result.Err)); const recovered=await recoverCksolPayment(account,localStorage); if(live(e,text)&&recovered) adoptPaidIntent(recovered);});
    } catch(err) { if(live(e,text)) error=String(err); } finally { await signerAction?.close().catch(() => {}); if(live(e,text)) busy=false; }
  }
  async function resumePaidDispatch() {
    if(!owner||!paymentIntent||busy) return; const e=epoch,text=ownerText,account=owner,displayed=paymentIntent; busy=true;error='';
    let signerAction: CksolOisyAction | null = null;
    try { if(!navigator.locks) throw new Error('Web Locks support is required to resume safely.');
      if(oisy) { signerAction=beginCksolOisyAction(account); await signerAction.ready; }
      if(!live(e,text)) return;
      await navigator.locks.request(cksolPaymentKey(text),{ifAvailable:true},async lock=>{if(!lock) throw new Error('A paid ckSOL payment is active in another tab.'); if(!live(e,text)) return; const resumed=await resumeFundedCksolPayment(account,displayed,localStorage,()=>live(e,text)); if(!live(e,text))return; adoptPaidIntent(resumed);await checkStatus(false);});
    } catch(err) { if(live(e,text)) error=String(err); } finally { await signerAction?.close().catch(() => {}); if(live(e,text)) busy=false; }
  }
  async function continuePaidApproval() {
    if(!owner||!paymentIntent||busy) return;const e=epoch,text=ownerText,account=owner,displayed=paymentIntent;busy=true;error='';
    let signerAction: CksolOisyAction | null = null;
    try { if(!navigator.locks) throw new Error('Web Locks support is required to continue safely.');
      if(oisy) { signerAction=beginCksolOisyAction(account); await signerAction.ready; }
      if(!live(e,text)) return;
      await navigator.locks.request(cksolPaymentKey(text),{ifAvailable:true},async lock=>{if(!lock) throw new Error('A paid ckSOL payment is active in another tab.');if(!live(e,text))return;const latest=loadCksolPayment(localStorage,text);if(!latest||latest.sequence!==displayed.sequence)throw new Error('Saved payment changed in another tab. Refresh it before continuing.');const continued=await continueApprovedCksolPayment(account,latest,localStorage,()=>live(e,text));if(live(e,text)){adoptPaidIntent(continued);await checkStatus(false);}});
    } catch(err) { if(live(e,text)) error=String(err); } finally { await signerAction?.close().catch(() => {}); if(live(e,text))busy=false; }
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
        if (legacyAttempt?.status && 'Queued' in legacyAttempt.status && BigInt(legacyAttempt.status.Queued) === requestedDepositId)
          legacyBlocked = !('Minted' in raw || 'Dropped' in raw);
        if ('Minted' in raw) { mintedAmount=BigInt(raw.Minted.minted_amount); mintBlock=BigInt(raw.Minted.block_index); notice=`${formatSol(mintedAmount)} ckSOL minted. Ledger block ${mintBlock}.`; }
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
    busy = false; error = ''; notice = ''; depositAddress = ''; depositId = null; solBalance=null; balanceUnavailable=false; copied=false; mintedAmount=null; mintBlock=null;
    depositStatus = null; depositBlocked = false; legacyAttempt=null;legacyBlocked=false;withdrawalStatus = null; withdrawalStatusFor = null; intent = null; invalidSavedIntent = false;
    balance = null; quote = null; amount = ''; destination = ''; oisy = isCksolOisyWallet(); paymentQuote=null;cyclesBalance=null;paymentIntent=null;
    icpReady=false;icpBusy=false;icpPending=false;icpReceipt=null;paymentMethod='icp';
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
    heartbeat=setInterval(refreshWhenVisible,15_000);
    window.addEventListener('focus',refreshWhenVisible);
    document.addEventListener('visibilitychange',refreshWhenVisible);
    void collateralStore.fetchSupportedCollateral();
  });
  onDestroy(() => { destroyed=true; epoch++; stopPoll(); if(heartbeat) clearInterval(heartbeat); window.removeEventListener('focus',refreshWhenVisible); document.removeEventListener('visibilitychange',refreshWhenVisible); unsubscribers.forEach(fn=>fn()); });
</script>

<svelte:head><title>ckSOL Minter | Rumi</title><meta name="description" content="Mint ckSOL from SOL, redeem ckSOL to Solana, and use active ckSOL collateral on Rumi." /></svelte:head>

<main class="mx-auto max-w-4xl px-4 py-10 text-gray-100">
  <div class="mb-8"><p class="text-sm font-medium text-emerald-400">SOLANA · INTERNET COMPUTER</p><h1 class="mt-2 flex items-center gap-3 text-3xl font-semibold"><img src="/cksol-logo.svg" alt="" class="h-9 w-9" />ckSOL Minter</h1><p class="mt-3 max-w-2xl text-gray-400">Turn SOL into ckSOL in your connected ICP wallet, or redeem ckSOL back to SOL.</p></div>
  <div class="mb-6 rounded-xl border border-gray-700 bg-gray-800/60 p-4 text-sm text-gray-300">Send SOL on <strong>Solana mainnet</strong> only. Your deposit address belongs to the ICP account shown below.</div>
  <div class="mb-6 flex gap-3" role="tablist" aria-label="Conversion direction">
    <button role="tab" aria-selected={selectedTab === 'mint'} class="rounded-lg px-4 py-2" class:bg-gray-700={selectedTab === 'mint'} onclick={() => selectedTab = 'mint'}>Mint ckSOL</button>
    <button role="tab" aria-selected={selectedTab === 'redeem'} class="rounded-lg px-4 py-2" class:bg-gray-700={selectedTab === 'redeem'} onclick={() => selectedTab = 'redeem'}>Redeem SOL</button>
  </div>
  {#if !owner}<p class="mb-6 rounded-lg bg-gray-800 p-4">Connect your ICP wallet using the wallet button above to continue.</p>{/if}
  {#if error}<p role="alert" class="mb-4 break-words rounded-lg border border-red-700 bg-red-950/40 p-4 text-sm">{error}</p>{/if}
  {#if notice && selectedTab === 'redeem'}<p role="status" class="mb-4 break-words rounded-lg border border-gray-600 bg-gray-800 p-4 text-sm">{notice}</p>{/if}
  <section class="rounded-xl border border-gray-700 bg-gray-900 p-6">
    {#if selectedTab === 'mint'}
      <div class="flex items-center justify-between gap-4"><h2 class="text-xl font-semibold">SOL → ckSOL</h2><span class="text-xs text-emerald-400">Solana mainnet</span></div>
      <ol class="mt-5 grid grid-cols-3 gap-3 text-sm text-gray-400" aria-label="Mint progress"><li>1. Check processing costs</li><li>2. Send SOL</li><li>3. Mint ckSOL</li></ol>
      <div class="mt-5 flex flex-wrap gap-2" role="group" aria-label="Processing payment method">
        <button class="rounded-lg border px-3 py-2 text-sm" class:border-emerald-500={paymentMethod==='icp'} disabled={busy||icpBusy||pendingPayment} onclick={()=>paymentMethod='icp'}>Pay with ICP</button>
        <button class="rounded-lg border px-3 py-2 text-sm" class:border-emerald-500={paymentMethod==='cycles'} disabled={busy||icpBusy||icpPending} onclick={()=>paymentMethod='cycles'}>Use existing cycles</button>
      </div>
      {#if owner && paymentMethod==='icp'}
        {#key epoch}<CksolIcpPaymentPanel {owner} canMint={fundsReceived||balanceUnavailable} blocked={depositBlocked||blockingLegacy||minterPending||pendingPayment}
          onReady={(ready)=>icpReady=ready} onBusy={(active)=>icpBusy=active} onActive={(active)=>icpPending=active} onReceipt={adoptIcpReceipt}/>{/key}
      {/if}
      <div class="mt-6 rounded-xl border border-gray-700 bg-gray-800/60 p-4" aria-live="polite">
        {#if blockingLegacy || depositBlocked || minterPending}
          <h3 class="font-semibold">{depositBlocked || legacyAttempt?.status && 'SubmittedOrUnknown' in legacyAttempt.status ? 'Deposit needs attention' : depositStatus?.label ?? 'Checking your previous deposit'}</h3>
          <p class="mt-2 text-sm text-gray-300">{depositBlocked || legacyAttempt?.status && 'SubmittedOrUnknown' in legacyAttempt.status ? 'The previous deposit has an uncertain outcome. Its record is preserved. Contact support with the receipt below before trying another payment.' : 'We check the official minter automatically. You can leave this page and return later.'}</p>
        {:else if pendingPayment}
          <h3 class="font-semibold">{uncertainPayment ? 'Checking your previous request' : resumeApproval || resumeFunding ? 'Finish authorizing your mint' : refundReady ? 'Your cycles refund is ready' : 'Mint in progress'}</h3>
          <p class="mt-2 text-sm text-gray-300">{paymentMessage}</p>
        {:else if fundsReceived}
          <h3 class="text-lg font-semibold text-emerald-300">{formatSol(solBalance!)} SOL received</h3>
          <p class="mt-2 text-sm text-gray-300">Ready to mint to your connected ICP account. The address balance includes any rent reserve; the minter calculates the final amount.</p>
        {:else if depositStatus?.success}
          <h3 class="text-lg font-semibold text-emerald-300">{mintedAmount !== null ? `${formatSol(mintedAmount)} ckSOL received` : 'ckSOL minted to your ICP account'}</h3>
          <p class="mt-2 text-sm text-gray-300">Your previous deposit is complete. You can reuse the address below for another deposit.</p>
        {:else}
          <h3 class="font-semibold">{addressReady ? 'Send SOL to your deposit address' : 'Check processing funds before sending SOL'}</h3>
          <p class="mt-2 text-sm text-gray-300">{busy ? 'Checking your account…' : addressReady ? 'After sending SOL, select the mint action above. Your wallet will guide the confirmations.' : 'Minting needs a separate processing payment. Check your ICP balance and the maximum cost above, or choose existing cycles.'}</p>
        {/if}
        {#if balanceUnavailable}<p class="mt-2 text-sm text-amber-300">SOL balance checks are temporarily unavailable. If you already sent SOL to this address, you can still mint; the official minter verifies the balance.</p>{/if}
      </div>
      <div class="mt-5 rounded-xl bg-gray-800 p-4">
        <p class="text-sm font-medium">Your reusable SOL deposit address</p>
        {#if depositAddress && addressReady}
          <p class="mt-3 break-all font-mono text-sm text-gray-100">{depositAddress}</p>
          <button class="mt-3 rounded-lg border border-gray-500 px-3 py-2 text-sm" onclick={copyAddress}>{copied ? 'Address copied' : 'Copy address'}</button>
          <p class="mt-3 text-xs text-gray-400">Credits ICP account <span class="break-all">{ownerText}</span>. Reuse only with this account.</p>
        {:else}<p class="mt-3 text-sm text-gray-400">{!owner ? 'Connect your ICP wallet to see your address.' : legacyBlocked || depositBlocked ? 'Your existing deposit is being checked. You do not need to send more SOL.' : 'Your address will appear when processing funds are ready. If you already sent SOL, we still check that address automatically; do not send another deposit.'}</p>{/if}
        <p class="mt-3 text-sm text-gray-400">Minimum deposit balance: {info ? formatSol(BigInt(info.minimum_deposit_amount)) : 'Loading…'} SOL. Send SOL on Solana mainnet only.</p>
      </div>
      {#if paymentMethod==='cycles'}
      {#if paymentQuote}
        <div class="mt-5 text-sm text-gray-300"><p>Your connected account needs <strong>{formatCycles(paymentQuote.approval_amount + paymentQuote.ledger_fee)}</strong> temporarily for processing. This is a balance requirement, not the final fee. Unused cycles return as cycles, not ICP.</p><p class="mt-2 text-gray-400">SOL network fees are deducted before minting. The first deposit also leaves a small rent reserve on the address.</p></div>
      {:else if owner}<p class="mt-5 text-sm text-amber-300">{notice || 'Checking processing costs…'}</p>{/if}
      {#if paymentQuote && !cyclesReady && !pendingPayment}<p role="status" class="mt-3 text-sm text-amber-300">Not enough cycles to start processing. Choose Pay with ICP instead of buying cycles yourself. If you already sent SOL, keep this account connected and finish the existing deposit; do not send more SOL.</p>{/if}
      <button class="mt-6 w-full rounded-xl bg-emerald-600 px-5 py-3 font-semibold disabled:opacity-40" disabled={!mintEnabled} onclick={mint}>{busy ? 'Checking request…' : primaryLabel}</button>
      <p class="mt-3 text-sm text-gray-400">Your wallet will ask you to approve the processing payment and authorize minting. Keep it open until both confirmations finish. Status updates automatically afterward.</p>
      {/if}
      <p class="mt-5 text-sm text-gray-400">SOL waiting at this address has not become ckSOL yet. Redeem SOL withdraws minted ckSOL; it does not cancel an unprocessed deposit. To recover a deposit that needs attention, keep its receipt and contact support.</p>
      <details class="mt-6 border-t border-gray-700 pt-4 text-sm text-gray-400"><summary class="cursor-pointer">Receipt and processing details</summary>
        {#if cyclesBalance !== null}<p class="mt-3">Cycles ledger balance: {formatCycles(cyclesBalance)}.</p>{/if}
        {#if paymentQuote}<p class="mt-2">Processing budget: {formatCycles(paymentQuote.funding_cycles)}, including {formatCycles(paymentQuote.processing_fee)} processing fee. Approval cap: {formatCycles(paymentQuote.approval_amount)}. Approval transaction fee: {formatCycles(paymentQuote.ledger_fee)}. Approval expires after 10 minutes.</p>{/if}
        {#if resumeFunding}<button class="mt-3 rounded-lg border border-gray-600 px-3 py-2" disabled={busy} onclick={refundPaidCycles}>Cancel mint and return funded cycles</button>{/if}
        {#if paymentIntent}<p class="mt-3">Paid payment sequence: {paymentIntent.sequence}; state: {paidPhase}.</p><p class="mt-2">{paymentMessage}</p>
          {#if paymentIntent.receipt?.funding_block?.[0] !== undefined}<p class="mt-2">Funding ledger block: {paymentIntent.receipt.funding_block[0].toString()}</p>{/if}
          {#if paymentIntent.receipt?.refund_block?.[0] !== undefined}<p class="mt-2">Refund ledger block: {paymentIntent.receipt.refund_block[0].toString()}</p>{/if}
        {/if}
        {#if mintedAmount !== null}<p class="mt-3">Minted: {formatSol(mintedAmount)} ckSOL. Ledger block: {mintBlock?.toString()}.</p>{/if}
        {#if depositId !== null}<p class="mt-3">Deposit ID: {depositId.toString()}</p><p class="mt-2">{depositStatus?.label ?? 'Checking minter status…'}</p>{#if depositStatus?.signature}<a class="mt-2 block text-emerald-400 underline" href="https://solscan.io/tx/{depositStatus.signature}" target="_blank" rel="noreferrer">View sweep on Solana</a>{/if}{/if}
        <button class="mt-4 rounded-lg border border-gray-600 px-3 py-2" disabled={busy} onclick={refresh}>Check for updates</button>
        <a class="mt-3 block text-emerald-400 underline" href="https://internetcomputer.org/docs/current/developer-docs/smart-contracts/maintain/cycles-management/" target="_blank" rel="noreferrer">How to add cycles</a>
      </details>
    {:else}
      <h2 class="text-xl font-medium">ckSOL → SOL</h2>
      <p class="mt-3 text-sm text-gray-400">This withdraws ckSOL already minted to your connected account. SOL awaiting processing must finish minting first. Minting and redemption have separate fees and confirmations.</p>
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
  <div class="mt-6 flex flex-wrap items-center gap-4 text-sm">{#if selectedTab === 'redeem'}<button class="text-gray-300 underline" disabled={busy} onclick={refresh}>Refresh balance and minter terms</button>{/if}{#if activeCollateral?.status === 'Active'}<a class="text-emerald-400 underline" href="/?collateral={CANISTER_IDS.CKSOL_LEDGER}">Borrow icUSD with ckSOL</a>{/if}<a class="text-gray-400 underline" href="https://internetcomputer.org/docs/current/developer-docs/smart-contracts/maintain/cycles-management/" target="_blank" rel="noreferrer">Cycles ledger documentation</a><a class="text-gray-400 underline" href="https://github.com/dfinity/cksol" target="_blank" rel="noreferrer">Official ckSOL documentation</a></div>
</main>
