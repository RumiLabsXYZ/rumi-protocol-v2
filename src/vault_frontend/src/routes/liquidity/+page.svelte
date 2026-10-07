<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { walletStore as wallet } from '$lib/stores/wallet';
  import { protocolService } from '$lib/services/protocol';
  import { formatNumber } from '$lib/utils/format';
  import ProtocolStats from '$lib/components/dashboard/ProtocolStats.svelte';
  import { CONFIG } from '$lib/config';
  import { captureActionBoundContext, assertActionBoundContextCurrent } from '$lib/services/protocol/walletOperations';
import { formatLiquidityAmountRaw, liquidityV2ActionLockName, liquidityV2ClaimIdentityMatches, liquidityV2IntentKey, parseLiquidityAmountRaw, parseLiquidityV2Intent, type LiquidityV2Intent, type LiquidityV2Operation } from '$lib/utils/liquidityV2Intent';
  import type { LiquidityV2StatusView } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
  import type { BoundLiquidityV2Result } from '$lib/services/protocol/apiClient';

  // Component state
  let isConnected = false;
  let liquidityStatus = {
    liquidityProvided: 0,
    totalLiquidityProvided: 0,
    liquidityPoolShare: 0,
    availableLiquidityReward: 0,
    totalAvailableReturns: 0
  };
  let isLoading = true;
  let actionInProgress = false;
  let errorMessage = '';
  let successMessage = '';

  // Form values
  let provideAmount = '';
  let withdrawAmount = '';
  let liquidityRecoveryNotice = '';
  let candidateBlockIndex = '';
  let showCandidateRecovery = false;
  let providedRaw = 0n;
  let availableRewardRaw = 0n;
  let lastRestoredOwner = '';
  let componentActive = true;

  // Store the current wallet state
  let currentWalletState: any;

  // Subscribe to wallet state
  wallet.subscribe(state => {
    isConnected = state.isConnected;
    currentWalletState = state;
    const owner = state.isConnected ? state.principal?.toText() ?? '' : '';
    if (owner !== lastRestoredOwner) {
      lastRestoredOwner = owner;
      provideAmount = '';
      withdrawAmount = '';
      providedRaw = 0n;
      availableRewardRaw = 0n;
      liquidityRecoveryNotice = '';
      showCandidateRecovery = false;
      if (owner) restoreLiquidityIntent(owner);
    }
  });
  onDestroy(() => { componentActive = false; });

  const liquidityNetworkKey = () => `${CONFIG.host}|${CONFIG.currentCanisterId}`;

  function restoreLiquidityIntent(owner: string) {
    if (typeof localStorage === 'undefined') return;
    const key = liquidityV2IntentKey(owner, liquidityNetworkKey());
    const raw = localStorage.getItem(key);
    if (!raw) return;
    const intent = parseLiquidityV2Intent(raw);
    if (!intent || intent.owner !== owner || intent.network !== liquidityNetworkKey()) {
      liquidityRecoveryNotice = 'A saved liquidity request could not be validated. Do not submit another liquidity action; reconcile the owner journal first.';
      return;
    }
    if (intent.operation === 'Provide') provideAmount = formatLiquidityAmountRaw(BigInt(intent.amountRaw));
    if (intent.operation === 'Withdraw') withdrawAmount = formatLiquidityAmountRaw(BigInt(intent.amountRaw));
    showCandidateRecovery = intent.backendDispatchAttempted;
    liquidityRecoveryNotice = !intent.ledgerPrincipal
      ? `Saved request #${intent.requestId} predates ledger pinning. Its exact owner journal must identify the original ledger before it can resume.`
      : `Saved ${intent.operation} request #${intent.requestId} for ${formatLiquidityAmountRaw(BigInt(intent.amountRaw))} ${intent.operation === 'ClaimReturns' ? 'ICP' : 'icUSD'}. Reconcile or replay only those exact arguments.`;
  }

  function captureLiquidityContext() {
    const base = captureActionBoundContext();
    const network = liquidityNetworkKey();
    const assertCurrent = () => base.assertCurrent() && componentActive &&
      CONFIG.host === network.split('|')[0] && CONFIG.currentCanisterId === network.split('|')[1];
    return { ...base, assertCurrent };
  }

  // Fetch protocol data and user's liquidity status
  async function fetchData() {
    isLoading = true;
    errorMessage = '';
    successMessage = '';
    
    try {
      // Get user's liquidity status if connected
      if (isConnected && currentWalletState?.principal) {
        try {
          const userLiquidityStatus = await protocolService.getLiquidityStatus(currentWalletState.principal);
          providedRaw = BigInt(userLiquidityStatus.liquidity_provided || 0);
          availableRewardRaw = BigInt(userLiquidityStatus.available_liquidity_reward || 0);
          
          // Convert to numbers safely, defaulting to 0 for invalid values
          const liquidityProvided = Number(userLiquidityStatus.liquidity_provided || 0) / 100_000_000;
          const totalLiquidityProvided = Number(userLiquidityStatus.total_liquidity_provided || 0) / 100_000_000;
          
          // Calculate pool share safely - avoid division by zero or infinity
          let liquidityPoolShare = 0;
          if (totalLiquidityProvided > 0 && isFinite(userLiquidityStatus.liquidity_pool_share)) {
            liquidityPoolShare = userLiquidityStatus.liquidity_pool_share * 100; // Convert to percentage
          } else if (liquidityProvided > 0 && totalLiquidityProvided > 0) {
            // Calculate manually if the backend ratio is invalid
            liquidityPoolShare = (liquidityProvided / totalLiquidityProvided) * 100;
          }
          
          // Ensure we have valid numbers for rewards
          const availableLiquidityReward = Number(userLiquidityStatus.available_liquidity_reward || 0) / 100_000_000;
          const totalAvailableReturns = Number(userLiquidityStatus.total_available_returns || 0) / 100_000_000;
          
          liquidityStatus = {
            liquidityProvided,
            totalLiquidityProvided,
            liquidityPoolShare,
            availableLiquidityReward,
            totalAvailableReturns
          };
        } catch (statusError) {
          console.error('Error fetching liquidity status:', statusError);
          // Keep the default zero values in liquidityStatus
        }
      }
    } catch (error) {
      console.error('Error fetching data:', error);
      errorMessage = 'Failed to load liquidity data';
    } finally {
      isLoading = false;
    }
  }
  
  function operationFromStatus(status: LiquidityV2StatusView): LiquidityV2Operation {
    if ('Provide' in status.kind) return 'Provide';
    if ('Withdraw' in status.kind) return 'Withdraw';
    return 'ClaimReturns';
  }

  function expectedLiquidityLedger(operation: LiquidityV2Operation): string {
    return operation === 'ClaimReturns' ? CONFIG.currentIcpLedgerId : CONFIG.currentIcusdLedgerId;
  }

  async function handleLiquidityAction(operation: LiquidityV2Operation) {
    if (!isConnected || !currentWalletState?.principal) return;
    if (!navigator.locks?.request) {
      errorMessage = 'This browser cannot coordinate liquidity requests safely. No liquidity action was submitted.';
      return;
    }
    const initialRaw = operation === 'Provide' ? parseLiquidityAmountRaw(provideAmount)
      : operation === 'Withdraw' ? parseLiquidityAmountRaw(withdrawAmount) : availableRewardRaw;
    if (operation !== 'ClaimReturns' && initialRaw === null) {
      errorMessage = 'Enter an exact amount with up to 8 decimal places.';
      return;
    }
    if (operation === 'Withdraw' && initialRaw !== null && initialRaw > providedRaw) {
      errorMessage = 'Withdrawal amount exceeds the current icUSD liquidity position.';
      return;
    }

    actionInProgress = true;
    errorMessage = '';
    successMessage = '';
    const network = liquidityNetworkKey();
    const owner = currentWalletState.principal.toText();
    const key = liquidityV2IntentKey(owner, network);
    let activeIntent: LiquidityV2Intent | null = null;
    let result: BoundLiquidityV2Result | null = null;
    try {
      await navigator.locks.request(liquidityV2ActionLockName(owner, network), async () => {
        const ctx = captureLiquidityContext();
        assertActionBoundContextCurrent(ctx);
        const state = await protocolService.getLiquidityV2RequestStateBound(ctx);
        assertActionBoundContextCurrent(ctx);
        const existingRaw = localStorage.getItem(key);
        activeIntent = parseLiquidityV2Intent(existingRaw);
        if (existingRaw && (!activeIntent || activeIntent.owner !== owner || activeIntent.network !== network)) {
          throw new Error('Saved liquidity intent is unreadable or belongs to another identity. Reconcile the exact owner journal before proceeding.');
        }

        const active = state.active_request[0] ?? null;
        const latest = state.latest_result[0] ?? null;
        for (const row of [active, latest]) {
          if (!row) continue;
          if (row.owner.toText() !== owner) {
            throw new Error('The liquidity journal returned an unexpected owner. No approval or action was submitted.');
          }
        }
        if (activeIntent && !activeIntent.ledgerPrincipal) {
          const matchingOldRow = [active, latest].find((row) => row && row.request_id === BigInt(activeIntent!.requestId) &&
            row.owner.toText() === owner && activeIntent!.operation in row.kind &&
            (activeIntent!.operation === 'ClaimReturns' || row.amount_raw === BigInt(activeIntent!.amountRaw)));
          if (matchingOldRow) {
            activeIntent.ledgerPrincipal = matchingOldRow.ledger.toText();
            localStorage.setItem(key, JSON.stringify(activeIntent));
          } else if (!activeIntent.approvalAttempted && !activeIntent.backendDispatchAttempted) {
            activeIntent.ledgerPrincipal = expectedLiquidityLedger(activeIntent.operation);
            localStorage.setItem(key, JSON.stringify(activeIntent));
          } else {
            throw new Error('The saved request predates ledger pinning and may already have used a previous ledger. Its ledger cannot be inferred safely; reconcile it before proceeding.');
          }
        }
        if (active) {
          const rowOperation = operationFromStatus(active);
          const recovered: LiquidityV2Intent = {
            version: 1, owner, network, requestId: active.request_id.toString(), operation: rowOperation,
            ledgerPrincipal: active.ledger.toText(),
            amountRaw: active.amount_raw.toString(), approvalAttempted: true, backendDispatchAttempted: true,
          };
          if (!activeIntent) {
            activeIntent = recovered;
            localStorage.setItem(key, JSON.stringify(recovered));
            showCandidateRecovery = true;
            if (rowOperation === 'Provide') provideAmount = formatLiquidityAmountRaw(active.amount_raw);
            if (rowOperation === 'Withdraw') withdrawAmount = formatLiquidityAmountRaw(active.amount_raw);
            liquidityRecoveryNotice = `Recovered ${rowOperation} request #${recovered.requestId} for ${formatLiquidityAmountRaw(active.amount_raw)} ${rowOperation === 'ClaimReturns' ? 'ICP' : 'icUSD'}. It will replay only with the original tuple.`;
          }
          if (rowOperation !== operation) throw new Error(`A ${rowOperation} request #${active.request_id} is unresolved. Resume its exact action first.`);
          if (activeIntent.requestId !== active.request_id.toString() || activeIntent.operation !== rowOperation || activeIntent.amountRaw !== active.amount_raw.toString())
            throw new Error('Saved liquidity intent does not match the active backend request. No approval or action was submitted.');
          if (operation !== 'ClaimReturns' && initialRaw !== active.amount_raw) {
            throw new Error(`Recovered request #${active.request_id}; the exact amount ${formatLiquidityAmountRaw(active.amount_raw)} was restored. Click again to resume it.`);
          }
        } else if (activeIntent) {
          if (activeIntent.operation !== operation) throw new Error(`Request #${activeIntent.requestId} is saved for ${activeIntent.operation}. Resume its exact operation before starting another.`);
          if (operation !== 'ClaimReturns' && initialRaw?.toString() !== activeIntent.amountRaw) {
            if (operation === 'Provide') provideAmount = formatLiquidityAmountRaw(BigInt(activeIntent.amountRaw));
            if (operation === 'Withdraw') withdrawAmount = formatLiquidityAmountRaw(BigInt(activeIntent.amountRaw));
            throw new Error(`Request #${activeIntent.requestId} is bound to ${formatLiquidityAmountRaw(BigInt(activeIntent.amountRaw))}. The exact amount was restored; click again to resume it.`);
          }
        } else {
          if (operation === 'ClaimReturns' && availableRewardRaw <= 0n) throw new Error('There are no ICP liquidity returns available to claim.');
          if (state.next_request_id <= 0n) throw new Error('Backend liquidity request sequence is exhausted.');
          const amountRaw = initialRaw as bigint;
          activeIntent = {
            version: 1, owner, network, requestId: state.next_request_id.toString(), operation,
            ledgerPrincipal: expectedLiquidityLedger(operation),
            amountRaw: amountRaw.toString(), approvalAttempted: false, backendDispatchAttempted: false,
          };
          localStorage.setItem(key, JSON.stringify(activeIntent));
        }

        const persist = () => localStorage.setItem(key, JSON.stringify(activeIntent));
        result = await protocolService.liquidityV2Bound(ctx, activeIntent!,
          () => { assertActionBoundContextCurrent(ctx); activeIntent!.approvalAttempted = true; persist(); },
          () => typeof window !== 'undefined' && window.confirm('A previous icUSD approval may have succeeded, but the allowance is still below this exact provide request plus ledger fee. Retrying may charge another fee. Keep the same request ID?'),
          () => { assertActionBoundContextCurrent(ctx); activeIntent!.backendDispatchAttempted = true; persist(); });
        assertActionBoundContextCurrent(ctx);
      });

      const resolvedResult = result as BoundLiquidityV2Result | null;
      const resolvedIntent = activeIntent as LiquidityV2Intent | null;
      if (!resolvedResult || !resolvedIntent) return;
      if (resolvedResult.status && resolvedIntent.operation === 'ClaimReturns' &&
          (resolvedResult.amountAdopted || resolvedResult.status.amount_raw.toString() === resolvedIntent.amountRaw)) {
        resolvedIntent.amountRaw = resolvedResult.status.amount_raw.toString();
        localStorage.setItem(key, JSON.stringify(resolvedIntent));
      }
      if (resolvedResult.kind === 'dispatched_ok' && resolvedResult.status) {
        localStorage.removeItem(key);
        liquidityRecoveryNotice = '';
        if (operation === 'Provide') provideAmount = '';
        if (operation === 'Withdraw') withdrawAmount = '';
        await fetchData();
        successMessage = operation === 'ClaimReturns'
          ? `Successfully claimed ${formatLiquidityAmountRaw(BigInt(resolvedIntent.amountRaw))} ICP returns.`
          : operation === 'Provide'
            ? `Successfully provided ${formatLiquidityAmountRaw(BigInt(resolvedIntent.amountRaw))} icUSD to the liquidity pool.`
            : `Successfully withdrew ${formatLiquidityAmountRaw(BigInt(resolvedIntent.amountRaw))} icUSD from the liquidity pool.`;
        return;
      }
      if (resolvedResult.kind === 'dispatched_err') {
        localStorage.removeItem(key);
        liquidityRecoveryNotice = '';
        errorMessage = resolvedResult.errorMessage ?? 'The liquidity request was rejected with no effect.';
        return;
      }
      if (resolvedResult.kind === 'predispatch_aborted' && !resolvedResult.approvalMayHaveMutated && !resolvedIntent.backendDispatchAttempted) {
        localStorage.removeItem(key);
        errorMessage = resolvedResult.errorMessage ?? 'Liquidity request was not submitted.';
        return;
      }
      liquidityRecoveryNotice = `Liquidity request #${resolvedIntent.requestId} is pending or held. Keep the exact amount and operation; do not start another liquidity action.`;
      showCandidateRecovery = resolvedIntent.backendDispatchAttempted;
      errorMessage = resolvedResult.errorMessage ?? liquidityRecoveryNotice;
    } catch (error) {
      errorMessage = error instanceof Error ? error.message : 'Liquidity request outcome is unresolved.';
      const failedIntent = activeIntent as LiquidityV2Intent | null;
      if (failedIntent) {
        liquidityRecoveryNotice = `Liquidity request #${failedIntent.requestId} remains saved for exact reconciliation.`;
        showCandidateRecovery = failedIntent.backendDispatchAttempted;
      }
    } finally {
      actionInProgress = false;
    }
  }

  async function handleProvideLiquidity() { await handleLiquidityAction('Provide'); }
  async function handleWithdrawLiquidity() { await handleLiquidityAction('Withdraw'); }
  async function handleClaimRewards() { await handleLiquidityAction('ClaimReturns'); }

  async function handleLiquidityCandidateRecovery() {
    if (!isConnected || !currentWalletState?.principal || !/^\d+$/.test(candidateBlockIndex.trim())) {
      errorMessage = 'Enter a known ledger block index for the saved liquidity request.';
      return;
    }
    const owner = currentWalletState.principal.toText();
    const network = liquidityNetworkKey();
    if (!navigator.locks?.request) {
      errorMessage = 'This browser cannot coordinate liquidity recovery safely.';
      return;
    }
    const key = liquidityV2IntentKey(owner, network);
    const intent = parseLiquidityV2Intent(localStorage.getItem(key));
    if (!intent || intent.owner !== owner || intent.network !== network || !intent.backendDispatchAttempted) {
      errorMessage = 'No matching dispatched liquidity intent is available for candidate verification.';
      return;
    }
    actionInProgress = true;
    errorMessage = '';
    try {
      const result = await navigator.locks.request(liquidityV2ActionLockName(owner, network), async () => {
        const ctx = captureLiquidityContext();
        const attached = await protocolService.attachLiquidityV2CandidateBound(ctx, intent, BigInt(candidateBlockIndex.trim()));
        assertActionBoundContextCurrent(ctx);
        return attached;
      });
      if (result.kind === 'dispatched_ok' && result.status?.result_block_index[0] !== undefined) {
        localStorage.removeItem(key);
        liquidityRecoveryNotice = '';
        showCandidateRecovery = false;
        candidateBlockIndex = '';
        await fetchData();
        successMessage = `Liquidity request #${intent.requestId} was verified with receipt block ${result.status.result_block_index[0]}.`;
      } else {
        liquidityRecoveryNotice = `Liquidity request #${intent.requestId} remains held. The supplied block did not verify the exact transfer.`;
        errorMessage = result.errorMessage ?? liquidityRecoveryNotice;
      }
    } catch (error) {
      errorMessage = error instanceof Error ? error.message : 'Candidate verification is unresolved.';
      liquidityRecoveryNotice = `Liquidity request #${intent.requestId} remains saved for exact reconciliation.`;
    } finally {
      actionInProgress = false;
    }
  }

  onMount(() => {
    fetchData();
  });
</script>

<svelte:head>
  <title>Liquidity | Rumi Protocol</title>
</svelte:head>

<div class="container mx-auto px-4 max-w-6xl">
  <section class="mb-12">
    <div class="text-center mb-10">
      <h1 class="text-4xl font-bold mb-4 bg-clip-text text-transparent bg-gradient-to-r from-pink-400 to-purple-600">
        Liquidity Pool
      </h1>
      <p class="text-xl text-gray-300 max-w-2xl mx-auto">
        Provide liquidity to the Rumi Protocol and earn rewards
      </p>
    </div>
    
    <ProtocolStats />
  </section>

  {#if isConnected}
    <!-- User Liquidity Status -->
    <section class="mb-12">
      <div class="glass-card max-w-4xl mx-auto mb-8">
        <h2 class="text-2xl font-semibold mb-6">Your Liquidity Position</h2>
        
        {#if isLoading}
          <div class="flex justify-center py-12">
            <div class="w-8 h-8 border-4 border-t-transparent border-purple-500 rounded-full animate-spin"></div>
          </div>
        {:else}
          <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
            <div class="p-4 bg-gray-800/60 rounded-lg">
              <div class="text-sm text-gray-400 mb-1">Your Provided Liquidity</div>
              <div class="text-2xl font-bold">{formatNumber(liquidityStatus.liquidityProvided)} icUSD</div>
              <div class="text-sm text-gray-400">≈ ${formatNumber(liquidityStatus.liquidityProvided)}</div>
            </div>
            
            <div class="p-4 bg-gray-800/60 rounded-lg">
              <div class="text-sm text-gray-400 mb-1">Pool Share</div>
              <div class="text-2xl font-bold">{formatNumber(liquidityStatus.liquidityPoolShare)}%</div>
              <div class="text-sm text-gray-400">of total {formatNumber(liquidityStatus.totalLiquidityProvided)} icUSD</div>
            </div>
          </div>
          
          <div class="mt-6 p-4 bg-gray-800/60 rounded-lg">
            <div class="flex justify-between items-center mb-2">
              <div class="text-lg font-semibold">Available Rewards</div>
              <button 
                class="px-4 py-1 bg-green-700 hover:bg-green-600 disabled:opacity-50 rounded-lg text-white text-sm"
                disabled={actionInProgress || availableRewardRaw <= 0n}
                on:click={handleClaimRewards}
              >
                {actionInProgress ? 'Processing...' : 'Claim Rewards'}
              </button>
            </div>
            <div class="text-xl font-bold">{formatNumber(liquidityStatus.availableLiquidityReward)} ICP</div>
            <div class="text-sm text-gray-400">System-wide rewards available: {formatNumber(liquidityStatus.totalAvailableReturns)} ICP</div>
          </div>
        {/if}
      </div>
    </section>
    
    <!-- Provide and Withdraw Liquidity -->
    <section class="mb-16 grid grid-cols-1 md:grid-cols-2 gap-8">
      <!-- Provide Liquidity -->
      <div class="glass-card">
        <h3 class="text-xl font-semibold mb-4">Provide Liquidity</h3>
        
        <div class="space-y-4">
          <div>
            <label for="provide-amount" class="block text-sm font-medium text-gray-300 mb-1">
              icUSD Amount
            </label>
            <input
              id="provide-amount"
              type="text"
              inputmode="decimal"
              bind:value={provideAmount}
              class="w-full bg-gray-800/50 border border-gray-700 rounded-lg px-4 py-3 text-white"
              placeholder="0.00"
              disabled={actionInProgress}
            />
          </div>
          
          <button
            class="w-full py-3 px-6 bg-gradient-to-r from-blue-600 to-purple-600 hover:from-blue-700 hover:to-purple-700 rounded-lg text-white font-medium transition-colors disabled:opacity-50"
            on:click={handleProvideLiquidity}
            disabled={actionInProgress || parseLiquidityAmountRaw(provideAmount) === null}
          >
            {actionInProgress ? 'Processing...' : 'Provide Liquidity'}
          </button>
        </div>
      </div>
      
      <!-- Withdraw Liquidity -->
      <div class="glass-card">
        <h3 class="text-xl font-semibold mb-4">Withdraw Liquidity</h3>
        
        <div class="space-y-4">
          <div>
            <label for="withdraw-amount" class="block text-sm font-medium text-gray-300 mb-1">
              icUSD Amount
            </label>
            <div class="relative">
              <input
                id="withdraw-amount"
                type="text"
                inputmode="decimal"
                bind:value={withdrawAmount}
                class="w-full bg-gray-800/50 border border-gray-700 rounded-lg px-4 py-3 text-white"
                placeholder="0.00"
                disabled={actionInProgress}
              />
              <div class="absolute inset-y-0 right-0 flex items-center pr-4 pointer-events-none">
                <span class="text-gray-400">icUSD</span>
              </div>
            </div>
            
            {#if isConnected && !isLoading && providedRaw > 0n}
              <div class="text-xs text-right mt-1">
                <button 
                  class="text-blue-400 hover:text-blue-300" 
                  on:click={() => withdrawAmount = formatLiquidityAmountRaw(providedRaw)}
                  disabled={actionInProgress}
                >
                  Max: {formatLiquidityAmountRaw(providedRaw)}
                </button>
              </div>
            {/if}
          </div>
          
          <button
            class="w-full py-3 px-6 bg-gradient-to-r from-purple-600 to-blue-600 hover:from-purple-700 hover:to-blue-700 rounded-lg text-white font-medium transition-colors disabled:opacity-50"
            on:click={handleWithdrawLiquidity}
            disabled={actionInProgress || parseLiquidityAmountRaw(withdrawAmount) === null || (parseLiquidityAmountRaw(withdrawAmount) ?? 0n) > providedRaw}
          >
            {actionInProgress ? 'Processing...' : 'Withdraw Liquidity'}
          </button>
        </div>
      </div>
    </section>
    
    {#if liquidityRecoveryNotice}
      <div class="p-3 bg-yellow-900/30 border border-yellow-700 rounded-lg text-yellow-100 text-sm mt-6">{liquidityRecoveryNotice}</div>
    {/if}

    {#if liquidityRecoveryNotice && showCandidateRecovery}
      <div class="mt-3 p-4 border border-yellow-700/70 rounded-lg text-sm text-yellow-100">
        <p>A known ledger block can be checked against this exact held request. This does not search ledger history. If the block index is unknown, keep the request held and locate it in the ledger history before retrying.</p>
        <div class="mt-3 flex gap-2">
          <input aria-label="Known ledger block index" type="text" inputmode="numeric" bind:value={candidateBlockIndex} placeholder="Known block index" class="min-w-0 flex-1 bg-gray-800/50 border border-gray-700 rounded-lg px-3 py-2 text-white" disabled={actionInProgress} />
          <button class="px-4 py-2 rounded-lg bg-yellow-700 hover:bg-yellow-600 disabled:opacity-50" on:click={handleLiquidityCandidateRecovery} disabled={actionInProgress || !/^\d+$/.test(candidateBlockIndex.trim())}>Verify block</button>
        </div>
      </div>
    {/if}

    {#if errorMessage}
      <div class="p-3 bg-red-900/30 border border-red-800 rounded-lg text-red-200 text-sm mt-6">
        {errorMessage}
      </div>
    {/if}
    
    {#if successMessage}
      <div class="p-3 bg-green-900/30 border border-green-800 rounded-lg text-green-200 text-sm mt-6">
        {successMessage}
      </div>
    {/if}
  {/if}
  
  <section class="mb-16">
    <div class="max-w-4xl mx-auto">
      <h2 class="text-2xl font-semibold mb-6 text-center">How The Liquidity Pool Works</h2>
      
      <div class="grid grid-cols-1 md:grid-cols-3 gap-6">
        <div class="glass-card h-full">
          <div class="text-pink-400 text-3xl font-bold mb-2">1</div>
          <h3 class="text-lg font-medium mb-2">Provide Liquidity</h3>
          <p class="text-gray-300">Deposit icUSD into the protocol's liquidity pool.</p>
        </div>
        
        <div class="glass-card h-full">
          <div class="text-pink-400 text-3xl font-bold mb-2">2</div>
          <h3 class="text-lg font-medium mb-2">Earn Returns</h3>
          <p class="text-gray-300">Earn ICP returns proportional to your icUSD contribution.</p>
        </div>
        
        <div class="glass-card h-full">
          <div class="text-pink-400 text-3xl font-bold mb-2">3</div>
          <h3 class="text-lg font-medium mb-2">Withdraw Anytime</h3>
          <p class="text-gray-300">Withdraw your liquidity and claim your earned rewards when you want.</p>
        </div>
      </div>
    </div>
  </section>
</div>

<style>
  .glass-card {
    @apply bg-gray-800/40 backdrop-blur-lg border border-gray-700/50 rounded-lg p-6;
  }

  input::-webkit-outer-spin-button,
  input::-webkit-inner-spin-button {
    -webkit-appearance: none;
    margin: 0;
  }

  input[type=number] {
    -moz-appearance: textfield;
  }
</style>
