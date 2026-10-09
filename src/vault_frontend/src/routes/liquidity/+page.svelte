<script lang="ts">
  import { onMount } from 'svelte';
  import { walletStore as wallet } from '$lib/stores/wallet';
  import { walletSessionGeneration } from '$lib/services/auth';
  import { protocolService } from '$lib/services/protocol';
  import { formatNumber } from '$lib/utils/format';
  import ProtocolStats from '$lib/components/dashboard/ProtocolStats.svelte';

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
  let withdrawAmount = '';
  let startNewWithdrawal = false;
  let liquidityProvidedRaw = 0n;
  let totalLiquidityProvidedRaw = 0n;

  function formatIcusdE8s(raw: bigint): string {
    const whole = raw / 100_000_000n;
    const fraction = (raw % 100_000_000n).toString().padStart(8, '0').replace(/0+$/, '');
    return fraction ? `${whole}.${fraction}` : whole.toString();
  }

  // Store the current wallet state
  let currentWalletState: any;
  let lastWalletOwner: string | null | undefined;
  let walletViewGeneration = 0;
  let sessionSettling = false;

  function clearOwnerView() {
    walletViewGeneration += 1;
    withdrawAmount = '';
    startNewWithdrawal = false;
    liquidityProvidedRaw = 0n;
    totalLiquidityProvidedRaw = 0n;
    liquidityStatus = {
      liquidityProvided: 0,
      totalLiquidityProvided: 0,
      liquidityPoolShare: 0,
      availableLiquidityReward: 0,
      totalAvailableReturns: 0
    };
    successMessage = '';
  }

  // Fetch protocol data and user's liquidity status
  async function fetchData() {
    const viewGeneration = walletViewGeneration;
    const owner = currentWalletState?.principal;
    isLoading = true;
    errorMessage = '';
    
    try {
      // Get user's liquidity status if connected
      if (isConnected && owner) {
        try {
          const userLiquidityStatus = await protocolService.getLiquidityStatus(owner);
          if (viewGeneration !== walletViewGeneration) return;
          liquidityProvidedRaw = BigInt(userLiquidityStatus.liquidity_provided || 0);
          totalLiquidityProvidedRaw = BigInt(userLiquidityStatus.total_liquidity_provided || 0);
          
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
          if (viewGeneration !== walletViewGeneration) return;
          console.error('Error fetching liquidity status:', statusError);
          // Keep the default zero values in liquidityStatus
        }
      }
    } catch (error) {
      if (viewGeneration !== walletViewGeneration) return;
      console.error('Error fetching data:', error);
      errorMessage = 'Failed to load liquidity data';
    } finally {
      if (viewGeneration === walletViewGeneration) isLoading = false;
    }
  }
  
  // Handle withdrawing liquidity
  async function handleWithdrawLiquidity() {
    if (!isConnected || sessionSettling || currentWalletState?.loading || !withdrawAmount) return;
    const actionViewGeneration = walletViewGeneration;
    
    actionInProgress = true;
    errorMessage = '';
    successMessage = '';
    
    try {
      // Call protocol service to withdraw liquidity
      const submittedAmount = withdrawAmount;
      const result = await protocolService.withdrawLiquidity(submittedAmount, startNewWithdrawal);
      if (actionViewGeneration !== walletViewGeneration) return;
      // A selected fresh intent is one-shot. Any retry must recover the
      // request that may already have reached the backend.
      startNewWithdrawal = false;
      
      if (result.success) {
        successMessage = `Withdrawal request confirmed: ${submittedAmount} icUSD minted to your wallet or confirmed from its earlier ledger receipt.`;
        withdrawAmount = '';
        startNewWithdrawal = false;
        // Refresh data
        await fetchData();
      } else {
        errorMessage = result.error || 'Failed to withdraw liquidity';
      }
    } catch (error) {
      if (actionViewGeneration !== walletViewGeneration) return;
      console.error('Error withdrawing liquidity:', error);
      errorMessage = error instanceof Error ? error.message : 'An unexpected error occurred';
    } finally {
      actionInProgress = false;
    }
  }
  
  onMount(() => {
    const unsubscribe = wallet.subscribe(state => {
      const nextOwner = state.isConnected ? state.principal?.toText() ?? null : null;
      const ownerChanged = nextOwner !== lastWalletOwner;
      isConnected = state.isConnected;
      currentWalletState = state;
      if (ownerChanged || (sessionSettling && !state.loading && !state.error)) {
        lastWalletOwner = nextOwner;
        sessionSettling = false;
        clearOwnerView();
        fetchData();
      }
    });
    let initialGeneration = true;
    const unsubscribeGeneration = walletSessionGeneration.subscribe(() => {
      if (initialGeneration) { initialGeneration = false; return; }
      // A transition begins before the old principal is cleared. Freeze the
      // action until a settled wallet state is published, even for same-owner
      // provider switches and failed disconnects.
      sessionSettling = true;
      clearOwnerView();
      isLoading = true;
    });
    return () => { unsubscribe(); unsubscribeGeneration(); };
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
        Withdraw existing icUSD from the legacy liquidity pool. New deposits are closed; historical ICP return claims are held pending safe payout reconciliation.
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
              <div class="text-sm text-gray-400 mb-1">Your icUSD in the Pool</div>
              <div class="text-2xl font-bold">{formatIcusdE8s(liquidityProvidedRaw)} icUSD</div>
            </div>
            
            <div class="p-4 bg-gray-800/60 rounded-lg">
              <div class="text-sm text-gray-400 mb-1">Pool Share</div>
              <div class="text-2xl font-bold">{formatNumber(liquidityStatus.liquidityPoolShare)}%</div>
              <div class="text-sm text-gray-400">of total {formatIcusdE8s(totalLiquidityProvidedRaw)} icUSD</div>
            </div>
          </div>
          
          <div class="mt-6 p-4 bg-gray-800/60 rounded-lg">
            <div class="text-lg font-semibold mb-2">Historical ICP Returns Held</div>
            <div class="text-xl font-bold">{formatNumber(liquidityStatus.availableLiquidityReward)} ICP</div>
            <div class="text-sm text-gray-400">Historical ICP returns available system-wide: {formatNumber(liquidityStatus.totalAvailableReturns)} ICP</div>
            <div class="text-sm text-amber-300 mt-2">Claims are held pending safe payout reconciliation.</div>
          </div>
        {/if}
      </div>
    </section>
    
    <!-- Exit-only legacy liquidity pool -->
    <section class="mb-16 grid grid-cols-1 md:grid-cols-2 gap-8">
      <!-- New deposits are closed until a durable ingress path exists. -->
      <div class="glass-card">
        <h3 class="text-xl font-semibold mb-4">New Deposits Closed</h3>
        <p class="text-gray-300">The legacy pool is withdrawal-only. If an earlier deposit has an uncertain outcome, do not retry it; contact support for reconciliation.</p>
      </div>
      
      <!-- Withdraw Liquidity -->
      <div class="glass-card">
        <h3 class="text-xl font-semibold mb-4">Withdraw icUSD</h3>
        
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
                disabled={actionInProgress || sessionSettling || currentWalletState?.loading}
              />
              <div class="absolute inset-y-0 right-0 flex items-center pr-4 pointer-events-none">
                <span class="text-gray-400">icUSD</span>
              </div>
            </div>
            
            {#if isConnected && !isLoading && liquidityProvidedRaw > 0n}
              <div class="text-xs text-right mt-1">
                <button 
                  class="text-blue-400 hover:text-blue-300" 
                  on:click={() => withdrawAmount = formatIcusdE8s(liquidityProvidedRaw)}
                  disabled={actionInProgress || sessionSettling || currentWalletState?.loading}
                >
                  Max: {formatIcusdE8s(liquidityProvidedRaw)} icUSD
                </button>
              </div>
            {/if}
          </div>
          
          {#if withdrawAmount}
            <div class="p-3 bg-gray-800/70 rounded-lg">
              <div class="flex justify-between text-sm">
                <span class="text-gray-300">Minted to your wallet on withdrawal:</span>
                <span class="text-white font-medium">{withdrawAmount} icUSD</span>
              </div>
            </div>
          {/if}
          <p class="text-xs text-gray-400">To resume an uncertain withdrawal, enter its original amount even if your current pool balance is lower. The backend checks balances for new requests.</p>
          <label class="flex items-start gap-2 text-sm text-gray-300">
            <input type="checkbox" bind:checked={startNewWithdrawal} disabled={actionInProgress || sessionSettling || currentWalletState?.loading} />
            <span>Start a new withdrawal. Leave unchecked to recover the latest request; selecting this asks the backend for a new request ID after the previous one is finished.</span>
          </label>
          
          <button
            class="w-full py-3 px-6 bg-gradient-to-r from-purple-600 to-blue-600 hover:from-purple-700 hover:to-blue-700 rounded-lg text-white font-medium transition-colors disabled:opacity-50"
            on:click={handleWithdrawLiquidity}
            disabled={isLoading || sessionSettling || currentWalletState?.loading || actionInProgress || !withdrawAmount}
          >
            {actionInProgress ? 'Processing...' : 'Withdraw Liquidity'}
          </button>
        </div>
      </div>
    </section>
    
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
          <h3 class="text-lg font-medium mb-2">Existing icUSD</h3>
          <p class="text-gray-300">The legacy pool records existing icUSD contributions. New deposits are closed.</p>
        </div>
        
        <div class="glass-card h-full">
          <div class="text-pink-400 text-3xl font-bold mb-2">2</div>
          <h3 class="text-lg font-medium mb-2">Historical ICP Returns</h3>
          <p class="text-gray-300">Previously recorded ICP returns remain visible, but claims are held pending safe payout reconciliation. The pool does not currently generate new returns.</p>
        </div>
        
        <div class="glass-card h-full">
          <div class="text-pink-400 text-3xl font-bold mb-2">3</div>
          <h3 class="text-lg font-medium mb-2">Request a Withdrawal</h3>
          <p class="text-gray-300">A successful withdrawal mints icUSD to your wallet and reduces your recorded contribution. An uncertain result must be reconciled before a new request. Historical ICP return claims remain held pending safe payout reconciliation.</p>
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
