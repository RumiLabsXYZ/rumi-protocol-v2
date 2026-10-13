<script lang="ts">
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import { walletStore as wallet } from "../../lib/stores/wallet";
  import { TreasuryService, TreasuryManagementService } from '../../lib/services/protocol/apiClient';
  import { formatNumber } from '../../lib/utils/format';
  import { protocolService } from '../../lib/services/protocol';

  let isController = false;
  let treasuryStatus: any = null;
  let feeSummary: any = null;
  let revenueEstimate: any = null;
  let isLoading = true;
  let error = '';
  let icpPrice = 0;

  // Withdrawal form state
  let withdrawalInProgress = false;
  let withdrawalError = '';
  let withdrawalSuccess = '';
  let withdrawalStatusText = '';
  type PendingWithdrawal = {
    requestId: string;
    ownerPrincipal: string;
    assetType: 'ICUSD' | 'ICP' | 'CKBTC';
    amountE8s: string;
    to: string;
    memoSha256: string;
  };
  const WITHDRAWAL_STORAGE_PREFIX = 'rumi:treasury:pending-withdrawal:v1:';
  const LEGACY_WITHDRAWAL_STORAGE_KEY = 'rumi:treasury:pending-withdrawal:v1';
  const WITHDRAWAL_LOCK_NAME = 'rumi:treasury-withdrawal';
  const MAX_NAT64 = 18_446_744_073_709_551_615n;
  let pendingWithdrawal: PendingWithdrawal | null = null;
  let pendingStorageInvalid = false;
  let serverActiveWithdrawal: any | null = null;
  let withdrawalForm = {
    assetType: 'ICUSD' as 'ICUSD' | 'ICP' | 'CKBTC',
    amount: '',
    to: '',
    memo: ''
  };

  $: isConnected = $wallet.isConnected;

  function readPendingWithdrawals(): PendingWithdrawal[] {
    try {
      const pending: PendingWithdrawal[] = [];
      for (let index = 0; index < localStorage.length; index += 1) {
        const key = localStorage.key(index);
        if (!key || (!key.startsWith(WITHDRAWAL_STORAGE_PREFIX) && key !== LEGACY_WITHDRAWAL_STORAGE_KEY)) continue;
        const raw = localStorage.getItem(key);
        if (!raw) continue;
        const parsed = JSON.parse(raw) as PendingWithdrawal;
        if (!parsed.requestId || !parsed.ownerPrincipal || !parsed.amountE8s || !parsed.to || !parsed.memoSha256) throw new Error('Invalid saved request');
        if (key.startsWith(WITHDRAWAL_STORAGE_PREFIX) && key !== `${WITHDRAWAL_STORAGE_PREFIX}${parsed.requestId}`) throw new Error('Mismatched saved request key');
        pending.push(parsed);
      }
      if (pending.length > 1) throw new Error('Multiple unresolved withdrawal requests exist');
      return pending;
    } catch {
      pendingStorageInvalid = true;
      withdrawalError = 'Withdrawal storage contains multiple or unreadable requests. Do not start another withdrawal until every saved request is reconciled.';
      return [];
    }
  }

  function readPendingWithdrawal(): PendingWithdrawal | null {
    return readPendingWithdrawals()[0] || null;
  }

  function savePendingWithdrawal(pending: PendingWithdrawal) {
    localStorage.setItem(`${WITHDRAWAL_STORAGE_PREFIX}${pending.requestId}`, JSON.stringify(pending));
    pendingWithdrawal = pending;
  }

  function clearPendingWithdrawal() {
    if (pendingWithdrawal) {
      localStorage.removeItem(`${WITHDRAWAL_STORAGE_PREFIX}${pendingWithdrawal.requestId}`);
      const legacy = localStorage.getItem(LEGACY_WITHDRAWAL_STORAGE_KEY);
      if (legacy && (JSON.parse(legacy) as PendingWithdrawal).requestId === pendingWithdrawal.requestId) {
        localStorage.removeItem(LEGACY_WITHDRAWAL_STORAGE_KEY);
      }
    }
    pendingWithdrawal = null;
    withdrawalStatusText = '';
  }

  function parseAmountE8s(value: string): bigint | null {
    const match = /^(0|[1-9]\d*)(?:\.(\d{1,8}))?$/.exec(value.trim());
    if (!match) return null;
    return BigInt(match[1]) * 100_000_000n + BigInt((match[2] || '').padEnd(8, '0') || '0');
  }

  async function memoDigest(memo: string): Promise<string> {
    const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(memo));
    return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
  }

  function randomRequestId(): bigint {
    const words = new Uint32Array(2);
    crypto.getRandomValues(words);
    const id = (BigInt(words[0]) << 32n) | BigInt(words[1]);
    return id === 0n ? 1n : id;
  }

  function restorePendingForm() {
    if (!pendingWithdrawal) return;
    const amount = BigInt(pendingWithdrawal.amountE8s);
    const fraction = (amount % 100_000_000n).toString().padStart(8, '0').replace(/0+$/, '');
    withdrawalForm = {
      ...withdrawalForm,
      assetType: pendingWithdrawal.assetType,
      amount: fraction ? `${amount / 100_000_000n}.${fraction}` : (amount / 100_000_000n).toString(),
      to: pendingWithdrawal.to
    };
  }

  function statusVariant(status: any): string {
    return status && typeof status === 'object' ? Object.keys(status)[0] || 'Unknown' : 'Unknown';
  }

  async function reconcilePendingWithdrawal(): Promise<any | null> {
    if (!pendingWithdrawal) return null;
    const queryingPrincipal = $wallet.principal?.toString();
    if (queryingPrincipal !== pendingWithdrawal.ownerPrincipal) throw new Error('Reconnect the controller identity saved with this withdrawal before checking its status.');
    const status = await TreasuryService.getWithdrawalIntentStatus(BigInt(pendingWithdrawal.requestId));
    if ($wallet.principal?.toString() !== queryingPrincipal) throw new Error('Wallet identity changed while checking Treasury status. The saved request remains locked.');
    if (!status) {
      withdrawalStatusText = 'No journal row is visible, but the prior invocation outcome cannot be proven. This request is held for operator review; do not resubmit it.';
      return null;
    }
    const asset = status.asset_type && Object.keys(status.asset_type)[0];
    if (status.request_id.toString() !== pendingWithdrawal.requestId || asset !== pendingWithdrawal.assetType
      || status.amount.toString() !== pendingWithdrawal.amountE8s || status.to.toString() !== pendingWithdrawal.to) {
      throw new Error('Treasury journal payload does not match the saved request. This request is locked for review.');
    }
    const variant = statusVariant(status.status);
    withdrawalStatusText = `Treasury journal status: ${variant}`;
    if (variant === 'InFlight' || variant === 'TransportAmbiguous') {
      withdrawalStatusText = `Treasury journal status: ${variant}. The request may have reached the ledger; automatic replay is disabled. Keep this request ID for reconciliation.`;
    }
    if (variant === 'Completed') {
      withdrawalSuccess = `Withdrawal completed. Block index: ${status.status.Completed.block_index.toString()}`;
      clearPendingWithdrawal();
    } else if (variant === 'Rejected') {
      withdrawalError = status.status.Rejected.error || 'Withdrawal was rejected with no ledger effect.';
      clearPendingWithdrawal();
    }
    return status;
  }

  async function checkServerForActiveWithdrawal(): Promise<boolean> {
    const queryingPrincipal = $wallet.principal?.toString();
    try {
      const active = await TreasuryService.getActiveWithdrawalIntentStatus();
      if ($wallet.principal?.toString() !== queryingPrincipal) {
        throw new Error('Wallet identity changed while checking for an active withdrawal.');
      }
      serverActiveWithdrawal = active;
      if (active) {
        pendingStorageInvalid = true;
        withdrawalStatusText = `Treasury has an unresolved withdrawal request ${active.request_id.toString()} (${statusVariant(active.status)}). Its original memo and signer binding cannot be recovered from the server view, so this page is locked for reconciliation.`;
        return false;
      }
      return true;
    } catch (activeError) {
      pendingStorageInvalid = true;
      withdrawalStatusText = activeError instanceof Error
        ? activeError.message
        : 'Could not prove that Treasury has no unresolved withdrawal. New requests are disabled.';
      return false;
    }
  }

  async function loadTreasuryData() {
    try {
      isLoading = true;
      error = '';
      const loadingPrincipal = $wallet.principal?.toString();
      if (!pendingWithdrawal) {
        pendingStorageInvalid = false;
        pendingWithdrawal = readPendingWithdrawal();
        restorePendingForm();
      }

      // Check if user is controller
      const controllerResult = await TreasuryService.isController();
      if ($wallet.principal?.toString() !== loadingPrincipal) {
        isController = false;
        error = 'Wallet identity changed while loading Treasury data. Reconnect and try again.';
        return;
      }
      isController = controllerResult;
      
      if (!isController) {
        error = 'Access denied. Only the treasury controller can access this page.';
        return;
      }

      if (pendingWithdrawal && pendingWithdrawal.ownerPrincipal !== $wallet.principal?.toString()) {
        withdrawalStatusText = 'A withdrawal is saved for a different controller identity. Reconnect the original identity to check its status.';
      } else if (pendingWithdrawal) {
        try {
          await reconcilePendingWithdrawal();
        } catch (statusError) {
          withdrawalStatusText = statusError instanceof Error ? statusError.message : 'Status unavailable; the saved request remains locked.';
        }
      } else {
        await checkServerForActiveWithdrawal();
      }

      // Load treasury data
      const [status, summary, price] = await Promise.all([
        TreasuryService.getTreasuryStatus(),
        TreasuryManagementService.getFeeSummary(),
        protocolService.getICPPrice()
      ]);
      if ($wallet.principal?.toString() !== loadingPrincipal) {
        isController = false;
        error = 'Wallet identity changed while loading Treasury data. Reconnect and try again.';
        return;
      }

      treasuryStatus = status;
      feeSummary = summary;
      icpPrice = price;

      // Calculate revenue estimate
      revenueEstimate = await TreasuryManagementService.getRevenueEstimate(icpPrice);

    } catch (err) {
      console.error('Error loading treasury data:', err);
      error = err instanceof Error ? err.message : 'Failed to load treasury data';
    } finally {
      isLoading = false;
    }
  }

  async function handleWithdrawal() {
    if (withdrawalInProgress) return;
    if (!navigator.locks) {
      withdrawalError = 'This browser cannot coordinate Treasury submissions across tabs. No withdrawal was sent.';
      return;
    }
    withdrawalInProgress = true;
    try {
      const acquired = await navigator.locks.request(WITHDRAWAL_LOCK_NAME, { ifAvailable: true }, async (lock) => {
        if (!lock) return false;
        pendingStorageInvalid = false;
        pendingWithdrawal = readPendingWithdrawal();
        restorePendingForm();
        if (pendingStorageInvalid) return false;
        await submitWithdrawal();
        return true;
      });
      if (!acquired) withdrawalError = pendingStorageInvalid
        ? 'Withdrawal storage contains unresolved requests and is locked for review.'
        : 'Another Treasury action is active in a different tab. Wait for it to finish, then reconcile the saved request.';
    } catch (err) {
      withdrawalError = err instanceof Error ? err.message : 'Could not coordinate the Treasury request safely.';
    } finally {
      withdrawalInProgress = false;
    }
  }

  async function submitWithdrawal() {
    if (pendingStorageInvalid) return;
    if (pendingWithdrawal && pendingWithdrawal.ownerPrincipal !== $wallet.principal?.toString()) {
      withdrawalError = 'This saved withdrawal belongs to a different controller identity. Reconnect that identity before continuing.';
      return;
    }
    const amountE8s = parseAmountE8s(withdrawalForm.amount);
    if (amountE8s === null || amountE8s <= 0n || amountE8s > MAX_NAT64 || !withdrawalForm.to.trim()) {
      withdrawalError = 'Please fill in all required fields with valid values';
      return;
    }
    if (new TextEncoder().encode(withdrawalForm.memo).length > 24) {
      withdrawalError = 'Memo must be 24 UTF-8 bytes or fewer; Treasury adds the request ID on the ledger.';
      return;
    }
    let normalizedDestination: string;
    try {
      normalizedDestination = Principal.fromText(withdrawalForm.to.trim()).toText();
    } catch {
      withdrawalError = 'Enter a valid destination Principal before starting the withdrawal.';
      return;
    }

    let requestId: bigint;
    if (pendingWithdrawal) {
      const memoMatches = await memoDigest(withdrawalForm.memo) === pendingWithdrawal.memoSha256;
      const payloadMatches = withdrawalForm.assetType === pendingWithdrawal.assetType
        && amountE8s.toString() === pendingWithdrawal.amountE8s
        && normalizedDestination === pendingWithdrawal.to;
      if (!payloadMatches || !memoMatches) {
        withdrawalError = 'This request ID is bound to the saved withdrawal. Restore the exact destination, amount, asset, and memo to check its status.';
        return;
      }
      try {
        const status = await reconcilePendingWithdrawal();
        if (!pendingWithdrawal) return;
        if (status) withdrawalStatusText = `Treasury already has a journal row (${statusVariant(status.status)}). Automatic replay is disabled; keep this request held for reconciliation.`;
        withdrawalError = 'This saved request is held for operator review. Do not submit it again.';
        return;
      } catch (statusError) {
        withdrawalError = statusError instanceof Error ? statusError.message : 'Treasury status is unavailable; the request remains held for operator review.';
        return;
      }
      requestId = BigInt(pendingWithdrawal.requestId);
    } else {
      if (!await checkServerForActiveWithdrawal()) {
        withdrawalError = withdrawalStatusText || 'Treasury has an unresolved withdrawal; no new request was created.';
        return;
      }
      requestId = randomRequestId();
      const ownerPrincipal = $wallet.principal?.toString();
      if (!ownerPrincipal) {
        withdrawalError = 'Connect the controller identity before starting a withdrawal.';
        return;
      }
      try {
        savePendingWithdrawal({
          requestId: requestId.toString(),
          ownerPrincipal,
          assetType: withdrawalForm.assetType,
          amountE8s: amountE8s.toString(),
          to: normalizedDestination,
          memoSha256: await memoDigest(withdrawalForm.memo)
        });
      } catch {
        withdrawalError = 'Could not persist the request safely. No withdrawal was sent.';
        return;
      }
    }

    try {
      withdrawalError = '';
      withdrawalSuccess = '';
      const ownerPrincipal = pendingWithdrawal?.ownerPrincipal;
      if (!ownerPrincipal || $wallet.principal?.toString() !== ownerPrincipal) {
        withdrawalError = 'Wallet identity changed before the withdrawal was sent. The saved request remains locked.';
        return;
      }

      const result = await TreasuryService.withdrawFromTreasury(
        withdrawalForm.assetType,
        amountE8s,
        normalizedDestination,
        requestId,
        ownerPrincipal,
        withdrawalForm.memo || undefined
      );

      if (result.success) {
        withdrawalSuccess = `Successfully withdrew ${withdrawalForm.amount} ${withdrawalForm.assetType}. Block index: ${result.blockIndex}`;
        clearPendingWithdrawal();
        
        // Reset form
        withdrawalForm = {
          assetType: 'ICUSD',
          amount: '',
          to: '',
          memo: ''
        };

        // Reload data
        await loadTreasuryData();
      } else {
        withdrawalError = result.error || 'Withdrawal failed';
        try {
          if (pendingWithdrawal) await reconcilePendingWithdrawal();
        } catch {
          // Keep the persisted request ID if reconciliation cannot be proven.
        }
      }
    } catch (err) {
      console.error('Error during withdrawal:', err);
      withdrawalError = err instanceof Error ? err.message : 'Unknown error occurred';
      try {
        if (pendingWithdrawal) await reconcilePendingWithdrawal();
      } catch {
        // A missing or failed status query must never clear an uncertain request.
      }
    }
  }

  function getAvailableBalance(assetType: string): number {
    if (!treasuryStatus) return 0;
    return treasuryStatus.balances[assetType] || 0;
  }

  onMount(() => {
    if (isConnected) {
      loadTreasuryData();
    }
  });

  // Watch for wallet connection changes
  $: if (isConnected) {
    loadTreasuryData();
  }
</script>

<svelte:head>
  <title>Treasury | Rumi Protocol</title>
</svelte:head>

<div class="container mx-auto px-4 max-w-6xl">
  <div class="mb-8">
    <h1 class="text-4xl font-bold mb-4 page-title">
      Treasury Management
    </h1>
    <p class="text-xl text-gray-300">
      View and manage protocol fee collections
    </p>
  </div>

  {#if !isConnected}
    <div class="glass-card p-8 text-center">
      <p class="text-lg text-gray-300 mb-4">Please connect your wallet to access treasury management</p>
    </div>
  {:else if isLoading}
    <div class="glass-card p-8 text-center">
      <div class="animate-spin w-8 h-8 border-4 border-purple-500 border-t-transparent rounded-full mx-auto mb-4"></div>
      <p class="text-gray-300">Loading treasury data...</p>
    </div>
  {:else if error}
    <div class="glass-card p-8 text-center bg-red-900/20 border-red-500/30">
      <p class="text-red-300 text-lg">{error}</p>
    </div>
  {:else if !isController}
    <div class="glass-card p-8 text-center bg-yellow-900/20 border-yellow-500/30">
      <p class="text-yellow-300 text-lg">Access denied. Only the treasury controller can access this page.</p>
    </div>
  {:else}
    <!-- Treasury Overview -->
    <div class="grid grid-cols-1 md:grid-cols-3 gap-6 mb-8">
      <div class="glass-card">
        <h3 class="text-lg font-semibold mb-2 text-purple-300">Total Deposits</h3>
        <p class="text-3xl font-bold">{treasuryStatus.totalDeposits}</p>
        <p class="text-sm text-gray-400">Fee collections</p>
      </div>

      <div class="glass-card">
        <h3 class="text-lg font-semibold mb-2 text-green-300">Estimated Revenue</h3>
        <p class="text-3xl font-bold">${formatNumber(revenueEstimate?.totalUSD || 0)}</p>
        <p class="text-sm text-gray-400">USD equivalent</p>
      </div>

      <div class="glass-card">
        <h3 class="text-lg font-semibold mb-2 text-blue-300">ICP Price</h3>
        <p class="text-3xl font-bold">${formatNumber(icpPrice)}</p>
        <p class="text-sm text-gray-400">Current rate</p>
      </div>
    </div>

    <!-- Asset Balances -->
    <div class="glass-card mb-8">
      <h2 class="text-2xl font-semibold mb-6">Treasury Balances</h2>
      <div class="grid grid-cols-1 md:grid-cols-3 gap-6">
        {#each Object.entries(treasuryStatus.balances) as [asset, balance]}
          <div class="bg-gray-800/50 p-4 rounded-lg">
            <div class="flex justify-between items-center mb-2">
              <span class="text-gray-300">{asset}</span>
              <span class="text-2xl font-bold text-white">{formatNumber(Number(balance))}</span>
            </div>
            {#if revenueEstimate?.breakdown[asset]}
              <p class="text-sm text-gray-400">≈ ${formatNumber(revenueEstimate.breakdown[asset])}</p>
            {/if}
          </div>
        {/each}
      </div>
    </div>

    <!-- Fee Breakdown -->
    <div class="glass-card mb-8">
      <h2 class="text-2xl font-semibold mb-6">Fee Breakdown</h2>
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        {#each Object.entries(feeSummary.totalByType) as [feeType, amount]}
          <div class="bg-gray-800/50 p-4 rounded-lg">
            <div class="flex justify-between items-center">
              <span class="text-gray-300">{feeType.replace('_', ' ')}</span>
              <span class="font-semibold">{formatNumber(Number(amount))}</span>
            </div>
          </div>
        {/each}
      </div>
    </div>

    <!-- Recent Activity -->
    <div class="glass-card mb-8">
      <h2 class="text-2xl font-semibold mb-6">Recent Fee Collections</h2>
      <div class="overflow-x-auto">
        <table class="w-full">
          <thead>
            <tr class="border-b border-gray-700">
              <th class="text-left py-2">Type</th>
              <th class="text-left py-2">Asset</th>
              <th class="text-left py-2">Amount</th>
              <th class="text-left py-2">Date</th>
              <th class="text-left py-2">Block</th>
            </tr>
          </thead>
          <tbody>
            {#each feeSummary.recentActivity as activity}
              <tr class="border-b border-gray-800">
                <td class="py-2 text-sm">{activity.feeType}</td>
                <td class="py-2 text-sm">{activity.assetType}</td>
                <td class="py-2 text-sm font-mono">{formatNumber(activity.amount)}</td>
                <td class="py-2 text-sm">{activity.timestamp.toLocaleDateString()}</td>
                <td class="py-2 text-sm text-blue-400">{activity.blockIndex}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </div>

    <!-- Withdrawal Form -->
    <div class="glass-card">
      <h2 class="text-2xl font-semibold mb-6">Withdraw Funds</h2>
      
      {#if withdrawalError}
        <div class="bg-red-900/30 border border-red-500 p-4 rounded-lg mb-4">
          <p class="text-red-300">{withdrawalError}</p>
        </div>
      {/if}

      {#if withdrawalStatusText}
        <div class="bg-yellow-900/30 border border-yellow-500 p-4 rounded-lg mb-4">
          <p class="text-yellow-200">{withdrawalStatusText}</p>
          {#if pendingWithdrawal}
            <p class="text-xs text-yellow-100/80 mt-2">Saved request ID: {pendingWithdrawal.requestId}</p>
          {/if}
        </div>
      {/if}

      {#if withdrawalSuccess}
        <div class="bg-green-900/30 border border-green-500 p-4 rounded-lg mb-4">
          <p class="text-green-300">{withdrawalSuccess}</p>
        </div>
      {/if}

      <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
        <div>
          <label for="asset-type" class="block text-sm font-medium text-gray-300 mb-2">
            Asset Type
          </label>
          <select 
            id="asset-type"
            bind:value={withdrawalForm.assetType}
            class="w-full bg-gray-800/50 border border-gray-700 rounded-lg px-4 py-3 text-white"
            disabled={withdrawalInProgress || Boolean(pendingWithdrawal)}
          >
            <option value="ICUSD">icUSD</option>
            <option value="ICP">ICP</option>
            <option value="CKBTC">ckBTC</option>
          </select>
          <p class="text-sm text-gray-400 mt-1">
            Available: {formatNumber(getAvailableBalance(withdrawalForm.assetType))} {withdrawalForm.assetType}
          </p>
        </div>

        <div>
          <label for="amount" class="block text-sm font-medium text-gray-300 mb-2">
            Amount
          </label>
          <input
            id="amount"
            type="text"
            inputmode="decimal"
            bind:value={withdrawalForm.amount}
            class="w-full bg-gray-800/50 border border-gray-700 rounded-lg px-4 py-3 text-white"
            placeholder="0.00"
            disabled={withdrawalInProgress || Boolean(pendingWithdrawal)}
          />
        </div>

        <div class="md:col-span-2">
          <label for="to" class="block text-sm font-medium text-gray-300 mb-2">
            Destination Principal *
          </label>
          <input
            id="to"
            type="text"
            bind:value={withdrawalForm.to}
            class="w-full bg-gray-800/50 border border-gray-700 rounded-lg px-4 py-3 text-white"
            placeholder="rrkah-fqaaa-aaaaa-aaaaq-cai"
            disabled={withdrawalInProgress || Boolean(pendingWithdrawal)}
          />
        </div>

        <div class="md:col-span-2">
          <label for="memo" class="block text-sm font-medium text-gray-300 mb-2">
            Memo (Optional)
          </label>
          <input
            id="memo"
            type="text"
            bind:value={withdrawalForm.memo}
            class="w-full bg-gray-800/50 border border-gray-700 rounded-lg px-4 py-3 text-white"
            placeholder="Treasury operations"
            disabled={withdrawalInProgress}
          />
          <p class="mt-1 text-xs text-gray-400">Maximum 24 UTF-8 bytes.</p>
        </div>

        <div class="md:col-span-2">
          <button
            class="w-full py-3 px-6 bg-gradient-to-r from-pink-500 to-purple-600 hover:from-pink-600 hover:to-purple-700 rounded-lg text-white font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            on:click={handleWithdrawal}
            disabled={withdrawalInProgress || pendingStorageInvalid || !withdrawalForm.amount || !withdrawalForm.to}
          >
            {#if withdrawalInProgress}
              <div class="flex items-center justify-center gap-2">
                <div class="w-4 h-4 border-2 border-white border-t-transparent rounded-full animate-spin"></div>
                Processing Withdrawal...
              </div>
            {:else}
              {pendingWithdrawal ? 'Check saved withdrawal status' : `Withdraw ${withdrawalForm.amount} ${withdrawalForm.assetType}`}
            {/if}
          </button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .glass-card {
    background-color: rgba(31, 41, 55, 0.4);
    backdrop-filter: blur(16px);
    border: 1px solid rgba(75, 85, 99, 0.5);
    border-radius: 0.5rem;
    padding: 1.5rem;
  }
</style>
