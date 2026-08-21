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
    IS_PRODUCTION_CANARY,
    MIN_CR,
    MIN_DEBT_E8S,
    openTermsFor,
  } from "./config";
  import { backend, errText, statusName, type ChainVault } from "./backend";
  import {
    canaryStorageKey,
    applyFailedTransactionFinality,
    isRecoverableOpenCandidate,
    manualRecoveryTarget,
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
    connectDevKey,
    connectInjected,
    connectLegacyInjected,
    fmtCfx,
    fmtIcusd,
    getInjectedWallets,
    hasLegacyInjected,
    icusdBalance,
    isExplicitWalletRejection,
    parseEther,
    refreshInjectedWallets,
    sendDeposit,
    subscribeWallets,
    toE8s,
    txUrl,
    waitForTransactionFinality,
    type EIP6963ProviderDetail,
    type Wallet,
  } from "./evm";
  import VaultCard from "./VaultCard.svelte";

  let wallet = $state<Wallet | null>(null);
  let vaults = $state<ChainVault[]>([]);
  let cfx = $state(0n);
  let icusd = $state(0n);
  let nonce = $state(0n); // next per-owner nonce; auto-synced on a bad-nonce reject
  let productionAcknowledged = $state(false);
  let productionInventoryVerified = $state(false);

  let busy = $state<string | null>(null);
  let err = $state<string | null>(null);
  let ok = $state<string | null>(null);
  let canary = $state<CanaryRecord | null>(null);
  let receiptWatching = $state<string | null>(null);
  let receiptRetryGeneration = $state(0);
  let recoveryAcknowledged = $state(false);
  const receiptAttempts = new Set<string>();

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
      canary.phase === "deposit-submitted" || canary.phase === "burn-authorizing" ||
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

  const requestedDebtE8s = $derived(toE8s(debtInput));
  const requestedCfxWei = $derived.by(() => {
    const d = parseFloat(debtInput) || 0;
    const p = parseFloat(cfxPrice) || 0;
    if (d <= 0 || p <= 0) return 0n;
    const cfxNeeded = (d * MIN_CR / p) * 1.02; // +2% buffer over the floor
    return parseEther(cfxNeeded.toFixed(6));
  });
  const openTerms = $derived(openTermsFor(DEPLOYMENT, requestedCfxWei, requestedDebtE8s));

  function reset() { err = null; ok = null; }

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
    if (!IS_PRODUCTION_CANARY) return run();
    if (!wallet || typeof navigator === "undefined" || !navigator.locks) {
      throw new Error("Production canary requires browser cross-tab locks; use a current injected-wallet browser.");
    }
    await navigator.locks.request(`${canaryStorageKey(wallet.address)}:action`, async () => {
      // Another tab may have advanced the durable state while this tab waited.
      loadCanary(wallet!.address);
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

  async function refresh() {
    if (!wallet) return;
    try {
      const be = await backend();
      const nextVaults = await be.list_chain_vaults(CHAIN_ID);
      vaults = nextVaults;
      cfx = await cfxBalance(wallet.address);
      icusd = await icusdBalance(wallet.address);
      if (IS_PRODUCTION_CANARY) productionInventoryVerified = true;
      reconcileCanary(nextVaults);
    } catch (e: any) { err = `Refresh failed: ${e?.message ?? e}`; }
  }

  function canConnect(): boolean {
    return !IS_PRODUCTION_CANARY || productionAcknowledged;
  }

  async function connectWith(detail: EIP6963ProviderDetail) {
    reset();
    if (!canConnect()) { err = "Acknowledge the production warning before connecting."; return; }
    busy = `Connecting ${detail.info.name}…`;
    try { wallet = await connectInjected(detail); loadCanary(wallet.address); await refresh(); }
    catch (e: any) { err = e?.message ?? String(e); }
    finally { busy = null; }
  }
  async function connectLegacy() {
    reset();
    if (!canConnect()) { err = "Acknowledge the production warning before connecting."; return; }
    busy = "Connecting…";
    try { wallet = await connectLegacyInjected(); loadCanary(wallet.address); await refresh(); }
    catch (e: any) { err = e?.message ?? String(e); }
    finally { busy = null; }
  }
  async function connectDev() {
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
    receiptWatching = null;
    receiptAttempts.clear();
    recoveryAcknowledged = false;
    productionInventoryVerified = false;
    reset();
  }

  /** Sign + submit an intent, auto-syncing the per-owner nonce on a bad-nonce reject. */
  async function submit(
    action: number, vaultId: bigint, collateralWei: bigint, debt: bigint,
    call: (be: Awaited<ReturnType<typeof backend>>, i: any, sig: Uint8Array) => Promise<{ Ok?: unknown; Err?: any }>
  ): Promise<{ Ok?: unknown; Err?: any }> {
    const be = await backend();
    const w = wallet!;
    for (let attempt = 0; attempt < 2; attempt++) {
      const input: VaultIntentInput = {
        action, owner: w.address, vaultId, collateralWei, debtE8s: debt,
        nonce, deadlineSecs: BigInt(Math.floor(Date.now() / 1000) + 3600),
      };
      // toCandidIntent forces recipient to the same normalized owner address.
      const sig = await signIntent(w.client, w.account as any, input);
      const res = await call(be, toCandidIntent(input), sig);
      if ("Ok" in res) { nonce += 1n; return res; }
      const msg = errText(res.Err);
      const m = msg.match(/expected (\d+)/);
      if (m) {
        nonce = BigInt(m[1]);
        // Testnet preserves the convenient one-retry behavior. Production never
        // initiates a second signature request from a single user click.
        if (attempt === 0 && !IS_PRODUCTION_CANARY) continue;
        if (IS_PRODUCTION_CANARY) {
          return { Err: { EvmAuth: "Nonce synchronized. Review the intent and click again to sign." } };
        }
      }
      return res;
    }
    return { Err: { GenericError: "nonce sync failed" } };
  }

  async function doOpenUnlocked() {
    reset();
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
    if (openTerms.debtE8s < MIN_DEBT_E8S) { err = `Minimum debt is ${fmtIcusd(MIN_DEBT_E8S)} icUSD`; return; }
    if (openTerms.collateralWei <= 0n) { err = "Enter a debt and CFX price"; return; }
    if (IS_PRODUCTION_CANARY &&
        (openTerms.collateralWei !== CANARY_COLLATERAL_WEI || openTerms.debtE8s !== CANARY_DEBT_E8S)) {
      err = "Production canary terms are not the exact 5 CFX / 0.10 icUSD envelope.";
      return;
    }
    let openLock: CanaryRecord | null = null;
    try {
      if (IS_PRODUCTION_CANARY) {
        busy = "Verifying the one-lifecycle inventory…";
        const currentVaults = await (await backend()).list_chain_vaults(CHAIN_ID);
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
      busy = "Sign the Open intent in your wallet…";
      const res = await submit(ACTION.Open, 0n, openTerms.collateralWei, openTerms.debtE8s,
        (be, i, sig) => be.open_chain_vault_evm(i, sig));
      if ("Ok" in res) {
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
        err = errText(res.Err);
      }
    } catch (e: any) {
      const explicitRejection = isExplicitWalletRejection(e);
      const rejectionLockCleared = openLock && explicitRejection ? clearOpenLock(openLock) : true;
      err = explicitRejection
        ? (rejectionLockCleared ? (e?.shortMessage ?? e?.message ?? "Wallet signature rejected.") : err)
        : (openLock
          ? `Open result is ambiguous, so its persisted safety lock remains. Do not sign Open again; refresh to recover backend state. ${e?.shortMessage ?? e?.message ?? e}`
          : (e?.message ?? String(e)));
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
    const currentVaults = await (await backend()).list_chain_vaults(CHAIN_ID);
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

  // Actions invoked only by explicit buttons in VaultCard.
  async function onActionUnlocked(kind: string, vault: ChainVault, amountE8s?: bigint) {
    reset();
    const w = wallet!;
    try {
      let current = vault;
      if (IS_PRODUCTION_CANARY) {
        busy = "Refreshing exact vault state…";
        current = await latestVault(vault.vault_id);
      }
      if (kind === "deposit") {
        if (IS_PRODUCTION_CANARY) {
          const refusal = validateCanaryAction(canary, snapshot(current), "deposit");
          if (refusal) throw new Error(`Refusing deposit: ${refusal}`);
        }
        const previous = IS_PRODUCTION_CANARY ? persistActionLock("deposit-authorizing") : null;
        busy = `Confirm the ${fmtCfx(current.collateral_amount_e18)} CFX deposit in your wallet…`;
        let hash: `0x${string}`;
        try {
          hash = await sendDeposit(w, current.custody_address as `0x${string}`, current.collateral_amount_e18);
        } catch (e) {
          if (IS_PRODUCTION_CANARY && previous && isExplicitWalletRejection(e)) {
            restoreCanaryLock(previous, "deposit-authorizing");
          }
          if (IS_PRODUCTION_CANARY && !isExplicitWalletRejection(e)) {
            throw new Error("Deposit provider result is ambiguous. The pre-transaction lock remains; do not repeat the 5 CFX transfer. Refresh and wait for backend observation.");
          }
          throw e;
        }
        if (IS_PRODUCTION_CANARY && canary) {
          canary = recordTransaction(canary, "deposit-submitted", "deposit", hash);
          persistCanary();
          void observePendingTransaction();
        }
        ok = "Deposit submitted — waiting for the observer to report the vault Open.";
      } else if (kind === "repay") {
        const amt = IS_PRODUCTION_CANARY ? CANARY_DEBT_E8S : (amountE8s ?? current.debt_e8s);
        if (IS_PRODUCTION_CANARY) {
          const refusal = validateCanaryAction(canary, snapshot(current), "burn");
          if (refusal || amt !== CANARY_DEBT_E8S) {
            throw new Error(`Refusing burn: ${refusal ?? "amount must be exactly 0.10 icUSD."}`);
          }
        }
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
          throw e;
        }
        if (IS_PRODUCTION_CANARY && canary) {
          canary = recordTransaction(canary, "burn-submitted", "burn", hash);
          persistCanary();
          void observePendingTransaction();
        }
        ok = `Burn submitted — waiting for the observer to report zero debt.`;
      } else if (kind === "borrow") {
        if (IS_PRODUCTION_CANARY) throw new Error("Borrow-more is disabled in production-canary mode.");
        busy = "Sign the Borrow intent…";
        const res = await submit(ACTION.Borrow, current.vault_id, 0n, amountE8s ?? 0n,
          (be, i, sig) => be.borrow_chain_vault_evm(i, sig));
        if ("Ok" in res) ok = "Borrow signed — the mint will land shortly."; else err = errText(res.Err);
      } else if (kind === "withdraw") {
        if (IS_PRODUCTION_CANARY) throw new Error("Partial withdrawal is disabled in production-canary mode.");
        busy = "Sign the Withdraw intent…";
        const res = await submit(ACTION.WithdrawCollateral, current.vault_id, amountE8s ?? 0n, 0n,
          (be, i, sig) => be.withdraw_chain_collateral_evm(i, sig));
        if ("Ok" in res) ok = "Withdraw signed."; else err = errText(res.Err);
      } else if (kind === "close") {
        if (IS_PRODUCTION_CANARY) {
          const refusal = validateCanaryAction(canary, snapshot(current), "close");
          if (refusal) throw new Error(`Refusing close: ${refusal}`);
        } else if (current.debt_e8s !== 0n) {
          throw new Error("Refusing close until the observer reports zero debt.");
        }
        const previous = IS_PRODUCTION_CANARY ? persistActionLock("close-authorizing") : null;
        busy = "Sign the Close intent…";
        let res: { Ok?: unknown; Err?: any };
        try {
          res = await submit(ACTION.Close, current.vault_id, 0n, 0n,
            (be, i, sig) => be.close_chain_vault_evm(i, sig));
        } catch (e) {
          if (IS_PRODUCTION_CANARY && previous && isExplicitWalletRejection(e)) {
            restoreCanaryLock(previous, "close-authorizing");
          }
          if (IS_PRODUCTION_CANARY && !isExplicitWalletRejection(e)) {
            throw new Error("Close result is ambiguous. The pre-signature lock remains; do not submit another Close. Refresh and wait for backend observation.");
          }
          throw e;
        }
        if ("Ok" in res) {
          if (IS_PRODUCTION_CANARY) setCanaryPhase("close-submitted");
          ok = "Close submitted — waiting for the backend to report Closed.";
        } else {
          if (IS_PRODUCTION_CANARY && previous) restoreCanaryLock(previous, "close-authorizing");
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
</script>

<div class="wrap" class:production={IS_PRODUCTION_CANARY}>
  <header class="top">
    <div class="brand">
      <div class="logo">R</div>
      <div>
        <h1>icUSD on Conflux eSpace</h1>
        <div class="sub">Self-serve CDP · sign with your EVM wallet</div>
      </div>
    </div>
    <span class="badge" class:testnet={!IS_PRODUCTION_CANARY} class:mainnet={IS_PRODUCTION_CANARY}>
      {IS_PRODUCTION_CANARY ? "PRODUCTION · chain 1030" : "eSpace testnet · chain 71 · staging"}
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
      {#if !IS_PRODUCTION_CANARY}
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
        <p class="hint">Enter the icUSD you want to mint. Required CFX is the {Math.round(MIN_CR * 100)}% min-CR
          floor (+2% buffer) at your price — the real CR check runs on the canister.</p>
        <div class="row">
          <div class="field" style="flex:1">
            <label for="debt">icUSD debt</label>
            <input id="debt" type="number" min="0.1" step="0.1" bind:value={debtInput} />
          </div>
          <div class="field" style="flex:1">
            <label for="cfxprice">CFX price (USD, hint)</label>
            <input id="cfxprice" type="number" min="0" step="0.01" bind:value={cfxPrice} />
          </div>
        </div>
        <div class="kv"><span class="k">Required CFX (≈)</span><span class="v">{fmtCfx(openTerms.collateralWei)}</span></div>
      {/if}
      <div class="row" style="margin-top:14px">
        <button class="primary" onclick={doOpen} disabled={!!busy || productionLifecycleUsed || (IS_PRODUCTION_CANARY && !productionInventoryVerified)}>Sign & open</button>
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
    </div>

    {#each owned as v (v.vault_id)}
      <VaultCard
        vault={v}
        busy={busy}
        productionCanary={IS_PRODUCTION_CANARY}
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
  {#if canaryPolling || receiptWatching}<div class="notice info"><span class="spin"></span>Read-only receipt/status polling is active; no wallet action will happen automatically.</div>{/if}
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
    {:else}
      Testnet only. The chains rail is experimental — not on production.
    {/if}
  </div>
</div>
