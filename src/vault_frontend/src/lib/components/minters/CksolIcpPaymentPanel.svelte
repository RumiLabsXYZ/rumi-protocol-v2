<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import type { Principal } from '@dfinity/principal';
	import { get } from 'svelte/store';
	import { auth } from '$lib/services/auth';
	import { beginCksolOisyAction } from '$lib/services/cksolOisySigner';
	import { cksolPaymentKey } from '$lib/services/cksolPayment';
	import { cksolIcpNeedsReconciliation } from '$lib/services/cksolPaymentJournal';
	import {
		quoteCksolIcpPayment,
		recoverCksolIcpPayment,
		submitCksolIcpPayment,
		continueCksolIcpApproval,
		resumeCksolIcpPayment,
		reconcileCksolIcpFunding,
		loadCksolIcpPayment,
		icpPaymentSettled,
		type CksolIcpQuote,
		type CksolIcpIntent
	} from '$lib/services/cksolIcpPayment';
	export let owner: Principal;
	export let canMint = false;
	export let blocked = false;
	export let onReady: (ready: boolean) => void = () => {};
	export let onReceipt: (receipt: any) => void = () => {};
	export let onBusy: (busy: boolean) => void = () => {};
	export let onActive: (active: boolean) => void = () => {};
	let quote: CksolIcpQuote | null = null,
		payment: CksolIcpIntent | null = null;
	let busy = false,
		loading = true,
		alive = true,
		error = '',
		unavailable = '',
		damaged = false;
	let candidateBlock = '', recoveryNotice = '';
	let timer: ReturnType<typeof setInterval>;
	const icp = (amount: bigint) =>
		`${amount / 100_000_000n}.${(amount % 100_000_000n).toString().padStart(8, '0')} ICP`;
	$: phase = payment?.receipt?.phase ?? payment?.phase;
	$: interrupted = cksolIcpNeedsReconciliation(payment?.receipt);
	$: pending = interrupted || !icpPaymentSettled(payment);
	$: onActive(pending || damaged);
	$: enough = !!quote && quote.balance >= quote.total;
	$: reserve =
		!!quote &&
		quote.terms.available &&
		BigInt(quote.terms.reserve_available_cycles) >= BigInt(quote.terms.attached_cycles);
	$: onReady(!blocked && !damaged && !pending && enough && reserve && !unavailable);
	$: checkingRefund = ['RefundReady', 'RefundPending', 'RefundUnknown'].includes(phase ?? '');
	$: fundingHeld = ['FundingPending', 'FundingUnknown'].includes(phase ?? '') && !payment?.receipt?.funding_block?.length;
	$: validCandidate = /^(0|[1-9][0-9]{0,19})$/.test(candidateBlock) && BigInt(candidateBlock) <= (1n << 64n) - 1n;
	$: actionable = phase === 'Approved' || phase === 'Funded' || checkingRefund;
	$: label =
		phase === 'Approved'
			? 'Continue approved mint'
			: phase === 'Funded'
				? 'Continue funded mint'
				: phase === 'RefundReady'
						? interrupted
							? 'Return processing ICP'
							: 'Return unused ICP'
					: checkingRefund
						? 'Check ICP refund'
							: interrupted
								? 'New mint held for review'
								: 'Mint with ICP';
	$: enabled =
		!busy &&
		!loading &&
		!damaged &&
		(checkingRefund || (!blocked && (actionable || (!pending && enough && reserve && canMint))));
	$: message =
		phase === 'Complete'
			? interrupted
				? 'ICP payment settled. SOL outcome remains unknown; the minting hold stays in place.'
				: 'Processing payment settled. Mint progress is shown separately below.'
			: phase === 'FundingRejected'
				? 'No ICP deposit was collected. Review the cost before trying again.'
				: phase === 'Approved'
					? 'Your ICP allowance is approved. Continue without another approval.'
					: phase === 'Funded'
						? 'Your ICP deposit is recorded. Continue this request without paying again.'
						: phase === 'RefundReady'
							? 'Your unused ICP is ready to return. No new processing deposit is needed.'
							: phase === 'RefundPending' || phase === 'RefundUnknown'
								? 'Your ICP refund is awaiting confirmation. Its receipt is preserved; do not pay again.'
								: pending
									? 'Checking the existing payment. Do not approve another deposit while its result is uncertain.'
									: '';
	async function refresh() {
		if (!alive || busy) return;
		try {
			if (!navigator.locks)
				throw new Error(
					'This browser cannot coordinate payment receipts safely. Use a browser with Web Locks support.'
				);
			const recovered = await navigator.locks.request(
				cksolPaymentKey(owner.toText()),
				{ ifAvailable: true },
				async (lock) =>
					lock
						? recoverCksolIcpPayment(owner, localStorage)
						: loadCksolIcpPayment(localStorage, owner.toText())
			);
			if (!alive) return;
			payment = recovered;
			if (!candidateBlock && payment?.fundingCandidateBlock) candidateBlock = payment.fundingCandidateBlock;
			if (payment?.receipt) onReceipt(payment.receipt);
			const q = await quoteCksolIcpPayment(owner, get(auth).walletType ?? 'unknown');
			if (alive) {
				quote = q;
				unavailable = '';
			}
		} catch (e) {
			if (alive) {
				unavailable = String(e);
				quote = null;
				try {
					payment = loadCksolIcpPayment(localStorage, owner.toText());
				} catch {
					damaged = true;
					error =
						'The saved ICP payment needs support. Its record is preserved; do not start another payment.';
				}
			}
		} finally {
			if (alive) loading = false;
		}
	}
	async function mint() {
		if (!enabled) return;
		busy = true;
		onBusy(true);
		error = '';
		let action: ReturnType<typeof beginCksolOisyAction> | null = null;
		try {
			// Open the wallet synchronously in the click before any lock/network await.
			if (get(auth).walletType === 'oisy') {
				action = beginCksolOisyAction(owner);
				await action.ready;
			}
			if (!alive) return;
			await navigator.locks.request(
				cksolPaymentKey(owner.toText()),
				{ ifAvailable: true },
				async (lock) => {
					if (!lock) throw new Error('A ckSOL payment is active in another tab.');
					if (!alive) return;
					const result =
						phase === 'Approved' && payment
							? await continueCksolIcpApproval(payment, owner, localStorage, () => alive)
							: actionable && payment
								? await resumeCksolIcpPayment(payment, owner, localStorage, () => alive)
								: quote
									? await submitCksolIcpPayment(quote, owner, localStorage, () => alive)
									: null;
					if (alive && result) {
						payment = result;
						if (result.message) error = result.message;
						if (result.receipt) onReceipt(result.receipt);
					}
				}
			);
		} catch (e) {
			if (alive) error = String(e);
		} finally {
			await action?.close().catch(() => {});
			if (alive) {
				busy = false;
				onBusy(false);
				void refresh();
			}
		}
	}
	async function verifyFunding() {
		if (busy || loading || damaged || !fundingHeld || !validCandidate || !payment) return;
		const held = payment, block = candidateBlock;
		busy = true; onBusy(true); error = ''; recoveryNotice = '';
		let action: ReturnType<typeof beginCksolOisyAction> | null = null;
		try {
			if (get(auth).walletType === 'oisy') { action = beginCksolOisyAction(owner); await action.ready; }
			if (!alive) return;
			if (!navigator.locks) throw new Error('Web Locks support is required to verify this payment safely.');
			await navigator.locks.request(cksolPaymentKey(owner.toText()), { ifAvailable: true }, async lock => {
				if (!lock) throw new Error('A ckSOL payment is active in another tab.');
				if (!alive) return;
				const result = await reconcileCksolIcpFunding(held, block, owner, localStorage, () => alive);
				if (alive) {
					payment = result;
					if (result.receipt) onReceipt(result.receipt);
					recoveryNotice = 'Collection verified. Select Return unused ICP to request the refund. No new ICP payment or mint request was submitted.';
				}
			});
		} catch (e) { if (alive) error = String(e); }
		finally {
			await action?.close().catch(() => {});
			if (alive) { busy = false; onBusy(false); void refresh(); }
		}
	}
	onMount(() => {
		void refresh();
		timer = setInterval(() => {
			void refresh();
		}, 15_000);
	});
	onDestroy(() => {
		alive = false;
		if (timer) clearInterval(timer);
		onReady(false);
		onBusy(false);
	});
</script>

<div
	class="mt-5 space-y-3 rounded-xl border border-emerald-900 bg-emerald-950/20 p-4"
	aria-label="ICP processing payment"
>
		<h3 class="font-semibold">{interrupted ? 'ICP processing recovery' : 'Pay processing costs with ICP'}</h3>
	{#if interrupted}
		<div role="status" class="space-y-2 text-sm text-amber-300">
			<h4 class="font-semibold">Processing interrupted; SOL outcome unknown</h4>
				<p>
					The mint request may have reached the official minter. An ICP refund does not return SOL
					or confirm whether ckSOL was minted.
				</p>
				<p>
					No minter, service or operating charge is retained for this interrupted attempt. ICP ledger
					fees still apply.
				</p>
				<p>
					New minting with this account remains blocked, including after this ICP refund. Do not send
					more SOL. Keep payment #{payment?.sequence} and contact support to reconcile the original deposit.
				</p>
		</div>
	{:else}
			<p class="text-sm text-gray-300">
				Rumi supplies the cycles needed to process your SOL. You approve a maximum ICP deposit, and
				unused ICP is returned automatically when the processing charge is known.
			</p>
			<p class="text-sm text-gray-300">
				<strong>Service fee: 5% of the minter’s net cycles charge.</strong> The operating fee and ICP ledger
				fees are additional. A failed processing attempt can still incur these costs.
			</p>
	{/if}
	{#if quote && !interrupted}
		<dl class="grid grid-cols-1 gap-2 text-sm sm:grid-cols-2">
			<div>
				<dt class="text-gray-400">Maximum ICP deposit</dt>
				<dd class="font-semibold">{icp(quote.deposit)}</dd>
			</div>
			<div>
				<dt class="text-gray-400">Maximum wallet debit, including approval and collection fees</dt>
				<dd>{icp(quote.total)}</dd>
			</div>
			<div>
				<dt class="text-gray-400">Connected ICP balance</dt>
				<dd>{icp(quote.balance)}</dd>
			</div>
			<div>
				<dt class="text-gray-400">Quoted conversion rate</dt>
				<dd>{Number(quote.rate) / 10_000} XDR per ICP</dd>
			</div>
		</dl>
		<p class="text-xs text-gray-400">
			The rate and maximum are fixed when this attempt is accepted. Rumi keeps only the measured
			processing cost, a 5% service fee, and the disclosed operating and ledger fees. Unused funds
			return as ICP.
		</p>
		<details class="text-xs text-gray-400">
			<summary class="cursor-pointer">Fee breakdown</summary>
			<p class="mt-2">
				Operating fee: 0.001T cycles, converted at the quoted rate. Approval fee: {icp(quote.fee)}.
				Collection fee: {icp(quote.fee)}. Refund transaction fee: {icp(quote.fee)}, deducted from
				returned ICP. The deposit includes a refund-fee reserve.
			</p>
		</details>
		{#if !reserve}<p role="status" class="text-sm text-amber-300">
				ICP-paid processing is temporarily unavailable while Rumi’s processing reserve is
				replenished. Do not send more SOL. Existing deposits remain linked to this account.
			</p>
		{:else if !enough && !pending}<p role="status" class="text-sm text-amber-300">
				Add at least {icp(quote.total - quote.balance)} to this connected ICP account to cover processing.
				You do not need to buy cycles. If you already sent SOL, finish that deposit here; do not send
				more SOL.
			</p>
		{:else if !canMint && !pending}<p class="text-sm text-gray-300">
				Your ICP balance covers the maximum. Send SOL using the address below, then select Mint with
				ICP.
			</p>{/if}
	{:else if loading && !interrupted}<p class="text-sm text-gray-400">Checking ICP balance and processing costs…</p>
	{:else if !interrupted}<p role="status" class="text-sm text-amber-300">
			ICP-paid processing is unavailable. Do not send SOL for a new mint. {unavailable}
		</p>{/if}
	{#if message}<p role="status" class="text-sm text-gray-300">{message}</p>{/if}
		{#if recoveryNotice}<p role="status" class="text-sm text-emerald-300">{recoveryNotice}</p>{/if}
		{#if fundingHeld}
			<details class="space-y-3 rounded-lg border border-amber-800 p-3 text-sm text-gray-300">
				<summary class="cursor-pointer">Recover an ICP collection with its ledger block</summary>
				<p class="mt-3">If your wallet shows an ICP collection for this payment, enter its ICP ledger block number, not the approval block. Rumi verifies it against this saved payment.</p>
				<p>This check cannot collect ICP or submit a mint. A matching block makes the original ICP deposit available to return, less the refund transaction fee. You request the refund separately.</p>
				<p>Only blocks still served directly by the ICP ledger can be checked here. Archived or nonmatching blocks leave this payment held. Keep the receipt and contact support if verification cannot complete.</p>
				<label class="block" for="cksol-icp-collection-block">ICP collection block number</label>
				<input id="cksol-icp-collection-block" aria-label="ICP collection block number" class="w-full rounded-lg border border-gray-600 bg-gray-900 p-2" type="text" inputmode="numeric" maxlength="20" bind:value={candidateBlock} disabled={busy} />
				<button class="rounded-lg border border-emerald-600 px-3 py-2 disabled:opacity-40" disabled={busy || loading || damaged || !validCandidate} onclick={verifyFunding}>Verify collection block</button>
			</details>
		{/if}
	{#if payment?.receipt?.phase === 'Complete' && payment.receipt.refund_block?.[0] !== undefined}
		<p class="text-sm text-emerald-300">
			ICP refund confirmed. Processing charge: {icp(BigInt(payment.receipt.charged_e8s))}. Returned: {icp(
				BigInt(payment.receipt.deposit_e8s) -
					BigInt(payment.receipt.charged_e8s) -
					BigInt(payment.receipt.actual_refund_fee_e8s ?? 0)
			)}.
		</p>
	{/if}
	{#if error}<p role="alert" class="break-words text-sm text-red-300">{error}</p>{/if}
	<button
		class="w-full rounded-lg bg-emerald-600 px-5 py-3 font-semibold disabled:opacity-40"
		disabled={!enabled}
		onclick={mint}>{busy ? 'Checking payment…' : label}</button
	>
	<p class="text-xs text-gray-400">
			{#if fundingHeld}
				The collection block check only verifies an existing debit. It does not approve an allowance,
				collect ICP or submit a mint.
			{:else if checkingRefund}
				This action requests only the existing ICP refund. No new ICP payment or mint request is submitted.
			{:else if interrupted}
				This receipt records the ICP payment only. The original SOL deposit still needs reconciliation.
			{:else}
				Wallet confirmations approve the ICP allowance and authorize this mint. Payment settlement does
				not mean ckSOL has finished minting.
			{/if}
	</p>
	{#if payment}<details class="text-xs text-gray-400">
			<summary class="cursor-pointer">ICP payment receipt</summary>
			<p class="mt-2">Payment #{payment.sequence}; {phase}.</p>
			{#if payment.receipt?.funding_block?.[0] !== undefined}<p>
					Collection block: {payment.receipt.funding_block[0].toString()}.
				</p>{/if}{#if payment.receipt?.refund_block?.[0] !== undefined}<p>
					Refund block: {payment.receipt.refund_block[0].toString()}.
				</p>{/if}{#if payment.receipt?.message?.[0]}<p>{payment.receipt.message[0]}</p>{/if}
		</details>{/if}
</div>
