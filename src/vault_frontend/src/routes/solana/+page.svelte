<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { createCksolReadPolling, type CksolReadPolling } from '$lib/utils/cksolReadPolling';
  import type { CksolIcpQuote } from '$lib/services/cksolIcpPayment';
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
  let refreshing = false;
  let initialized = false;
  let refreshGeneration = 0;
  let polling: CksolReadPolling | null = null;
  let newDeposit = false;
  let icpCost: CksolIcpQuote | null = null;
  let icpPanel: CksolIcpPaymentPanel | null = null;
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

  let copied = false;
  let depositId: bigint | null = null;
  let depositStatus: StatusView | null = null;
  let mintedAmount: bigint | null = null;
  let mintBlock: bigint | null = null;
  let depositBlocked = false;
  let legacyAttempt: any = null;
  let legacyBlocked = false;
  let legacyReadFailed = false;
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
  let mintActionStarted = false;
  let mintRequestMode: 'cycles' | 'icp' | null = null;
  let mintReceiptOpen = false;
  let statusChecking = false;
  let lastMintCheck: number | null = null;
  let lastPaymentCheck: { key: string; at: number } | null = null;
  let mintCheckFailed = false;
  let pageVisible = true;
  let icpProgress: { phase?: string; sequence?: string; depositId: bigint | null; checking: boolean; lastChecked: number | null; unavailable: boolean } | null = null;
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
    if (!mintRequestMode || (!paidIntentSettled(value) && !icpBusy)) mintRequestMode = 'cycles';
    const id = value.receipt?.deposit_id?.[0];
    if (id !== undefined && depositId !== BigInt(id)) { newDeposit=false; mintActionStarted=false; depositId=BigInt(id); depositStatus=null; mintedAmount=null; mintBlock=null; lastMintCheck=null; mintCheckFailed=false; solBalance=null; balanceUnavailable=false; pollCount=0; }
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
  // A matching queued ID is an active financial hold, not an unknown mint.
  // Keep that hold unchanged while distinguishing its known minter progress.
  $: matchedLegacyDeposit = depositId !== null && legacyAttempt?.status && 'Queued' in legacyAttempt.status
    && BigInt(legacyAttempt.status.Queued) === depositId;
  $: legacyNeedsAttention = legacyReadFailed || (blockingLegacy && !(matchedLegacyDeposit
    && ['Queued','Swept','Finalized','Minted'].includes(depositStatus?.kind ?? '')));
  // A positive balance may be only the address's rent reserve or dust. Reveal
  // a new-send address only after processing is ready; a deposit at the
  // minter's minimum remains visible so its owner can finish processing it.
  $: addressReady = fundsReceived || (paymentMethod === 'icp' ? icpReady : cyclesReady);
  $: mintEnabled = !!owner && initialized && !busy && !icpBusy && !icpPending && (refundReady || ((!unmatchedMintRequest || resumeApproval || resumeFunding) && !depositBlocked && !blockingLegacy && !minterPending &&
    (resumeApproval || resumeFunding || (cyclesReady && !pendingPayment && (fundsReceived || balanceUnavailable)))));
  $: primaryLabel = resumeApproval || resumeFunding ? 'Continue minting' : refundReady ? 'Return unused cycles' : 'Mint ckSOL';
  $: paymentMessage = paidPhase === 'Complete' ? 'Processing payment settled. Any unused cycles have been returned.' :
    paidPhase === 'FundingRejected' ? 'No processing payment was taken. You can try minting again.' :
    paidPhase === 'Approved' ? 'Payment approved. Continue minting to finish authorization.' :
    paidPhase === 'Funded' ? 'Processing is funded. Continue minting without another cycles approval.' :
    paidPhase === 'RefundReady' ? 'Unused cycles are ready to return to your wallet.' :
    ['ApprovalUnknown','StartUnknown','FundingUnknown','RefundUnknown'].includes(paidPhase ?? '') ? 'Checking an uncertain payment result. Your saved request is retained; do not start another payment.' :
    pendingPayment ? 'Processing your request. This page checks its progress automatically.' : '';
  $: complete = !!depositStatus?.success && !pendingPayment && !icpPending && !depositBlocked && !blockingLegacy && !unmatchedMintRequest;
  // An old Minted ID is historical evidence while a different payment/request
  // has no matching deposit ID. Payment Complete alone never proves a mint.
  $: cyclesReceiptId = paymentIntent?.receipt?.deposit_id?.[0];
  $: cyclesRequestUnmatched = mintRequestMode === 'cycles' && !!paymentIntent && !['NotStarted', 'FundingRejected'].includes(paidPhase ?? '')
    && (pendingPayment || paymentIntent.receipt) && (cyclesReceiptId === undefined || BigInt(cyclesReceiptId) !== depositId);
  $: icpRequestUnmatched = mintRequestMode === 'icp' && !!icpProgress?.sequence && !['NotStarted', 'FundingRejected'].includes(icpProgress.phase ?? '')
    && (icpProgress.depositId === null || icpProgress.depositId !== depositId);
  $: unmatchedMintRequest = depositId !== null && (cyclesRequestUnmatched || icpRequestUnmatched);
  $: showMintSuccess = !!depositStatus?.success && mintedAmount !== null && mintBlock !== null
    && !newDeposit && !mintActionStarted && !cyclesRequestUnmatched && !icpRequestUnmatched && !legacyNeedsAttention;
  $: displayedPaymentPhase = mintRequestMode === 'icp' ? icpProgress?.phase : paidPhase;
  $: resumeIcpRequest = mintRequestMode === 'icp' && ['Approved','Funded'].includes(icpProgress?.phase ?? '');
  $: continuationNeeded = !showMintSuccess && !busy && !icpBusy && !legacyNeedsAttention && !depositBlocked
    && !minterPending && ['Approved','Funded'].includes(displayedPaymentPhase ?? '');
  $: continuationLabel = mintRequestMode === 'icp'
    ? displayedPaymentPhase === 'Approved' ? 'Continue approved mint' : 'Continue funded mint'
    : 'Continue minting';
  $: preparingAnotherDeposit = newDeposit && !mintActionStarted && !pendingPayment && !icpPending && !unmatchedMintRequest;
  $: mintRequestKey = `${ownerText}|${mintRequestMode ?? 'minter'}|${mintRequestMode === 'icp' ? icpProgress?.sequence ?? '' : paymentIntent?.sequence ?? ''}`;
  $: lastRequestPaymentCheck = mintRequestMode === 'icp' ? icpProgress?.lastChecked
    : lastPaymentCheck?.key === mintRequestKey ? lastPaymentCheck.at : null;
  $: mintStatusTitle = showMintSuccess ? 'Mint complete'
    : busy || icpBusy ? 'Mint action in progress'
    : mintCheckFailed || legacyNeedsAttention || depositBlocked ? 'Checking mint status'
    : minterPending && !newDeposit && !cyclesRequestUnmatched && !icpRequestUnmatched ? depositStatus?.kind === 'Finalized' ? 'ckSOL mint pending'
      : depositStatus?.kind === 'Swept' ? 'Processing SOL deposit' : 'Mint request submitted'
    : continuationNeeded ? displayedPaymentPhase === 'Approved' ? 'Action needed: approval recorded' : 'Action needed: processing funds recorded'
    : displayedPaymentPhase === 'Starting' ? 'Confirming submission'
    : uncertainPayment || (mintRequestMode === 'icp' && icpProgress?.unavailable) ? 'Checking mint status'
    : preparingAnotherDeposit ? 'Ready for another deposit'
    : displayedPaymentPhase === 'Approving' ? 'Awaiting wallet confirmation'
    : cyclesRequestUnmatched || icpRequestUnmatched ? 'Processing request'
    : minterPending ? depositStatus?.kind === 'Finalized' ? 'ckSOL mint pending'
      : depositStatus?.kind === 'Swept' ? 'Processing SOL deposit' : 'Mint request submitted'
    : 'Mint status';
  $: mintStatusActive = !showMintSuccess && (busy || icpBusy || minterPending || pendingPayment || icpPending || blockingLegacy || depositBlocked || unmatchedMintRequest);
  $: mintStatusChecking = statusChecking || refreshing || (paymentMethod === 'icp' && !!icpProgress?.checking);
  $: knownPendingPayment = pendingPayment || (icpPending && !!icpProgress?.sequence
    && !['Complete','FundingRejected','NotStarted'].includes(icpProgress.phase ?? ''));
  $: postMintPaymentMessage = knownPendingPayment
    ? 'Minting is complete, but the processing payment or refund is still pending. We keep checking that request; do not pay again.'
    : 'Minting is complete. Processing payment status is still being checked. No new payment is needed.';
  $: autoPolling = !!owner && !busy && !icpBusy && (pendingPayment || icpPending || minterPending || blockingLegacy || depositBlocked || unmatchedMintRequest || fundsReceived || newDeposit || (!complete && addressReady));
  $: pollingFingerprint = [ownerText, paymentIntent?.sequence, paidPhase, icpReceipt?.sequence, icpReceipt?.phase, depositId, depositStatus?.label, mintedAmount, solBalance, balanceUnavailable, legacyBlocked, depositBlocked].join('|');
  $: if (polling) { autoPolling; pollingFingerprint; polling.sync(); }
  const formatIcp = (value: bigint) => `${value / 100_000_000n}.${(value % 100_000_000n).toString().padStart(8, '0')} ICP`;
  function pollFingerprint() {
    return pollingFingerprint;
  }
  async function checkAgain() {
    if (busy || icpBusy) return;
    await Promise.all([polling ? polling.checkAgain() : refresh(), icpPanel?.checkAgain()]);
  }
  function startAnotherDeposit() { newDeposit = true; mintActionStarted = false; void checkAgain(); }
  function icpBusyChanged(active: boolean) { if (active) refreshGeneration++; icpBusy = active; }
  function observeIcpProgress(progress: NonNullable<typeof icpProgress>) {
    icpProgress = progress;
    if (progress.sequence && (!mintRequestMode || (progress.phase && !['Complete', 'FundingRejected', 'NotStarted'].includes(progress.phase) && !busy))) mintRequestMode = 'icp';
  }
  async function mint() {
    if (!mintEnabled) return;
    refreshGeneration++;
    if (!refundReady) { mintActionStarted = true; mintRequestMode = 'cycles'; }
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
    pageVisible = document.visibilityState !== 'hidden';
    if (document.visibilityState === 'hidden') polling?.sync();
    else if (autoPolling) void polling?.checkAgain();
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
    if (busy || icpBusy || refreshing) return;
    const e = epoch, account = owner, text = ownerText;
    const request = ++refreshGeneration;
    const currentRead = () => live(e, text) && request === refreshGeneration && !busy && !icpBusy;
    error = ''; refreshing = true;
    try {
      const data = await getCksolMinter().get_minter_info();
      if (!currentRead()) return;
      info = data;
      if (account) {
        const recovered=await recoverPaid(account);
        if (!currentRead()) return;
        if (recovered) adoptPaidIntent(recovered);
        if (recovered) lastPaymentCheck = { key: `${text}|cycles|${recovered.sequence}`, at: Date.now() };
        if (recovered && !paidIntentSettled(recovered)) paymentMethod = 'cycles';
        try {
          const q=await quoteCksolPayment(account,isCksolOisyWallet()?'oisy':(get(auth).walletType ?? 'unknown'));
          if (!currentRead()) return;
          paymentQuote=q; cyclesBalance=q.balance;
        } catch {
          if (!currentRead()) return;
          paymentQuote=null;
          notice='Processing payment is temporarily unavailable. Do not send SOL to a new address. Existing deposits remain recorded; status checks will continue.';
        }
      }
      if (!account || !currentRead()) return;
      const [funds, attempt] = await Promise.all([
        getCksolLedger().icrc1_balance_of({ owner: account, subaccount: [] }),
        getCksolRelay().get_cksol_deposit(account),
      ]);
      if (!currentRead()) return;
      legacyReadFailed = false;
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
      if (!currentRead()) return;
      if (paymentQuote && !legacyBlocked && !depositBlocked && !depositAddress) {
        const address=await getSolDepositAddress(account);
        if (!currentRead()) return;
        depositAddress=address;
      }
      if (depositAddress) {
        try {
          const observed=await readSolDepositBalance(depositAddress);
          if (currentRead()) { solBalance=observed; balanceUnavailable=false; }
        } catch { if (currentRead()) { solBalance=null; balanceUnavailable=true; } }
      }
    } catch (err) { if (currentRead()) { error = String(err); legacyBlocked=true; legacyReadFailed=true; } }
    finally { if (!destroyed && epoch === e) { refreshing = false; if (request === refreshGeneration) initialized = true; }
      await tick(); polling?.sync(); }
  }
  function adoptIcpReceipt(receipt: any) {
    icpReceipt=receipt;
    if (!mintRequestMode) mintRequestMode = 'icp';
    const id = receipt.deposit_id?.[0];
    if (id !== undefined && depositId !== BigInt(id)) {
      newDeposit=false;mintActionStarted=false;depositId=BigInt(id);depositStatus=null;mintedAmount=null;mintBlock=null;lastMintCheck=null;mintCheckFailed=false;solBalance=null;balanceUnavailable=false;pollCount=0;
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
    statusChecking = requestedDepositId !== null;
    let pending = false;
    try {
      if (requestedDepositId !== null) {
        const raw = await getCksolMinter().deposit_sol_status(requestedDepositId);
        if (!currentRequest() || depositId !== requestedDepositId) return;
        lastMintCheck = Date.now(); mintCheckFailed = false;
        depositStatus = depositView(raw); depositBlocked = 'Quarantined' in raw || 'NotFound' in raw; pending ||= !depositStatus.terminal;
        if (legacyAttempt?.status && 'Queued' in legacyAttempt.status && BigInt(legacyAttempt.status.Queued) === requestedDepositId)
          legacyBlocked = !('Minted' in raw || 'Dropped' in raw);
        if ('Minted' in raw) {
          const nextBlock = BigInt(raw.Minted.block_index), account = owner;
          const newlyConfirmed = mintBlock !== nextBlock;
          mintedAmount=BigInt(raw.Minted.minted_amount); mintBlock=nextBlock;
          notice=`${formatSol(mintedAmount)} ckSOL minted. Ledger block ${mintBlock}.`;
          if (newlyConfirmed && account) {
            // The last automatic read must not leave the pre-mint wallet balance on screen.
            balance = null;
            try {
              const currentBalance = await getCksolLedger().icrc1_balance_of({ owner: account, subaccount: [] });
              if (!currentRequest() || depositId !== requestedDepositId) return;
              balance = BigInt(currentBalance);
            } catch {
              if (!currentRequest() || depositId !== requestedDepositId) return;
              balance = null;
            }
          }
        }
      }
      if (requestedBurnBlock) {
        const raw = await getCksolMinter().withdraw_sol_status({ block_index: BigInt(requestedBurnBlock) });
        if (!currentRequest() || intent?.id !== requestedIntentId || intent?.burnBlock !== requestedBurnBlock) return;
        withdrawalStatus = withdrawalView(raw); withdrawalStatusFor = { intentId: requestedIntentId!, burnBlock: requestedBurnBlock }; pending ||= !withdrawalStatus.terminal;
      }
      if (schedule && pending && pollCount++ < 60) timer = setTimeout(() => { void checkStatus(true); }, 5_000);
      else if (pending && schedule) notice = 'Automatic checking paused after five minutes. Use Check status to continue.';
    } catch (err) { if (currentRequest()) { mintCheckFailed = true; error = `Status check failed: ${String(err)}. Keep the saved ID and check again.`; } }
    finally { if (currentRequest()) statusChecking = false; }
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
    epoch++; refreshGeneration++; statusGeneration++; stopPoll(); polling?.destroy(); polling = null; pollCount = 0; newDeposit = false; initialized = false; refreshing = false; icpCost = null;
    const wallet = get(walletStore);
    owner = wallet.isConnected && !wallet.loading ? wallet.principal : null; ownerText = owner?.toText() ?? '';
    busy = false; error = ''; notice = ''; depositAddress = ''; depositId = null; solBalance=null; balanceUnavailable=false; copied=false; mintedAmount=null; mintBlock=null;
    depositStatus = null; depositBlocked = false; legacyAttempt=null;legacyBlocked=false;legacyReadFailed=false;withdrawalStatus = null; withdrawalStatusFor = null; intent = null; invalidSavedIntent = false;
    balance = null; quote = null; amount = ''; destination = ''; oisy = isCksolOisyWallet(); paymentQuote=null;cyclesBalance=null;paymentIntent=null;
    icpReady=false;icpBusy=false;icpPending=false;icpReceipt=null;paymentMethod='icp';
    mintActionStarted=false;mintRequestMode=null;mintReceiptOpen=false;statusChecking=false;lastMintCheck=null;lastPaymentCheck=null;mintCheckFailed=false;icpProgress=null;
    if(owner) { try { paymentIntent=loadCksolPayment(localStorage,ownerText); } catch { error='Saved paid ckSOL request is invalid. Preserve it for support.'; } }
    if (owner) loadSaved();
    polling = createCksolReadPolling({ read: async () => { await refresh(); await tick(); }, active: () => autoPolling, fingerprint: pollFingerprint, visible: () => document.visibilityState !== 'hidden' });
    void refresh();
  }
  onMount(() => {
    pageVisible = document.visibilityState !== 'hidden';
    unsubscribers.push(walletStore.subscribe((wallet) => {
      const nextOwner = wallet.isConnected && !wallet.loading ? wallet.principal?.toText() ?? '' : '';
      if (nextOwner !== ownerText) resetSession();
    }));
    // Generation invalidates immediately when a wallet transition begins, including A -> B -> A.
    unsubscribers.push(walletSessionGeneration.subscribe(() => resetSession()));

    window.addEventListener('focus',refreshWhenVisible);
    document.addEventListener('visibilitychange',refreshWhenVisible);
    void collateralStore.fetchSupportedCollateral();
  });
  onDestroy(() => { destroyed=true; epoch++; stopPoll(); polling?.destroy(); window.removeEventListener('focus',refreshWhenVisible); document.removeEventListener('visibilitychange',refreshWhenVisible); unsubscribers.forEach(fn=>fn()); });
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
      <h2 class="text-xl font-semibold">SOL → ckSOL</h2>
      <p class="mt-2 text-sm text-gray-400">Follow these steps with the same connected ICP account. Detecting SOL does not mint it automatically.</p>
      <div class="mt-6 space-y-6">
        {#if !showMintSuccess}
        <section aria-label="Step 1: Check processing costs" class="rounded-xl border border-gray-700 p-4">
          <h3 class="text-lg font-semibold">1. Check processing costs</h3>
          <div class="mt-3 flex flex-wrap gap-2" role="group" aria-label="Processing payment method">
            <button class="rounded-lg border px-3 py-2 text-sm" class:border-emerald-500={paymentMethod==='icp'} disabled={busy||icpBusy||pendingPayment} onclick={()=>paymentMethod='icp'}>Pay with ICP</button>
            <button class="rounded-lg border px-3 py-2 text-sm" class:border-emerald-500={paymentMethod==='cycles'} disabled={busy||icpBusy||icpPending} onclick={()=>paymentMethod='cycles'}>Use existing cycles</button>
          </div>
          {#if paymentMethod === 'icp'}
            <p class="mt-3 text-sm text-gray-300">Rumi supplies the cycles. You approve a maximum ICP deposit; unused ICP is returned. The service fee is 5% of the minter’s net cycles charge, plus operating and ledger fees.</p>
            {#if icpCost}<p class="mt-3 text-sm">Maximum wallet debit: <strong>{formatIcp(icpCost.total)}</strong>. Connected balance: {formatIcp(icpCost.balance)}.</p>{/if}
            {#if icpCost && (!icpCost.terms.available || BigInt(icpCost.terms.reserve_available_cycles) < BigInt(icpCost.terms.attached_cycles))}<p class="mt-2 text-sm text-amber-300">ICP-paid processing is temporarily unavailable. Choose existing cycles or Check again. Do not send SOL for a new mint.</p>
            {:else if icpCost && icpCost.balance < icpCost.total && !icpPending}<p class="mt-2 text-sm text-amber-300">Add at least {formatIcp(icpCost.total - icpCost.balance)} to this ICP account before a new deposit.</p>
            {:else if owner && !icpReady && !icpPending}<p class="mt-2 text-sm text-amber-300">Processing is not ready. Review the ICP availability and cost details in step 3 before sending a new deposit.</p>{/if}
          {:else if paymentQuote}
            <p class="mt-3 text-sm text-gray-300">Keep <strong>{formatCycles(paymentQuote.approval_amount + paymentQuote.ledger_fee)}</strong> in your connected account temporarily. This is a balance requirement, not the final fee. Unused cycles return as cycles.</p>
            {#if !cyclesReady && !pendingPayment}<p class="mt-2 text-sm text-amber-300">Not enough cycles. Choose Pay with ICP or add cycles to this account. If you already sent SOL, do not send it again.</p>{/if}
          {:else}<p class="mt-3 text-sm text-amber-300">{!owner ? 'Connect your ICP wallet to check processing costs.' : notice || 'Processing costs are unavailable. Select Check again; do not send a new deposit yet.'}</p>{/if}
        </section>

        <section aria-label="Step 2: Send SOL" class="rounded-xl border border-gray-700 p-4">
          <h3 class="text-lg font-semibold">2. Send SOL on Solana mainnet</h3>
          {#if minterPending || pendingPayment || icpPending || mintActionStarted || cyclesRequestUnmatched || icpRequestUnmatched}
            <p class="mt-3 text-sm text-amber-300">Keep the current request and its receipt. Do not send or pay again while its outcome is being checked.</p>
          {:else}
            <p class="mt-3 text-sm text-gray-300">Send SOL to this account’s reusable deposit address. If you already sent it, check detection below instead of sending again.</p>
          {/if}
          {#if depositAddress && addressReady}
            <p class="mt-3 break-all rounded-lg bg-gray-800 p-3 font-mono text-sm">{depositAddress}</p>
            <button class="mt-3 rounded-lg border border-gray-500 px-3 py-2 text-sm" onclick={copyAddress}>{copied ? 'Address copied' : 'Copy address'}</button>
            <p class="mt-3 break-words text-xs text-gray-400">Credits ICP account {ownerText}. Reuse only with this account.</p>
          {:else}<p class="mt-3 text-sm text-gray-400">{!owner ? 'Connect your ICP wallet to see your address.' : legacyBlocked || depositBlocked ? 'Your existing deposit is being checked. You do not need to send more SOL.' : 'Check processing funds before sending SOL. Your address appears when processing is ready; existing deposits are still checked.'}</p>{/if}
          <p class="mt-3 text-sm text-gray-400">Minimum address balance: {info ? formatSol(BigInt(info.minimum_deposit_amount)) : 'Loading…'} SOL. SOL network fees and any first-deposit rent reserve reduce the final minted amount.</p>
          <div class="mt-4 rounded-lg bg-gray-800 p-3 text-sm" aria-live="polite">
            {#if legacyNeedsAttention || depositBlocked}<p class="text-amber-300">Deposit needs attention. Its outcome is uncertain; keep the receipt and contact support before another payment.</p>
            {:else if minterPending}<p>{depositStatus?.label ?? 'Checking your submitted deposit'} — SOL processing is already underway. Do not send or pay again.</p>
            {:else if cyclesRequestUnmatched || icpRequestUnmatched || mintActionStarted}<p>Checking the current processing request. A previous completed mint is not confirmation of this request.</p>
            {:else if balanceUnavailable}<p class="text-amber-300">SOL balance checks are temporarily unavailable. If you already sent SOL here, the mint action can ask the official minter to verify it. Detection has not confirmed a new deposit.</p>
            {:else if fundsReceived}<p class="font-semibold text-emerald-300">{formatSol(solBalance!)} SOL received</p><p class="mt-1">Next: use the mint action in step 3. It only enables when processing and previous-request checks permit.</p>
            {:else if complete && !newDeposit}<p>Automatic checking stopped after completion. Select Start another deposit to watch for a new transfer.</p>
            {:else}<p>Waiting for SOL at this address. We check automatically while this tab is visible. You can also select Check again.</p>{/if}
            {#if refreshing}<p class="mt-2 text-gray-400">Checking balances and saved receipts… No mint or payment is submitted by this check.</p>{/if}
            <button class="mt-3 rounded-lg border border-gray-500 px-3 py-2 disabled:opacity-40" disabled={!owner || busy || icpBusy || refreshing} onclick={checkAgain}>Check again</button>
          </div>
        </section>
        {/if}

        <section aria-label="Step 3: Mint ckSOL" class="rounded-xl border border-gray-700 p-4">
          <h3 class="text-lg font-semibold">3. Mint ckSOL</h3>
          <section aria-label="Mint progress" aria-live="polite" aria-atomic="true" data-request={mintRequestKey}
            class={`mt-4 rounded-xl border p-5 ${showMintSuccess ? 'border-emerald-500 bg-emerald-950/30' : 'border-gray-600'}`}>
            <div class="flex items-center gap-3">
              {#if ((mintStatusActive && !continuationNeeded) || mintStatusChecking) && !showMintSuccess}<span aria-hidden="true" class="h-5 w-5 shrink-0 animate-spin rounded-full border-2 border-gray-600 border-t-emerald-400 motion-reduce:animate-none"></span>{/if}
              <h4 class="text-lg font-semibold" class:text-emerald-300={showMintSuccess}>{mintStatusTitle}</h4>
            </div>
            {#if showMintSuccess}
              <p class="mt-3 text-2xl font-semibold text-emerald-300">{formatSol(mintedAmount!)} ckSOL minted</p>
              <p class="mt-2 text-sm text-gray-300">Confirmed by the official minter for deposit #{depositId?.toString()}. Your ckSOL is on the Internet Computer.</p>
              <dl class="mt-4 space-y-2 text-sm">
                <div><dt class="text-gray-400">ICP destination account</dt><dd class="break-all font-mono">{ownerText}</dd></div>
                <div><dt class="text-gray-400">ckSOL ledger block</dt><dd class="font-mono">{mintBlock?.toString()}</dd></div>
              </dl>
              <a class="mt-3 inline-block text-sm text-emerald-300 underline" href="#cksol-mint-receipt" onclick={()=>mintReceiptOpen=true}>View receipt and processing details</a>
              {#if pendingPayment || icpPending}<p class="mt-4 text-sm text-amber-300">{postMintPaymentMessage}</p>{/if}
              <button class="mt-4 rounded-lg border border-emerald-500 px-4 py-2 text-sm font-medium disabled:opacity-40" disabled={!complete || busy || icpBusy} onclick={startAnotherDeposit}>Start another deposit</button>
              {#if !complete}<p class="mt-2 text-xs text-gray-400">A new payment remains blocked until the existing request is settled or reconciled.</p>{/if}
            {:else if busy || icpBusy}
              <p class="mt-3 text-sm text-gray-300">Follow any wallet confirmations. We are waiting for this action’s result; do not submit another payment.</p>
            {:else if mintCheckFailed || legacyNeedsAttention || depositBlocked}
              <p class="mt-3 text-sm text-amber-300">The latest mint result could not be confirmed. Keep this request and its receipt. Check again reads its status; it does not submit a mint or payment.</p>
            {:else if continuationNeeded}
              <p class="mt-3 text-sm text-gray-300">{displayedPaymentPhase === 'Approved' ? 'Your wallet allowance is approved. This does not mean a processing payment was collected or ckSOL was minted.' : 'Your processing payment is funded. Continue this existing request without another payment; ckSOL mint completion is not confirmed.'}</p>
              <p class="mt-2 text-sm font-medium text-amber-300">Select {continuationLabel} below. Status checks will not submit this request automatically.</p>
            {:else if preparingAnotherDeposit}
              <p class="mt-3 text-sm text-gray-300">Your previous mint receipt is retained below. Check processing costs and deposit detection before authorizing another mint.</p>
            {:else if cyclesRequestUnmatched || icpRequestUnmatched}
              <p class="mt-3 text-sm text-gray-300">{displayedPaymentPhase === 'Complete' ? 'Processing payment settled, but no matching ckSOL mint has been confirmed for this request.' : displayedPaymentPhase === 'Approving' ? 'The saved request is awaiting its wallet approval result.' : displayedPaymentPhase === 'Starting' ? 'Checking whether the saved submission was accepted. Acceptance by the official minter is not confirmed yet.' : 'Checking the current processing request. Payment progress does not confirm that ckSOL was minted.'} Do not pay again.</p>
            {:else if depositId !== null && depositStatus}
              <p class="mt-3 text-sm text-gray-300">{depositStatus.label} {#if !depositStatus.terminal}This request is still in progress; do not send or pay again.{/if}</p>
            {:else if depositId !== null}
              <p class="mt-3 text-sm text-gray-300">Deposit #{depositId.toString()} is recorded. Checking its status with the official minter; mint completion is not confirmed yet.</p>
            {:else}
              <p class="mt-3 text-sm text-gray-300">Authorize the mint action after checking your deposit and processing costs. A detected SOL balance alone does not confirm a mint.</p>
            {/if}
            {#if lastMintCheck !== null && !newDeposit && !mintActionStarted && !cyclesRequestUnmatched && !icpRequestUnmatched}
              <p class="mt-3 text-xs text-gray-400">Last successful mint check: {new Date(lastMintCheck).toLocaleTimeString()}.{#if mintCheckFailed} The latest check failed; the last known result is retained.{/if}</p>
            {:else if lastRequestPaymentCheck && !busy && !icpBusy}
              <p class="mt-3 text-xs text-gray-400">Last successful payment check: {new Date(lastRequestPaymentCheck).toLocaleTimeString()}. This is not confirmation of a mint.</p>
            {/if}
            {#if (mintStatusActive || pendingPayment || icpPending) && !busy && !icpBusy}
              <p class="mt-3 text-xs text-gray-400">{!pageVisible ? 'Automatic checking is paused while this tab is hidden. It resumes when you return.' : mintStatusChecking ? 'Checking the saved request now…' : 'Checking this request automatically. You can also select Check again.'}</p>
            {/if}
            <button class="mt-4 rounded-lg border border-gray-500 px-3 py-2 text-sm disabled:opacity-40" disabled={!owner || busy || icpBusy || refreshing || statusChecking} onclick={checkAgain}>Check again</button>
            <p class="mt-2 text-xs text-gray-400">Status checks never start a new payment or mint.</p>
          </section>
          {#if !showMintSuccess && !mintStatusActive && !continuationNeeded}
          <p class="mt-2 text-sm text-gray-300">Click the mint action below and follow any wallet confirmations. This is a deliberate action; sending SOL or checking its balance never starts a payment.</p>
          {/if}
          {#if owner && paymentMethod === 'icp'}
            {#key epoch}<CksolIcpPaymentPanel bind:this={icpPanel} {owner} compact mintComplete={showMintSuccess} pollingActive={autoPolling} canMint={initialized && (fundsReceived||balanceUnavailable)} blocked={depositBlocked||blockingLegacy||minterPending||pendingPayment||(unmatchedMintRequest&&!resumeIcpRequest)}
              onQuote={(q)=>icpCost=q} onReady={(ready)=>icpReady=ready} onBusy={icpBusyChanged} onActive={(active)=>icpPending=active} onReceipt={adoptIcpReceipt}
              onMintStart={()=>{mintActionStarted=true;mintRequestMode='icp';}} onProgress={observeIcpProgress}/>{/key}
            {#if !showMintSuccess && !icpPending && !fundsReceived && !balanceUnavailable}<p class="mt-3 text-sm text-gray-400">The mint action is waiting for a new SOL deposit meeting the minimum in step 2.</p>{/if}
          {:else if !showMintSuccess || refundReady}
            <button class="mt-4 w-full rounded-xl bg-emerald-600 px-5 py-3 font-semibold disabled:opacity-40" disabled={!mintEnabled} onclick={mint}>{busy ? 'Processing your action…' : primaryLabel}</button>
            <p class="mt-3 text-sm text-gray-400">{!owner ? 'Connect your ICP wallet to continue.' : pendingPayment || minterPending || blockingLegacy || depositBlocked ? paymentMessage || 'The previous request must finish or be reconciled before a new mint.' : !initialized ? 'Checking initial account status before enabling minting.' : !cyclesReady ? 'Processing funds are not ready. Review step 1.' : !fundsReceived && !balanceUnavailable ? 'Waiting for a new SOL deposit meeting the minimum in step 2.' : 'Ready for your mint action. Keep any wallet confirmation window open until it finishes.'}</p>
          {/if}
          {#if paymentMessage && !paidIntentSettled(paymentIntent)}<p class="mt-3 text-sm text-gray-300">{paymentMessage}</p>{/if}
        </section>

        <section aria-label="Step 4: Check your ICP wallet" class="rounded-xl border border-gray-700 p-4">
          <h3 class="text-lg font-semibold">4. Check your connected ICP wallet</h3>
          {#if depositStatus?.success}<p class="mt-3 text-sm text-gray-300">Last completed deposit: {mintedAmount !== null ? `${formatSol(mintedAmount)} ckSOL received` : 'ckSOL minted to this ICP account'}. {#if !showMintSuccess}This is the previous receipt, not confirmation of a new request.{/if}</p>{/if}
          <p class="mt-3 text-sm">{balance === null ? showMintSuccess ? 'Mint verified. Check your wallet or select Check again to refresh its current balance.' : 'Your ckSOL balance will appear after an account check.' : `Connected wallet balance: ${formatSol(balance)} ckSOL.`}</p>
          <p class="mt-2 text-sm text-gray-400">Look for ckSOL on the Internet Computer in this same wallet. A settled processing payment alone does not confirm mint completion.</p>
          {#if showMintSuccess && (pendingPayment || icpPending)}<p class="mt-2 text-sm text-amber-300">{postMintPaymentMessage}</p>{/if}
          {#if activeCollateral?.status === 'Active'}<a class="mt-3 inline-block text-emerald-400 underline" href="/?collateral={CANISTER_IDS.CKSOL_LEDGER}">Use ckSOL as collateral</a>{/if}
        </section>
      </div>
      <p class="mt-5 text-xs text-gray-400">Redeem SOL withdraws already minted ckSOL. It cannot cancel SOL still awaiting processing.</p>
      <details id="cksol-mint-receipt" bind:open={mintReceiptOpen} class="mt-6 border-t border-gray-700 pt-4 text-sm text-gray-400"><summary class="cursor-pointer">Receipt and processing details</summary>
        {#if cyclesBalance !== null}<p class="mt-3">Cycles ledger balance: {formatCycles(cyclesBalance)}.</p>{/if}
        {#if paymentQuote}<p class="mt-2">Processing budget: {formatCycles(paymentQuote.funding_cycles)}, including {formatCycles(paymentQuote.processing_fee)} processing fee. Approval cap: {formatCycles(paymentQuote.approval_amount)}. Approval transaction fee: {formatCycles(paymentQuote.ledger_fee)}. Approval expires after 10 minutes.</p>{/if}
        {#if resumeFunding}<button class="mt-3 rounded-lg border border-gray-600 px-3 py-2" disabled={busy || icpBusy || refreshing} onclick={refundPaidCycles}>Cancel mint and return funded cycles</button>{/if}
        {#if paymentIntent}<p class="mt-3">Paid payment sequence: {paymentIntent.sequence}; state: {paidPhase}.</p><p class="mt-2">{paymentMessage}</p>
          {#if paymentIntent.receipt?.funding_block?.[0] !== undefined}<p class="mt-2">Funding ledger block: {paymentIntent.receipt.funding_block[0].toString()}</p>{/if}
          {#if paymentIntent.receipt?.refund_block?.[0] !== undefined}<p class="mt-2">Refund ledger block: {paymentIntent.receipt.refund_block[0].toString()}</p>{/if}
        {/if}
        {#if mintedAmount !== null}<p class="mt-3">Minted: {formatSol(mintedAmount)} ckSOL. Ledger block: {mintBlock?.toString()}.</p>{/if}
        {#if depositId !== null}<p class="mt-3">Deposit ID: {depositId.toString()}</p><p class="mt-2">{depositStatus?.label ?? 'Checking minter status…'}</p>{#if depositStatus?.signature}<a class="mt-2 block text-emerald-400 underline" href="https://solscan.io/tx/{depositStatus.signature}" target="_blank" rel="noreferrer">View sweep on Solana</a>{/if}{/if}
        <button class="mt-4 rounded-lg border border-gray-600 px-3 py-2" disabled={busy || icpBusy || refreshing} onclick={checkAgain}>Check for updates</button>
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
  <div class="mt-6 flex flex-wrap items-center gap-4 text-sm">{#if selectedTab === 'redeem'}<button class="text-gray-300 underline" disabled={busy} onclick={refresh}>Refresh balance and minter terms</button>{/if}{#if selectedTab === 'redeem' && activeCollateral?.status === 'Active'}<a class="text-emerald-400 underline" href="/?collateral={CANISTER_IDS.CKSOL_LEDGER}">Borrow icUSD with ckSOL</a>{/if}<a class="text-gray-400 underline" href="https://github.com/dfinity/cksol" target="_blank" rel="noreferrer">Official ckSOL documentation</a></div>
</main>
