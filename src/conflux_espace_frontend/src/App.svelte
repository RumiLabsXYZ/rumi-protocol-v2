<script lang="ts">
  import {
    ACTION,
    BACKEND_CANISTER_ID,
    CANARY_COLLATERAL_WEI,
    CANARY_DEBT_E8S,
    CHAIN_ID,
    DEPLOYMENT,
    ESPACE_EXPLORER,
    ICUSD_CONTRACT,
    IS_MAINNET,
    IS_PRODUCTION_CANARY,
    IS_PRODUCTION_PUBLIC,
    MIN_CR,
    MIN_DEBT_E8S,
    PUBLIC_CANONICAL_ORIGIN,
    openTermsFor,
    suggestedCollateralWei,
    suggestedCollateralWeiAtRatio,
    signatureAttemptLimit,
  } from "./config";
  import { backend, errText, statusName, type ChainPublicLaunchStatus, type ChainVault } from "./backend";
  import { blockingReasonText, priceE8, publicActionRefusal, publicWriteRefusal, ratioE4, variantName } from "./launchStatus";
  import { listCompleteChainVaultInventory } from "./inventory";
  import { publicOriginRefusal } from "./origin";
  import {
    finalizedFailureProvesNonExecution,
    isSignedMainnetAction,
    mainnetActionObserved,
    mainnetStorageKey,
    markMainnetAmbiguous,
    markMainnetSubmitted,
    newMainnetActionLock,
    parseMainnetActionLock,
    sendFreshDepositAfterPreflight,
    signedActionResolvedByNonce,
    withMainnetNonce,
    withMainnetReceiptSuccess,
    withMainnetTransaction,
    withMainnetVaultId,
    type MainnetActionKind,
    type MainnetActionLock,
    type MainnetVaultSnapshot,
  } from "./mainnetState";
  import {
    canaryStorageKey,
    applyFailedTransactionFinality,
    isRecoverableOpenCandidate,
    manualRecoveryTarget,
    markTransactionReceiptSucceeded,
    newCanaryOpenLock,
    newCanaryRecord,
    parseCanaryRecord,
    pendingTransaction,
    productionLifecycleUsed as hasUsedProductionLifecycle,
    reconcileCanaryPhase,
    recordTransaction,
    replaceLatestTransactionHash,
    validateCanaryAction,
    type CanaryPhase,
    type CanaryRecord,
    type CanaryVaultSnapshot,
  } from "./canaryState";
  import { signIntent, toCandidIntent, type VaultIntentInput } from "./eip712";
  import {
    addressUrl,
    burnIcusd,
    cfxBalance,
    connectInjected,
    connectLegacyInjected,
    fmtCfx,
    fmtIcusd,
    getInjectedWallets,
    hasLegacyInjected,
    icusdBalance,
    isExplicitWalletRejection,
    refreshInjectedWallets,
    sendDeposit,
    subscribeWallets,
    toE8s,
    txUrl,
    waitForTransactionFinality,
    walletChainId,
    walletStillControlsAddress,
    type EIP6963ProviderDetail,
    type Wallet,
  } from "./evm";
  import { connectDevKey } from "./devWallet";
  import VaultCard from "./VaultCard.svelte";

  let wallet = $state<Wallet | null>(null);
  let vaults = $state<ChainVault[]>([]);
  let cfx = $state(0n);
  let icusd = $state(0n);
  let productionAcknowledged = $state(false);
  let productionInventoryVerified = $state(false);
  let publicStatus = $state<ChainPublicLaunchStatus | null>(null);
  let publicStatusError = $state<string | null>(null);
  let walletChainValid = $state(false);
  let walletAddressValid = $state(false);
  let mainnetLock = $state<MainnetActionLock | null>(null);
  let mainnetRecoveryAcknowledged = $state(false);
  let inventoryComplete = $state(false);

  let busy = $state<string | null>(null);
  let err = $state<string | null>(null);
  let ok = $state<string | null>(null);
  let canary = $state<CanaryRecord | null>(null);
  let receiptWatching = $state<string | null>(null);
  let receiptRetryGeneration = $state(0);
  let recoveryAcknowledged = $state(false);
  const receiptAttempts = new Set<string>();
  const mainnetReceiptAttempts = new Set<string>();

  // EIP-6963: discovered injected wallets (Rabby, MetaMask, ...).
  let injectedWallets = $state<EIP6963ProviderDetail[]>(getInjectedWallets());
  $effect(() => {
    refreshInjectedWallets();
    return subscribeWallets((list) => { injectedWallets = list; });
  });

  // Mirror durable state across same-wallet tabs. Web Locks still serialize
  // every production action; this listener keeps the visible state current.
  $effect(() => {
    if (!IS_PRODUCTION_CANARY || !wallet || typeof window === "undefined") return;
    const owner = wallet.address;
    const key = canaryStorageKey(owner);
    const onStorage = (event: StorageEvent) => {
      if (event.key !== key) return;
      loadCanary(owner);
      recoveryAcknowledged = false;
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  });

  $effect(() => {
    if (!IS_PRODUCTION_PUBLIC || !wallet || typeof window === "undefined") return;
    const owner = wallet.address;
    const key = mainnetStorageKey(owner);
    const onStorage = (event: StorageEvent) => {
      if (event.key !== key) return;
      loadMainnetLock(owner);
      mainnetRecoveryAcknowledged = false;
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  });

  // Testnet open form. The production-canary build ignores these inputs and
  // supplies its compile-time exact terms directly to the signed Open intent.
  let debtInput = $state("0.2");
  let cfxPrice = $state("0.15"); // UX hint only — the real CR check is server-side
  let showDevKey = $state(false);
  let devKey = $state("0x" + "00".repeat(31) + "01"); // scalar=1 demo key

  const owned = $derived(
    wallet
      ? vaults.filter((v) => v.owner_evm.length && v.owner_evm[0]!.toLowerCase() === wallet!.address.toLowerCase())
      : []
  );
  const pendingExists = $derived(owned.some((v) => {
    const s = statusName(v.status);
    return s === "AwaitingDeposit" || s === "MintPending" || s === "Closing";
  }));
  const productionLifecycleUsed = $derived(IS_PRODUCTION_CANARY && hasUsedProductionLifecycle(canary, owned.length));
  const canaryPolling = $derived(canary !== null &&
    (canary.phase === "open-authorizing" || canary.phase === "deposit-authorizing" ||
      canary.phase === "deposit-submitted" || canary.phase === "deposit-observed" || canary.phase === "burn-authorizing" ||
      canary.phase === "deposit-replaced" || canary.phase === "burn-submitted" ||
      canary.phase === "burn-replaced" || canary.phase === "close-authorizing" || canary.phase === "close-submitted"));
  const unresolvedAuthorization = $derived(canary !== null &&
    (canary.phase === "open-authorizing" || canary.phase === "deposit-authorizing" ||
      canary.phase === "deposit-replaced" || canary.phase === "burn-authorizing" ||
      canary.phase === "burn-replaced" || canary.phase === "close-authorizing"));
  const transactions = $derived((canary?.transactions ?? []).map((tx) => ({
    label: tx.kind === "deposit" ? "CFX deposit" : "icUSD burn",
    hash: tx.hash,
    url: txUrl(tx.hash),
  })));

  const liveMinCr = $derived(publicStatus ? ratioE4(publicStatus.min_cr_e4) : null);
  const liveLiquidationCr = $derived(publicStatus ? ratioE4(publicStatus.liquidation_threshold_e4) : null);
  const liveCfxPrice = $derived(publicStatus ? priceE8(publicStatus.collateral_price_e8) : null);
  const liveLiquidationDigestMatches = $derived(!!publicStatus &&
    !!publicStatus.liquidation_config_digest[0] &&
    !!publicStatus.expected_liquidation_config_digest &&
    publicStatus.liquidation_config_digest[0] === publicStatus.expected_liquidation_config_digest);
  const publicOriginBlocker = $derived(IS_PRODUCTION_PUBLIC
    ? publicOriginRefusal(PUBLIC_CANONICAL_ORIGIN, typeof window === "undefined" ? "" : window.location.origin)
    : null);
  function publicActionBlocker(requiresPublicReadiness: boolean): string | null {
    if (!IS_PRODUCTION_PUBLIC) return null;
    if (publicOriginBlocker) return publicOriginBlocker;
    const backendRefusal = publicActionRefusal(publicStatus, requiresPublicReadiness);
    if (backendRefusal) return backendRefusal;
    if (!wallet) return "Connect an injected wallet to continue.";
    if (!walletChainValid) return `Switch the connected wallet to Conflux eSpace chain ${CHAIN_ID}.`;
    if (!walletAddressValid) return "The wallet's active address changed. Disconnect and reconnect the intended address.";
    if (!inventoryComplete) return "Complete vault inventory is unavailable. Refresh before any write.";
    if (mainnetLock) return "A previous production action is still resolving. Refresh and do not repeat it.";
    return null;
  }
  const publicBackendLaunchRefusal = $derived(IS_PRODUCTION_PUBLIC ? publicWriteRefusal(publicStatus) : null);
  const publicLaunchRefusal = $derived(publicOriginBlocker ?? publicBackendLaunchRefusal);
  const publicLaunchReady = $derived(IS_PRODUCTION_PUBLIC && publicLaunchRefusal === null);
  const publicRiskWriteBlocker = $derived.by(() => publicActionBlocker(true));
  const publicRecoveryWriteBlocker = $derived.by(() => publicActionBlocker(false));
  const publicRiskWritesEnabled = $derived(!IS_PRODUCTION_PUBLIC || publicRiskWriteBlocker === null);
  const publicRecoveryWritesEnabled = $derived(!IS_PRODUCTION_PUBLIC || publicRecoveryWriteBlocker === null);

  const requestedDebtE8s = $derived(toE8s(debtInput));
  const requestedCfxWei = $derived.by(() => {
    const d = parseFloat(debtInput) || 0;
    if (IS_PRODUCTION_PUBLIC) {
      return suggestedCollateralWeiAtRatio(d, liveCfxPrice ?? 0, liveMinCr ?? 0);
    }
    return suggestedCollateralWei(d, parseFloat(cfxPrice) || 0);
  });
  const openTerms = $derived(openTermsFor(DEPLOYMENT, requestedCfxWei, requestedDebtE8s));

  function reset() { err = null; ok = null; }
  const yesNo = (value: boolean) => value ? "Clear" : "Blocked";
  const optIcusd = (value: [] | [bigint]) => value.length ? `${fmtIcusd(value[0])} icUSD` : "No ceiling";
  const ageText = (value: [] | [bigint]) => value.length
    ? `${(Number(value[0]) / 60_000_000_000).toLocaleString(undefined, { maximumFractionDigits: 1 })} min`
    : "Unavailable";
  const timestampText = (value: [] | [bigint]) => value.length
    ? new Date(Number(value[0] / 1_000_000n)).toLocaleString()
    : "Unavailable";

  function snapshot(vault: ChainVault): CanaryVaultSnapshot {
    return {
      vaultId: vault.vault_id,
      chainId: vault.collateral_chain,
      owner: vault.owner_evm[0] ?? null,
      recipient: vault.mint_recipient,
      collateralWei: vault.collateral_amount_e18,
      debtE8s: vault.debt_e8s,
      pendingMintE8s: vault.pending_mint_e8s,
      pendingInterestMintE8s: vault.pending_interest_mint_e8s,
      status: statusName(vault.status),
    };
  }

  function mainnetSnapshot(vault: ChainVault): MainnetVaultSnapshot {
    return {
      vaultId: vault.vault_id,
      owner: vault.owner_evm[0] ?? null,
      status: statusName(vault.status),
      debtE8s: vault.debt_e8s,
      pendingMintE8s: vault.pending_mint_e8s,
      collateralWei: vault.collateral_amount_e18,
    };
  }

  function canPersistMainnet(): boolean {
    if (!wallet || typeof localStorage === "undefined") return false;
    try {
      const key = `${mainnetStorageKey(wallet.address)}:probe`;
      localStorage.setItem(key, "1");
      localStorage.removeItem(key);
      return true;
    } catch {
      return false;
    }
  }

  function persistMainnetLock(): boolean {
    if (!wallet || !mainnetLock || typeof localStorage === "undefined") return false;
    try {
      const encoded = JSON.stringify(mainnetLock);
      const key = mainnetStorageKey(wallet.address);
      localStorage.setItem(key, encoded);
      if (localStorage.getItem(key) !== encoded) throw new Error("storage verification failed");
      return true;
    } catch {
      err = "Production action state could not be persisted. Do not reload or repeat the action.";
      return false;
    }
  }

  function loadMainnetLock(owner: `0x${string}`) {
    if (!IS_PRODUCTION_PUBLIC || typeof localStorage === "undefined") return;
    mainnetLock = parseMainnetActionLock(localStorage.getItem(mainnetStorageKey(owner)), owner);
  }

  function clearMainnetLock(): boolean {
    if (!wallet || typeof localStorage === "undefined") return false;
    try {
      const key = mainnetStorageKey(wallet.address);
      localStorage.removeItem(key);
      if (localStorage.getItem(key) !== null) throw new Error("storage verification failed");
      mainnetLock = null;
      mainnetReceiptAttempts.clear();
      return true;
    } catch {
      err = "The production action lock could not be cleared. Do not repeat the action.";
      return false;
    }
  }

  function beginMainnetLock(kind: MainnetActionKind, vault?: ChainVault, amount = 0n, openCollateralWei = 0n): MainnetActionLock | null {
    if (!IS_PRODUCTION_PUBLIC) return null;
    if (!wallet || mainnetLock) throw new Error("A previous production action is still resolving. Do not repeat it.");
    if (!canPersistMainnet()) throw new Error("Production mainnet requires browser storage so action locks survive reloads.");
    mainnetLock = newMainnetActionLock({
      owner: wallet.address,
      kind,
      vaultId: vault?.vault_id,
      amount,
      baselineVaultIds: vaults
        .filter((v) => v.owner_evm[0]?.toLowerCase() === wallet!.address.toLowerCase())
        .map((v) => v.vault_id),
      baselineStatus: vault ? statusName(vault.status) : null,
      baselineDebtE8s: vault?.debt_e8s,
      baselineCollateralWei: vault?.collateral_amount_e18 ?? openCollateralWei,
    });
    if (!persistMainnetLock()) {
      mainnetLock = null;
      throw new Error("Could not persist the production safety lock. No wallet request was made.");
    }
    return mainnetLock;
  }

  function restoreExplicitlyRejectedMainnet(lock: MainnetActionLock | null) {
    if (!lock || !IS_PRODUCTION_PUBLIC) return;
    clearMainnetLock();
  }

  function recordMainnetNonce(nonce: bigint) {
    if (!IS_PRODUCTION_PUBLIC || !mainnetLock) return;
    mainnetLock = withMainnetNonce(mainnetLock, nonce);
    if (!persistMainnetLock()) throw new Error("Could not persist the signed intent nonce. Do not repeat the action.");
  }

  function canPersistCanary(): boolean {
    if (!wallet || typeof localStorage === "undefined") return false;
    try {
      const key = `${canaryStorageKey(wallet.address)}:probe`;
      localStorage.setItem(key, "1");
      localStorage.removeItem(key);
      return true;
    } catch {
      return false;
    }
  }

  function persistCanary(): boolean {
    if (!wallet || !canary || typeof localStorage === "undefined") return false;
    try {
      const key = canaryStorageKey(wallet.address);
      const encoded = JSON.stringify(canary);
      localStorage.setItem(key, encoded);
      if (localStorage.getItem(key) !== encoded) throw new Error("storage verification failed");
      return true;
    } catch {
      err = "Canary state could not be persisted. Do not reload or repeat an action.";
      return false;
    }
  }

  function loadCanary(owner: `0x${string}`) {
    if (!IS_PRODUCTION_CANARY || typeof localStorage === "undefined") return;
    canary = parseCanaryRecord(localStorage.getItem(canaryStorageKey(owner)), owner);
  }

  function setCanaryPhase(phase: CanaryPhase) {
    if (!canary) return;
    canary = { ...canary, phase };
    persistCanary();
  }

  function persistActionLock(phase: "deposit-authorizing" | "burn-authorizing" | "close-authorizing"): CanaryRecord {
    if (!canary) throw new Error("The persisted canary lifecycle is missing.");
    const previous = canary;
    canary = { ...canary, phase };
    if (!persistCanary()) {
      canary = previous;
      throw new Error("Could not persist the pre-transaction safety lock. No wallet request was made.");
    }
    return previous;
  }

  function restoreCanaryLock(previous: CanaryRecord, lockedPhase: "deposit-authorizing" | "burn-authorizing" | "close-authorizing") {
    canary = previous;
    if (!persistCanary()) {
      // The durable value is still the pre-broadcast lock. Keep memory equally
      // fail-closed rather than showing a retry button that reload would reject.
      canary = { ...previous, phase: lockedPhase };
      throw new Error("The wallet rejected the request, but the safety lock could not be cleared. Reload and inspect before any retry.");
    }
  }

  function clearOpenLock(lock: CanaryRecord): boolean {
    if (!wallet || typeof localStorage === "undefined") return false;
    const key = canaryStorageKey(wallet.address);
    try {
      localStorage.removeItem(key);
      if (localStorage.getItem(key) !== null) throw new Error("storage verification failed");
      canary = null;
      return true;
    } catch {
      canary = lock;
      err = "The Open safety lock could not be cleared. Reload and inspect before trying again.";
      return false;
    }
  }

  async function withCanaryExclusivity(run: () => Promise<void>) {
    if (!IS_MAINNET) return run();
    if (!wallet || typeof navigator === "undefined" || !navigator.locks) {
      throw new Error("Production mainnet requires browser cross-tab locks; use a current injected-wallet browser.");
    }
    const key = IS_PRODUCTION_CANARY ? canaryStorageKey(wallet.address) : mainnetStorageKey(wallet.address);
    await navigator.locks.request(`${key}:action`, async () => {
      // Another tab may have advanced the durable state while this tab waited.
      if (IS_PRODUCTION_CANARY) loadCanary(wallet!.address);
      if (IS_PRODUCTION_PUBLIC) loadMainnetLock(wallet!.address);
      await run();
    });
  }

  function reconcileCanary(nextVaults: ChainVault[]) {
    if (!canary) return;
    if (canary.phase === "open-authorizing" && canary.vaultId === "0") {
      const candidates = nextVaults.filter((v) => isRecoverableOpenCandidate(canary!, snapshot(v)));
      if (candidates.length !== 1) return;
      canary = newCanaryRecord(canary.owner, candidates[0]!.vault_id);
      persistCanary();
    }
    const vault = nextVaults.find((v) => v.vault_id.toString() === canary!.vaultId);
    if (!vault) return;
    const before = canary.phase;
    const next = reconcileCanaryPhase(canary, snapshot(vault));
    if (next.phase === before) return;
    canary = next;
    persistCanary();
    ok = next.phase === "mint-observed"
      ? "Mint observed. The vault is Open."
      : next.phase === "burn-observed"
        ? "The exact 0.10 icUSD burn was observed. Debt is zero."
        : next.phase === "complete"
          ? "Closed observed. The production canary lifecycle is complete. This build will not open another."
          : ok;
  }

  async function observePendingTransaction() {
    if (!canary) return;
    const tx = pendingTransaction(canary);
    if (!tx || receiptWatching === tx.hash || receiptAttempts.has(tx.hash)) return;
    receiptWatching = tx.hash;
    receiptAttempts.add(tx.hash);
    try {
      const result = await waitForTransactionFinality(tx.hash);
      if (!canary || pendingTransaction(canary)?.hash !== tx.hash) return;
      if (result.hash !== tx.hash) {
        canary = replaceLatestTransactionHash(canary, tx.kind, result.hash);
      }
      if (!result.ok) {
        const resolved = applyFailedTransactionFinality(canary, tx.kind, result.replacementReason);
        canary = resolved;
        persistCanary();
        if (resolved.phase === "deposit-failed" || resolved.phase === "burn-failed") {
          err = result.replacementReason === "cancelled"
            ? `The ${tx.kind} transaction was cancelled. Review the explorer entry before retrying.`
            : `The ${tx.kind} transaction reverted. Review the explorer entry before retrying.`;
        } else {
          // A different replacement may itself have transferred/burned funds.
          // Preserve the submitted lock until backend state resolves it.
          err = `The ${tx.kind} transaction was semantically replaced. It remains locked because the replacement may have moved funds; do not retry it.`;
        }
      } else {
        canary = markTransactionReceiptSucceeded(canary, tx.kind, result.hash);
        persistCanary();
      }
    } catch (e: any) {
      // Keep the persisted lock. A reload safely retries receipt recovery.
      err = `Transaction receipt check paused: ${e?.shortMessage ?? e?.message ?? e}. The action remains locked.`;
      setTimeout(() => {
        receiptAttempts.delete(tx.hash);
        receiptRetryGeneration += 1;
      }, 10_000);
    } finally {
      receiptWatching = null;
    }
  }

  async function refreshPublicStatus(): Promise<ChainPublicLaunchStatus | null> {
    if (!IS_PRODUCTION_PUBLIC) return null;
    try {
      const status = await (await backend()).get_chain_public_launch_status(CHAIN_ID);
      publicStatus = status;
      publicStatusError = null;
      if (wallet) {
        try {
          walletChainValid = (await walletChainId(wallet)) === CHAIN_ID;
          walletAddressValid = await walletStillControlsAddress(wallet);
        } catch {
          walletChainValid = false;
          walletAddressValid = false;
        }
      }
      return status;
    } catch (e: any) {
      publicStatus = null;
      const message = String(e?.message ?? e);
      publicStatusError = /no query method ['\"]?get_chain_public_launch_status/i.test(message)
        ? "The production backend has not published its readiness endpoint yet. This build will remain read-only until the backend upgrade is verified."
        : "The production readiness check could not reach the backend. This build remains read-only and will retry automatically.";
      walletChainValid = false;
      walletAddressValid = false;
      return null;
    }
  }

  function requiresPublicReadiness(kind: MainnetActionKind): boolean {
    return kind === "open" || kind === "deposit" || kind === "borrow" || kind === "withdraw";
  }

  async function assertPublicWriteReady(kind: MainnetActionKind) {
    if (!IS_PRODUCTION_PUBLIC) return;
    const originRefusal = publicOriginRefusal(
      PUBLIC_CANONICAL_ORIGIN,
      typeof window === "undefined" ? "" : window.location.origin,
    );
    if (originRefusal) throw new Error(originRefusal);
    const status = await refreshPublicStatus();
    const refusal = publicActionRefusal(status, requiresPublicReadiness(kind));
    if (refusal) throw new Error(`Production writes are paused: ${refusal}`);
    if (!wallet || !/^0x[0-9a-fA-F]{40}$/.test(wallet.address)) throw new Error("The connected wallet address is invalid.");
    if (!walletChainValid) throw new Error(`Switch the connected wallet to Conflux eSpace chain ${CHAIN_ID}.`);
    if (!walletAddressValid) throw new Error("The wallet's active address changed. Disconnect and reconnect the intended address.");
    if (!inventoryComplete) throw new Error("Complete vault inventory is unavailable. Refresh before any write.");
    if (mainnetLock) throw new Error("A previous production action is still resolving. Do not repeat it.");
  }

  async function reconcileMainnetLock(nextVaults: ChainVault[]) {
    if (!IS_PRODUCTION_PUBLIC || !mainnetLock || !wallet) return;
    const observed = mainnetActionObserved(mainnetLock, nextVaults.map(mainnetSnapshot));
    let exactNonceSuccess = false;
    let nonceAdvancedPastLock = false;
    if (mainnetLock.kind !== "open" && isSignedMainnetAction(mainnetLock.kind) && mainnetLock.nonce !== null) {
      try {
        const res = await (await backend()).get_expected_evm_nonce(CHAIN_ID, wallet.address);
        if ("Ok" in res) {
          const expected = BigInt(res.Ok);
          exactNonceSuccess = signedActionResolvedByNonce(mainnetLock, expected);
          nonceAdvancedPastLock = expected > BigInt(mainnetLock.nonce) + 1n;
        }
      } catch {
        // Keep the lock when the read cannot prove resolution.
      }
    }
    if (observed || exactNonceSuccess) {
      const kind = mainnetLock.kind;
      if (clearMainnetLock()) ok = `${kind[0]!.toUpperCase()}${kind.slice(1)} resolved by fresh backend state.`;
    } else if (nonceAdvancedPastLock && mainnetLock.phase !== "ambiguous") {
      mainnetLock = markMainnetAmbiguous(mainnetLock);
      persistMainnetLock();
      err = "The backend nonce advanced beyond this action's exact success value. The lock remains for manual investigation; do not repeat it.";
    }
  }

  async function observeMainnetTransaction() {
    if (!IS_PRODUCTION_PUBLIC || !mainnetLock?.txHash ||
        mainnetReceiptAttempts.has(mainnetLock.txHash) || receiptWatching === mainnetLock.txHash) return;
    const original = mainnetLock;
    const originalHash = original.txHash!;
    receiptWatching = originalHash;
    mainnetReceiptAttempts.add(originalHash);
    try {
      const result = await waitForTransactionFinality(originalHash);
      if (!mainnetLock || mainnetLock.txHash !== originalHash) return;
      if (result.hash !== originalHash) {
        mainnetLock = withMainnetTransaction(mainnetLock, result.hash);
        persistMainnetLock();
      }
      if (!result.ok) {
        if (!finalizedFailureProvesNonExecution(result.ok, result.replacementReason)) {
          mainnetLock = markMainnetAmbiguous(mainnetLock);
          persistMainnetLock();
          err = `The ${original.kind} transaction was replaced by a different transaction. It remains locked; do not repeat it until backend state is reconciled.`;
        } else if (clearMainnetLock()) {
          // Only a cryptographically finalized non-execution reaches here.
          // Clearing exposes a later explicit button; it never retries itself.
          err = result.replacementReason === "cancelled"
            ? `The ${original.kind} transaction was cancelled. Its safety lock is cleared after the receipt resolved.`
            : `The ${original.kind} transaction reverted. Its safety lock is cleared after the receipt resolved.`;
        }
      } else {
        mainnetLock = withMainnetReceiptSuccess(mainnetLock);
        persistMainnetLock();
        await refresh();
      }
    } catch (e: any) {
      err = `Transaction receipt check paused: ${e?.shortMessage ?? e?.message ?? e}. The production action remains locked.`;
      setTimeout(() => {
        mainnetReceiptAttempts.delete(originalHash);
        if (mainnetLock?.txHash === originalHash) void observeMainnetTransaction();
      }, 10_000);
    } finally {
      receiptWatching = null;
    }
  }

  async function refresh() {
    if (!wallet) return;
    try {
      if (IS_PRODUCTION_PUBLIC) await refreshPublicStatus();
      const be = await backend();
      const nextVaults = await completeInventory(be);
      vaults = nextVaults;
      cfx = await cfxBalance(wallet.address);
      icusd = await icusdBalance(wallet.address);
      if (IS_PRODUCTION_CANARY) productionInventoryVerified = true;
      reconcileCanary(nextVaults);
      await reconcileMainnetLock(nextVaults);
      if (mainnetLock?.txHash) void observeMainnetTransaction();
    } catch (e: any) { err = `Refresh failed: ${e?.message ?? e}`; }
  }

  async function completeInventory(be: Awaited<ReturnType<typeof backend>>): Promise<ChainVault[]> {
    try {
      const complete = await listCompleteChainVaultInventory(be, CHAIN_ID);
      inventoryComplete = true;
      return complete;
    } catch (e) {
      inventoryComplete = false;
      throw e;
    }
  }

  function canConnect(): boolean {
    return (!IS_MAINNET || productionAcknowledged) && (!IS_PRODUCTION_PUBLIC || publicOriginBlocker === null);
  }

  async function connectWith(detail: EIP6963ProviderDetail) {
    reset();
    if (!canConnect()) { err = publicOriginBlocker ?? "Acknowledge the production warning before connecting."; return; }
    busy = `Connecting ${detail.info.name}…`;
    try {
      wallet = await connectInjected(detail);
      loadCanary(wallet.address);
      loadMainnetLock(wallet.address);
      walletChainValid = (await walletChainId(wallet)) === CHAIN_ID;
      walletAddressValid = await walletStillControlsAddress(wallet);
      await refresh();
    }
    catch (e: any) { err = e?.message ?? String(e); }
    finally { busy = null; }
  }
  async function connectLegacy() {
    reset();
    if (!canConnect()) { err = publicOriginBlocker ?? "Acknowledge the production warning before connecting."; return; }
    busy = "Connecting…";
    try {
      wallet = await connectLegacyInjected();
      loadCanary(wallet.address);
      loadMainnetLock(wallet.address);
      walletChainValid = (await walletChainId(wallet)) === CHAIN_ID;
      walletAddressValid = await walletStillControlsAddress(wallet);
      await refresh();
    }
    catch (e: any) { err = e?.message ?? String(e); }
    finally { busy = null; }
  }
  async function connectDev() {
    if (!__ENABLE_DEV_KEY__) return;
    reset(); busy = "Loading dev key…";
    try { wallet = connectDevKey(devKey.trim()); await refresh(); }
    catch (e: any) { err = `Bad key: ${e?.message ?? e}`; }
    finally { busy = null; }
  }
  function disconnect() {
    if (busy) { err = "Wait for the active wallet/backend action to finish before disconnecting."; return; }
    wallet = null;
    vaults = [];
    canary = null;
    mainnetLock = null;
    inventoryComplete = false;
    walletChainValid = false;
    walletAddressValid = false;
    receiptWatching = null;
    receiptAttempts.clear();
    mainnetReceiptAttempts.clear();
    recoveryAcknowledged = false;
    productionInventoryVerified = false;
    reset();
  }

  /** Resolve the authoritative nonce immediately before each signature. Testnet
   * alone retains one convenience retry; a mainnet click can create at most one
   * wallet prompt. */
  async function submit(
    action: number, vaultId: bigint, collateralWei: bigint, debt: bigint,
    call: (be: Awaited<ReturnType<typeof backend>>, i: any, sig: Uint8Array) => Promise<{ Ok?: unknown; Err?: any }>,
    onNonce?: (nonce: bigint) => void,
  ): Promise<{ Ok?: unknown; Err?: any }> {
    const be = await backend();
    const w = wallet!;
    const attempts = signatureAttemptLimit(IS_MAINNET);
    for (let attempt = 0; attempt < attempts; attempt++) {
      const nonceResult = await be.get_expected_evm_nonce(CHAIN_ID, w.address);
      if ("Err" in nonceResult) return nonceResult;
      const nonce = BigInt(nonceResult.Ok);
      onNonce?.(nonce);
      const input: VaultIntentInput = {
        action, owner: w.address, vaultId, collateralWei, debtE8s: debt,
        nonce, deadlineSecs: BigInt(Math.floor(Date.now() / 1000) + 3600),
      };
      // toCandidIntent forces recipient to the same normalized owner address.
      const sig = await signIntent(w.client, w.account as any, input);
      const res = await call(be, toCandidIntent(input), sig);
      if ("Ok" in res) return res;
      const msg = errText(res.Err);
      const m = msg.match(/expected (\d+)/);
      if (m) {
        if (attempt === 0 && !IS_MAINNET) continue;
        return { Err: { EvmAuth: "Nonce changed during submission. Refresh, review the intent, and click again to sign." } };
      }
      return res;
    }
    return { Err: { GenericError: "nonce sync failed" } };
  }

  async function doOpenUnlocked() {
    reset();
    await assertPublicWriteReady("open");
    if (productionLifecycleUsed) {
      err = "This production-canary build is permanently limited to one lifecycle for this wallet.";
      return;
    }
    if (IS_PRODUCTION_CANARY && !productionInventoryVerified) {
      err = "Production vault inventory must refresh successfully before Open is available.";
      return;
    }
    if (IS_PRODUCTION_CANARY && !canPersistCanary()) {
      err = "Production canary requires browser storage so action locks survive reloads.";
      return;
    }
    const minimumDebt = IS_PRODUCTION_PUBLIC
      ? (publicStatus?.effective_debt_config[0]?.min_vault_debt_e8s ?? 0n)
      : MIN_DEBT_E8S;
    if (openTerms.debtE8s < minimumDebt) { err = `Minimum debt is ${fmtIcusd(minimumDebt)} icUSD`; return; }
    if (openTerms.collateralWei <= 0n) { err = "Enter a debt and CFX price"; return; }
    if (IS_PRODUCTION_CANARY &&
        (openTerms.collateralWei !== CANARY_COLLATERAL_WEI || openTerms.debtE8s !== CANARY_DEBT_E8S)) {
      err = "Production canary terms are not the exact 5 CFX / 0.10 icUSD envelope.";
      return;
    }
    let openLock: CanaryRecord | null = null;
    let publicOpenLock: MainnetActionLock | null = null;
    try {
      if (IS_PRODUCTION_CANARY) {
        busy = "Verifying the one-lifecycle inventory…";
        const currentVaults = await completeInventory(await backend());
        vaults = currentVaults;
        if (currentVaults.some((v) => v.owner_evm[0]?.toLowerCase() === wallet!.address.toLowerCase())) {
          throw new Error("This wallet already has a chain-1030 vault. A second production lifecycle is refused.");
        }
        if (canary) throw new Error("This wallet already has a persisted production lifecycle. A second Open is refused.");
        openLock = newCanaryOpenLock(wallet!.address);
        canary = openLock;
        if (!persistCanary()) {
          canary = null;
          openLock = null;
          throw new Error("Could not persist the Open safety lock. No signature was requested.");
        }
      }
      if (IS_PRODUCTION_PUBLIC) {
        busy = "Refreshing complete vault inventory…";
        vaults = await completeInventory(await backend());
        await assertPublicWriteReady("open");
      }
      publicOpenLock = beginMainnetLock("open", undefined, openTerms.debtE8s, openTerms.collateralWei);
      busy = "Sign the Open intent in your wallet…";
      const res = await submit(ACTION.Open, 0n, openTerms.collateralWei, openTerms.debtE8s,
        (be, i, sig) => be.open_chain_vault_evm(i, sig), recordMainnetNonce);
      if ("Ok" in res) {
        if (IS_PRODUCTION_PUBLIC && mainnetLock) {
          mainnetLock = withMainnetVaultId(mainnetLock, BigInt(res.Ok as bigint));
          mainnetLock = markMainnetSubmitted(mainnetLock);
          if (!persistMainnetLock()) throw new Error("Vault opened, but its exact id could not be persisted. Do not sign Open again.");
          publicOpenLock = null;
        }
        if (IS_PRODUCTION_CANARY) {
          canary = newCanaryRecord(wallet!.address, BigInt(res.Ok as bigint));
          if (!persistCanary()) {
            // The already-persisted Open lock stays terminal on reload.
            throw new Error("Vault opened, but its id could not be persisted. Do not sign Open again; refresh to recover it from inventory.");
          }
          openLock = null;
        }
        ok = `Vault #${res.Ok} opened — verify the returned custody address, then deposit.`;
        await refresh();
      }
      else {
        if (openLock && !clearOpenLock(openLock)) return;
        if (publicOpenLock || (IS_PRODUCTION_PUBLIC && mainnetLock)) clearMainnetLock();
        err = errText(res.Err);
      }
    } catch (e: any) {
      const explicitRejection = isExplicitWalletRejection(e);
      const rejectionLockCleared = openLock && explicitRejection ? clearOpenLock(openLock) : true;
      if (IS_PRODUCTION_PUBLIC && mainnetLock) {
        if (explicitRejection) restoreExplicitlyRejectedMainnet(publicOpenLock ?? mainnetLock);
        else {
          mainnetLock = markMainnetAmbiguous(mainnetLock);
          persistMainnetLock();
        }
      }
      err = explicitRejection
        ? (rejectionLockCleared ? (e?.shortMessage ?? e?.message ?? "Wallet signature rejected.") : err)
        : (openLock
          ? `Open result is ambiguous, so its persisted safety lock remains. Do not sign Open again; refresh to recover backend state. ${e?.shortMessage ?? e?.message ?? e}`
          : (publicOpenLock
            ? `Open result is ambiguous, so its production safety lock remains. Do not sign Open again; refresh to reconcile backend state. ${e?.shortMessage ?? e?.message ?? e}`
            : (e?.message ?? String(e))));
    }
    finally { busy = null; }
  }

  async function doOpen() {
    reset();
    try { await withCanaryExclusivity(doOpenUnlocked); }
    catch (e: any) { err = e?.message ?? String(e); busy = null; }
  }

  async function latestVault(vaultId: bigint): Promise<ChainVault> {
    const found = await (await backend()).get_chain_vault(vaultId);
    if (!found.length) throw new Error(`Vault #${vaultId} no longer exists.`);
    const current = found[0]!;
    vaults = vaults.map((v) => v.vault_id === vaultId ? current : v);
    return current;
  }

  async function clearUnresolvedAuthorizationUnlocked() {
    if (!wallet || !canary || !unresolvedAuthorization || !recoveryAcknowledged) {
      throw new Error("Confirm the unresolved-action recovery statement first.");
    }
    busy = "Rechecking backend state before recovery…";
    const currentVaults = await completeInventory(await backend());
    vaults = currentVaults;
    reconcileCanary(currentVaults);
    if (!canary || !unresolvedAuthorization) {
      ok = "Backend state resolved the action; the safety lock advanced without retrying it.";
      recoveryAcknowledged = false;
      return;
    }
    if (canary.phase === "open-authorizing") {
      if (currentVaults.some((v) => v.owner_evm[0]?.toLowerCase() === wallet!.address.toLowerCase())) {
        throw new Error("An owned vault exists, so the Open lock cannot be cleared.");
      }
      const lock = canary;
      if (!clearOpenLock(lock)) throw new Error("The Open safety lock could not be cleared.");
    } else {
      const vault = currentVaults.find((v) => v.vault_id.toString() === canary!.vaultId);
      if (!vault) throw new Error("The tracked vault is missing; keep the lock and investigate.");
      const target = manualRecoveryTarget(canary);
      if (!target) throw new Error("This lifecycle phase has no manual recovery path.");
      const refusal = validateCanaryAction({ ...canary, phase: target.phase }, snapshot(vault), target.action);
      if (refusal) throw new Error(`Backend state does not permit clearing this lock: ${refusal}`);
      canary = { ...canary, phase: target.phase };
      if (!persistCanary()) throw new Error("The recovered lifecycle state could not be persisted.");
    }
    recoveryAcknowledged = false;
    ok = "Unresolved authorization lock cleared after your no-authorization confirmation and a fresh backend check.";
  }

  async function clearUnresolvedAuthorization() {
    reset();
    try { await withCanaryExclusivity(clearUnresolvedAuthorizationUnlocked); }
    catch (e: any) { err = e?.message ?? String(e); }
    finally { busy = null; }
  }

  async function clearMainnetAuthorizationUnlocked() {
    if (!wallet || !mainnetLock || !mainnetRecoveryAcknowledged) {
      throw new Error("Confirm the unresolved-action recovery statement first.");
    }
    busy = "Rechecking production state before recovery…";
    const be = await backend();
    const currentVaults = await completeInventory(be);
    vaults = currentVaults;
    const locked = mainnetLock;
    await reconcileMainnetLock(currentVaults);
    if (!mainnetLock) {
      ok = "Fresh backend state resolved the action; no retry was performed.";
      mainnetRecoveryAcknowledged = false;
      return;
    }
    if (locked.txHash) {
      throw new Error("A transaction hash exists. Keep the lock until its receipt and backend state resolve; do not clear it manually.");
    }
    if (isSignedMainnetAction(locked.kind) && locked.nonce !== null) {
      const nonceResult = await be.get_expected_evm_nonce(CHAIN_ID, wallet.address);
      if ("Err" in nonceResult) throw new Error(`Nonce reconciliation failed: ${errText(nonceResult.Err)}`);
      if (BigInt(nonceResult.Ok) !== BigInt(locked.nonce)) {
        throw new Error("The backend nonce advanced, so the signed action may have been accepted. Keep the lock and investigate.");
      }
    }
    if (mainnetActionObserved(locked, currentVaults.map(mainnetSnapshot))) {
      throw new Error("Backend state indicates the action occurred. Keep the lock and refresh.");
    }
    if (!clearMainnetLock()) throw new Error("The production action lock could not be cleared.");
    mainnetRecoveryAcknowledged = false;
    ok = "Unresolved production authorization cleared after a fresh backend check and your confirmation that no wallet action occurred.";
  }

  async function clearMainnetAuthorization() {
    reset();
    try { await withCanaryExclusivity(clearMainnetAuthorizationUnlocked); }
    catch (e: any) { err = e?.message ?? String(e); }
    finally { busy = null; }
  }

  // Actions invoked only by explicit buttons in VaultCard.
  async function onActionUnlocked(kind: string, vault: ChainVault, amountE8s?: bigint) {
    reset();
    const w = wallet!;
    try {
      const gateKind = kind === "repay" ? "burn" : kind as MainnetActionKind;
      let current = vault;
      if (kind === "deposit") {
        let publicLock: MainnetActionLock | null = null;
        let previous: CanaryRecord | null = null;
        let hash: `0x${string}`;
        try {
          hash = await sendFreshDepositAfterPreflight(
            async () => {
              if (IS_MAINNET) {
                busy = IS_PRODUCTION_PUBLIC ? "Refreshing complete inventory and exact vault state…" : "Refreshing exact vault state…";
                if (IS_PRODUCTION_PUBLIC) {
                  vaults = await completeInventory(await backend());
                  await assertPublicWriteReady(gateKind);
                }
              }
            },
            () => IS_MAINNET ? latestVault(vault.vault_id) : Promise.resolve(vault),
            (fresh) => variantName(fresh.status),
            (fresh) => {
              current = fresh;
              if (IS_PRODUCTION_PUBLIC && fresh.owner_evm[0]?.toLowerCase() !== w.address.toLowerCase()) {
                throw new Error("The freshly loaded vault is not owned by the connected wallet.");
              }
              if (IS_PRODUCTION_CANARY) {
                const refusal = validateCanaryAction(canary, snapshot(fresh), "deposit");
                if (refusal) throw new Error(`Refusing deposit: ${refusal}`);
              }
              busy = `Confirm the ${fmtCfx(fresh.collateral_amount_e18)} CFX deposit in your wallet…`;
            },
            (fresh) => {
              publicLock = beginMainnetLock("deposit", fresh, fresh.pending_mint_e8s);
              previous = IS_PRODUCTION_CANARY ? persistActionLock("deposit-authorizing") : null;
            },
            (fresh) => sendDeposit(w, fresh.custody_address as `0x${string}`, fresh.collateral_amount_e18),
          );
        } catch (e) {
          if (IS_PRODUCTION_CANARY && previous && isExplicitWalletRejection(e)) {
            restoreCanaryLock(previous, "deposit-authorizing");
          }
          if (IS_PRODUCTION_CANARY && previous && !isExplicitWalletRejection(e)) {
            throw new Error("Deposit provider result is ambiguous. The pre-transaction lock remains; do not repeat the 5 CFX transfer. Refresh and wait for backend observation.");
          }
          if (IS_PRODUCTION_PUBLIC && publicLock) {
            if (isExplicitWalletRejection(e)) restoreExplicitlyRejectedMainnet(publicLock);
            else {
              mainnetLock = markMainnetAmbiguous(mainnetLock!);
              persistMainnetLock();
              throw new Error("Deposit provider result is ambiguous. The production lock remains; do not repeat the transfer. Refresh and wait for receipt/backend reconciliation.");
            }
          }
          throw e;
        }
        if (IS_PRODUCTION_CANARY && canary) {
          canary = recordTransaction(canary, "deposit-submitted", "deposit", hash);
          persistCanary();
          void observePendingTransaction();
        }
        if (IS_PRODUCTION_PUBLIC && mainnetLock) {
          mainnetLock = withMainnetTransaction(mainnetLock, hash);
          persistMainnetLock();
          void observeMainnetTransaction();
        }
        ok = "Deposit submitted — waiting for the observer to report the vault Open.";
      } else {
        if (IS_PRODUCTION_PUBLIC) {
          busy = "Refreshing complete inventory and exact vault state…";
          vaults = await completeInventory(await backend());
          await assertPublicWriteReady(gateKind);
        }
        if (IS_MAINNET) {
          if (!IS_PRODUCTION_PUBLIC) busy = "Refreshing exact vault state…";
          current = await latestVault(vault.vault_id);
          if (IS_PRODUCTION_PUBLIC && current.owner_evm[0]?.toLowerCase() !== w.address.toLowerCase()) {
            throw new Error("The freshly loaded vault is not owned by the connected wallet.");
          }
        }
      }
      if (kind === "repay") {
        const amt = IS_PRODUCTION_CANARY ? CANARY_DEBT_E8S : (amountE8s ?? current.debt_e8s);
        if (amt <= 0n || amt > current.debt_e8s) throw new Error("Repay must be greater than zero and no more than the freshly observed vault debt.");
        if (IS_PRODUCTION_CANARY) {
          const refusal = validateCanaryAction(canary, snapshot(current), "burn");
          if (refusal || amt !== CANARY_DEBT_E8S) {
            throw new Error(`Refusing burn: ${refusal ?? "amount must be exactly 0.10 icUSD."}`);
          }
        }
        const publicLock = beginMainnetLock("burn", current, amt);
        const previous = IS_PRODUCTION_CANARY ? persistActionLock("burn-authorizing") : null;
        busy = `Confirm the ${fmtIcusd(amt)} icUSD burn in your wallet…`;
        let hash: `0x${string}`;
        try {
          hash = await burnIcusd(w, amt, current.vault_id);
        } catch (e) {
          if (IS_PRODUCTION_CANARY && previous && isExplicitWalletRejection(e)) {
            restoreCanaryLock(previous, "burn-authorizing");
          }
          if (IS_PRODUCTION_CANARY && !isExplicitWalletRejection(e)) {
            throw new Error("Burn provider result is ambiguous. The pre-transaction lock remains; do not repeat the 0.10 icUSD burn. Refresh and wait for backend observation.");
          }
          if (IS_PRODUCTION_PUBLIC && publicLock) {
            if (isExplicitWalletRejection(e)) restoreExplicitlyRejectedMainnet(publicLock);
            else {
              mainnetLock = markMainnetAmbiguous(mainnetLock!);
              persistMainnetLock();
              throw new Error("Burn provider result is ambiguous. The production lock remains; do not repeat the burn. Refresh and wait for receipt/backend reconciliation.");
            }
          }
          throw e;
        }
        if (IS_PRODUCTION_CANARY && canary) {
          canary = recordTransaction(canary, "burn-submitted", "burn", hash);
          persistCanary();
          void observePendingTransaction();
        }
        if (IS_PRODUCTION_PUBLIC && mainnetLock) {
          mainnetLock = withMainnetTransaction(mainnetLock, hash);
          persistMainnetLock();
          void observeMainnetTransaction();
        }
        ok = `Burn submitted — waiting for the observer to report zero debt.`;
      } else if (kind === "borrow") {
        if (IS_PRODUCTION_CANARY) throw new Error("Borrow-more is disabled in production-canary mode.");
        const borrowAmount = amountE8s ?? 0n;
        if (borrowAmount <= 0n) throw new Error("Borrow amount must be greater than zero.");
        const publicLock = beginMainnetLock("borrow", current, amountE8s ?? 0n);
        busy = "Sign the Borrow intent…";
        try {
          const res = await submit(ACTION.Borrow, current.vault_id, 0n, amountE8s ?? 0n,
            (be, i, sig) => be.borrow_chain_vault_evm(i, sig), recordMainnetNonce);
          if ("Ok" in res) {
            if (IS_PRODUCTION_PUBLIC && mainnetLock) { mainnetLock = markMainnetSubmitted(mainnetLock); persistMainnetLock(); }
            ok = "Borrow signed — the mint will land shortly.";
          } else {
            if (publicLock) clearMainnetLock();
            err = errText(res.Err);
          }
        } catch (e) {
          if (publicLock && isExplicitWalletRejection(e)) restoreExplicitlyRejectedMainnet(publicLock);
          else if (publicLock && mainnetLock) { mainnetLock = markMainnetAmbiguous(mainnetLock); persistMainnetLock(); }
          throw e;
        }
      } else if (kind === "withdraw") {
        if (IS_PRODUCTION_CANARY) throw new Error("Partial withdrawal is disabled in production-canary mode.");
        if ((amountE8s ?? 0n) <= 0n || (amountE8s ?? 0n) > current.collateral_amount_e18) {
          throw new Error("Withdrawal must be greater than zero and no more than the freshly observed collateral.");
        }
        const publicLock = beginMainnetLock("withdraw", current, amountE8s ?? 0n);
        busy = "Sign the Withdraw intent…";
        try {
          const res = await submit(ACTION.WithdrawCollateral, current.vault_id, amountE8s ?? 0n, 0n,
            (be, i, sig) => be.withdraw_chain_collateral_evm(i, sig), recordMainnetNonce);
          if ("Ok" in res) {
            if (IS_PRODUCTION_PUBLIC && mainnetLock) { mainnetLock = markMainnetSubmitted(mainnetLock); persistMainnetLock(); }
            ok = "Withdraw signed.";
          } else {
            if (publicLock) clearMainnetLock();
            err = errText(res.Err);
          }
        } catch (e) {
          if (publicLock && isExplicitWalletRejection(e)) restoreExplicitlyRejectedMainnet(publicLock);
          else if (publicLock && mainnetLock) { mainnetLock = markMainnetAmbiguous(mainnetLock); persistMainnetLock(); }
          throw e;
        }
      } else if (kind === "close") {
        if (IS_PRODUCTION_CANARY) {
          const refusal = validateCanaryAction(canary, snapshot(current), "close");
          if (refusal) throw new Error(`Refusing close: ${refusal}`);
        } else if (current.debt_e8s !== 0n) {
          throw new Error("Refusing close until the observer reports zero debt.");
        }
        const publicLock = beginMainnetLock("close", current);
        const previous = IS_PRODUCTION_CANARY ? persistActionLock("close-authorizing") : null;
        busy = "Sign the Close intent…";
        let res: { Ok?: unknown; Err?: any };
        try {
          res = await submit(ACTION.Close, current.vault_id, 0n, 0n,
            (be, i, sig) => be.close_chain_vault_evm(i, sig), recordMainnetNonce);
        } catch (e) {
          if (IS_PRODUCTION_CANARY && previous && isExplicitWalletRejection(e)) {
            restoreCanaryLock(previous, "close-authorizing");
          }
          if (IS_PRODUCTION_CANARY && !isExplicitWalletRejection(e)) {
            throw new Error("Close result is ambiguous. The pre-signature lock remains; do not submit another Close. Refresh and wait for backend observation.");
          }
          if (IS_PRODUCTION_PUBLIC && publicLock) {
            if (isExplicitWalletRejection(e)) restoreExplicitlyRejectedMainnet(publicLock);
            else if (mainnetLock) { mainnetLock = markMainnetAmbiguous(mainnetLock); persistMainnetLock(); }
          }
          throw e;
        }
        if ("Ok" in res) {
          if (IS_PRODUCTION_CANARY) setCanaryPhase("close-submitted");
          if (IS_PRODUCTION_PUBLIC && mainnetLock) { mainnetLock = markMainnetSubmitted(mainnetLock); persistMainnetLock(); }
          ok = "Close submitted — waiting for the backend to report Closed.";
        } else {
          if (IS_PRODUCTION_CANARY && previous) restoreCanaryLock(previous, "close-authorizing");
          if (publicLock) clearMainnetLock();
          err = errText(res.Err);
        }
      }
      await refresh();
    } catch (e: any) { err = e?.message ?? String(e); }
    finally { busy = null; }
  }

  async function onAction(kind: string, vault: ChainVault, amountE8s?: bigint) {
    reset();
    try { await withCanaryExclusivity(() => onActionUnlocked(kind, vault, amountE8s)); }
    catch (e: any) { err = e?.message ?? String(e); busy = null; }
  }

  // Read-only polling observes backend state; it never signs or broadcasts.
  $effect(() => {
    if (!wallet || (!pendingExists && !canaryPolling)) return;
    const id = setInterval(refresh, 8000);
    return () => clearInterval(id);
  });

  $effect(() => {
    receiptRetryGeneration;
    if (wallet && canary && pendingTransaction(canary)) void observePendingTransaction();
  });

  $effect(() => {
    if (!IS_PRODUCTION_PUBLIC) return;
    void refreshPublicStatus();
    const id = setInterval(refreshPublicStatus, 10_000);
    return () => clearInterval(id);
  });

  $effect(() => {
    if (!wallet || !IS_PRODUCTION_PUBLIC || !mainnetLock) return;
    if (mainnetLock.txHash) void observeMainnetTransaction();
    const id = setInterval(refresh, 8_000);
    return () => clearInterval(id);
  });
</script>

<div class="wrap" class:production={IS_MAINNET}>
  <header class="top">
    <div class="brand">
      <div class="logo">R</div>
      <div>
        <h1>icUSD on Conflux eSpace</h1>
        <div class="sub">Self-serve CDP · sign with your EVM wallet</div>
      </div>
    </div>
    <span class="badge" class:testnet={!IS_MAINNET} class:mainnet={IS_MAINNET}>
      {IS_MAINNET ? `PRODUCTION · chain ${CHAIN_ID}` : "eSpace testnet · chain 71 · staging"}
    </span>
  </header>

  {#if IS_PRODUCTION_CANARY}
    <section class="production-warning" aria-label="Production warning">
      <strong>REAL FUNDS · PRODUCTION CANARY</strong>
      <p>This private build targets Conflux eSpace mainnet and the production Rumi backend. It is locked to one lifecycle: declare 5 CFX, mint 0.10 icUSD, burn exactly 0.10 icUSD, then close. Every signature and transaction requires your wallet confirmation.</p>
      {#if !wallet}
        <label class="ack"><input type="checkbox" bind:checked={productionAcknowledged} /> I understand this uses real CFX on mainnet.</label>
      {/if}
    </section>
    <section class="card canary-steps">
      <h2>Guided canary</h2>
      <ol>
        <li>Sign Open: <b>5 CFX declared collateral · 0.10 icUSD debt</b>.</li>
        <li>Verify the returned custody address, then confirm the 5 CFX deposit.</li>
        <li>Wait here until the observer reports <b>Open</b> and 0.10 icUSD arrives.</li>
        <li>Confirm an exact 0.10 icUSD burn; wait for zero debt.</li>
        <li>Sign Close; wait until the vault reports <b>Closed</b>.</li>
      </ol>
    </section>
  {:else if IS_PRODUCTION_PUBLIC}
    <section class="production-warning" aria-label="Production warning">
      <strong>REAL FUNDS · CONFLUX MAINNET</strong>
      <p>This public build uses the production Rumi backend and production icUSD contract. Every signature and transaction requires a separate click and wallet confirmation. Live backend readiness can pause all new writes at any time.</p>
      {#if !wallet}
        <label class="ack"><input type="checkbox" bind:checked={productionAcknowledged} /> I understand this uses real CFX and icUSD on mainnet.</label>
      {/if}
      {#if publicOriginBlocker && PUBLIC_CANONICAL_ORIGIN}
        <div class="notice err canonical-origin-block">
          Wallet connection and every write are blocked on this origin.
          <a href={PUBLIC_CANONICAL_ORIGIN}>Open the canonical production site</a>.
        </div>
      {/if}
    </section>
  {/if}

  {#if IS_PRODUCTION_PUBLIC}
    <section class="card launch-status" aria-label="Public launch status">
      <div class="row spread">
        <h2>Live production status</h2>
        <span class="pill" class:Open={publicLaunchReady} class:AwaitingDeposit={!publicLaunchReady}>
          {publicLaunchReady ? "OPEN FOR WRITES" : "WRITES PAUSED"}
        </span>
      </div>
      {#if publicStatus}
        <p class="hint">Read directly from backend <span class="mono">{BACKEND_CANISTER_ID}</span>. RPC provider count and agreement are structural configuration only; this page does not claim the providers are independently operated.</p>
        <div class="status-grid">
          <div class="kv"><span class="k">Chain / native asset</span><span class="v">{publicStatus.chain_id} / {publicStatus.native_symbol[0] ?? "Unavailable"}</span></div>
          <div class="kv"><span class="k">Chain state</span><span class="v">{variantName(publicStatus.status[0])}</span></div>
          <div class="kv"><span class="k">Configured / registered</span><span class="v">{publicStatus.configured ? "Yes" : "No"} / {publicStatus.registered ? "Yes" : "No"}</span></div>
          <div class="kv"><span class="k">icUSD binding / exact match</span><span class="v mono">{publicStatus.bound_icusd_contract[0] ?? "Missing"} / {publicStatus.icusd_contract_matches_expected ? "Yes" : "No"}</span></div>
          <div class="kv"><span class="k">Minimum debt</span><span class="v">{publicStatus.effective_debt_config[0] ? fmtIcusd(publicStatus.effective_debt_config[0].min_vault_debt_e8s) : "Unavailable"} icUSD</span></div>
          <div class="kv"><span class="k">Debt ceiling</span><span class="v">{publicStatus.effective_debt_config[0] ? optIcusd(publicStatus.effective_debt_config[0].debt_ceiling_e8s) : "Unavailable"}</span></div>
          <div class="kv"><span class="k">Minimum / liquidation CR</span><span class="v">{liveMinCr === null ? "Unavailable" : `${(liveMinCr * 100).toFixed(0)}%`} / {liveLiquidationCr === null ? "Unavailable" : `${(liveLiquidationCr * 100).toFixed(0)}%`}</span></div>
          <div class="kv"><span class="k">Collateral / debt config exact</span><span class="v">{publicStatus.collateral_config_matches_expected ? "Yes" : "No"} / {publicStatus.debt_config_matches_expected ? "Yes" : "No"}</span></div>
          <div class="kv"><span class="k">CFX price / age</span><span class="v">{liveCfxPrice === null ? "Unavailable" : `$${liveCfxPrice.toLocaleString(undefined, { maximumFractionDigits: 8 })}`} / {ageText(publicStatus.collateral_price_age_ns)}</span></div>
          <div class="kv"><span class="k">Liquidation configured / enabled / price</span><span class="v">{publicStatus.liquidation_configured ? "Yes" : "No"} / {publicStatus.liquidation_enabled ? "Enabled" : "Disabled"} / {publicStatus.collateral_price_is_fresh ? "Fresh" : "Not fresh"}</span></div>
          <div class="kv"><span class="k">Liquidation config / digest exact</span><span class="v">{publicStatus.liquidation_config_matches_expected ? "Yes" : "No"} / {liveLiquidationDigestMatches ? "Yes" : "No"}</span></div>
          <div class="kv"><span class="k">RPC endpoints / floor / effective agreement</span><span class="v">{publicStatus.rpc_endpoint_count} / {publicStatus.rpc_min_quorum_providers} / {publicStatus.rpc_effective_agreement_requirement}</span></div>
          <div class="kv"><span class="k">RPC configuration</span><span class="v">{publicStatus.rpc_configuration_sufficient ? "Sufficient" : "Insufficient"}</span></div>
          <div class="kv"><span class="k">Effective EVM RPC canister / exact</span><span class="v mono">{publicStatus.effective_evm_rpc_principal.toText()} / {publicStatus.evm_rpc_principal_matches_expected ? "Yes" : "No"}</span></div>
          <div class="kv"><span class="k">Chain-signing key / exact</span><span class="v mono">{publicStatus.chains_ecdsa_key_name || "Unavailable"} / {publicStatus.chains_ecdsa_key_matches_expected ? "Yes" : "No"}</span></div>
          <div class="kv"><span class="k">Finality depth / burn cursor</span><span class="v">{publicStatus.finality_depth[0] ?? "Unavailable"} blocks / {publicStatus.burn_cursor}</span></div>
          <div class="kv"><span class="k">Supply / backing / pending burns</span><span class="v">{fmtIcusd(publicStatus.chain_supply_e8s)} / {fmtIcusd(publicStatus.chain_reserve_backing_e8s)} / {fmtIcusd(publicStatus.chain_pending_burn_e8s)} icUSD</span></div>
          <div class="kv"><span class="k">Supply / reorg / bad-debt breakers</span><span class="v">{yesNo(!publicStatus.invariant_halted)} / {yesNo(!publicStatus.reorg_halted)} / {yesNo(!publicStatus.bad_debt_circuit_tripped)}</span></div>
          <div class="kv"><span class="k">Bad debt / circuit threshold</span><span class="v">{fmtIcusd(publicStatus.bad_debt_e8s)} / {publicStatus.bad_debt_threshold_e8s.length ? fmtIcusd(publicStatus.bad_debt_threshold_e8s[0]) : "Unavailable"} icUSD</span></div>
          <div class="kv"><span class="k">Protocol mode / frozen</span><span class="v">{variantName(publicStatus.protocol_mode)} / {publicStatus.protocol_frozen ? "Frozen" : "Not frozen"}</span></div>
          <div class="kv"><span class="k">Hot wallet / minimum / ready</span><span class="v">{publicStatus.hot_wallet_balance_e18.length ? fmtCfx(publicStatus.hot_wallet_balance_e18[0]) : "Unavailable"} / {fmtCfx(publicStatus.hot_wallet_min_balance_e18)} CFX / {publicStatus.hot_wallet_ready.length ? (publicStatus.hot_wallet_ready[0] ? "Yes" : "No") : "Unavailable"}</span></div>
          <div class="kv"><span class="k">Hot-wallet balance refresh</span><span class="v">{timestampText(publicStatus.hot_wallet_balance_refreshed_at_ns)}</span></div>
          <div class="kv"><span class="k">Hot-wallet observation age / maximum / fresh</span><span class="v">{ageText(publicStatus.hot_wallet_balance_age_ns)} / {ageText([publicStatus.hot_wallet_balance_max_age_ns])} / {publicStatus.hot_wallet_balance_is_fresh ? "Fresh" : "Not fresh"}</span></div>
        </div>
        <p class="hint status-disclaimer">These are bounded backend readiness facts. External EVM <span class="mono">totalSupply()</span> and operator reconciliation are separate launch evidence.</p>
        {#if publicLaunchRefusal}
          <div class="notice err">
            <strong>New writes are paused:</strong>
            {#if publicOriginBlocker}
              <p>{publicOriginBlocker}</p>
            {:else if publicStatus.blocking_reasons.length}
              <ul>{#each publicStatus.blocking_reasons as reason}<li>{blockingReasonText(reason)}</li>{/each}</ul>
            {:else}
              <p>{publicLaunchRefusal}</p>
            {/if}
          </div>
        {:else}
          <div class="notice ok">All backend public-open readiness checks are clear.</div>
        {/if}
      {:else}
        <div class="notice err">Live backend readiness is unavailable. New writes are disabled. {publicStatusError ?? "Waiting for the first status response."}</div>
      {/if}
    </section>
  {/if}

  {#if !wallet}
    <div class="card">
      <h2>Connect</h2>
      <p class="hint">Open a CFX-collateralized icUSD vault by signing an EIP-712 intent — no IC login.
        Your wallet is the only identity; the canister verifies the signature.</p>
      {#if injectedWallets.length > 0}
        <div class="wallets">
          {#each injectedWallets as w (w.info.rdns)}
            <button class="wallet" onclick={() => connectWith(w)} disabled={!!busy || !canConnect()}>
              <img class="wicon" src={w.info.icon} alt="" />
              <span>Connect {w.info.name}</span>
            </button>
          {/each}
        </div>
      {:else if hasLegacyInjected()}
        <div class="row">
          <button class="primary" onclick={connectLegacy} disabled={!!busy || !canConnect()}>Connect wallet</button>
        </div>
      {:else}
        <p class="hint" style="margin:0 0 4px">No EVM wallet detected. Install
          <a href="https://rabby.io" target="_blank" rel="noreferrer">Rabby</a>
          (or another EVM wallet) and reload.</p>
      {/if}
      {#if __ENABLE_DEV_KEY__}
        <div class="row" style="margin-top:12px">
          <button class="ghost sm" onclick={() => (showDevKey = !showDevKey)}>{showDevKey ? "Hide" : "Use a dev key"}</button>
        </div>
        {#if showDevKey}
          <div class="divider"></div>
          <label for="devkey">Private key (testnet only — for the no-wallet demo path)</label>
          <div class="field"><div class="row">
            <input id="devkey" class="mono" bind:value={devKey} spellcheck="false" />
            <button onclick={connectDev} disabled={!!busy}>Load</button>
          </div></div>
          <p class="hint">Pre-filled with the scalar=1 demo key (<span class="mono">0x7e5f…95bdf</span>) — the same one the staging round-trip used.</p>
        {/if}
      {:else}
        <p class="hint injected-only">Injected wallets only. This build has no private-key input.</p>
      {/if}
    </div>
  {:else}
    <div class="card">
      <div class="row spread">
        <h2>Wallet</h2>
        <div class="row">
          <button class="ghost sm" onclick={refresh} disabled={!!busy}>Refresh</button>
          <button class="ghost sm" onclick={disconnect} disabled={!!busy}>Disconnect</button>
        </div>
      </div>
      <div class="kv"><span class="k">Address</span><span class="v mono">{wallet.address}</span></div>
      <div class="kv"><span class="k">CFX</span><span class="v">{fmtCfx(cfx)}</span></div>
      <div class="kv"><span class="k">icUSD</span><span class="v">{fmtIcusd(icusd)}</span></div>
      <div class="kv"><span class="k">Signer</span><span class="v">{wallet.walletName}</span></div>
    </div>

    <div class="card">
      <h2>Open a vault</h2>
      {#if IS_PRODUCTION_CANARY}
        <p class="hint">The signed Open intent is compile-time locked. Owner and recipient are both your connected wallet.</p>
        <div class="kv"><span class="k">Declared collateral</span><span class="v">5 CFX</span></div>
        <div class="kv"><span class="k">Debt / mint</span><span class="v">0.10 icUSD</span></div>
        <div class="kv"><span class="k">Recipient</span><span class="v">Same as owner</span></div>
      {:else}
        <p class="hint">Enter the icUSD you want to mint. Required CFX uses the {IS_PRODUCTION_PUBLIC ? (liveMinCr === null ? "live unavailable" : `${Math.round(liveMinCr * 100)}% live`) : `${Math.round(MIN_CR * 100)}%`} min-CR
          floor (+2% buffer) — the real CR check runs on the canister.</p>
        <div class="row">
          <div class="field" style="flex:1">
            <label for="debt">icUSD debt</label>
            <input id="debt" type="number" min="0.1" step="0.1" bind:value={debtInput} />
          </div>
          {#if IS_PRODUCTION_PUBLIC}
            <div class="field" style="flex:1">
              <span class="field-label">Live CFX price</span>
              <div class="readonly-field">{liveCfxPrice === null ? "Unavailable" : `$${liveCfxPrice.toLocaleString(undefined, { maximumFractionDigits: 8 })}`}</div>
            </div>
          {:else}
            <div class="field" style="flex:1">
              <label for="cfxprice">CFX price (USD, hint)</label>
              <input id="cfxprice" type="number" min="0" step="0.01" bind:value={cfxPrice} />
            </div>
          {/if}
        </div>
        <div class="kv"><span class="k">Required CFX (≈)</span><span class="v">{fmtCfx(openTerms.collateralWei)}</span></div>
      {/if}
      <div class="row" style="margin-top:14px">
        <button class="primary" onclick={doOpen} disabled={!!busy || !publicRiskWritesEnabled || productionLifecycleUsed || (IS_PRODUCTION_CANARY && !productionInventoryVerified)}>Sign & open</button>
      </div>
      {#if IS_PRODUCTION_CANARY && canary}
        <div class="notice info">Persisted lifecycle: <b>{canary.phase}</b>. Submitted actions remain locked across reloads until receipt/backend resolution.</div>
      {:else if IS_PRODUCTION_CANARY && owned.length > 0}
        <div class="notice err">This wallet already has a chain-1030 vault, but it was not opened by this browser's persisted canary record. Actions are read-only and a second lifecycle is disabled.</div>
      {/if}
      {#if IS_PRODUCTION_CANARY && unresolvedAuthorization}
        <div class="notice err">
          The action result is ambiguous. It stays locked because repeating it could move funds twice.
          First refresh and inspect the linked replacement transaction or wallet activity. Clear this lock only after confirming the intended transfer, burn, or signature did not occur.
          <label class="ack"><input type="checkbox" bind:checked={recoveryAcknowledged} /> I verified the intended action did not occur.</label>
          <button class="danger" disabled={!!busy || !recoveryAcknowledged} onclick={clearUnresolvedAuthorization}>Clear unresolved authorization lock</button>
        </div>
      {/if}
      {#if IS_PRODUCTION_PUBLIC && publicRiskWriteBlocker}
        <div class="notice err">Risk-increasing writes are unavailable: {publicRiskWriteBlocker}</div>
      {/if}
      {#if IS_PRODUCTION_PUBLIC && mainnetLock}
        <div class="notice info">
          Persisted production action: <b>{mainnetLock.kind}</b> ({mainnetLock.phase}). It remains locked across reloads until a receipt or fresh backend state resolves it.
          {#if mainnetLock.txHash}<br /><a href={txUrl(mainnetLock.txHash)} target="_blank" rel="noreferrer">View submitted transaction ↗</a>{/if}
        </div>
        {#if !mainnetLock.txHash}
          <div class="notice err">
            Only clear this authorization lock after checking your wallet activity and confirming that the intended action did not occur. A fresh backend nonce and vault-state check runs before clearing.
            <label class="ack"><input type="checkbox" bind:checked={mainnetRecoveryAcknowledged} /> I verified the intended wallet action did not occur.</label>
            <button class="danger" disabled={!!busy || !mainnetRecoveryAcknowledged} onclick={clearMainnetAuthorization}>Clear unresolved production lock</button>
          </div>
        {/if}
      {/if}
    </div>

    {#each owned as v (v.vault_id)}
      <VaultCard
        vault={v}
        busy={busy}
        productionCanary={IS_PRODUCTION_CANARY}
        productionPublic={IS_PRODUCTION_PUBLIC}
        riskWritesEnabled={publicRiskWritesEnabled}
        riskWriteDisabledReason={publicRiskWriteBlocker}
        recoveryWritesEnabled={publicRecoveryWritesEnabled}
        recoveryWriteDisabledReason={publicRecoveryWriteBlocker}
        isCanaryVault={canary?.vaultId === v.vault_id.toString()}
        canaryPhase={canary?.vaultId === v.vault_id.toString() ? canary.phase : null}
        {onAction}
      />
    {/each}
    {#if owned.length === 0}
      <div class="card"><p class="hint" style="margin:0">No vaults yet for this address. Open one above.</p></div>
    {/if}
  {/if}

  {#if busy}<div class="notice info"><span class="spin"></span>{busy}</div>{/if}
  {#if canaryPolling || receiptWatching || (IS_PRODUCTION_PUBLIC && mainnetLock)}<div class="notice info"><span class="spin"></span>Read-only receipt/status polling is active; no wallet action will happen automatically.</div>{/if}
  {#if err}<div class="notice err">{err}</div>{/if}
  {#if ok}<div class="notice ok">{ok}</div>{/if}

  {#if transactions.length > 0}
    <div class="card tx-list">
      <h2>Submitted transactions</h2>
      {#each transactions as tx (tx.hash)}
        <a href={tx.url} target="_blank" rel="noreferrer">{tx.label}: {tx.hash.slice(0, 12)}… ↗</a>
      {/each}
    </div>
  {/if}

  <div class="foot">
    Backend <span class="mono">{BACKEND_CANISTER_ID}</span> ·
    <a href={addressUrl(ICUSD_CONTRACT)} target="_blank" rel="noreferrer">IcUSD <span class="mono">{ICUSD_CONTRACT.slice(0, 10)}…</span> ↗</a><br />
    {#if IS_PRODUCTION_CANARY}
      <strong>Production canary · chain {CHAIN_ID}</strong> · <a href={ESPACE_EXPLORER} target="_blank" rel="noreferrer">ConfluxScan ↗</a>
    {:else if IS_PRODUCTION_PUBLIC}
      <strong>Production public · chain {CHAIN_ID}</strong> · <a href={ESPACE_EXPLORER} target="_blank" rel="noreferrer">ConfluxScan ↗</a>
    {:else}
      Testnet only. The chains rail is experimental — not on production.
    {/if}
  </div>
</div>
